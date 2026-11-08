//! The neutral request every adapter translates (`spec/behaviour.md` §9) and what comes back: messages with images
//! ([`RequestMessage`]), tool specs, the current question's tool exchange ([`ToolTurn`]), stream events and the
//! result. Plus the transcript shaping every adapter shares (`core/messages.js`): user/assistant turns only, no empty
//! turns, same-role turns merged, the first turn from the user, and images that are what they say.

use crate::json::{Map, Value};
use crate::settings::Settings;
use crate::transport::{AiError, CancelToken};
use regex::Regex;
use std::sync::OnceLock;

pub use crate::conversation::{Image, RequestMessage, Role};
pub use crate::tools::{ToolCall, ToolResult, ToolSpec, ToolTurn, WireCall};

/// `data:<mime>;base64,<data>`.
pub fn data_url(image: &Image) -> String {
    format!("data:{};base64,{}", image.mime, image.data)
}

/// Images that are what they say: an allowed type and base64 data of at least 16 characters (`cleanImages`).
pub fn clean_images(list: &[Image]) -> Vec<Image> {
    static MIME: OnceLock<Regex> = OnceLock::new();
    static B64: OnceLock<Regex> = OnceLock::new();
    let mime = MIME.get_or_init(|| Regex::new(r"^image/(png|jpeg|webp|gif)$").expect("mime"));
    let b64 = B64.get_or_init(|| Regex::new(r"^[A-Za-z0-9+/]+={0,2}$").expect("base64"));
    list.iter()
        .filter(|i| mime.is_match(&i.mime) && crate::text::utf16_len(&i.data) >= 16 && b64.is_match(&i.data))
        .cloned()
        .collect()
}

/// What every adapter sends: trimmed, no empty turns, same-role turns merged (`\n\n`), user images cleaned, and the
/// first turn from the user (`normalizeMessages`). Turns of other roles must be left out before (see
/// [`messages_from_value`]).
pub fn normalize_messages(list: &[RequestMessage]) -> Vec<RequestMessage> {
    let mut out: Vec<RequestMessage> = Vec::new();
    for m in list {
        let content = crate::text::js_trim(&m.content).to_string();
        let images = if m.role == Role::User { clean_images(&m.images) } else { Vec::new() };
        if content.is_empty() && images.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.role == m.role => {
                last.content = [last.content.as_str(), content.as_str()]
                    .iter()
                    .filter(|s| !s.is_empty())
                    .copied()
                    .collect::<Vec<_>>()
                    .join("\n\n");
                last.images.extend(images);
            }
            _ => out.push(RequestMessage { role: m.role, content, images }),
        }
    }
    if out.first().is_some_and(|m| m.role == Role::Assistant) {
        out.insert(
            0,
            RequestMessage { role: Role::User, content: "(The conversation continues.)".into(), images: Vec::new() },
        );
    }
    out
}

/// Messages from JSON as the JS runtime takes them: only `user` and `assistant` turns (others are dropped),
/// `content` as text, `images` that have string data.
pub fn messages_from_value(v: &Value) -> Vec<RequestMessage> {
    let Some(list) = v.as_array() else { return Vec::new() };
    list.iter()
        .filter_map(|m| {
            let role = match m.get("role").and_then(|r| r.as_str()) {
                Some("user") => Role::User,
                Some("assistant") => Role::Assistant,
                _ => return None,
            };
            let content = m.get("content").filter(|c| !c.is_null()).map(|c| c.js_string()).unwrap_or_default();
            let images = m
                .get("images")
                .and_then(|i| i.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|i| {
                            let data = i.get("data")?.as_str()?.to_string();
                            let mime = i.get("mime").filter(|x| x.truthy()).map(|x| x.js_string()).unwrap_or_default();
                            Some(Image { mime, data })
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(RequestMessage { role, content, images })
        })
        .collect()
}

/// Characters of image data in a request (allowed on top of the text cap): `imageChars`.
pub fn image_chars(messages: &[RequestMessage], tool_turns: &[ToolTurn]) -> usize {
    let m: usize = messages.iter().flat_map(|m| &m.images).map(|i| crate::text::utf16_len(&i.data)).sum();
    let t: usize = tool_turns
        .iter()
        .flat_map(|t| &t.results)
        .flat_map(|r| &r.images)
        .map(|i| crate::text::utf16_len(&i.data))
        .sum();
    m + t
}

/// The neutral request: what a session asks a model.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatRequest {
    pub system: String,
    pub messages: Vec<RequestMessage>,
    pub tools: Vec<ToolSpec>,
    pub tool_turns: Vec<ToolTurn>,
    /// One id per question (a public relay counts one question per chain of tool steps).
    pub turn_id: String,
}

impl ChatRequest {
    /// `{ system, messages, tools, toolTurns }`, as the `tool-loop` vectors record it.
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("system", self.system.as_str());
        m.insert("messages", crate::json::to_value(&self.messages));
        m.insert("tools", crate::json::to_value(&self.tools));
        m.insert("toolTurns", crate::json::to_value(&self.tool_turns));
        Value::Object(m)
    }
}

