//! The HTTP transport of the adapters (`core/transport.js`): one JSON request, or one streamed (SSE) request; error
//! classification into a small set of codes with guidance; size caps; and redaction, so no error text ever carries a
//! key. The HTTP client itself is a trait ([`HttpClient`]): `crate::http::UreqClient` with the `http` feature, a
//! scripted one in tests.

use crate::json::Value;
use crate::sse::{SseParser, SseRecord, Utf8Stream};
use crate::text::js_head;
use regex::Regex;
use std::fmt;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_IMAGE_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// What went wrong, in the drawer's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    Auth,
    MissingModel,
    BadEndpoint,
    Network,
    Timeout,
    RateLimit,
    Refused,
    Malformed,
    Cancelled,
    Budget,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 10] = [
        ErrorCode::Auth,
        ErrorCode::MissingModel,
        ErrorCode::BadEndpoint,
        ErrorCode::Network,
        ErrorCode::Timeout,
        ErrorCode::RateLimit,
        ErrorCode::Refused,
        ErrorCode::Malformed,
        ErrorCode::Cancelled,
        ErrorCode::Budget,
    ];

    pub fn id(self) -> &'static str {
        match self {
            ErrorCode::Auth => "auth",
            ErrorCode::MissingModel => "missing-model",
            ErrorCode::BadEndpoint => "bad-endpoint",
            ErrorCode::Network => "network",
            ErrorCode::Timeout => "timeout",
            ErrorCode::RateLimit => "rate-limit",
            ErrorCode::Refused => "refused",
            ErrorCode::Malformed => "malformed",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::Budget => "budget",
        }
    }

    /// The code with this id; an unknown id is `malformed` (as `new AiError(code)` does).
    pub fn parse(id: &str) -> ErrorCode {
        ErrorCode::ALL.into_iter().find(|c| c.id() == id).unwrap_or(ErrorCode::Malformed)
    }

    fn known(id: &str) -> bool {
        ErrorCode::ALL.iter().any(|c| c.id() == id)
    }

    /// The guidance a message about this code carries, when it has one.
    pub fn guide(self) -> Option<&'static str> {
        Some(match self {
            ErrorCode::Auth => "The provider rejected the credentials. Check the API key in Settings.",
            ErrorCode::MissingModel => "The server does not have that model (or none is loaded). Pick one with \"Load models\" in Settings.",
            ErrorCode::BadEndpoint => "That address does not offer this API. Check the base URL, the port and the provider type.",
            ErrorCode::RateLimit => "The provider is rate limiting requests. Wait a moment and try again.",
            ErrorCode::Timeout => "The model took too long. Loading a model can be slow the first time; try again, or raise the timeout in Settings.",
            ErrorCode::Budget => "The request is too large. Lower \"Max screen content\" or \"Conversation memory\" in Settings.",
            _ => return None,
        })
    }
}

/// An error of the AI layer (`AiError`).
#[derive(Debug, Clone, PartialEq)]
pub struct AiError {
    pub code: ErrorCode,
    pub message: String,
    pub status: Option<u16>,
    pub param: Option<String>,
    pub hints: Vec<String>,
}

impl AiError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> AiError {
        AiError { code, message: message.into(), status: None, param: None, hints: Vec::new() }
    }

    pub fn with_status(mut self, status: u16) -> AiError {
        self.status = Some(status);
        self
    }

    pub fn cancelled() -> AiError {
        AiError::new(ErrorCode::Cancelled, "Cancelled.")
    }
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AiError {}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("transport pattern"))
}

