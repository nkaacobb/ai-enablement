//! Google Gemini generateContent (`adapters/gemini.js`).
//!
//! - `POST {base}/v1beta/models/{model}:streamGenerateContent?alt=sse`, header `x-goog-api-key`.
//! - Body `{ contents:[{role:'user'|'model', parts:[{text}]}], systemInstruction:{parts}, generationConfig:{
//!   maxOutputTokens, temperature } }`. The reasoning setting is not translated (thinkingConfig is model-dependent).
//! - Stream: `data: {candidates:[{content:{parts:[{text, thought?}]}, finishReason}], promptFeedback?,
//!   usageMetadata}`; parts flagged `thought: true` are the model's reasoning.
//! - Tools: `body.tools [{functionDeclarations:[{name, description, parameters}]}]`; the reply has parts
//!   `{functionCall:{name, args}}` (whole, not in pieces), possibly with a `thoughtSignature` that must be sent back;
//!   results go back as a user turn of `{functionResponse:{name, response:{result}}}` parts.
//! - `finishReason: 'MAX_TOKENS'`: the reply was cut off.
//! - Images: `{inlineData:{mimeType, data}}` parts after the text of a user turn; an image a tool returned follows
//!   the `functionResponse` parts of its round in the same user turn.
//! - Models: `GET {base}/v1beta/models` → `{ models:[{ name:'models/…', displayName, supportedGenerationMethods }] }`.

use super::{
    finite, first_of, get_options, object, parse_record, post_options, truthy_text, AdapterConfig, ChatCall, Fragment,
    ModelInfo, WholeCall, WireRequest,
};
use crate::chat::{image_chars, normalize_messages, ChatResult, EventKind, Image, Role, StreamEvent, ToolTurn};
use crate::json::{Map, Value};
use crate::tools::{read_arguments, tool_call};
use crate::transport::{join_url, request_json, request_stream, AiError, CancelToken, ErrorCode, HttpClient, Streamed};

/// Finish reasons that mean the provider filtered the reply.
pub const BLOCKED: [&str; 6] = ["SAFETY", "PROHIBITED_CONTENT", "BLOCKLIST", "SPII", "RECITATION", "IMAGE_SAFETY"];

fn headers(key: &str) -> Vec<(String, String)> {
    vec![("x-goog-api-key".to_string(), key.to_string())]
}

/// JS `encodeURIComponent`.
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The model's path segment: without a `models/` prefix, encoded (`modelPath`).
pub fn model_path(model: &str) -> String {
    encode_uri_component(model.strip_prefix("models/").unwrap_or(model))
}

fn inline(i: &Image) -> Value {
    object([("inlineData", object([("mimeType", i.mime.as_str().into()), ("data", i.data.as_str().into())]))])
}

/// The current question's tool exchange, as Gemini contents (`toolTurnContents`).
pub fn tool_turn_contents(turns: &[ToolTurn]) -> Vec<Value> {
    let mut out = Vec::new();
    for turn in turns {
        let mut parts = Vec::new();
        if !turn.text.is_empty() {
            parts.push(object([("text", turn.text.as_str().into())]));
        }
        for c in &turn.calls {
            let mut p = Map::new();
            p.insert(
                "functionCall",
                object([("name", c.name.as_str().into()), ("args", Value::Object(c.arguments.clone()))]),
            );
            if let Some(sig) = c.signature.as_deref().filter(|s| !s.is_empty()) {
                p.insert("thoughtSignature", sig);
            }
            parts.push(Value::Object(p));
        }
        out.push(object([("role", "model".into()), ("parts", Value::Array(parts))]));
        let mut parts: Vec<Value> = turn
            .results
            .iter()
            .map(|r| {
                object([(
                    "functionResponse",
                    object([
                        ("name", r.name.as_str().into()),
                        ("response", object([("result", r.content.as_str().into())])),
                    ]),
                )])
            })
            .collect();
        parts.extend(turn.results.iter().flat_map(|r| r.images.iter().map(inline)));
        out.push(object([("role", "user".into()), ("parts", Value::Array(parts))]));
    }
    out
}

