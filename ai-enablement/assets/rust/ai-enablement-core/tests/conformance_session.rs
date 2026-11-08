//! The session's conformance vectors (`execute-tool`, `tool-loop`): one tool call, and one question with its tool
//! rounds, through [`Session`] with the scripted harness the suites describe — tools return fixed values, the model
//! replies from a script, the user's clicks come from a list, the clock is fixed. Built exactly as
//! `scripts/lib/conformance-runtime.mjs` builds the JS one.

mod common;

use common::{assert_suites_exist, run_suite};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use ai_enablement_core::agents::parse_agent;
use ai_enablement_core::chat::{ChatModel, ChatRequest, ChatResult, EventKind, StreamEvent};
use ai_enablement_core::context::{ContextManager, Page, Source};
use ai_enablement_core::conversation::Message;
use ai_enablement_core::json::{to_value, Map, Value};
use ai_enablement_core::memory::MemoryStore;
use ai_enablement_core::permissions::normalize_policy;
use ai_enablement_core::session::{Card, Confirm, Reply, RowId, ScreenSource, Screenshot, Session, ToolRunner, View};
use ai_enablement_core::settings::Settings;
use ai_enablement_core::skills::parse_skill;
use ai_enablement_core::time::parse_date;
use ai_enablement_core::tools::{normalize_tool, tool_call, ToolContract};
use ai_enablement_core::transport::{AiError, CancelToken, ErrorCode};

#[test]
fn suite_files_exist() {
    assert_suites_exist(&["execute-tool", "tool-loop"]);
}

fn clock() -> f64 {
    parse_date("2026-01-02T03:04:05.000Z").expect("clock")
}

/* ------------------------------------------------------------------------------------------ the harness */

#[derive(Default)]
struct Row {
    title: String,
    call: String,
    statuses: Vec<String>,
    label: Option<String>,
    shot: Option<String>,
    undo: bool,
}

#[derive(Default)]
struct Record {
    answers: VecDeque<String>,
    rows: Vec<Row>,
    cards: Vec<Value>,
    notices: Vec<String>,
    ran: Vec<Value>,
}

struct ScriptView(Rc<RefCell<Record>>);

fn card_value(card: &Card) -> Value {
    let mut m = Map::new();
    m.insert("kind", card.kind.id());
    m.insert("buttons", card.buttons.iter().map(|b| b.id).collect::<Vec<_>>());
    if let Some(t) = &card.tool {
        m.insert("tool", t.name.as_str());
    }
    if !card.reason.is_empty() {
        m.insert("reason", card.reason.as_str());
    }
    if card.kind.id() == "confirm" {
        m.insert("decision", card.decision.map(|d| d.as_str()).unwrap_or(""));
        let confirm = match &card.confirm {
            Some(Confirm::MemoryDelete { id, text }) => {
                let mut c = Map::new();
                c.insert("kind", "memory-delete");
                c.insert("id", id.as_str());
                c.insert("text", text.as_str());
                Value::Object(c)
            }
            None => Value::Null,
        };
        m.insert("confirm", confirm);
    }
    Value::Object(m)
}

impl View for ScriptView {
    fn notice(&mut self, text: &str) {
        self.0.borrow_mut().notices.push(text.to_string());
    }
    fn row(&mut self, title: &str, call: &str) -> RowId {
        let mut r = self.0.borrow_mut();
        r.rows.push(Row { title: title.into(), call: call.into(), ..Default::default() });
        r.rows.len() - 1
    }
    fn row_status(&mut self, row: RowId, status: &str, _summary: &str) {
        self.0.borrow_mut().rows[row].statuses.push(status.into());
    }
    fn row_label(&mut self, row: RowId, text: &str) {
        self.0.borrow_mut().rows[row].label = Some(text.into());
    }
    fn row_shot(&mut self, row: RowId, id: &str, _thumb: &str) {
        self.0.borrow_mut().rows[row].shot = Some(id.into());
    }
    fn row_undo(&mut self, row: RowId, _undo: &ai_enablement_core::session::Undo) {
        self.0.borrow_mut().rows[row].undo = true;
    }
    fn decide(&mut self, _row: RowId, card: &Card, cancel: &CancelToken) -> String {
        let mut r = self.0.borrow_mut();
        r.cards.push(card_value(card));
        let choice = r.answers.pop_front().unwrap_or_else(|| card.decline.to_string());
        if choice == "cancel" {
            cancel.cancel();
        }
        choice
    }
}

