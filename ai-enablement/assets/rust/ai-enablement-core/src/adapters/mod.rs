//! The provider adapters (`adapters/*.js`): one per wire protocol. Every adapter has the same contract:
//!
//! - `build_chat(call)` → the request it sends (`{ url, headers, body }`);
//! - `stream(client, call, on_event, cancel)` → streams text / reasoning events as they arrive and returns the
//!   [`ChatResult`] (`{ usage?, toolCalls, truncated? }`);
//! - `list_models(client, cfg, key, cancel)` → the models the provider offers.
//!
//! Plus each protocol's parsing (streamed chunks, a whole JSON reply, tool calls assembled from their pieces, usage,
//! truncation, in-stream errors), public so a relay server written in Rust can reuse it. [`Adapter`] picks the
//! adapter of a protocol; `crate::client` resolves the settings into an [`AdapterConfig`] and calls it.

pub mod anthropic;
pub mod gemini;
pub mod openai_chat;
pub mod relay;

use crate::chat::{ChatRequest, ChatResult, StreamEvent};
use crate::json::{Map, Value};
use crate::providers::Protocol;
use crate::transport::{AiError, CancelToken, HttpClient, RequestOptions};

/// The adapters: the three provider protocols, and the app relay (which speaks for any provider).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Adapter {
    OpenAiChat,
    AnthropicMessages,
    GeminiGenerate,
    Relay,
}

impl Adapter {
    /// The adapter of a provider protocol (`adapterFor`).
    pub fn for_protocol(protocol: Protocol) -> Adapter {
        match protocol {
            Protocol::OpenAiChat => Adapter::OpenAiChat,
            Protocol::AnthropicMessages => Adapter::AnthropicMessages,
            Protocol::GeminiGenerate => Adapter::GeminiGenerate,
        }
    }

    /// The protocol id (`openai-chat`, `anthropic-messages`, `gemini-generate`, `relay`).
    pub fn id(self) -> &'static str {
        match self {
            Adapter::OpenAiChat => "openai-chat",
            Adapter::AnthropicMessages => "anthropic-messages",
            Adapter::GeminiGenerate => "gemini-generate",
            Adapter::Relay => "relay",
        }
    }

    /// The request this adapter sends for a call (`buildChat`, streaming).
    pub fn build_chat(self, call: &ChatCall<'_>) -> WireRequest {
        match self {
            Adapter::OpenAiChat => openai_chat::build_chat(call),
            Adapter::AnthropicMessages => anthropic::build_chat(call),
            Adapter::GeminiGenerate => gemini::build_chat(call),
            Adapter::Relay => relay::build_chat(call),
        }
    }

    /// Stream one reply (`stream`): events go to `on_event` as they arrive.
    pub fn stream(
        self,
        client: &dyn HttpClient,
        call: &ChatCall<'_>,
        on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> Result<ChatResult, AiError> {
        match self {
            Adapter::OpenAiChat => openai_chat::stream(client, call, on_event, cancel),
            Adapter::AnthropicMessages => anthropic::stream(client, call, on_event, cancel),
            Adapter::GeminiGenerate => gemini::stream(client, call, on_event, cancel),
            Adapter::Relay => relay::stream(client, call, on_event, cancel),
        }
    }

    /// The models the provider offers (`listModels`).
    pub fn list_models(
        self,
        client: &dyn HttpClient,
        cfg: &AdapterConfig,
        key: &str,
        cancel: &CancelToken,
    ) -> Result<Vec<ModelInfo>, AiError> {
        match self {
            Adapter::OpenAiChat => openai_chat::list_models(client, cfg, key, cancel),
            Adapter::AnthropicMessages => anthropic::list_models(client, cfg, key, cancel),
            Adapter::GeminiGenerate => gemini::list_models(client, cfg, key, cancel),
            Adapter::Relay => relay::list_models(client, cfg, key, cancel),
        }
    }
}