/// Replace known secrets, and anything that looks like a credential.
pub fn redact(text: &str, secrets: &[&str]) -> String {
    static BEARER: OnceLock<Regex> = OnceLock::new();
    static KEYS: OnceLock<Regex> = OnceLock::new();
    static QUERY: OnceLock<Regex> = OnceLock::new();
    static HEADER: OnceLock<Regex> = OnceLock::new();
    let mut s = text.to_string();
    for k in secrets {
        if k.chars().count() >= 4 {
            s = s.replace(k, "[redacted]");
        }
    }
    let s = re(&BEARER, r#"(?i)(bearer\s+)[^\s"',;]+"#).replace_all(&s, "${1}[redacted]");
    let s = re(&KEYS, r"\b(sk-[A-Za-z0-9_-]{6,}|AIza[0-9A-Za-z_-]{10,})").replace_all(&s, "[redacted]");
    let s = re(&QUERY, r#"(?i)([?&](?:key|api_key|token)=)[^&\s"']+"#).replace_all(&s, "${1}[redacted]");
    let s = re(&HEADER, r#"(?i)((?:x-api-key|x-goog-api-key|authorization)["']?\s*[:=]\s*["']?)[^\s"',;]+"#)
        .replace_all(&s, "${1}[redacted]");
    s.into_owned()
}

fn model_hint() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    re(
        &RE,
        r"(?i)model\b[^\n]{0,80}?\b(not[ _-]?found|does ?n[o']t exist|not exist|invalid|unknown|is not (found|supported)|not available|not loaded)|(invalid|unknown|no such) model|model_not_found|no models? loaded|models/[\w.-]+ is not found",
    )
}

/// An HTTP body as the server sent it: parsed JSON, or text.
#[derive(Debug, Clone)]
pub enum Body<'a> {
    Json(&'a Value),
    Text(&'a str),
}

/// What [`classify_http`] decides.
#[derive(Debug, Clone, PartialEq)]
pub struct Classified {
    pub code: ErrorCode,
    pub message: String,
    pub param: Option<String>,
}

fn nullish(v: Option<&Value>) -> Option<&Value> {
    v.filter(|x| !x.is_null())
}

/// Map an HTTP failure to an error code (`classifyHttp`).
pub fn classify_http(status: u16, body: Body<'_>) -> Classified {
    let (err, msg): (Option<&Value>, String) = match body {
        Body::Text(t) => (None, t.to_string()),
        Body::Json(Value::String(s)) => (None, s.clone()),
        Body::Json(v) => {
            let obj = if v.as_object().is_some() { Some(v) } else { None };
            let err = obj.map(|o| match o.get("error") {
                Some(e) if e.as_object().is_some() => e,
                _ => o,
            });
            let message = match nullish(err.and_then(|e| e.get("message"))) {
                Some(m) => m.js_string(),
                None => obj.and_then(|o| o.get("error")).and_then(|e| e.as_str()).unwrap_or("").to_string(),
            };
            (err, message)
        }
    };
    let pcode = err
        .and_then(|e| nullish(e.get("code")).or_else(|| nullish(e.get("type"))).or_else(|| nullish(e.get("status"))))
        .map(|v| v.js_string())
        .unwrap_or_default();
    let param = err.and_then(|e| e.get("param")).and_then(|p| p.as_str()).map(str::to_string);
    let model_issue = param.as_deref() == Some("model")
        || pcode.to_ascii_lowercase().contains("model_not_found")
        || model_hint().is_match(&msg);
    let code = match status {
        401 | 403 => ErrorCode::Auth,
        429 => ErrorCode::RateLimit,
        400 | 404 | 422 => {
            if model_issue {
                ErrorCode::MissingModel
            } else if status == 404 {
                ErrorCode::BadEndpoint
            } else {
                ErrorCode::Refused
            }
        }
        405 | 501 => ErrorCode::BadEndpoint,
        408 | 504 => ErrorCode::Timeout,
        413 => ErrorCode::Budget,
        s if s >= 500 => {
            if model_issue {
                ErrorCode::MissingModel
            } else {
                ErrorCode::Network
            }
        }
        _ => ErrorCode::Refused,
    };
    Classified { code, message: msg, param }
}

/// The error for an HTTP failure (`httpError`). A relay reports the drawer's own code and a message for the user.
pub fn http_error(status: u16, text: &str, secrets: &[&str], relay: bool) -> AiError {
    let json = if text.is_empty() { None } else { Value::parse(text).ok() };
    if relay {
        if let Some(re) = json.as_ref().and_then(|j| j.get("error")).filter(|e| e.as_object().is_some()) {
            if let (Some(code), Some(message)) =
                (re.get("code").and_then(|c| c.as_str()), re.get("message").and_then(|m| m.as_str()))
            {
                if ErrorCode::known(code) {
                    let detail = match re.get("detail").and_then(|d| d.as_str()).filter(|d| !d.is_empty()) {
                        Some(d) => format!(" ({})", js_head(&redact(d, secrets), 400)),
                        None => String::new(),
                    };
                    return AiError::new(
                        ErrorCode::parse(code),
                        format!("{}{}", js_head(&redact(message, secrets), 600), detail),
                    )
                    .with_status(status);
                }
            }
        }
    }
    let c = match json.as_ref().filter(|j| !j.is_null()) {
        Some(j) => classify_http(status, Body::Json(j)),
        None => classify_http(status, Body::Text(text)),
    };
    let said = if c.message.is_empty() {
        String::new()
    } else {
        format!(" Server said: \"{}\"", js_head(&redact(&c.message, secrets), 400))
    };
    let guide = c.code.guide().unwrap_or(if status >= 500 {
        "The server reported an internal error."
    } else {
        "The request was refused."
    });
    let mut e = AiError::new(c.code, format!("HTTP {status}. {guide}{said}")).with_status(status);
    e.param = c.param;
    e
}

/// Some servers answer HTTP 200 with only an error body (LM Studio for unknown path prefixes): `bodyError`.
pub fn body_error(json: &Value, status: u16, secrets: &[&str]) -> Option<AiError> {
    static ENDPOINT: OnceLock<Regex> = OnceLock::new();
    json.as_object()?;
    let error = json.get("error").filter(|e| e.truthy())?;
    let msg = match error {
        Value::String(s) => s.clone(),
        other => other.get("message")?.as_str()?.to_string(),
    };
    let code = if re(&ENDPOINT, r"(?i)unexpected endpoint|no route|unknown (path|url|endpoint)").is_match(&msg)
        && !model_hint().is_match(&msg)
    {
        ErrorCode::BadEndpoint
    } else {
        let n = error.get("code").map(|c| c.js_number()).unwrap_or(f64::NAN);
        let s = if (400.0..65536.0).contains(&n) { n as u16 } else { 400 };
        classify_http(s, Body::Json(json)).code
    };
    let guide = code.guide().unwrap_or("The request was refused.");
    Some(
        AiError::new(code, format!("{guide} Server said: \"{}\"", js_head(&redact(&msg, secrets), 400)))
            .with_status(status),
    )
}

/// Join a base URL and a path without doubling `/v1` or slashes (`joinUrl`).
pub fn join_url(base: &str, path: &str) -> String {
    let mut b = crate::text::js_trim(base).trim_end_matches('/').to_string();
    let p = if path.starts_with('/') { path.to_string() } else { format!("/{path}") };
    for prefix in ["/v1/", "/v1beta/"] {
        if p.starts_with(prefix) && b.ends_with(&prefix[..prefix.len() - 1]) {
            b.truncate(b.len() - (prefix.len() - 1));
            break;
        }
    }
    b + &p
}

/// Guidance for a request that never got an HTTP response (native: no CORS, no mixed content).
pub fn network_guidance(url: &str) -> String {
    match crate::providers::url_host(url) {
        Some(h) if crate::providers::is_loopback_url(url) => {
            let origin = url.split('/').take(3).collect::<Vec<_>>().join("/");
            let _ = h;
            format!("Could not reach {origin}. Is the server running on that port? For LM Studio: Developer tab > Start server. Use 127.0.0.1 rather than localhost on Windows.")
        }
        _ => "The request could not be completed (offline, a wrong address, or a firewall).".to_string(),
    }
}

/// Stops a request from another thread.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// One HTTP request, ready to send.
#[derive(Debug, Clone, PartialEq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    /// The idle timeout: no response (or no new bytes) for this long fails the request.
    pub timeout_ms: u64,
}

/// A response: its status, content type, and a body to read from.
pub struct HttpResponse {
    pub status: u16,
    pub content_type: String,
    pub body: Box<dyn Read + Send>,
}

/// Why a request got no response, or its body broke off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpFailure {
    Timeout,
    Cancelled,
    Network(String),
}

/// Sends HTTP requests. `crate::http::UreqClient` with the `http` feature; tests script their own.
pub trait HttpClient: Send + Sync {
    fn send(&self, request: &HttpRequest, cancel: &CancelToken) -> Result<HttpResponse, HttpFailure>;
}

/// What a request to send needs besides the client.
pub struct RequestOptions<'a> {
    pub method: &'a str,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<&'a Value>,
    pub timeout_ms: u64,
    pub secrets: Vec<String>,
    pub relay: bool,
    /// Characters of image data in the body (they do not count against the request cap; they have their own).
    pub image_chars: usize,
    pub cancel: CancelToken,
}