struct ScriptTool {
    result: Value,
    throws: Option<String>,
    set_content: Option<Value>,
}

struct ScriptRunner {
    record: Rc<RefCell<Record>>,
    tools: Vec<(String, ScriptTool)>,
    content: Arc<Mutex<Value>>,
}

impl ToolRunner for ScriptRunner {
    fn run(&mut self, tool: &ToolContract, args: &Map, _call_id: &str, _cancel: &CancelToken) -> Result<Value, String> {
        let mut ran = Map::new();
        ran.insert("name", tool.name.as_str());
        ran.insert("args", Value::Object(args.clone()));
        self.record.borrow_mut().ran.push(Value::Object(ran));
        let (_, t) = self.tools.iter().find(|(n, _)| *n == tool.name).expect("scripted tool");
        if let Some(c) = &t.set_content {
            *self.content.lock().unwrap() = c.clone();
        }
        match &t.throws {
            Some(e) => Err(e.clone()),
            None => Ok(t.result.clone()),
        }
    }
}

struct ScriptScreen {
    gesture: bool,
}

impl ScreenSource for ScriptScreen {
    fn method(&self) -> &str {
        "hook"
    }
    fn needs_gesture(&self) -> bool {
        self.gesture
    }
    fn take(&mut self) -> Result<Screenshot, String> {
        Ok(Screenshot {
            id: "shot1".into(),
            width: 1280,
            height: 720,
            mime: "image/jpeg".into(),
            data: "QUJDREVGR0hJSktMTU5PUA==".into(),
            thumb: "data:image/jpeg;base64,QUJD".into(),
        })
    }
}

struct ScriptModel {
    steps: VecDeque<Value>,
    requests: Rc<RefCell<Vec<Value>>>,
}

impl ChatModel for ScriptModel {
    fn chat(
        &mut self,
        request: &ChatRequest,
        _settings: &Settings,
        on_event: &mut dyn FnMut(StreamEvent),
        _cancel: &CancelToken,
    ) -> Result<ChatResult, AiError> {
        self.requests.borrow_mut().push(request.to_value());
        let Some(s) = self.steps.pop_front() else {
            return Err(AiError::new(ErrorCode::Malformed, "The script has no reply for this request."));
        };
        if let Some(e) = s.get("error") {
            return Err(AiError::new(
                ErrorCode::parse(e.at("code").as_str().unwrap_or("")),
                e.at("message").as_str().unwrap_or(""),
            ));
        }
        if let Some(r) = s.get("reasoning").and_then(|v| v.as_str()).filter(|t| !t.is_empty()) {
            on_event(StreamEvent::new(EventKind::Reasoning, r));
        }
        if let Some(t) = s.get("text").and_then(|v| v.as_str()).filter(|t| !t.is_empty()) {
            on_event(StreamEvent::new(EventKind::Text, t));
        }
        let calls = s.get("toolCalls").and_then(|v| v.as_array()).cloned().unwrap_or_default();
        Ok(ChatResult {
            tool_calls: calls
                .iter()
                .map(|c| {
                    tool_call(c.at("id").as_str().unwrap_or(""), c.at("name").as_str().unwrap_or(""), c.at("arguments"))
                })
                .collect(),
            truncated: s.get("truncated").and_then(|v| v.as_bool()).unwrap_or(false),
            provider: "scripted".into(),
            label: "Scripted".into(),
            model: "model-1".into(),
            ..Default::default()
        })
    }
}

struct Harness {
    session: Session,
    view: ScriptView,
    runner: ScriptRunner,
    model: ScriptModel,
    record: Rc<RefCell<Record>>,
    requests: Rc<RefCell<Vec<Value>>>,
    cancel: CancelToken,
    has_memory: bool,
}

