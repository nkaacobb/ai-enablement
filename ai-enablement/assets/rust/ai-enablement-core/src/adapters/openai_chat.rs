//! OpenAI-compatible Chat Completions (`adapters/openai-chat.js`): LM Studio, Ollama, OpenAI, DeepSeek, OpenRouter and
//! any custom server.
//!
//! - `POST {base}{chatPath}` with `{ model?, messages[system, user, assistant…], stream, max_tokens |
//!   max_completion_tokens, temperature?, …the provider's "reasoning off" switch }`. An empty model is sent without a
//!   `model` field: LM Studio then answers with whatever model is loaded.
//! - Stream: `data: {choices:[{delta:{content, reasoning_content | reasoning}}]}` … `data: [DONE]`.
//! - Tools: `body.tools [{type:'function', function:{name, description, parameters}}]`; the reply streams
//!   `delta.tool_calls [{index, id, function:{name, arguments (JSON, in pieces)}}]`; the exchange goes back as an
//!   assistant message with `tool_calls`, then one `{role:'tool', tool_call_id, content}` per result.
//! - `finish_reason: 'length'`: the reply stopped at max tokens, so the call being written may be cut off.
//! - Images: a user message with images becomes content parts `[{type:'text'}, {type:'image_url', image_url:{url:
//!   'data:<mime>;base64,…'}}]`. Tool messages are text only, so an image a tool returned follows the tool messages of
//!   its round in a user message.
//! - Models: `GET {base}{modelsPath}` → `{ data:[{id}] }` (LM Studio native: `{ models:[{key, type,
//!   loaded_instances}] }`).

use super::{
    assemble, finite, first_of, get_options, nullish, object, parse_record, post_options, role_name, truthy_text,
    AdapterConfig, ChatCall, Fragment, ModelInfo, ToolPiece, WireRequest,
};
use crate::chat::{data_url, image_chars, normalize_messages, ChatResult, EventKind, Image, StreamEvent, ToolTurn};
use crate::json::{number_to_string, to_json, Map, Value};
use crate::tools::{tool_call, ToolCall};
use crate::transport::{join_url, request_json, request_stream, AiError, CancelToken, ErrorCode, HttpClient, Streamed};

fn auth(key: &str) -> Vec<(String, String)> {
    if key.is_empty() {
        Vec::new()
    } else {
        vec![("Authorization".to_string(), format!("Bearer {key}"))]
    }
}

/// The current question's tool exchange, in chat-completions form (`toolTurnMessages`).
pub fn tool_turn_messages(turns: &[ToolTurn]) -> Vec<Value> {
    let mut out = Vec::new();
    for turn in turns {
        let calls: Vec<Value> = turn
            .calls
            .iter()
            .map(|c| {
                object([
                    ("id", c.id.as_str().into()),
                    ("type", "function".into()),
                    (
                        "function",
                        object([
                            ("name", c.name.as_str().into()),
                            ("arguments", to_json(&Value::Object(c.arguments.clone())).into()),
                        ]),
                    ),
                ])
            })
            .collect();
        out.push(object([
            ("role", "assistant".into()),
            ("content", turn.text.as_str().into()),
            ("tool_calls", Value::Array(calls)),
        ]));
        for r in &turn.results {
            out.push(object([
                ("role", "tool".into()),
                ("tool_call_id", r.id.as_str().into()),
                ("content", r.content.as_str().into()),
            ]));
        }
        for r in &turn.results {
            if !r.images.is_empty() {
                out.push(object([
                    ("role", "user".into()),
                    ("content", with_images("[The image returned by the tool call above]", &r.images)),
                ]));
            }
        }
    }
    out
}

/// Text plus images as chat-completions content parts (`withImages`).
pub fn with_images(text: &str, images: &[Image]) -> Value {
    let mut parts =
        vec![object([("type", "text".into()), ("text", if text.is_empty() { "(image)" } else { text }.into())])];
    for i in images {
        parts.push(object([("type", "image_url".into()), ("image_url", object([("url", data_url(i).into())]))]));
    }
    Value::Array(parts)
}

