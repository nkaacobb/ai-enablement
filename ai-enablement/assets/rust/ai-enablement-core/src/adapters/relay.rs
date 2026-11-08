//! The app relay (`adapters/relay.js`): the application's own server calls the provider (keys stay on the server, no
//! CORS). Reference servers: `assets/relay/relay.php` and `assets/relay/relay.mjs`.
//!
//! - `POST {relayUrl}` JSON `{ action:'chat', provider, baseUrl, model, apiKey, system, messages:[{role, content,
//!   images?}], maxTokens, temperature, reasoning, tools?, toolTurns?, turnId? }` → `text/event-stream` with events
//!   `delta {text}`, `reasoning {text}`, `notice {message}`, `status {message}`, `tool_call {id, name, arguments,
//!   signature?, raw?}`, `error {message, code?, hints?}`, `done {usage?, provider?, model?, truncated?}`; or JSON
//!   `{ text, reasoning?, usage?, toolCalls? }` | `{ error:{ message, code? } }`.
//! - `tools` and `toolTurns` are the neutral formats; the relay translates them for the provider. `turnId` lets a
//!   public relay count one question per chain of tool steps.
//! - Images (relay 1.3+): a relay says how many images it accepts per request in its GET reply (`images`), so a
//!   request with images first asks the relay once and fails clearly when it cannot pass them.
//! - `POST {relayUrl}` `{ action:'models', provider, baseUrl, apiKey }` → `{ ok, models:[{id, label, loaded}],
//!   error? }`.
//! - Every request carries `X-Requested-With: ai-agent-drawer`. An empty `apiKey` makes the relay use its own key.

use super::{
    object, parse_record, post_options, set_header, text_or_empty, truthy_text, AdapterConfig, ChatCall, ModelInfo,
    WireRequest,
};
use crate::chat::{image_chars, normalize_messages, ChatResult, EventKind, RequestMessage, StreamEvent, ToolTurn};
use crate::json::{to_value, Map, Value};
use crate::tools::{tool_call, ToolCall};
use crate::transport::{
    request_json, request_stream, AiError, CancelToken, ErrorCode, HttpClient, HttpRequest, RequestOptions, Streamed,
};
use std::collections::HashMap;
use std::io::Read;
use std::sync::{Mutex, OnceLock};

/// How long the relay's GET probe may take (`PROBE_TIMEOUT_MS`).
pub const PROBE_TIMEOUT_MS: u64 = 2500;

/// The headers of every relay request: `X-Requested-With: ai-agent-drawer`, then the configured extra headers.
pub fn relay_headers(cfg: &AdapterConfig) -> Vec<(String, String)> {
    let mut h = vec![("X-Requested-With".to_string(), "ai-agent-drawer".to_string())];
    for (k, v) in &cfg.relay_headers {
        set_header(&mut h, k, v);
    }
    h
}

/// The request (`buildChat`): the neutral request as it is.
pub fn build_chat(call: &ChatCall<'_>) -> WireRequest {
    let cfg = call.cfg;
    let req = call.request;
    let mut body = Map::new();
    body.insert("action", "chat");
    body.insert("provider", cfg.provider.as_str());
    body.insert("baseUrl", cfg.base_url.as_str());
    body.insert("model", cfg.model.as_str());
    body.insert("apiKey", call.key);
    body.insert("system", req.system.as_str());
    body.insert("messages", to_value(&normalize_messages(&req.messages)));
    if let Some(n) = call.max_tokens {
        body.insert("maxTokens", n);
    }
    if let Some(t) = call.temperature {
        body.insert("temperature", t);
    }
    body.insert("reasoning", cfg.reasoning.as_str());
    if !req.tools.is_empty() {
        body.insert("tools", to_value(&req.tools));
    }
    if !req.tool_turns.is_empty() {
        body.insert("toolTurns", to_value(&req.tool_turns));
    }
    if !req.turn_id.is_empty() {
        body.insert("turnId", req.turn_id.as_str());
    }
    WireRequest { url: cfg.relay_url.clone(), headers: relay_headers(cfg), body: Value::Object(body) }
}

/// Images in a request: the user messages' and the tool results' (`countImages`).
fn count_images(messages: &[RequestMessage], turns: &[ToolTurn]) -> usize {
    messages.iter().map(|m| m.images.len()).sum::<usize>()
        + turns.iter().flat_map(|t| &t.results).map(|r| r.images.len()).sum::<usize>()
}

