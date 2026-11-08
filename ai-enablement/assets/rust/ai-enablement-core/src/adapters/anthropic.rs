//! Anthropic Messages API (`adapters/anthropic.js`).
//!
//! - `POST {base}/v1/messages`, headers `x-api-key`, `anthropic-version: 2023-06-01` and
//!   `anthropic-dangerous-direct-browser-access: true` (the JS runtime calls from a browser; sent here too, so both
//!   runtimes send the same request).
//! - Body `{ model, max_tokens (required), system, messages (user first, alternating), stream, thinking:{type:
//!   'disabled'} when reasoning is off }`. Temperature is not sent: current models only accept the default.
//! - Stream events: `content_block_delta {delta:{type:'text_delta', text} | {type:'thinking_delta', thinking}}`,
//!   `message_delta {usage}`, `error {error:{message}}`.
//! - Tools: `body.tools [{name, description, input_schema}]`; the reply streams `content_block_start {type:
//!   'tool_use', id, name}` and `input_json_delta` pieces; the exchange goes back as an assistant message with
//!   `tool_use` blocks and a user message with `tool_result` blocks.
//! - `stop_reason: 'max_tokens'`: the reply was cut off.
//! - Images: a user message with images becomes `[{type:'image', source:{type:'base64', media_type, data}},
//!   {type:'text'}]`; an image a tool returned goes inside its `tool_result` block.
//! - Models: `GET {base}/v1/models` → `{ data:[{ id, display_name }] }`.

use super::{
    assemble, finite, get_options, object, parse_record, post_options, role_name, truthy_text, AdapterConfig, ChatCall,
    Fragment, ModelInfo, ToolPiece, WireRequest,
};
use crate::chat::{image_chars, normalize_messages, ChatResult, EventKind, Image, StreamEvent, ToolTurn};
use crate::json::{Map, Value};
use crate::tools::{tool_call, ToolCall};
use crate::transport::{join_url, request_json, request_stream, AiError, CancelToken, ErrorCode, HttpClient, Streamed};

pub const VERSION: &str = "2023-06-01";

fn headers(key: &str) -> Vec<(String, String)> {
    vec![
        ("x-api-key".to_string(), key.to_string()),
        ("anthropic-version".to_string(), VERSION.to_string()),
        ("anthropic-dangerous-direct-browser-access".to_string(), "true".to_string()),
    ]
}

fn image_block(i: &Image) -> Value {
    object([
        ("type", "image".into()),
        (
            "source",
            object([
                ("type", "base64".into()),
                ("media_type", i.mime.as_str().into()),
                ("data", i.data.as_str().into()),
            ]),
        ),
    ])
}

/// The current question's tool exchange, as Messages API turns (`toolTurnMessages`).
pub fn tool_turn_messages(turns: &[ToolTurn]) -> Vec<Value> {
    let mut out = Vec::new();
    for turn in turns {
        let mut content = Vec::new();
        if !turn.text.is_empty() {
            content.push(object([("type", "text".into()), ("text", turn.text.as_str().into())]));
        }
        for c in &turn.calls {
            content.push(object([
                ("type", "tool_use".into()),
                ("id", c.id.as_str().into()),
                ("name", c.name.as_str().into()),
                ("input", Value::Object(c.arguments.clone())),
            ]));
        }
        out.push(object([("role", "assistant".into()), ("content", Value::Array(content))]));
        let results: Vec<Value> = turn
            .results
            .iter()
            .map(|r| {
                let content = if r.images.is_empty() {
                    Value::from(r.content.as_str())
                } else {
                    let text = if r.content.is_empty() { "Done." } else { r.content.as_str() };
                    let mut blocks = vec![object([("type", "text".into()), ("text", text.into())])];
                    blocks.extend(r.images.iter().map(image_block));
                    Value::Array(blocks)
                };
                object([("type", "tool_result".into()), ("tool_use_id", r.id.as_str().into()), ("content", content)])
            })
            .collect();
        out.push(object([("role", "user".into()), ("content", Value::Array(results))]));
    }
    out
}