/// The request (`buildChat`, streaming).
pub fn build_chat(call: &ChatCall<'_>) -> WireRequest {
    let cfg = call.cfg;
    let req = call.request;
    let mut contents: Vec<Value> = normalize_messages(&req.messages)
        .into_iter()
        .map(|m| {
            let role = if m.role == Role::Assistant { "model" } else { "user" };
            let text = if m.content.is_empty() { "(image)" } else { m.content.as_str() };
            let mut parts = vec![object([("text", text.into())])];
            parts.extend(m.images.iter().map(inline));
            object([("role", role.into()), ("parts", Value::Array(parts))])
        })
        .collect();
    contents.extend(tool_turn_contents(&req.tool_turns));
    let mut body = Map::new();
    body.insert("contents", contents);
    body.insert("generationConfig", Value::object());
    if !req.tools.is_empty() {
        let declarations: Vec<Value> = req
            .tools
            .iter()
            .map(|t| {
                object([
                    ("name", t.name.as_str().into()),
                    ("description", t.description.as_str().into()),
                    ("parameters", t.parameters.clone()),
                ])
            })
            .collect();
        body.insert("tools", vec![object([("functionDeclarations", Value::Array(declarations))])]);
    }
    if !req.system.is_empty() {
        body.insert(
            "systemInstruction",
            object([("parts", Value::Array(vec![object([("text", req.system.as_str().into())])]))]),
        );
    }
    let mut generation = Map::new();
    if let Some(n) = finite(call.max_tokens) {
        generation.insert("maxOutputTokens", n);
    }
    if let Some(t) = finite(call.temperature) {
        generation.insert("temperature", t);
    }
    body.insert("generationConfig", generation);
    let url =
        join_url(&cfg.base_url, &format!("/v1beta/models/{}:streamGenerateContent?alt=sse", model_path(&cfg.model)));
    WireRequest { url, headers: headers(call.key), body: Value::Object(body) }
}

/// One streamed chunk → fragments (`parseChunk`). Fails on an error, a blocked prompt, or a filtered reply.
pub fn parse_chunk(json: &Value) -> Result<Vec<Fragment>, AiError> {
    if let Some(err) = json.get("error").filter(|e| e.truthy()) {
        return Err(AiError::new(
            ErrorCode::Refused,
            truthy_text(err.get("message")).unwrap_or_else(|| "Gemini returned an error.".into()),
        ));
    }
    if json.get("promptFeedback").and_then(|p| p.get("blockReason")).is_some_and(Value::truthy) {
        return Err(AiError::new(ErrorCode::Refused, "The provider blocked this request."));
    }
    let Some(cand) = first_of(json, "candidates").filter(|c| c.truthy()) else { return Ok(Vec::new()) };
    let mut out = Vec::new();
    let parts = cand.get("content").and_then(|c| c.get("parts")).and_then(Value::as_array);
    for p in parts.map_or(&[][..], Vec::as_slice) {
        if let Some(Value::String(text)) = p.get("text") {
            if !text.is_empty() {
                let kind =
                    if p.get("thought").is_some_and(Value::truthy) { EventKind::Reasoning } else { EventKind::Text };
                out.push(Fragment::Event(StreamEvent::new(kind, text.as_str())));
            }
        }
        let f = p.get("functionCall");
        if let Some(name) = truthy_text(f.and_then(|f| f.get("name"))) {
            let f = f.unwrap_or(&Value::Null);
            out.push(Fragment::Call(WholeCall {
                id: truthy_text(f.get("id")).unwrap_or_default(),
                name,
                arguments: read_arguments(f.get("args").unwrap_or(&Value::Null)).args,
                signature: truthy_text(p.get("thoughtSignature")).unwrap_or_default(),
            }));
        }
    }
    let reason = cand.get("finishReason").and_then(Value::as_str);
    if out.is_empty() && reason.is_some_and(|r| BLOCKED.contains(&r)) {
        return Err(AiError::new(ErrorCode::Refused, "The provider filtered the reply."));
    }
    Ok(out)
}