/// What an adapter knows about its target: the provider preset and the settings, resolved (`cfg` of
/// `resolveTarget`, see `crate::client::resolve_target`).
#[derive(Debug, Clone, PartialEq)]
pub struct AdapterConfig {
    /// The provider id (`lmstudio`, `openai`, …).
    pub provider: String,
    pub protocol: Protocol,
    pub label: String,
    pub base_url: String,
    /// The model id; empty = the server's default (LM Studio: whatever is loaded).
    pub model: String,
    /// OpenAI-compatible only; empty = `/v1/chat/completions`.
    pub chat_path: String,
    /// Empty = the protocol's default.
    pub models_path: String,
    /// Asked when `models_path` fails (LM Studio's native list, then the OpenAI one).
    pub alt_models_path: String,
    /// The body field for the reply limit; empty = `max_tokens`.
    pub token_field: String,
    pub sends_temperature: bool,
    /// Merged into the body when reasoning is off (OpenAI-compatible), when the provider has such a switch.
    pub reasoning_off: Option<Value>,
    /// `show`, `hide` or `off`.
    pub reasoning: String,
    /// The idle timeout of a request.
    pub timeout_ms: u64,
    /// Set in relay mode only: the relay's address.
    pub relay_url: String,
    /// Extra headers for relay requests (after `X-Requested-With: ai-agent-drawer`; a same-named one replaces it).
    pub relay_headers: Vec<(String, String)>,
}

impl Default for AdapterConfig {
    fn default() -> Self {
        AdapterConfig {
            provider: String::new(),
            protocol: Protocol::OpenAiChat,
            label: String::new(),
            base_url: String::new(),
            model: String::new(),
            chat_path: String::new(),
            models_path: String::new(),
            alt_models_path: String::new(),
            token_field: String::new(),
            sends_temperature: true,
            reasoning_off: None,
            reasoning: "show".into(),
            timeout_ms: 120_000,
            relay_url: String::new(),
            relay_headers: Vec::new(),
        }
    }
}

/// One model call as an adapter takes it: the target, the key, the neutral request and the reply settings.
#[derive(Debug, Clone, Copy)]
pub struct ChatCall<'a> {
    pub cfg: &'a AdapterConfig,
    pub key: &'a str,
    pub request: &'a ChatRequest,
    /// The reply limit; `None` (JS: not a finite number) leaves it out (Anthropic, which requires it, sends 2048).
    pub max_tokens: Option<f64>,
    /// `None` leaves it out.
    pub temperature: Option<f64>,
}

/// A request as an adapter builds it.
#[derive(Debug, Clone, PartialEq)]
pub struct WireRequest {
    pub url: String,
    /// In order; names are compared exactly, as the keys of a JS object are.
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl WireRequest {
    /// The headers as a JSON object (as the JS adapter returns them).
    pub fn headers_value(&self) -> Value {
        Value::Object(self.headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect())
    }

    /// `{ url, headers, body }`.
    pub fn to_value(&self) -> Value {
        object([("url", self.url.as_str().into()), ("headers", self.headers_value()), ("body", self.body.clone())])
    }
}

/// A model a provider offers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ModelInfo {
    pub id: String,
    pub label: String,
    /// Loaded in the server's memory right now (local servers that say so).
    pub loaded: bool,
    /// Whether the model sees images, when the server says (LM Studio does; most servers do not).
    pub vision: Option<bool>,
}

impl ModelInfo {
    /// `{ id, label, loaded, vision? }`.
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("id", self.id.as_str());
        m.insert("label", self.label.as_str());
        m.insert("loaded", self.loaded);
        if let Some(v) = self.vision {
            m.insert("vision", v);
        }
        Value::Object(m)
    }
}

/// One fragment of a streamed reply, as an adapter's chunk parser returns it.
#[derive(Debug, Clone, PartialEq)]
pub enum Fragment {
    /// Text, reasoning, a notice or a status line: shown as it arrives.
    Event(StreamEvent),
    /// A piece of a tool call (OpenAI, Anthropic): the pieces are assembled by index when the reply ends.
    Piece(ToolPiece),
    /// A whole tool call (Gemini sends them whole).
    Call(WholeCall),
}

/// A piece of a streamed tool call.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolPiece {
    /// The call's position as the provider numbers it; `None` when the provider did not say (JS `undefined`).
    pub index: Option<Value>,
    pub id: String,
    pub name: String,
    pub args: String,
}

/// A tool call that arrived whole.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WholeCall {
    pub id: String,
    pub name: String,
    pub arguments: Map,
    /// Gemini's `thoughtSignature`, to send back with the call ('' when there is none).
    pub signature: String,
}

/* ------------------------------------------------------------------------------------------------ helpers */