impl RequestOptions<'_> {
    fn secret_refs(&self) -> Vec<&str> {
        self.secrets.iter().map(String::as_str).collect()
    }
}

fn fail(e: HttpFailure, o: &RequestOptions<'_>) -> AiError {
    match e {
        HttpFailure::Timeout => AiError::new(
            ErrorCode::Timeout,
            format!(
                "No response for {} s. {}",
                ((o.timeout_ms as f64) / 1000.0).round(),
                ErrorCode::Timeout.guide().unwrap_or("")
            ),
        ),
        HttpFailure::Cancelled => AiError::cancelled(),
        // The caller cancelled: whatever broke the connection, that is why (JS: `o.signal?.aborted`).
        HttpFailure::Network(_) if o.cancel.is_cancelled() => AiError::cancelled(),
        HttpFailure::Network(m) => AiError::new(
            ErrorCode::Network,
            format!("{} ({})", network_guidance(&o.url), js_head(&redact(&m, &o.secret_refs()), 200)),
        ),
    }
}

fn prepare(o: &RequestOptions<'_>) -> Result<HttpRequest, AiError> {
    let lower = o.url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return Err(AiError::new(ErrorCode::BadEndpoint, "The address must start with http:// or https://."));
    }
    let payload = o.body.map(crate::json::to_json);
    if o.image_chars > MAX_IMAGE_BYTES {
        return Err(AiError::new(
            ErrorCode::Budget,
            format!(
                "The attached images are larger than {} MB. Remove a screenshot and try again.",
                MAX_IMAGE_BYTES / 1024 / 1024
            ),
        ));
    }
    if let Some(p) = &payload {
        if crate::text::utf16_len(p).saturating_sub(o.image_chars) > MAX_REQUEST_BYTES {
            return Err(AiError::new(
                ErrorCode::Budget,
                format!(
                    "The request is larger than {} KB. {}",
                    MAX_REQUEST_BYTES / 1024,
                    ErrorCode::Budget.guide().unwrap_or("")
                ),
            ));
        }
    }
    let mut headers = Vec::new();
    if payload.is_some() {
        headers.push(("Content-Type".to_string(), "application/json".to_string()));
    }
    headers.extend(o.headers.iter().cloned());
    Ok(HttpRequest {
        method: if o.method.is_empty() {
            if payload.is_some() { "POST" } else { "GET" }.to_string()
        } else {
            o.method.to_string()
        },
        url: o.url.clone(),
        headers,
        body: payload,
        timeout_ms: o.timeout_ms.max(1),
    })
}

