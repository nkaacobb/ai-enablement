//! The conformance vectors of what a request carries and what each provider protocol sends and reads back
//! (`scripts/lib/conformance-runtime.mjs`): the request messages, the tool set of a request, text-mode tools, the
//! agent and skill prompt texts, each adapter's request, and each adapter reading a scripted HTTP response through
//! the transport. Where the vectors are found: `common/mod.rs`.

mod common;

use std::collections::{BTreeMap, VecDeque};
use std::io::Read;
use std::sync::Mutex;

use common::{assert_suites_exist, bool_or, obj, run_suite, str_of};

use ai_enablement_core::adapters::ChatCall;
use ai_enablement_core::agents::{agent_skill_names, agent_tool_names, build_agent_prompt, parse_agent};
use ai_enablement_core::chat::{messages_from_value, normalize_messages, tool_call_value, ChatRequest, Image};
use ai_enablement_core::client::resolve_target;
use ai_enablement_core::conversation::{
    build_request_messages, ImageFor, Message, RequestInput, RequestMessage, Role, Shot, DEFAULT_HISTORY_MESSAGES,
    DEFAULT_IMAGE_MESSAGES,
};
use ai_enablement_core::json::{from_value, to_value, Map, Value};
use ai_enablement_core::settings::Settings;
use ai_enablement_core::skills::{build_skills_prompt, parse_skill, skill_loaded_message, Skill, ToolState};
use ai_enablement_core::tools::{
    build_tool_prompt, classify_tools, normalize_tool, parse_text_tool_calls, request_tool_spec, text_turn_messages,
    tool_specs, ToolContract, ToolMode, ToolPrompt, ToolSpec, ToolTurn,
};
use ai_enablement_core::transport::{CancelToken, HttpClient, HttpFailure, HttpRequest, HttpResponse};

/// The suites this runner covers.
const SUITES: [&str; 6] =
    ["request-messages", "tool-set", "text-tools", "capability-prompts", "adapter-requests", "adapter-streams"];

/// The key of the adapter suites, unless a case gives one.
const KEY: &str = "sk-test-key-123456";

#[test]
fn suites_table_names_existing_files() {
    assert_suites_exist(&SUITES);
}

fn list<T: serde::de::DeserializeOwned>(v: Option<&Value>, what: &str) -> Vec<T> {
    v.filter(|v| !v.is_null()).map(|v| from_value(v).unwrap_or_else(|e| panic!("{what}: {e}"))).unwrap_or_default()
}

#[test]
fn request_messages() {
    run_suite("request-messages", |i| {
        if let Some(n) = i.get("normalize") {
            return to_value(&normalize_messages(&messages_from_value(n)));
        }
        let messages: Vec<Message> = list(i.get("messages"), "messages");
        let images = i.get("images").and_then(Value::as_object);
        let image_for = |s: &Shot| images.and_then(|m| m.get(&s.id)).and_then(|v| from_value::<Image>(v).ok());
        let input = RequestInput {
            messages: &messages,
            history_messages: i.at("historyMessages").as_f64().map_or(DEFAULT_HISTORY_MESSAGES, |n| n as i64),
            view_text: i.at("viewText").as_str().unwrap_or(""),
            unchanged_hash: i.at("unchangedHash").as_str(),
            image_for: images.is_some().then_some(&image_for as ImageFor),
            image_messages: i.at("imageMessages").as_f64().map_or(DEFAULT_IMAGE_MESSAGES, |n| n as usize),
        };
        to_value(&build_request_messages(&input))
    });
}