/// The request (`buildChat`, streaming).
pub fn build_chat(call: &ChatCall<'_>) -> WireRequest {
    let cfg = call.cfg;
    let req = call.request;
    let mut messages = Vec::new();
    if !req.system.is_empty() {
        messages.push(object([("role", "system".into()), ("content", req.system.as_str().into())]));
    }
    for m in normalize_messages(&req.messages) {
        let content = if m.images.is_empty() { Value::from(m.content) } else { with_images(&m.content, &m.images) };
        messages.push(object([("role", role_name(m.role).into()), ("content", content)]));
    }
    messages.extend(tool_turn_messages(&req.tool_turns));
    let mut body = Map::new();
    body.insert("messages", messages);
    body.insert("stream", true);
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                object([
                    ("type", "function".into()),
                    (
                        "function",
                        object([
                            ("name", t.name.as_str().into()),
                            ("description", t.description.as_str().into()),
                            ("parameters", t.parameters.clone()),
                        ]),
                    ),
                ])
            })
            .collect();
        body.insert("tools", tools);
        body.insert("tool_choice", "auto");
    }
    if !cfg.model.is_empty() {
        body.insert("model", cfg.model.as_str());
    }
    if let Some(n) = finite(call.max_tokens) {
        body.insert(if cfg.token_field.is_empty() { "max_tokens" } else { cfg.token_field.as_str() }, n);
    }
    if let Some(t) = finite(call.temperature) {
        if cfg.sends_temperature {
            body.insert("temperature", t);
        }
    }
    if cfg.reasoning == "off" {
        if let Some(Value::Object(off)) = &cfg.reasoning_off {
            for (k, v) in off.iter() {
                body.insert(k.as_str(), v.clone());
            }
        }
    }
    let path = if cfg.chat_path.is_empty() { "/v1/chat/completions" } else { cfg.chat_path.as_str() };
    WireRequest { url: join_url(&cfg.base_url, path), headers: auth(call.key), body: Value::Object(body) }
}

/// Message content as text: a string, or the text parts of an array (`textOf`).
fn text_of(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts.iter().filter_map(|p| p.get("text").and_then(Value::as_str)).collect(),
        _ => String::new(),
    }
}

/// One streamed chunk → fragments (`parseChunk`). Fails on an in-stream error object, or a filtered reply.
pub fn parse_chunk(json: &Value) -> Result<Vec<Fragment>, AiError> {
    if let Some(err) = json.get("error").filter(|e| e.truthy()) {
        let msg = match err {
            Value::String(s) => s.clone(),
            other => truthy_text(other.get("message")).unwrap_or_else(|| "The model returned an error.".into()),
        };
        let code =
            if msg.to_ascii_lowercase().contains("model") { ErrorCode::MissingModel } else { ErrorCode::Refused };
        return Err(AiError::new(code, msg));
    }
    let choice = first_of(json, "choices");
    let d = choice.and_then(|c| c.get("delta").filter(|v| v.truthy()).or_else(|| c.get("message")));
    let mut out = Vec::new();
    if let Some(d) = d.filter(|d| matches!(d, Value::Object(_) | Value::Array(_))) {
        if let Some(Value::String(r)) = nullish(d.get("reasoning_content")).or_else(|| d.get("reasoning")) {
            if !r.is_empty() {
                out.push(Fragment::Event(StreamEvent::new(EventKind::Reasoning, r.as_str())));
            }
        }
        let text = text_of(d.get("content"));
        if !text.is_empty() {
            out.push(Fragment::Event(StreamEvent::new(EventKind::Text, text)));
        }
        if let Some(Value::String(r)) = d.get("refusal") {
            if !r.is_empty() {
                out.push(Fragment::Event(StreamEvent::new(EventKind::Text, r.as_str())));
            }
        }
        if let Some(Value::Array(calls)) = d.get("tool_calls") {
            for (i, tc) in calls.iter().enumerate() {
                if !matches!(tc, Value::Object(_) | Value::Array(_)) {
                    continue;
                }
                let index = tc.get("index").and_then(Value::as_f64).filter(|n| n.is_finite() && n.fract() == 0.0);
                let f = tc.get("function");
                let args = match f.and_then(|f| f.get("arguments")) {
                    Some(Value::String(s)) => s.clone(),
                    Some(v @ (Value::Object(_) | Value::Array(_))) => to_json(v),
                    _ => String::new(),
                };
                out.push(Fragment::Piece(ToolPiece {
                    index: Some(Value::from(index.unwrap_or(i as f64))),
                    id: truthy_text(tc.get("id")).unwrap_or_default(),
                    name: truthy_text(f.and_then(|f| f.get("name"))).unwrap_or_default(),
                    args,
                }));
            }
        }
    }
    if choice.and_then(|c| c.get("finish_reason")).and_then(Value::as_str) == Some("content_filter") {
        return Err(AiError::new(ErrorCode::Refused, "The provider filtered the reply."));
    }
    Ok(out)
}