/// An object from pairs, in order.
pub(crate) fn object<const N: usize>(pairs: [(&str, Value); N]) -> Value {
    Value::Object(pairs.into_iter().collect())
}

/// `headers[name] = value`: a same-named header keeps its place.
pub(crate) fn set_header(headers: &mut Vec<(String, String)>, name: &str, value: &str) {
    match headers.iter_mut().find(|(k, _)| k == name) {
        Some(slot) => slot.1 = value.to_string(),
        None => headers.push((name.to_string(), value.to_string())),
    }
}

/// A present, non-null value (`x ?? …` takes the other side otherwise).
pub(crate) fn nullish(v: Option<&Value>) -> Option<&Value> {
    v.filter(|x| !x.is_null())
}

/// A truthy value as text (`String(x || fallback)`).
pub(crate) fn truthy_text(v: Option<&Value>) -> Option<String> {
    v.filter(|x| x.truthy()).map(Value::js_string)
}

/// `String(x ?? '')`.
pub(crate) fn text_or_empty(v: Option<&Value>) -> String {
    nullish(v).map(Value::js_string).unwrap_or_default()
}

/// One SSE record's data as JSON. Records that are not JSON are skipped, as in JS. A record that is JSON `null` is
/// skipped too (the JS adapters would throw a TypeError reading a field of it).
pub(crate) fn parse_record(data: &str) -> Option<Value> {
    Value::parse(data).ok().filter(|v| !v.is_null())
}

/// The first element of an array field (`Array.isArray(x.f) ? x.f[0] : null`).
pub(crate) fn first_of<'a>(v: &'a Value, field: &str) -> Option<&'a Value> {
    v.get(field).and_then(Value::as_array).and_then(|a| a.first())
}

/// A finite number (`Number.isFinite`).
pub(crate) fn finite(n: Option<f64>) -> Option<f64> {
    n.filter(|x| x.is_finite())
}

/// The role as the wire names it.
pub(crate) fn role_name(role: crate::chat::Role) -> &'static str {
    match role {
        crate::chat::Role::User => "user",
        crate::chat::Role::Assistant => "assistant",
    }
}

/// The transport options of a POST an adapter sends.
pub(crate) fn post_options<'a>(
    call: &ChatCall<'_>,
    wire: &'a WireRequest,
    image_chars: usize,
    relay: bool,
    cancel: &CancelToken,
) -> RequestOptions<'a> {
    RequestOptions {
        method: "",
        url: wire.url.clone(),
        headers: wire.headers.clone(),
        body: Some(&wire.body),
        timeout_ms: call.cfg.timeout_ms,
        secrets: vec![call.key.to_string()],
        relay,
        image_chars,
        cancel: cancel.clone(),
    }
}

/// The transport options of a GET (the model lists).
pub(crate) fn get_options(
    cfg: &AdapterConfig,
    key: &str,
    url: String,
    headers: Vec<(String, String)>,
    cancel: &CancelToken,
) -> RequestOptions<'static> {
    RequestOptions {
        method: "GET",
        url,
        headers,
        body: None,
        timeout_ms: cfg.timeout_ms,
        secrets: vec![key.to_string()],
        relay: false,
        image_chars: 0,
        cancel: cancel.clone(),
    }
}

/// Tool-call pieces assembled by index (a JS `Map` keyed by the index, then sorted by it).
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Assembled {
    pub index: Option<Value>,
    pub id: String,
    pub name: String,
    pub args: String,
}

impl Assembled {
    /// The index as a template literal writes it (`undefined` when there was none).
    pub fn index_text(&self) -> String {
        self.index.as_ref().map_or_else(|| "undefined".to_string(), Value::js_string)
    }
}