/// A fragment of a streamed reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Text,
    Reasoning,
    Notice,
    Status,
}

impl EventKind {
    pub fn id(self) -> &'static str {
        match self {
            EventKind::Text => "text",
            EventKind::Reasoning => "reasoning",
            EventKind::Notice => "notice",
            EventKind::Status => "status",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamEvent {
    pub kind: EventKind,
    pub text: String,
}

impl StreamEvent {
    pub fn new(kind: EventKind, text: impl Into<String>) -> StreamEvent {
        StreamEvent { kind, text: text.into() }
    }

    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("type", self.kind.id());
        m.insert("text", self.text.as_str());
        Value::Object(m)
    }
}

/// A model call as a value: `{ id, name, arguments, raw?, argsError?, signature? }` (as `toolCall` returns it).
pub fn tool_call_value(c: &ToolCall) -> Value {
    let mut m = Map::new();
    m.insert("id", c.id.as_str());
    m.insert("name", c.name.as_str());
    m.insert("arguments", Value::Object(c.arguments.clone()));
    if let Some(r) = &c.raw {
        m.insert("raw", r.as_str());
    }
    if let Some(e) = &c.args_error {
        m.insert("argsError", e.as_str());
    }
    if let Some(s) = &c.signature {
        m.insert("signature", s.as_str());
    }
    Value::Object(m)
}

/// What one model call returned (besides the streamed events).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatResult {
    /// The provider's token counts as the adapter reports them (`{ input, output }`), when it does.
    pub usage: Option<Value>,
    pub tool_calls: Vec<ToolCall>,
    /// The reply stopped at the max-tokens limit.
    pub truncated: bool,
    /// The provider that answered (its id and label), and the model.
    pub provider: String,
    pub label: String,
    pub model: String,
    /// The fallback provider answered.
    pub fell_back: bool,
}

impl ChatResult {
    /// `{ usage?, toolCalls, truncated? }`, as an adapter's `stream()` returns it.
    pub fn stream_value(&self) -> Value {
        let mut m = Map::new();
        if let Some(u) = &self.usage {
            m.insert("usage", u.clone());
        }
        m.insert("toolCalls", Value::Array(self.tool_calls.iter().map(tool_call_value).collect()));
        if self.truncated {
            m.insert("truncated", true);
        }
        Value::Object(m)
    }
}

/// A model the session talks to: the configured provider through an HTTP client (`crate::client::HttpModel`), or a
/// scripted one in tests. Events are delivered as they stream; the result comes at the end.
pub trait ChatModel {
    fn chat(
        &mut self,
        request: &ChatRequest,
        settings: &Settings,
        on_event: &mut dyn FnMut(StreamEvent),
        cancel: &CancelToken,
    ) -> Result<ChatResult, AiError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize() {
        let v = Value::parse(r#"[{"role":"assistant","content":"Hi."},{"role":"system","content":"x"},{"role":"user","content":"  "},{"role":"user","content":"A"},{"role":"user","content":"B","images":[{"mime":"image/jpeg","data":"QUJDREVGR0hJSktMTU5PUA=="},{"mime":"text/plain","data":"QUJDREVGR0hJSktMTU5PUA=="}]},{"role":"assistant","content":"C"}]"#).unwrap();
        let out = normalize_messages(&messages_from_value(&v));
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].content, "(The conversation continues.)");
        assert_eq!(out[2].content, "A\n\nB");
        assert_eq!(out[2].images.len(), 1);
    }
}