/// Relay address → how many images it accepts per request (asked once, when a request first carries images).
fn image_support() -> &'static Mutex<HashMap<String, u64>> {
    static CACHE: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Ask the relay (GET) how many images it accepts: `None` when it is not reachable or not a relay (the request then
/// reports it), else the count (0 = none; a relay older than 1.3 does not say). `probeRelay`, the part used here.
pub fn probe_images(
    client: &dyn HttpClient,
    url: &str,
    headers: &[(String, String)],
    cancel: &CancelToken,
) -> Option<u64> {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return None;
    }
    let mut h = vec![
        ("Accept".to_string(), "application/json".to_string()),
        ("X-Requested-With".to_string(), "ai-agent-drawer".to_string()),
    ];
    for (k, v) in headers {
        set_header(&mut h, k, v);
    }
    let req =
        HttpRequest { method: "GET".into(), url: url.into(), headers: h, body: None, timeout_ms: PROBE_TIMEOUT_MS };
    let mut res = client.send(&req, cancel).ok()?;
    let mut bytes = Vec::new();
    res.body.as_mut().take(1024 * 1024).read_to_end(&mut bytes).ok()?;
    let json = Value::parse(&String::from_utf8_lossy(&bytes)).ok()?;
    if json.get("relay").and_then(Value::as_str) != Some("ai-agent-drawer") {
        return None;
    }
    let n = json.get("images").and_then(Value::as_f64).filter(|n| n.fract() == 0.0 && *n > 0.0);
    Some(n.map_or(0, |n| n.min(u64::MAX as f64) as u64))
}

/// Forget what relays said about images (the next request with images asks again).
pub fn forget_image_support() {
    image_support().lock().unwrap_or_else(|e| e.into_inner()).clear();
}

fn check_images(
    client: &dyn HttpClient,
    cfg: &AdapterConfig,
    count: usize,
    cancel: &CancelToken,
) -> Result<(), AiError> {
    let cached = image_support().lock().unwrap_or_else(|e| e.into_inner()).get(&cfg.relay_url).copied();
    let max = match cached {
        Some(m) => m,
        None => {
            let Some(m) = probe_images(client, &cfg.relay_url, &relay_headers(cfg), cancel) else { return Ok(()) };
            image_support().lock().unwrap_or_else(|e| e.into_inner()).insert(cfg.relay_url.clone(), m);
            m
        }
    };
    if max == 0 {
        return Err(AiError::new(ErrorCode::Refused, "The relay cannot pass images to the model (it is older than version 1.3, or images are switched off in its configuration). Update the relay, or turn off \"This model can see images\" in Settings > Vision."));
    }
    if count as u64 > max {
        return Err(AiError::new(
            ErrorCode::Budget,
            format!(
                "The relay accepts at most {max} image{} per request (this one has {count}). Remove a screenshot, or start a new chat.",
                if max == 1 { "" } else { "s" }
            ),
        ));
    }
    Ok(())
}

/// A tool call the relay sent (`callFrom`): the raw text when it gives one (so a broken call is diagnosed here),
/// else the arguments. `n`: the calls so far (a call without an id is `relay_<n + 1>`).
fn call_from(c: &Value, n: usize) -> ToolCall {
    let id = truthy_text(c.get("id")).unwrap_or_else(|| format!("relay_{}", n + 1));
    let name = c.get("name").map(Value::js_string).unwrap_or_default();
    let raw = match c.get("raw") {
        Some(r @ Value::String(_)) => r,
        _ => c.get("arguments").unwrap_or(&Value::Null),
    };
    let mut call = tool_call(id, name, raw);
    if let Some(sig) = truthy_text(c.get("signature")) {
        call.signature = Some(sig);
    }
    call
}

/// An `error` event as the error it reports (with the relay's hints).
fn error_event(json: &Value) -> AiError {
    let code = truthy_text(json.get("code")).unwrap_or_else(|| "refused".into());
    let message = truthy_text(json.get("message")).unwrap_or_else(|| "The relay reported an error.".into());
    let detail = truthy_text(json.get("detail")).map(|d| format!(" ({d})")).unwrap_or_default();
    let mut e = AiError::new(ErrorCode::parse(&code), format!("{message}{detail}"));
    e.hints = json
        .get("hints")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(Value::js_string).collect())
        .unwrap_or_default();
    e
}