/// The request (`buildChat`, streaming).
pub fn build_chat(call: &ChatCall<'_>) -> WireRequest {
    let cfg = call.cfg;
    let req = call.request;
    let mut messages: Vec<Value> = normalize_messages(&req.messages)
        .into_iter()
        .map(|m| {
            let content = if m.images.is_empty() {
                Value::from(m.content)
            } else {
                let mut blocks: Vec<Value> = m.images.iter().map(image_block).collect();
                let text = if m.content.is_empty() { "(image)".to_string() } else { m.content };
                blocks.push(object([("type", "text".into()), ("text", text.into())]));
                Value::Array(blocks)
            };
            object([("role", role_name(m.role).into()), ("content", content)])
        })
        .collect();
    messages.extend(tool_turn_messages(&req.tool_turns));
    let mut body = Map::new();
    body.insert("model", cfg.model.as_str());
    body.insert("max_tokens", finite(call.max_tokens).unwrap_or(2048.0));
    body.insert("messages", messages);
    body.insert("stream", true);
    if !req.tools.is_empty() {
        let tools: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                object([
                    ("name", t.name.as_str().into()),
                    ("description", t.description.as_str().into()),
                    ("input_schema", t.parameters.clone()),
                ])
            })
            .collect();
        body.insert("tools", tools);
    }
    if !req.system.is_empty() {
        body.insert("system", req.system.as_str());
    }
    if cfg.reasoning == "off" {
        body.insert("thinking", object([("type", "disabled".into())]));
    }
    WireRequest { url: join_url(&cfg.base_url, "/v1/messages"), headers: headers(call.key), body: Value::Object(body) }
}

/// One stream event → fragments (`parseEvent`). Fails on an error event (`overloaded_error` is a rate limit).
pub fn parse_event(json: &Value) -> Result<Vec<Fragment>, AiError> {
    let ty = json.get("type").and_then(Value::as_str);
    let error = json.get("error");
    if ty == Some("error") || error.is_some_and(Value::truthy) {
        let e = error.unwrap_or(&Value::Null);
        let code = if e.get("type").and_then(Value::as_str) == Some("overloaded_error") {
            ErrorCode::RateLimit
        } else {
            ErrorCode::Refused
        };
        return Err(AiError::new(
            code,
            truthy_text(e.get("message")).unwrap_or_else(|| "Anthropic returned an error.".into()),
        ));
    }
    if ty == Some("content_block_start") {
        let block = json.get("content_block");
        if block.and_then(|b| b.get("type")).and_then(Value::as_str) == Some("tool_use") {
            return Ok(vec![Fragment::Piece(ToolPiece {
                index: json.get("index").cloned(),
                id: truthy_text(block.and_then(|b| b.get("id"))).unwrap_or_default(),
                name: truthy_text(block.and_then(|b| b.get("name"))).unwrap_or_default(),
                args: String::new(),
            })]);
        }
    }
    if ty != Some("content_block_delta") {
        return Ok(Vec::new());
    }
    let empty = Value::object();
    let d = json.get("delta").filter(|d| d.truthy()).unwrap_or(&empty);
    let text = |k: &str| truthy_text(d.get(k));
    Ok(match d.get("type").and_then(Value::as_str) {
        Some("text_delta") if text("text").is_some() => {
            vec![Fragment::Event(StreamEvent::new(EventKind::Text, text("text").unwrap_or_default()))]
        }
        Some("thinking_delta") if text("thinking").is_some() => {
            vec![Fragment::Event(StreamEvent::new(EventKind::Reasoning, text("thinking").unwrap_or_default()))]
        }
        Some("input_json_delta") => vec![Fragment::Piece(ToolPiece {
            index: json.get("index").cloned(),
            id: String::new(),
            name: String::new(),
            args: truthy_text(d.get("partial_json")).unwrap_or_default(),
        })],
        _ => Vec::new(),
    })
}