fn scripted(input: &Value) -> Harness {
    let settings = Settings::from_json(input.get("settings").unwrap_or(&Value::Null));
    let content = Arc::new(Mutex::new(Value::Null));
    let mut ctx = ContextManager::new();
    if let Some(app) = input.get("app") {
        ctx.set_app(Source::Value(app.clone()));
    }
    if let Some(p) = input.get("page") {
        let mut page = Page::from_value(p);
        if let Some(c) = p.get("content") {
            *content.lock().unwrap() = c.clone();
            let shared = content.clone();
            page.content = Some(Source::hook(move || Ok(shared.lock().unwrap().clone())));
        }
        ctx.set_page(Some(page));
    }
    let mut session = Session::new(ctx, settings);
    let mut scripted_tools = Vec::new();
    for def in input.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default() {
        let mut contract = def.as_object().cloned().unwrap_or_default();
        let result = contract.remove("result").unwrap_or(Value::Null);
        let throws = contract.remove("throws").and_then(|v| v.as_str().map(str::to_string));
        let set_content = contract.remove("setContent");
        let tool = normalize_tool(&Value::Object(contract)).expect("tool");
        scripted_tools.push((tool.name.clone(), ScriptTool { result, throws, set_content }));
        session.tools.push(tool);
    }
    for a in input.get("agents").and_then(|t| t.as_array()).cloned().unwrap_or_default() {
        if let Some(agent) = parse_agent(a.at("text").as_str().unwrap_or(""), a.at("file").as_str().unwrap_or("")).agent
        {
            session.agents.push(agent);
        }
    }
    for s in input.get("skills").and_then(|t| t.as_array()).cloned().unwrap_or_default() {
        let folder = s.at("folder").as_str().unwrap_or("");
        if let Some(skill) = parse_skill(s.at("text").as_str().unwrap_or(""), "", folder, "app").skill {
            session.skills.push(skill);
        }
    }
    session.policy = normalize_policy(input.get("permissions").unwrap_or(&Value::Null), &mut Vec::new());
    session.title = "AI agent".into();
    session.default_prompt = "B".into();
    session.attachments = false;
    session.clock = Box::new(clock);
    let has_memory = input.get("memory").is_some();
    if let Some(notes) = input.get("memory").and_then(|m| m.as_array()) {
        let mut store = MemoryStore::with_clock(Box::new(clock));
        let base: Vec<Value> = notes
            .iter()
            .map(|n| {
                let mut o = n.as_object().cloned().unwrap_or_default();
                o.insert("created", "2026-01-01T00:00:00.000Z");
                o.insert("source", "app");
                Value::Object(o)
            })
            .collect();
        store.set_base(&Value::Array(base));
        session.memory = Some(store);
    }
    if let Some(screen) = input.get("screen") {
        let gesture = screen.get("needsGesture").and_then(|v| v.as_bool()).unwrap_or(false);
        session.screen = Some(Box::new(ScriptScreen { gesture }));
    }
    session.init();
    for name in input.get("allowedForChat").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        session.allowed_tools.insert(name.as_str().unwrap_or("").to_string());
    }
    let record = Rc::new(RefCell::new(Record::default()));
    record.borrow_mut().answers = input
        .get("answers")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().map(|x| x.js_string()).collect())
        .unwrap_or_default();
    let requests = Rc::new(RefCell::new(Vec::new()));
    Harness {
        session,
        view: ScriptView(record.clone()),
        runner: ScriptRunner { record: record.clone(), tools: scripted_tools, content },
        model: ScriptModel {
            steps: input.get("steps").and_then(|v| v.as_array()).cloned().unwrap_or_default().into(),
            requests: requests.clone(),
        },
        record,
        requests,
        cancel: CancelToken::new(),
        has_memory,
    }
}

impl Harness {
    fn report(&self, out: &mut Map) {
        let r = self.record.borrow();
        let rows: Vec<Value> = r
            .rows
            .iter()
            .map(|row| {
                let mut m = Map::new();
                m.insert("title", row.title.as_str());
                m.insert("call", row.call.as_str());
                m.insert("statuses", row.statuses.clone());
                if let Some(l) = &row.label {
                    m.insert("label", l.as_str());
                }
                if let Some(s) = &row.shot {
                    m.insert("shot", s.as_str());
                }
                if row.undo {
                    m.insert("undo", true);
                }
                Value::Object(m)
            })
            .collect();
        out.insert("rows", rows);
        out.insert("cards", r.cards.clone());
        out.insert("notices", r.notices.clone());
        out.insert("ran", r.ran.clone());
        let states: Map = self.session.settings.tool_states.iter().map(|(k, v)| (k.clone(), Value::Bool(*v))).collect();
        out.insert("toolStates", states);
        out.insert("screenshotAuto", self.session.settings.screenshot_auto);
        out.insert("allowedForChat", self.session.allowed_tools.iter().cloned().collect::<Vec<_>>());
        if self.has_memory {
            let notes: Vec<Value> = self
                .session
                .memory
                .as_ref()
                .map(|m| m.list())
                .unwrap_or_default()
                .iter()
                .map(|n| {
                    let mut o = Map::new();
                    o.insert("id", n.id.as_str());
                    o.insert("text", n.text.as_str());
                    Value::Object(o)
                })
                .collect();
            out.insert("memory", notes);
        }
    }
}