/// Group pieces by index (first-seen order), merge each group with `merge`, then sort by index as
/// `(a, b) => a[0] - b[0]` does (a stable sort; an index that is not a number compares equal).
pub(crate) fn assemble(pieces: &[ToolPiece], merge: impl Fn(&mut Assembled, &ToolPiece)) -> Vec<Assembled> {
    let mut out: Vec<Assembled> = Vec::new();
    for p in pieces {
        let at = match out.iter().position(|a| a.index == p.index) {
            Some(i) => i,
            None => {
                out.push(Assembled { index: p.index.clone(), ..Default::default() });
                out.len() - 1
            }
        };
        merge(&mut out[at], p);
    }
    let num = |i: &Option<Value>| i.as_ref().map_or(f64::NAN, Value::js_number);
    out.sort_by(|a, b| {
        let d = num(&a.index) - num(&b.index);
        if d < 0.0 {
            std::cmp::Ordering::Less
        } else if d > 0.0 {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    out
}

/// A scripted HTTP client for unit tests: replies in order, one `read` per chunk, and the requests it was sent.
#[cfg(test)]
pub(crate) mod testing {
    use crate::transport::{CancelToken, HttpClient, HttpFailure, HttpRequest, HttpResponse};
    use std::collections::VecDeque;
    use std::io::Read;
    use std::sync::Mutex;

    pub struct Reply {
        pub status: u16,
        pub content_type: String,
        pub chunks: Vec<String>,
        pub failure: Option<HttpFailure>,
    }

    impl Reply {
        pub fn text(status: u16, content_type: &str, body: &str) -> Reply {
            Reply { status, content_type: content_type.into(), chunks: vec![body.into()], failure: None }
        }
        pub fn json(status: u16, body: &str) -> Reply {
            Reply::text(status, "application/json", body)
        }
        pub fn sse(chunks: &[&str]) -> Reply {
            Reply {
                status: 200,
                content_type: "text/event-stream".into(),
                chunks: chunks.iter().map(|c| c.to_string()).collect(),
                failure: None,
            }
        }
        pub fn fail(failure: HttpFailure) -> Reply {
            Reply { status: 0, content_type: String::new(), chunks: Vec::new(), failure: Some(failure) }
        }
    }

    struct Chunks(VecDeque<Vec<u8>>);

    impl Read for Chunks {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let Some(mut chunk) = self.0.pop_front() else { return Ok(0) };
            if chunk.len() > buf.len() {
                self.0.push_front(chunk.split_off(buf.len()));
            }
            buf[..chunk.len()].copy_from_slice(&chunk);
            Ok(chunk.len())
        }
    }

    #[derive(Default)]
    pub struct Scripted {
        replies: Mutex<VecDeque<Reply>>,
        sent: Mutex<Vec<HttpRequest>>,
    }

    impl Scripted {
        pub fn new(replies: Vec<Reply>) -> Scripted {
            Scripted { replies: Mutex::new(replies.into()), sent: Mutex::default() }
        }
        pub fn requests(&self) -> Vec<HttpRequest> {
            self.sent.lock().unwrap().clone()
        }
    }

    impl HttpClient for Scripted {
        fn send(&self, request: &HttpRequest, _cancel: &CancelToken) -> Result<HttpResponse, HttpFailure> {
            self.sent.lock().unwrap().push(request.clone());
            let Some(r) = self.replies.lock().unwrap().pop_front() else {
                return Err(HttpFailure::Network("no scripted reply left".into()));
            };
            if let Some(f) = r.failure {
                return Err(f);
            }
            let chunks = r.chunks.into_iter().map(String::into_bytes).filter(|c| !c.is_empty()).collect();
            Ok(HttpResponse { status: r.status, content_type: r.content_type, body: Box::new(Chunks(chunks)) })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers_replace_in_place() {
        let mut h = vec![("X-Requested-With".to_string(), "ai-agent-drawer".to_string())];
        set_header(&mut h, "Authorization", "Bearer x");
        set_header(&mut h, "X-Requested-With", "app");
        assert_eq!(h, vec![("X-Requested-With".into(), "app".into()), ("Authorization".into(), "Bearer x".into())]);
    }

    #[test]
    fn pieces_sorted_by_index_first_seen_merged() {
        let p = |i: Option<f64>, args: &str| ToolPiece {
            index: i.map(Value::from),
            args: args.into(),
            ..Default::default()
        };
        let out = assemble(&[p(Some(2.0), "a"), p(Some(0.0), "b"), p(Some(2.0), "c"), p(None, "d")], |a, x| {
            a.args.push_str(&x.args)
        });
        let got: Vec<(String, String)> = out.iter().map(|a| (a.index_text(), a.args.clone())).collect();
        // NaN compares equal to everything, so the undefined index stays where the stable sort leaves it.
        assert_eq!(got[..2], [("0".to_string(), "b".to_string()), ("2".to_string(), "ac".to_string())]);
        assert!(got.contains(&("undefined".to_string(), "d".to_string())));
    }
}