#[test]
fn tool_set() {
    run_suite("tool-set", |i| {
        let settings = Settings::from_json(i.at("settings"));
        let tools: Vec<ToolContract> = i
            .at("tools")
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|t| normalize_tool(t).expect("tool"))
            .collect();
        let classes = classify_tools(&tools, &settings.tool_states, &str_of(i.at("pageId")));
        let names = |l: &[&ToolContract]| Value::from(l.iter().map(|t| t.name.as_str()).collect::<Vec<_>>());
        let mut specs: Vec<ToolSpec> = tool_specs(&classes.callable);
        if !classes.off.is_empty() {
            specs.push(request_tool_spec(&classes.off));
        }
        let mode = match i.at("mode").as_str() {
            Some("text") => ToolMode::Text,
            Some("off") => ToolMode::Off,
            _ => ToolMode::Native,
        };
        let prompt = build_tool_prompt(&ToolPrompt {
            classes: &classes,
            mode,
            enabled: settings.tools_enabled,
            app_off: bool_or(i.at("appOff"), false),
            page_label: None,
        });
        obj(vec![
            ("callable", names(&classes.callable)),
            ("off", names(&classes.off)),
            ("elsewhere", names(&classes.elsewhere)),
            ("specs", to_value(&specs)),
            ("prompt", prompt.into()),
        ])
    });
}

#[test]
fn text_tools() {
    run_suite("text-tools", |i| {
        if let Some(turns) = i.get("toolTurns") {
            let turns: Vec<ToolTurn> = list(Some(turns), "toolTurns");
            return to_value(&text_turn_messages(&turns));
        }
        let r = parse_text_tool_calls(&str_of(i.at("text")), &str_of(i.at("idPrefix")), bool_or(i.at("cutOff"), false));
        obj(vec![("calls", Value::Array(r.calls.iter().map(tool_call_value).collect())), ("text", r.text.into())])
    });
}

#[test]
fn capability_prompts() {
    run_suite("capability-prompts", |i| {
        let agent = i.get("agent").and_then(|a| parse_agent(&str_of(a.at("text")), &str_of(a.at("file"))).agent);
        let strings =
            |v: &Value| -> Vec<String> { v.as_array().map_or(&[][..], Vec::as_slice).iter().map(str_of).collect() };
        if let Some(tools) = i.get("tools") {
            let toolsets: BTreeMap<String, Vec<String>> = i
                .at("toolsets")
                .as_object()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), strings(v))).collect())
                .unwrap_or_default();
            let (tool_names, tool_problems) = match &agent {
                Some(a) => agent_tool_names(a, &strings(tools), &toolsets),
                None => (None, Vec::new()),
            };
            let (skill_names, skill_problems) = match &agent {
                Some(a) => agent_skill_names(a, &strings(i.at("skillNames"))),
                None => (None, Vec::new()),
            };
            return obj(vec![
                ("tools", tool_names.map_or(Value::Null, Value::from)),
                ("toolProblems", tool_problems.into()),
                ("skills", skill_names.map_or(Value::Null, Value::from)),
                ("skillProblems", skill_problems.into()),
            ]);
        }
        if i.get("agent").is_some() {
            return agent.as_ref().map(build_agent_prompt).unwrap_or_default().into();
        }
        let parse = |s: &Value| -> Skill {
            parse_skill(&str_of(s.at("text")), "", &str_of(s.at("folder")), "app").skill.expect("skill")
        };
        if let Some(loaded) = i.get("loaded") {
            let states = i.at("states");
            let state = |t: &str| match states.at(t).as_str() {
                Some("on") => ToolState::On,
                Some("off") => ToolState::Off,
                Some("denied") => ToolState::Denied,
                _ => ToolState::Missing,
            };
            return skill_loaded_message(&parse(loaded), state).into();
        }
        let skills: Vec<Skill> = i.at("skills").as_array().map_or(&[][..], Vec::as_slice).iter().map(parse).collect();
        let refs: Vec<&Skill> = skills.iter().collect();
        build_skills_prompt(&refs, &strings(i.at("active")), bool_or(i.at("canLoad"), false)).into()
    });
}