/// A whole (non-streamed) reply, parsed (`parseFull`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Full {
    pub text: String,
    pub reasoning: String,
    pub usage: Option<Value>,
    pub tool_calls: Vec<ToolCall>,
    pub truncated: bool,
}

/// A whole (non-streamed) completion (`parseFull`).
pub fn parse_full(json: &Value) -> Result<Full, AiError> {
    let choice = first_of(json, "choices");
    let msg = choice.and_then(|c| c.get("message")).filter(|m| matches!(m, Value::Object(_) | Value::Array(_)));
    let Some(msg) = msg else { return Err(AiError::new(ErrorCode::Malformed, "The reply had no message.")) };
    let reasoning = match nullish(msg.get("reasoning_content")).or_else(|| msg.get("reasoning")) {
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let tool_calls = msg
        .get("tool_calls")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .enumerate()
                .map(|(i, tc)| {
                    let f = tc.get("function");
                    tool_call(
                        truthy_text(tc.get("id")).unwrap_or_else(|| format!("call_{}", i + 1)),
                        truthy_text(f.and_then(|f| f.get("name"))).unwrap_or_default(),
                        f.and_then(|f| f.get("arguments")).unwrap_or(&Value::Null),
                    )
                })
                .filter(|c| !c.name.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let mut text = text_of(msg.get("content"));
    if text.is_empty() {
        text = truthy_text(msg.get("refusal")).unwrap_or_default();
    }
    Ok(Full {
        text,
        reasoning,
        usage: usage_of(json.get("usage")),
        tool_calls,
        truncated: choice.and_then(|c| c.get("finish_reason")).and_then(Value::as_str) == Some("length"),
    })
}

/// Assemble streamed tool-call pieces (keyed by index) into complete calls (`collectToolCalls`).
pub fn collect_tool_calls(pieces: &[ToolPiece]) -> Vec<ToolCall> {
    assemble(pieces, |cur, p| {
        if !p.id.is_empty() {
            cur.id.clone_from(&p.id);
        }
        if !p.name.is_empty() {
            cur.name = if !cur.name.is_empty() && cur.name != p.name {
                format!("{}{}", cur.name, p.name)
            } else {
                p.name.clone()
            };
        }
        cur.args.push_str(&p.args);
    })
    .into_iter()
    .map(|c| {
        let id = if c.id.is_empty() {
            format!("call_{}", number_to_string(c.index.as_ref().map_or(f64::NAN, Value::js_number) + 1.0))
        } else {
            c.id
        };
        tool_call(id, c.name, &Value::String(c.args))
    })
    .filter(|c| !c.name.is_empty())
    .collect()
}

/// `{ input: prompt_tokens, output: completion_tokens }` (`usageOf`; fields the server left out stay out).
fn usage_of(u: Option<&Value>) -> Option<Value> {
    let u = u.filter(|u| u.truthy())?;
    let mut m = Map::new();
    if let Some(v) = u.get("prompt_tokens") {
        m.insert("input", v.clone());
    }
    if let Some(v) = u.get("completion_tokens") {
        m.insert("output", v.clone());
    }
    Some(Value::Object(m))
}

/// The model list: LM Studio's native `{ models:[…] }` and the OpenAI `{ data:[…] }` shape; loaded models first
/// (`parseModels`).
pub fn parse_models(json: &Value) -> Vec<ModelInfo> {
    let mut out: Vec<ModelInfo> = Vec::new();
    let mut add = |id: Option<String>, label: Option<String>, loaded: bool, vision: Option<bool>| {
        let Some(id) = id.filter(|i| !i.is_empty()) else { return };
        if out.iter().any(|m| m.id == id) {
            return;
        }
        let label = label.filter(|l| !l.is_empty()).unwrap_or_else(|| id.clone());
        out.push(ModelInfo { id, label, loaded, vision });
    };
    let first_truthy = |m: &Value, keys: &[&str]| keys.iter().find_map(|k| truthy_text(m.get(k)));
    // LM Studio native: { models:[{ type:'llm'|'embedding', key, display_name, loaded_instances:[], capabilities:{ vision } }] }
    for m in json.get("models").and_then(Value::as_array).map_or(&[][..], Vec::as_slice) {
        let ty = m.get("type").filter(|t| t.truthy());
        if !m.truthy() || ty.is_some_and(|t| t.as_str() != Some("llm") && t.as_str() != Some("vlm")) {
            continue;
        }
        let loaded = m.get("loaded_instances").and_then(Value::as_array).is_some_and(|a| !a.is_empty());
        let vision = match nullish(m.get("capabilities").and_then(|c| c.get("vision"))) {
            Some(v) => v.as_bool(),
            None => (ty.and_then(Value::as_str) == Some("vlm")).then_some(true),
        };
        add(first_truthy(m, &["key", "id", "name"]), truthy_text(m.get("display_name")), loaded, vision);
    }
    // OpenAI shape: { data:[{ id }] } (LM Studio v0 adds type 'llm'|'vlm' and state)
    for m in json.get("data").and_then(Value::as_array).map_or(&[][..], Vec::as_slice) {
        let ty = m.get("type").filter(|t| t.truthy());
        if !m.truthy() || ty.is_some_and(|t| t.js_string().to_ascii_lowercase().contains("embed")) {
            continue;
        }
        let name = m.get("name").filter(|n| n.truthy());
        let label = match name {
            Some(n) if Some(n) != m.get("id") => Some(n.js_string()),
            _ => m.get("id").filter(|i| !i.is_null()).map(Value::js_string),
        };
        let vision = match m.get("type").and_then(Value::as_str) {
            Some("vlm") => Some(true),
            Some("llm") if m.get("state").is_some() => Some(false),
            _ => None,
        };
        let loaded = m.get("state").and_then(Value::as_str) == Some("loaded");
        add(first_truthy(m, &["id", "name"]), label, loaded, vision);
    }
    let (mut loaded, rest): (Vec<_>, Vec<_>) = out.into_iter().partition(|m| m.loaded);
    loaded.extend(rest);
    loaded
}

/// Stream one reply (`stream`).
pub fn stream(
    client: &dyn HttpClient,
    call: &ChatCall<'_>,
    on_event: &mut dyn FnMut(StreamEvent),
    cancel: &CancelToken,
) -> Result<ChatResult, AiError> {
    let req = call.request;
    let wire = build_chat(call);
    let opts = post_options(call, &wire, image_chars(&req.messages, &req.tool_turns), false, cancel);
    let mut usage = None;
    let mut finish = Value::Null;
    let mut pieces = Vec::new();
    let res = request_stream(client, &opts, &mut |rec| {
        if rec.data.is_empty() || rec.data == "[DONE]" {
            return Ok(());
        }
        let Some(json) = parse_record(&rec.data) else { return Ok(()) };
        if let Some(u) = json.get("usage").filter(|u| u.truthy()) {
            usage = usage_of(Some(u));
        }
        if let Some(reason) = first_of(&json, "choices").and_then(|c| c.get("finish_reason")).filter(|r| r.truthy()) {
            finish = reason.clone();
        }
        for f in parse_chunk(&json)? {
            match f {
                Fragment::Piece(p) => pieces.push(p),
                Fragment::Event(e) => on_event(e),
                Fragment::Call(_) => {}
            }
        }
        Ok(())
    })?;
    if let Streamed::Json(json) = res {
        let full = parse_full(&json)?;
        if !full.reasoning.is_empty() {
            on_event(StreamEvent::new(EventKind::Reasoning, full.reasoning));
        }
        if !full.text.is_empty() {
            on_event(StreamEvent::new(EventKind::Text, full.text));
        }
        return Ok(ChatResult {
            usage: full.usage,
            tool_calls: full.tool_calls,
            truncated: full.truncated,
            ..Default::default()
        });
    }
    Ok(ChatResult {
        usage,
        tool_calls: collect_tool_calls(&pieces),
        truncated: finish.as_str() == Some("length"),
        ..Default::default()
    })
}

/// The models the server offers (`listModels`): the models path, then the alternative one (LM Studio's native list
/// first, then the OpenAI one) unless the first failed on the key or was cancelled.
pub fn list_models(
    client: &dyn HttpClient,
    cfg: &AdapterConfig,
    key: &str,
    cancel: &CancelToken,
) -> Result<Vec<ModelInfo>, AiError> {
    let get =
        |path: &str| request_json(client, &get_options(cfg, key, join_url(&cfg.base_url, path), auth(key), cancel));
    let path = if cfg.models_path.is_empty() { "/v1/models" } else { cfg.models_path.as_str() };
    match get(path) {
        Ok(json) => Ok(parse_models(&json)),
        Err(e) => {
            if cfg.alt_models_path.is_empty() || e.code == ErrorCode::Auth || e.code == ErrorCode::Cancelled {
                return Err(e);
            }
            Ok(parse_models(&get(&cfg.alt_models_path)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value {
        Value::parse(s).unwrap()
    }

    #[test]
    fn chunk_errors_and_refusals() {
        let e = parse_chunk(&v(r#"{"error":"Model not loaded"}"#)).unwrap_err();
        assert_eq!((e.code, e.message.as_str()), (ErrorCode::MissingModel, "Model not loaded"));
        // The default message names "model" itself, so JS files it as missing-model too.
        let e = parse_chunk(&v(r#"{"error":{"code":1}}"#)).unwrap_err();
        assert_eq!((e.code, e.message.as_str()), (ErrorCode::MissingModel, "The model returned an error."));
        let e = parse_chunk(&v(r#"{"error":{"message":"Context too long"}}"#)).unwrap_err();
        assert_eq!(e.code, ErrorCode::Refused);
        let e =
            parse_chunk(&v(r#"{"choices":[{"delta":{"content":"x"},"finish_reason":"content_filter"}]}"#)).unwrap_err();
        assert_eq!(e.message, "The provider filtered the reply.");
        let f =
            parse_chunk(&v(r#"{"choices":[{"delta":{"refusal":"No.","reasoning_content":null,"reasoning":"R"}}]}"#))
                .unwrap();
        assert_eq!(
            f,
            vec![
                Fragment::Event(StreamEvent::new(EventKind::Reasoning, "R")),
                Fragment::Event(StreamEvent::new(EventKind::Text, "No."))
            ]
        );
    }

    #[test]
    fn pieces_join_names_and_number_missing_ids() {
        let p = |i: f64, id: &str, name: &str, args: &str| ToolPiece {
            index: Some(Value::from(i)),
            id: id.into(),
            name: name.into(),
            args: args.into(),
        };
        let calls =
            collect_tool_calls(&[p(1.0, "", "set_", "{\"a\""), p(1.0, "", "size", ":1}"), p(0.0, "", "", "{}")]);
        // index 0 has no name: dropped; index 1 is "set_size", numbered call_2.
        assert_eq!(calls.len(), 1);
        assert_eq!((calls[0].id.as_str(), calls[0].name.as_str()), ("call_2", "set_size"));
        assert_eq!(calls[0].arguments.get("a"), Some(&Value::from(1.0)));
    }

    #[test]
    fn models_loaded_first_with_vision() {
        let json = v(
            r#"{"models":[{"type":"embedding","key":"e"},{"type":"llm","key":"a","display_name":"A"},{"type":"vlm","key":"b","loaded_instances":[{}]}],
            "data":[{"id":"a"},{"id":"c","type":"llm","state":"not-loaded"},{"id":"d","name":"D!"},{"id":"x-embed","type":"embeddings"}]}"#,
        );
        let got: Vec<Value> = parse_models(&json).iter().map(ModelInfo::to_value).collect();
        assert_eq!(
            Value::Array(got),
            v(r#"[{"id":"b","label":"b","loaded":true,"vision":true},{"id":"a","label":"A","loaded":false},
                {"id":"c","label":"c","loaded":false,"vision":false},{"id":"d","label":"D!","loaded":false}]"#)
        );
    }

    #[test]
    fn whole_reply_refusal_and_missing_message() {
        let full = parse_full(&v(r#"{"choices":[{"message":{"content":"","refusal":"I can't."}}]}"#)).unwrap();
        assert_eq!((full.text.as_str(), full.usage.is_none()), ("I can't.", true));
        assert_eq!(parse_full(&v(r#"{"choices":[]}"#)).unwrap_err().code, ErrorCode::Malformed);
    }
}