/// Stream one reply through the relay (`stream`).
pub fn stream(
    client: &dyn HttpClient,
    call: &ChatCall<'_>,
    on_event: &mut dyn FnMut(StreamEvent),
    cancel: &CancelToken,
) -> Result<ChatResult, AiError> {
    let cfg = call.cfg;
    if cfg.relay_url.is_empty() {
        return Err(AiError::new(
            ErrorCode::BadEndpoint,
            "No relay address is configured (Settings > Model > Advanced).",
        ));
    }
    let req = call.request;
    let wire = build_chat(call);
    let messages = normalize_messages(&req.messages);
    let images = count_images(&messages, &req.tool_turns);
    if images > 0 {
        check_images(client, cfg, images, cancel)?;
    }
    let opts = post_options(call, &wire, image_chars(&messages, &req.tool_turns), true, cancel);
    let mut out = ChatResult::default();
    let res = request_stream(client, &opts, &mut |rec| {
        let Some(json) = parse_record(&rec.data) else { return Ok(()) };
        match rec.event.as_str() {
            "delta" => on_event(StreamEvent::new(EventKind::Text, text_or_empty(json.get("text")))),
            "reasoning" => on_event(StreamEvent::new(EventKind::Reasoning, text_or_empty(json.get("text")))),
            "notice" => on_event(StreamEvent::new(EventKind::Notice, text_or_empty(json.get("message")))),
            "status" => on_event(StreamEvent::new(EventKind::Status, text_or_empty(json.get("message")))),
            "tool_call" if json.get("name").is_some_and(Value::truthy) => {
                let c = call_from(&json, out.tool_calls.len());
                out.tool_calls.push(c);
            }
            "error" => return Err(error_event(&json)),
            "done" => {
                out.usage = json.get("usage").cloned();
                out.truncated = json.get("truncated") == Some(&Value::Bool(true));
            }
            _ => {}
        }
        Ok(())
    })?;
    if let Streamed::Json(j) = res {
        if let Some(err) = j.get("error").filter(|e| e.truthy()) {
            let code = truthy_text(err.get("code")).unwrap_or_else(|| "refused".into());
            let message = err.get("message").filter(|m| m.truthy()).unwrap_or(err).js_string();
            return Err(AiError::new(ErrorCode::parse(&code), message));
        }
        if let Some(r) = truthy_text(j.get("reasoning")) {
            on_event(StreamEvent::new(EventKind::Reasoning, r));
        }
        if let Some(t) = truthy_text(j.get("text")) {
            on_event(StreamEvent::new(EventKind::Text, t));
        }
        for c in j.get("toolCalls").and_then(Value::as_array).map_or(&[][..], Vec::as_slice) {
            if c.get("name").is_some_and(Value::truthy) {
                let call = call_from(c, out.tool_calls.len());
                out.tool_calls.push(call);
            }
        }
        out.usage = j.get("usage").cloned();
        out.truncated = j.get("truncated") == Some(&Value::Bool(true));
    }
    Ok(out)
}

/// A model as the relay lists it. The JS adapter returns the relay's objects as they are; here they are read into
/// [`ModelInfo`] (a model without an id is left out).
fn model_from(m: &Value) -> Option<ModelInfo> {
    let id = truthy_text(m.get("id"))?;
    Some(ModelInfo {
        label: truthy_text(m.get("label")).unwrap_or_else(|| id.clone()),
        id,
        loaded: m.get("loaded").is_some_and(Value::truthy),
        vision: m.get("vision").and_then(Value::as_bool),
    })
}