/// `sanitizeSettings({ provider, transport, relayUrl, reasoning?, maxOutputTokens, temperature, profiles: { [id]:
/// { baseUrl?, model } } })`, as both adapter suites build it.
fn adapter_settings(i: &Value, max_tokens: Value, temperature: Value) -> Settings {
    let id = str_of(i.at("provider"));
    let mut profile = Map::new();
    if let Some(b) = i.get("baseUrl").filter(|b| b.truthy()) {
        profile.insert("baseUrl", b.clone());
    }
    profile.insert("model", i.get("model").cloned().unwrap_or_else(|| "".into()));
    let mut profiles = Map::new();
    profiles.insert(id.as_str(), profile);
    let text = |k: &str, default: &str| i.get(k).cloned().unwrap_or_else(|| default.into());
    Settings::from_json(&obj(vec![
        ("provider", id.as_str().into()),
        ("transport", text("transport", "direct")),
        ("relayUrl", text("relayUrl", "")),
        ("reasoning", text("reasoning", "show")),
        ("maxOutputTokens", max_tokens),
        ("temperature", temperature),
        ("profiles", Value::Object(profiles)),
    ]))
}

#[test]
fn adapter_requests() {
    run_suite("adapter-requests", |i| {
        let id = str_of(i.at("provider"));
        let key = i.get("key").map_or_else(|| KEY.to_string(), str_of);
        let settings = adapter_settings(i, i.at("maxTokens").clone(), i.at("temperature").clone());
        let t = match resolve_target(&settings, &|_| key.clone(), &id, false) {
            Ok(t) => t,
            Err(e) => {
                return obj(vec![("error", obj(vec![("code", e.code.id().into()), ("message", e.message.into())]))])
            }
        };
        let request = ChatRequest {
            system: str_of(i.at("system")),
            messages: messages_from_value(i.at("messages")),
            tools: list(i.get("tools"), "tools"),
            tool_turns: list(i.get("toolTurns"), "toolTurns"),
            turn_id: str_of(i.at("turnId")),
        };
        let call = ChatCall {
            cfg: &t.cfg,
            key: &t.key,
            request: &request,
            max_tokens: Some(settings.max_output_tokens as f64),
            temperature: Some(settings.temperature),
        };
        t.adapter.build_chat(&call).to_value()
    });
}

/// The response of an adapter-streams case: its status, content type, and the body one chunk per read.
struct Scripted(Mutex<Option<HttpResponse>>);

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

impl HttpClient for Scripted {
    fn send(&self, _request: &HttpRequest, _cancel: &CancelToken) -> Result<HttpResponse, HttpFailure> {
        self.0.lock().unwrap().take().ok_or_else(|| HttpFailure::Network("one response per case".into()))
    }
}

#[test]
fn adapter_streams() {
    run_suite("adapter-streams", |i| {
        let id = str_of(i.at("provider"));
        let settings = adapter_settings(i, 1000.0.into(), 0.3.into());
        let t = resolve_target(&settings, &|_| KEY.to_string(), &id, false).expect("target");
        let r = i.at("response");
        let chunks: VecDeque<Vec<u8>> = r
            .at("chunks")
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|c| str_of(c).into_bytes())
            .filter(|c| !c.is_empty())
            .collect();
        let client = Scripted(Mutex::new(Some(HttpResponse {
            status: r.at("status").as_f64().map_or(200, |s| s as u16),
            content_type: str_of(r.at("contentType")),
            body: Box::new(Chunks(chunks)),
        })));
        let request = ChatRequest {
            system: "S".into(),
            messages: vec![RequestMessage { role: Role::User, content: "Hi".into(), images: vec![] }],
            ..Default::default()
        };
        let call =
            ChatCall { cfg: &t.cfg, key: KEY, request: &request, max_tokens: Some(1000.0), temperature: Some(0.3) };
        let mut events = Vec::new();
        let result = t.adapter.stream(&client, &call, &mut |e| events.push(e.to_value()), &CancelToken::new());
        match result {
            Ok(r) => obj(vec![("events", Value::Array(events)), ("result", r.stream_value())]),
            Err(e) => {
                let mut error = obj(vec![("code", e.code.id().into()), ("message", e.message.as_str().into())]);
                if !e.hints.is_empty() {
                    error.as_object_mut().unwrap().insert("hints", e.hints.clone());
                }
                obj(vec![("events", Value::Array(events)), ("error", error)])
            }
        }
    });
}