/// A whole (non-streamed) reply, parsed (`parseFull`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Full {
    pub text: String,
    pub reasoning: String,
    pub usage: Value,
    pub tool_calls: Vec<ToolCall>,
    pub truncated: bool,
}

/// `[…].map((b) => b.field).join(sep)`: undefined and null join as ''.
fn join_field(blocks: &[&Value], field: &str, sep: &str) -> String {
    blocks
        .iter()
        .map(|b| b.get(field).filter(|v| !v.is_null()).map(Value::js_string).unwrap_or_default())
        .collect::<Vec<_>>()
        .join(sep)
}

/// A whole (non-streamed) reply (`parseFull`). A refusal without text fails.
pub fn parse_full(json: &Value) -> Result<Full, AiError> {
    let Some(content) = json.get("content").and_then(Value::as_array) else {
        return Err(AiError::new(ErrorCode::Malformed, "The Anthropic reply had no content."));
    };
    let of_type = |t: &str| -> Vec<&Value> {
        content.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some(t)).collect()
    };
    let text = join_field(&of_type("text"), "text", "");
    let reasoning = join_field(&of_type("thinking"), "thinking", "\n");
    let stop = json.get("stop_reason").and_then(Value::as_str);
    if text.is_empty() && stop == Some("refusal") {
        return Err(AiError::new(ErrorCode::Refused, "The model declined to answer."));
    }
    let tool_calls = of_type("tool_use")
        .into_iter()
        .filter(|b| b.get("name").is_some_and(Value::truthy))
        .map(|b| {
            tool_call(
                b.get("id").filter(|v| !v.is_null()).map(Value::js_string).unwrap_or_default(),
                b.get("name").map(Value::js_string).unwrap_or_default(),
                b.get("input").unwrap_or(&Value::Null),
            )
        })
        .collect();
    let mut usage = Map::new();
    let u = json.get("usage");
    if let Some(v) = u.and_then(|u| u.get("input_tokens")) {
        usage.insert("input", v.clone());
    }
    if let Some(v) = u.and_then(|u| u.get("output_tokens")) {
        usage.insert("output", v.clone());
    }
    Ok(Full { text, reasoning, usage: Value::Object(usage), tool_calls, truncated: stop == Some("max_tokens") })
}

/// The model list (`parseModels`).
pub fn parse_models(json: &Value) -> Vec<ModelInfo> {
    json.get("data")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|m| {
            let id = m.get("id").and_then(Value::as_str)?.to_string();
            let label = truthy_text(m.get("display_name")).unwrap_or_else(|| id.clone());
            Some(ModelInfo { id, label, loaded: false, vision: None })
        })
        .collect()
}

/// Stream one reply (`stream`). A model is required.
pub fn stream(
    client: &dyn HttpClient,
    call: &ChatCall<'_>,
    on_event: &mut dyn FnMut(StreamEvent),
    cancel: &CancelToken,
) -> Result<ChatResult, AiError> {
    if call.cfg.model.is_empty() {
        return Err(AiError::new(
            ErrorCode::MissingModel,
            "Choose an Anthropic model in Settings (try \"Load models\").",
        ));
    }
    let req = call.request;
    let wire = build_chat(call);
    let opts = post_options(call, &wire, image_chars(&req.messages, &req.tool_turns), false, cancel);
    let mut usage = Map::new();
    let mut stop = Value::Null;
    let mut pieces = Vec::new();
    let res = request_stream(client, &opts, &mut |rec| {
        let Some(json) = parse_record(&rec.data) else { return Ok(()) };
        let ty = json.get("type").and_then(Value::as_str);
        // `usage.x = undefined` leaves the key out of the JSON: same as not having it.
        let mut set = |k: &str, v: Option<&Value>| match v {
            Some(v) => usage.insert(k, v.clone()),
            None => {
                usage.remove(k);
            }
        };
        if ty == Some("message_start") {
            set("input", json.get("message").and_then(|m| m.get("usage")).and_then(|u| u.get("input_tokens")));
        }
        if ty == Some("message_delta") {
            if let Some(u) = json.get("usage").filter(|u| u.truthy()) {
                set("output", u.get("output_tokens"));
            }
            if let Some(s) = json.get("delta").and_then(|d| d.get("stop_reason")).filter(|s| s.truthy()) {
                stop = s.clone();
            }
        }
        for f in parse_event(&json)? {
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
            usage: Some(full.usage),
            tool_calls: full.tool_calls,
            truncated: full.truncated,
            ..Default::default()
        });
    }
    let tool_calls = assemble(&pieces, |cur, p| {
        if !p.id.is_empty() {
            cur.id.clone_from(&p.id);
        }
        if !p.name.is_empty() {
            cur.name.clone_from(&p.name);
        }
        cur.args.push_str(&p.args);
    })
    .into_iter()
    .filter(|c| !c.name.is_empty())
    .map(|c| {
        let id = if c.id.is_empty() { format!("toolu_{}", c.index_text()) } else { c.id };
        tool_call(id, c.name, &Value::String(c.args))
    })
    .collect();
    Ok(ChatResult {
        usage: Some(Value::Object(usage)),
        tool_calls,
        truncated: stop.as_str() == Some("max_tokens"),
        ..Default::default()
    })
}