/// The models the relay offers for the provider (`listModels`).
pub fn list_models(
    client: &dyn HttpClient,
    cfg: &AdapterConfig,
    key: &str,
    cancel: &CancelToken,
) -> Result<Vec<ModelInfo>, AiError> {
    if cfg.relay_url.is_empty() {
        return Err(AiError::new(ErrorCode::BadEndpoint, "No relay address is configured."));
    }
    let body = object([
        ("action", "models".into()),
        ("provider", cfg.provider.as_str().into()),
        ("baseUrl", cfg.base_url.as_str().into()),
        ("apiKey", key.into()),
    ]);
    let j = request_json(
        client,
        &RequestOptions {
            method: "",
            url: cfg.relay_url.clone(),
            headers: relay_headers(cfg),
            body: Some(&body),
            timeout_ms: cfg.timeout_ms,
            secrets: vec![key.to_string()],
            relay: true,
            image_chars: 0,
            cancel: cancel.clone(),
        },
    )?;
    if !j.get("ok").is_some_and(Value::truthy) {
        let code = truthy_text(j.get("code")).unwrap_or_else(|| "network".into());
        let message = truthy_text(j.get("error")).unwrap_or_else(|| "The relay could not list models.".into());
        return Err(AiError::new(ErrorCode::parse(&code), message));
    }
    Ok(j.get("models").and_then(Value::as_array).map_or(&[][..], Vec::as_slice).iter().filter_map(model_from).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::testing::{Reply, Scripted};
    use crate::chat::{ChatRequest, Image, Role};

    fn cfg(url: &str) -> AdapterConfig {
        AdapterConfig {
            provider: "openai".into(),
            relay_url: url.into(),
            relay_headers: vec![("X-App".into(), "1".into())],
            ..Default::default()
        }
    }

    fn with_images(n: usize) -> ChatRequest {
        let image = Image { mime: "image/png".into(), data: "iVBORw0KGgoAAAANSUhEUg==".into() };
        ChatRequest {
            messages: vec![RequestMessage { role: Role::User, content: "Look.".into(), images: vec![image; n] }],
            ..Default::default()
        }
    }

    fn run(client: &Scripted, cfg: &AdapterConfig, req: &ChatRequest) -> Result<ChatResult, AiError> {
        let call = ChatCall { cfg, key: "", request: req, max_tokens: Some(100.0), temperature: None };
        stream(client, &call, &mut |_| {}, &CancelToken::new())
    }

    #[test]
    fn images_ask_the_relay_once() {
        let url = "https://relay.test/images-once";
        let client = Scripted::new(vec![
            Reply::json(200, r#"{"ok":true,"relay":"ai-agent-drawer","available":true,"mode":"local","images":2}"#),
            Reply::sse(&["event: delta\ndata: {\"text\":\"ok\"}\n\n"]),
            Reply::sse(&["event: done\ndata: {}\n\n"]),
        ]);
        let c = cfg(url);
        run(&client, &c, &with_images(2)).unwrap();
        let e = run(&client, &c, &with_images(3)).unwrap_err();
        assert_eq!(e.code, ErrorCode::Budget);
        assert_eq!(e.message, "The relay accepts at most 2 images per request (this one has 3). Remove a screenshot, or start a new chat.");
        let sent = client.requests();
        assert_eq!(sent.len(), 2, "one probe, one chat; the refused request is not sent");
        assert_eq!(sent[0].method, "GET");
        assert!(sent[0].headers.contains(&("X-App".to_string(), "1".to_string())));
        assert_eq!(sent[1].headers[0], ("Content-Type".to_string(), "application/json".to_string()));
    }

    #[test]
    fn a_relay_without_images_refuses_them() {
        let url = "https://relay.test/no-images";
        let client = Scripted::new(vec![Reply::json(200, r#"{"ok":true,"relay":"ai-agent-drawer","mode":"local"}"#)]);
        let e = run(&client, &cfg(url), &with_images(1)).unwrap_err();
        assert_eq!(e.code, ErrorCode::Refused);
        assert!(e.message.starts_with("The relay cannot pass images to the model"));
    }

    #[test]
    fn not_a_relay_lets_the_request_report_it() {
        let url = "https://relay.test/static-server";
        let client = Scripted::new(vec![
            Reply::text(200, "text/plain", "<?php echo 1;"),
            Reply::text(404, "text/html", "Not here"),
        ]);
        let e = run(&client, &cfg(url), &with_images(1)).unwrap_err();
        assert_eq!(e.code, ErrorCode::BadEndpoint);
        // Not remembered: the next request asks again.
        assert!(image_support().lock().unwrap().get(url).is_none());
    }

    #[test]
    fn whole_json_reply_and_errors() {
        let client = Scripted::new(vec![
            Reply::json(
                200,
                r#"{"text":"Hi","reasoning":"R","toolCalls":[{"name":"find","arguments":{"q":1}},{"id":"x"}],"usage":{"input":1},"truncated":true}"#,
            ),
            Reply::sse(&["event: error\ndata: {\"code\":\"nope\",\"message\":\"Broken.\",\"detail\":\"d\"}\n\n"]),
        ]);
        let c = cfg("https://relay.test/json");
        let mut events = Vec::new();
        let req = ChatRequest::default();
        let call = ChatCall { cfg: &c, key: "", request: &req, max_tokens: None, temperature: None };
        let r = stream(&client, &call, &mut |e| events.push(e), &CancelToken::new()).unwrap();
        assert_eq!(events.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["R", "Hi"]);
        assert_eq!(r.stream_value(), Value::parse(r#"{"usage":{"input":1},"toolCalls":[{"id":"relay_1","name":"find","arguments":{"q":1}}],"truncated":true}"#).unwrap());
        let e = run(&client, &c, &req).unwrap_err();
        assert_eq!((e.code, e.message.as_str()), (ErrorCode::Malformed, "Broken. (d)"));
        let body = &client.requests()[0].body.clone().unwrap();
        let body = Value::parse(body).unwrap();
        assert_eq!(body.get("maxTokens"), None);
        assert_eq!(body.get("turnId"), None);
    }

    #[test]
    fn models() {
        let client = Scripted::new(vec![
            Reply::json(200, r#"{"ok":true,"models":[{"id":"a","label":"A","loaded":true},{"label":"no id"}]}"#),
            Reply::json(200, r#"{"ok":false}"#),
        ]);
        let c = cfg("https://relay.test/models");
        let m = list_models(&client, &c, "", &CancelToken::new()).unwrap();
        assert_eq!(m, vec![ModelInfo { id: "a".into(), label: "A".into(), loaded: true, vision: None }]);
        let e = list_models(&client, &c, "", &CancelToken::new()).unwrap_err();
        assert_eq!((e.code, e.message.as_str()), (ErrorCode::Network, "The relay could not list models."));
        assert!(list_models(&client, &cfg(""), "", &CancelToken::new()).is_err());
    }
}