/* ---------------------------------------------------------------------------------------------- suites */

#[test]
fn execute_tool() {
    run_suite("execute-tool", |input| {
        let mut h = scripted(input);
        let c = input.at("call");
        let mut call =
            tool_call(c.at("id").as_str().unwrap_or(""), c.at("name").as_str().unwrap_or(""), c.at("arguments"));
        call.raw = c.get("raw").and_then(|v| v.as_str()).map(str::to_string);
        call.args_error = c.get("argsError").and_then(|v| v.as_str()).map(str::to_string);
        call.cut_off = c.get("cutOff").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut reply = Reply::default();
        let mut actions = Vec::new();
        let r = h.session.execute_tool(&call, &mut reply, &mut h.view, &mut h.runner, &mut actions, &h.cancel);
        let mut result = Map::new();
        result.insert("id", r.id.as_str());
        result.insert("name", r.name.as_str());
        result.insert("content", r.content.as_str());
        result.insert("changed", r.changed);
        if !r.images.is_empty() {
            result.insert("images", to_value(&r.images));
        }
        let mut out = Map::new();
        out.insert("result", Value::Object(result));
        h.report(&mut out);
        out.insert("actions", to_value(&actions));
        Value::Object(out)
    });
}

#[test]
fn tool_loop() {
    run_suite("tool-loop", |input| {
        let mut h = scripted(input);
        if let Some(history) = input.get("history").and_then(|v| v.as_array()) {
            let current = h.session.ctx.snapshot_at(String::new()).hash;
            h.session.conv.messages = history
                .iter()
                .map(|m| {
                    let mut msg: Message = ai_enablement_core::json::from_value(m).expect("history message");
                    if let Some(s) = msg.snapshot.as_mut().filter(|s| s.hash == "__current__") {
                        s.hash = current.clone();
                    }
                    msg
                })
                .collect();
        }
        let settings = h.session.settings.clone();
        let turn = h.session.begin_turn(input.at("question").as_str().unwrap_or(""), &[], &[], None);
        let mut reply = Reply::default();
        let mut actions = Vec::new();
        let outcome = h.session.run_model(
            &settings,
            &turn,
            &mut reply,
            &mut h.model,
            &mut h.view,
            &mut h.runner,
            &mut actions,
            &h.cancel,
        );
        let (result, error) = match outcome {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e)),
        };
        let end = h.session.end_turn(&turn, &reply, actions, result.as_ref(), error.as_ref(), "");
        let mut out = Map::new();
        out.insert("requests", h.requests.borrow().clone());
        h.report(&mut out);
        let mut rep = Map::new();
        rep.insert("raw", reply.raw.as_str());
        rep.insert("native", reply.native.as_str());
        out.insert("reply", Value::Object(rep));
        out.insert("answer", end.answer.as_str());
        out.insert("kept", end.kept);
        out.insert(
            "error",
            match &error {
                Some(e) => {
                    let mut m = Map::new();
                    m.insert("code", e.code.id());
                    m.insert("message", e.message.as_str());
                    Value::Object(m)
                }
                None => Value::Null,
            },
        );
        let transcript: Vec<Value> = h
            .session
            .conv
            .messages
            .iter()
            .map(|m| {
                let mut o = Map::new();
                o.insert(
                    "role",
                    if m.role == ai_enablement_core::conversation::Role::User { "user" } else { "assistant" },
                );
                o.insert("content", m.content.as_str());
                if !m.sync.is_empty() {
                    o.insert("sync", m.sync.as_str());
                }
                if !m.skill.is_empty() {
                    o.insert("skill", m.skill.as_str());
                }
                if !m.actions.is_empty() {
                    let acts: Vec<Value> = m
                        .actions
                        .iter()
                        .map(|a| {
                            let mut x = Map::new();
                            x.insert("call", a.call.as_str());
                            x.insert("status", a.status.as_str());
                            x.insert("summary", a.summary.clone().unwrap_or_default());
                            Value::Object(x)
                        })
                        .collect();
                    o.insert("actions", acts);
                }
                if !m.meta.is_empty() {
                    o.insert("meta", m.meta.as_str());
                }
                Value::Object(o)
            })
            .collect();
        out.insert("transcript", transcript);
        Value::Object(out)
    });
}