fn read_all(body: &mut dyn Read, limit: usize, o: &RequestOptions<'_>) -> Result<String, AiError> {
    let mut bytes = Vec::new();
    let mut buf = [0u8; 16 * 1024];
    loop {
        if o.cancel.is_cancelled() {
            return Err(AiError::cancelled());
        }
        let n = body.read(&mut buf).map_err(|e| fail(io_failure(&e), o))?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&buf[..n]);
        if bytes.len() > limit {
            break;
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn io_failure(e: &std::io::Error) -> HttpFailure {
    match e.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => HttpFailure::Timeout,
        std::io::ErrorKind::Interrupted => HttpFailure::Cancelled,
        _ => HttpFailure::Network(e.to_string()),
    }
}

/// Send one JSON request and return the parsed body (`requestJson`).
pub fn request_json(client: &dyn HttpClient, o: &RequestOptions<'_>) -> Result<Value, AiError> {
    let req = prepare(o)?;
    if o.cancel.is_cancelled() {
        return Err(AiError::cancelled());
    }
    let mut res = client.send(&req, &o.cancel).map_err(|e| fail(e, o))?;
    let text = read_all(res.body.as_mut(), MAX_RESPONSE_BYTES, o)?;
    if text.len() > MAX_RESPONSE_BYTES {
        return Err(AiError::new(ErrorCode::Malformed, "The response was larger than the allowed size."));
    }
    let secrets = o.secret_refs();
    if !(200..300).contains(&res.status) {
        return Err(http_error(res.status, &text, &secrets, o.relay));
    }
    let json = Value::parse(&text).ok().filter(|j| j.as_object().is_some() || j.as_array().is_some());
    let Some(json) = json else {
        return Err(AiError::new(ErrorCode::Malformed, "The server replied, but not with JSON."));
    };
    if let Some(e) = body_error(&json, res.status, &secrets) {
        return Err(e);
    }
    Ok(json)
}

/// How a streaming request ended: streamed records, or a whole JSON reply.
#[derive(Debug, Clone, PartialEq)]
pub enum Streamed {
    Records,
    Json(Value),
}

/// Send one streaming request; SSE records go to `on_record` as they arrive (it may fail to stop the stream). A
/// server that answers with plain JSON instead gives [`Streamed::Json`] (`requestStream`).
pub fn request_stream(
    client: &dyn HttpClient,
    o: &RequestOptions<'_>,
    on_record: &mut dyn FnMut(SseRecord) -> Result<(), AiError>,
) -> Result<Streamed, AiError> {
    let req = prepare(o)?;
    if o.cancel.is_cancelled() {
        return Err(AiError::cancelled());
    }
    let mut res = client.send(&req, &o.cancel).map_err(|e| fail(e, o))?;
    let secrets = o.secret_refs();
    if !(200..300).contains(&res.status) {
        let text = read_all(res.body.as_mut(), 64 * 1024, o).unwrap_or_default();
        return Err(http_error(res.status, js_head(&text, 8000), &secrets, o.relay));
    }
    if !res.content_type.to_ascii_lowercase().contains("text/event-stream") {
        let text = read_all(res.body.as_mut(), MAX_RESPONSE_BYTES, o)?;
        let json = Value::parse(&text).ok().filter(|j| j.as_object().is_some() || j.as_array().is_some());
        let Some(json) = json else {
            // A server that streamed without the right content type: read it as SSE anyway.
            static SSE_LIKE: OnceLock<Regex> = OnceLock::new();
            if re(&SSE_LIKE, r"(?m)^\s*(data|event):").is_match(&text) {
                let mut p = SseParser::new();
                for r in p.push(&text).into_iter().chain(p.end()) {
                    on_record(r)?;
                }
                return Ok(Streamed::Records);
            }
            return Err(AiError::new(ErrorCode::Malformed, "The server replied, but neither as a stream nor as JSON."));
        };
        if let Some(e) = body_error(&json, res.status, &secrets) {
            return Err(e);
        }
        return Ok(Streamed::Json(json));
    }
    let mut parser = SseParser::new();
    let mut utf8 = Utf8Stream::new();
    let mut total = 0usize;
    let mut buf = [0u8; 16 * 1024];
    loop {
        if o.cancel.is_cancelled() {
            return Err(AiError::cancelled());
        }
        let n = res.body.read(&mut buf).map_err(|e| fail(io_failure(&e), o))?;
        if n == 0 {
            break;
        }
        total += n;
        if total > MAX_RESPONSE_BYTES {
            return Err(AiError::new(ErrorCode::Budget, "The streamed reply was larger than the allowed size."));
        }
        for r in parser.push(&utf8.push(&buf[..n])) {
            on_record(r)?;
        }
    }
    for r in parser.push(&utf8.end()).into_iter().chain(parser.end()) {
        on_record(r)?;
    }
    Ok(Streamed::Records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redaction() {
        let s = redact("key sk-abcdef123 and Bearer xyz, ?api_key=1&x=2 x-api-key: abc; mine", &["mine"]);
        assert_eq!(
            s,
            "key [redacted] and Bearer [redacted], ?api_key=[redacted]&x=2 x-api-key: [redacted]; [redacted]"
        );
    }

    #[test]
    fn join() {
        assert_eq!(
            join_url("https://api.openai.com/v1/", "/v1/chat/completions"),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            join_url("http://127.0.0.1:9000", "/v1/chat/completions"),
            "http://127.0.0.1:9000/v1/chat/completions"
        );
        assert_eq!(join_url("https://g.com/v1beta", "/v1beta/models/x"), "https://g.com/v1beta/models/x");
        assert_eq!(join_url("https://x.com/api", "chat"), "https://x.com/api/chat");
    }

    #[test]
    fn classification() {
        let j =
            Value::parse(r#"{"error":{"message":"The model `x` does not exist","code":"model_not_found"}}"#).unwrap();
        assert_eq!(classify_http(404, Body::Json(&j)).code, ErrorCode::MissingModel);
        assert_eq!(classify_http(404, Body::Text("Cannot POST /x")).code, ErrorCode::BadEndpoint);
        assert_eq!(classify_http(503, Body::Text("down")).code, ErrorCode::Network);
        assert_eq!(classify_http(413, Body::Text("")).code, ErrorCode::Budget);
        let e = http_error(
            401,
            r#"{"error":{"code":"refused","message":"Not from here.","detail":"origin x"}}"#,
            &[],
            true,
        );
        assert_eq!((e.code, e.message.as_str()), (ErrorCode::Refused, "Not from here. (origin x)"));
    }
}