/// The models the account can use (`listModels`).
pub fn list_models(
    client: &dyn HttpClient,
    cfg: &AdapterConfig,
    key: &str,
    cancel: &CancelToken,
) -> Result<Vec<ModelInfo>, AiError> {
    let path = if cfg.models_path.is_empty() { "/v1/models?limit=1000" } else { cfg.models_path.as_str() };
    let json = request_json(client, &get_options(cfg, key, join_url(&cfg.base_url, path), headers(key), cancel))?;
    Ok(parse_models(&json))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value {
        Value::parse(s).unwrap()
    }

    #[test]
    fn whole_reply() {
        let full = parse_full(&v(r#"{"content":[{"type":"thinking","thinking":"a"},{"type":"thinking","thinking":"b"},{"type":"text","text":"Hi"},
            {"type":"tool_use","id":"t1","name":"find","input":{"q":1}},{"type":"tool_use","id":"t2"}],"stop_reason":"max_tokens","usage":{"input_tokens":3}}"#)).unwrap();
        assert_eq!((full.text.as_str(), full.reasoning.as_str(), full.truncated), ("Hi", "a\nb", true));
        assert_eq!(full.usage, v(r#"{"input":3}"#));
        assert_eq!(full.tool_calls.len(), 1);
        let e = parse_full(&v(r#"{"content":[],"stop_reason":"refusal"}"#)).unwrap_err();
        assert_eq!((e.code, e.message.as_str()), (ErrorCode::Refused, "The model declined to answer."));
        assert_eq!(parse_full(&v(r#"{"x":1}"#)).unwrap_err().code, ErrorCode::Malformed);
    }

    #[test]
    fn error_events() {
        let e = parse_event(&v(r#"{"error":"boom"}"#)).unwrap_err();
        assert_eq!((e.code, e.message.as_str()), (ErrorCode::Refused, "Anthropic returned an error."));
        assert!(parse_event(&v(r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":""}}"#))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn tool_result_with_image_says_done_when_empty() {
        let turns: Vec<ToolTurn> = crate::json::from_value(&v(
            r#"[{"text":"","calls":[{"id":"c","name":"shot","arguments":{}}],"results":[{"id":"c","name":"shot","content":"","images":[{"mime":"image/png","data":"AAAA"}]}]}]"#,
        ))
        .unwrap();
        let out = Value::Array(tool_turn_messages(&turns));
        assert_eq!(
            out,
            v(r#"[{"role":"assistant","content":[{"type":"tool_use","id":"c","name":"shot","input":{}}]},
                {"role":"user","content":[{"type":"tool_result","tool_use_id":"c","content":[{"type":"text","text":"Done."},
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AAAA"}}]}]}]"#)
        );
    }
}