/// The model list: only models that can generate content (`parseModels`).
pub fn parse_models(json: &Value) -> Vec<ModelInfo> {
    json.get("models")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|m| {
            let name = m.get("name").and_then(Value::as_str)?;
            if let Some(methods) = m.get("supportedGenerationMethods").and_then(Value::as_array) {
                if !methods.iter().any(|x| x.as_str() == Some("generateContent")) {
                    return None;
                }
            }
            let id = name.strip_prefix("models/").unwrap_or(name).to_string();
            let label = truthy_text(m.get("displayName")).unwrap_or_else(|| id.clone());
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
        return Err(AiError::new(ErrorCode::MissingModel, "Choose a Gemini model in Settings (try \"Load models\")."));
    }
    let req = call.request;
    let wire = build_chat(call);
    let opts = post_options(call, &wire, image_chars(&req.messages, &req.tool_turns), false, cancel);
    let mut out = ChatResult::default();
    let mut finish = Value::Null;
    let mut handle = |json: &Value, on_event: &mut dyn FnMut(StreamEvent)| -> Result<(), AiError> {
        if let Some(u) = json.get("usageMetadata").filter(|u| u.truthy()) {
            let mut m = Map::new();
            if let Some(v) = u.get("promptTokenCount") {
                m.insert("input", v.clone());
            }
            if let Some(v) = u.get("candidatesTokenCount") {
                m.insert("output", v.clone());
            }
            out.usage = Some(Value::Object(m));
        }
        if let Some(reason) = first_of(json, "candidates").and_then(|c| c.get("finishReason")).filter(|r| r.truthy()) {
            finish = reason.clone();
        }
        for f in parse_chunk(json)? {
            match f {
                Fragment::Call(c) => {
                    let id = if c.id.is_empty() { format!("gemini_{}", out.tool_calls.len() + 1) } else { c.id };
                    let mut call = tool_call(id, c.name, &Value::Object(c.arguments));
                    if !c.signature.is_empty() {
                        call.signature = Some(c.signature);
                    }
                    out.tool_calls.push(call);
                }
                Fragment::Event(e) => on_event(e),
                Fragment::Piece(_) => {}
            }
        }
        Ok(())
    };
    let res = request_stream(client, &opts, &mut |rec| match parse_record(&rec.data) {
        Some(json) => handle(&json, on_event),
        None => Ok(()),
    })?;
    if let Streamed::Json(json) = res {
        handle(&json, on_event)?;
    }
    out.truncated = finish.as_str() == Some("MAX_TOKENS");
    Ok(out)
}

/// The models the key can use (`listModels`).
pub fn list_models(
    client: &dyn HttpClient,
    cfg: &AdapterConfig,
    key: &str,
    cancel: &CancelToken,
) -> Result<Vec<ModelInfo>, AiError> {
    let path = if cfg.models_path.is_empty() { "/v1beta/models?pageSize=1000" } else { cfg.models_path.as_str() };
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
    fn model_path_encoded() {
        assert_eq!(model_path("models/gemini-2.5-flash"), "gemini-2.5-flash");
        assert_eq!(model_path("a b/c?é"), "a%20b%2Fc%3F%C3%A9");
        assert_eq!(encode_uri_component("A-z_0.!~*'()"), "A-z_0.!~*'()");
    }

    #[test]
    fn filtered_only_without_content() {
        let e = parse_chunk(&v(r#"{"candidates":[{"finishReason":"SAFETY"}]}"#)).unwrap_err();
        assert_eq!(e.message, "The provider filtered the reply.");
        assert_eq!(
            parse_chunk(&v(r#"{"candidates":[{"content":{"parts":[{"text":"x"}]},"finishReason":"SAFETY"}]}"#))
                .unwrap()
                .len(),
            1
        );
        let e = parse_chunk(&v(r#"{"error":{"message":"Bad key"}}"#)).unwrap_err();
        assert_eq!((e.code, e.message.as_str()), (ErrorCode::Refused, "Bad key"));
    }

    #[test]
    fn models_that_generate() {
        let got = parse_models(&v(
            r#"{"models":[{"name":"models/a","displayName":"A","supportedGenerationMethods":["generateContent"]},
            {"name":"models/e","supportedGenerationMethods":["embedContent"]},{"name":"b"}]}"#,
        ));
        let ids: Vec<(&str, &str)> = got.iter().map(|m| (m.id.as_str(), m.label.as_str())).collect();
        assert_eq!(ids, [("a", "A"), ("b", "b")]);
    }
}
