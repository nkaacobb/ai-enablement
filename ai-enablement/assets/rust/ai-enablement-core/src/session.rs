//! The agent without its UI (`core/session.js`): which agent is active, what a request offers (tools, skills, memory,
//! context), the system prompt, the tool loop with the user's decisions, and saved chats. The `execute-tool` and
//! `tool-loop` conformance vectors pin it.
//!
//! The tool loop is straight-line code, as in JS: it asks the user through [`View::decide`] and runs the application's
//! tools through [`ToolRunner::run`], and either may block. A native drawer runs a question on a worker thread and
//! answers both from the UI thread (where the host is), over channels; tests answer them from a script. Built-in tools
//! (memory, screenshots, skills) run inside the session.
//!
//! Differences from JS, on purpose:
//! - Tool handlers are synchronous and have no timeout race (a native handler must return quickly).
//! - No development-workspace tools (in-app authoring on native stacks is data-only and comes later).
//! - Events go to [`Session::take_events`] instead of listeners.

use crate::agents::{
    agent_skill_names, agent_tool_names, build_agent_prompt, implicit_agent, pick_agent, Agent, MemoryMode,
};
use crate::chat::{ChatModel, ChatRequest, ChatResult, EventKind, Image, StreamEvent};
use crate::context::ContextManager;
use crate::conversation::{
    build_request_messages, context_state, has_files, plan_turn, snapshot_block, Action, FlagState, ImageFor, Message,
    Plan, RequestInput, RequestMessage, Role, Screen, Shot, SnapshotRecord, SyncReason, TurnInput,
    DEFAULT_IMAGE_MESSAGES,
};
use crate::files::FileRecord;
use crate::json::{Map, Value};
use crate::memory::{build_memory_prompt, MemoryNote, MemoryStore};
use crate::permissions::{
    decide_permission, merge_policies, needs_confirmation, ConfirmSettings, Decision, Policy, Subject,
};
use crate::prompt::{build_system_prompt, PromptParts};
use crate::reasoning::split_reasoning;
use crate::schema::Effect;
use crate::settings::Settings;
use crate::skills::{
    build_skills_prompt, parse_slash_command, skill_file_path, skill_loaded_message, Skill, ToolState, SKILL_FILE_MAX,
};
use crate::text::{js_head, js_slice, js_trim, locale_count, utf16_len};
use crate::tools::{
    arguments_problem, build_tool_prompt, call_detail, classify_tools, format_call, normalize_tool,
    parse_text_tool_calls, request_tool_spec, serialize_result, text_turn_messages, tool_available, tool_enabled,
    tool_specs, validate_args, wire_call, CallDetail, ToolCall, ToolClasses, ToolContract, ToolMode, ToolPrompt,
    ToolResult, ToolTurn, REQUEST_TOOL, RESULT_MAX_CHARS,
};
use crate::transport::{AiError, CancelToken, ErrorCode};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicU64, Ordering};

pub const CHAT_LIMIT: usize = 50;
pub const MESSAGE_LIMIT: usize = 200;
/// Characters of each part of a tool row's detail kept in saved chats.
pub const DETAIL_SAVED: usize = 2000;
/// The hard cap on tool rounds per question.
pub const MAX_ROUNDS: i64 = 64;

/* ------------------------------------------------------------------------------------------------ cards */

/// What a card asks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardKind {
    /// The model asked for a tool that is off (request_tool).
    Request,
    /// The model called a tool that is off.
    TurnOn,
    /// Confirmation before a tool runs.
    Confirm,
    /// The agent wants a screenshot and is not freed to take one.
    Look,
    /// The screen capture needs the user's gesture first.
    ShareTab,
}

impl CardKind {
    pub fn id(self) -> &'static str {
        match self {
            CardKind::Request => "request",
            CardKind::TurnOn => "turn-on",
            CardKind::Confirm => "confirm",
            CardKind::Look => "look",
            CardKind::ShareTab => "share-tab",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Button {
    pub id: &'static str,
    pub label: &'static str,
    pub primary: bool,
}

const fn button(id: &'static str, label: &'static str, primary: bool) -> Button {
    Button { id, label, primary }
}

/// The tool a card is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardTool {
    pub name: String,
    pub title: String,
    pub effect: Effect,
}

/// A built-in's own confirmation data (what the view shows instead of "Run X?").
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Confirm {
    /// `forget`: the note to delete.
    MemoryDelete { id: String, text: String },
}

/// A question to the user, as data. The view writes the question; `decline` is the answer of a session with no one
/// to ask; `cancel` is the answer when the question is stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub kind: CardKind,
    pub buttons: Vec<Button>,
    pub decline: &'static str,
    pub tool: Option<CardTool>,
    pub reason: String,
    /// The permission rule's decision (confirmations).
    pub decision: Option<Decision>,
    pub confirm: Option<Confirm>,
}

impl Card {
    fn new(kind: CardKind) -> Card {
        let (buttons, decline) = match kind {
            CardKind::Request => (vec![button("on", "Turn on", true), button("off", "Keep off", false)], "off"),
            CardKind::TurnOn => (vec![button("on", "Turn on and run", true), button("off", "Keep off", false)], "off"),
            CardKind::Confirm => (Vec::new(), "skip"),
            CardKind::Look => (
                vec![
                    button("once", "Allow once", true),
                    button("always", "Always allow", false),
                    button("no", "No", false),
                ],
                "no",
            ),
            CardKind::ShareTab => (vec![button("share", "Share this tab", true), button("no", "Not now", false)], "no"),
        };
        Card { kind, buttons, decline, tool: None, reason: String::new(), decision: None, confirm: None }
    }
}

/// Run / Allow for this chat (write tools only, unless the rule is ask) / Skip.
pub fn confirm_buttons(effect: Effect, decision: Option<Decision>) -> Vec<Button> {
    let mut b = vec![button("run", "Run", true)];
    if effect == Effect::Write && decision != Some(Decision::Ask) {
        b.push(button("chat", "Allow for this chat", false));
    }
    b.push(button("skip", "Skip", false));
    b
}

/// What a confirmation says a tool of each effect does.
pub fn effect_note(effect: Effect) -> &'static str {
    match effect {
        Effect::Destructive => "This removes or overwrites something.",
        Effect::External => "This reaches outside the application (it sends, publishes or calls another service).",
        Effect::System => "This changes the application itself.",
        Effect::Read => "The application asks before this tool runs.",
        _ => "This changes the application.",
    }
}

/* ------------------------------------------------------------------------------------------ the view */

/// A tool row's handle (the view's own numbering).
pub type RowId = usize;

/// What a tool row rolls down to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowDetail {
    pub name: String,
    pub title: String,
    pub status: String,
    pub detail: CallDetail,
}

/// What Undo restores (a memory the agent saved, changed or deleted): pass it to [`Session::undo`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undo {
    RestoreNote { id: String, text: String },
    RemoveNote { id: String },
    ReinsertNote(MemoryNote),
}

/// The reply being written: the streamed text (with any inline `<think>` reasoning), the reasoning the provider sends
/// separately, and the status line. The answer is `split_reasoning(raw).text`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    pub raw: String,
    pub native: String,
    pub status: String,
}

/// How the session shows a question's progress and asks the user. Every method but `row` and `decide` may do nothing.
pub trait View {
    fn paint(&mut self, _reply: &Reply, _now: bool) {}
    fn notice(&mut self, _text: &str) {}
    fn row(&mut self, title: &str, call: &str) -> RowId;
    fn row_status(&mut self, _row: RowId, _status: &str, _summary: &str) {}
    fn row_label(&mut self, _row: RowId, _text: &str) {}
    fn row_detail(&mut self, _row: RowId, _info: &RowDetail) {}
    fn row_shot(&mut self, _row: RowId, _id: &str, _thumb: &str) {}
    fn row_undo(&mut self, _row: RowId, _undo: &Undo) {}
    /// Ask the user; returns a button id, or `cancel` when `cancel` fires meanwhile.
    fn decide(&mut self, row: RowId, card: &Card, cancel: &CancelToken) -> String;
}

/// Runs the application's tools (where the host is).
pub trait ToolRunner {
    /// Run a tool with validated arguments: its value (null = "Done."), or the error message.
    fn run(&mut self, tool: &ToolContract, args: &Map, call_id: &str, cancel: &CancelToken) -> Result<Value, String>;
    /// Before the session reads the screen again after a round of actions: bring the context up to date.
    fn refresh_context(&mut self, _ctx: &mut ContextManager) {}
}

/// A screenshot taken for the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Screenshot {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub mime: String,
    pub data: String,
    pub thumb: String,
}

/// Where screenshots come from: the app's own window, or none.
pub trait ScreenSource: Send {
    /// How screenshots are taken; `none` means they cannot be.
    fn method(&self) -> &str;
    /// The first screenshot needs the user's gesture (the share card).
    fn needs_gesture(&self) -> bool {
        false
    }
    fn take(&mut self) -> Result<Screenshot, String>;
}

/// Reads a file of the capability folder (a skill's files).
pub trait FileSource: Send + Sync {
    fn read(&self, path: &str) -> Result<String, String>;
}

/// Where saved chats are kept: the same JSON list the JS runtime keeps in localStorage.
pub trait ChatStore: Send {
    fn read(&self) -> Option<String>;
    /// false when it could not be written (full).
    fn write(&mut self, json: &str) -> bool;
}

/// What happened, for the host (the JS runtime's events).
#[derive(Clone, Debug, PartialEq)]
pub enum SessionEvent {
    Skill {
        name: String,
        via: String,
    },
    Tool {
        name: String,
        args: Map,
        status: String,
        result: String,
    },
    ToolState {
        name: String,
        enabled: bool,
    },
    Send {
        text: String,
        attached: bool,
        reason: String,
        hash: Option<String>,
    },
    Reply {
        text: String,
        provider: String,
        model: String,
        stopped: bool,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    /// The settings changed (a tool turned on, screenshots freed): persist them.
    Settings,
    /// The notes changed: persist the user's layer (`MemoryStore::local_value`).
    Memory,
}

/* ------------------------------------------------------------------------------------------ built-ins */

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuiltinKind {
    Remember,
    Forget,
    TakeScreenshot,
    UseSkill,
    ReadSkillFile,
}

struct Builtin {
    kind: BuiltinKind,
    contract: ToolContract,
}

fn builtin_contract(def: &str, kind: &str) -> ToolContract {
    let v = Value::parse(def).expect("built-in tool definition");
    let mut t = normalize_tool(&v).expect("built-in tool contract");
    t.enabled = true;
    t.builtin = Some(kind.to_string());
    t
}

/// What a built-in returns.
#[derive(Default)]
struct BuiltinOut {
    content: Option<String>,
    status: Option<&'static str>,
    summary: Option<String>,
    images: Vec<Image>,
    thumb: String,
    shot: String,
    undo: Option<Undo>,
    label: Option<&'static str>,
}

impl BuiltinOut {
    fn error(content: String) -> BuiltinOut {
        BuiltinOut { status: Some("error"), content: Some(content), ..Default::default() }
    }
}

/* ---------------------------------------------------------------------------------------- the session */

/// A tool call's outcome, for the model.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallOutcome {
    pub id: String,
    pub name: String,
    pub content: String,
    /// A tool that is not read-only ran (the screen may have changed).
    pub changed: bool,
    pub images: Vec<Image>,
}

/// A question in progress (what [`Session::begin_turn`] prepared).
#[derive(Clone, Debug)]
pub struct Turn {
    pub user_id: String,
    pub plan: Plan,
    pub snapshot_hash: Option<String>,
    pub vision: bool,
    pub base: PromptParts,
    pub messages: Vec<RequestMessage>,
}

/// How a question ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnEnd {
    pub kept: bool,
    pub answer: String,
    pub cancelled: bool,
    pub meta: String,
}

/// The sync flag as it is now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub state: FlagState,
    pub pending: bool,
    pub reason: SyncReason,
    pub hash: Option<String>,
    pub synced_hash: Option<String>,
    pub page_id: String,
    pub title: String,
    pub chars: usize,
    pub truncated: bool,
    pub forced: bool,
}

/// The conversation: its id, transcript, and the skills active in it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Conversation {
    pub id: Option<String>,
    pub messages: Vec<Message>,
    pub skills: Vec<String>,
}

static IDS: AtomicU64 = AtomicU64::new(0);

fn base36(mut n: u64) -> String {
    let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let mut out = Vec::new();
    loop {
        out.push(digits[(n % 36) as usize]);
        n /= 36;
        if n == 0 {
            break;
        }
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

fn uid(prefix: &str, now: f64) -> String {
    format!("{prefix}{}{}", base36(now.max(0.0) as u64), base36(IDS.fetch_add(1, Ordering::Relaxed)))
}

/// A tool row's detail as saved chats keep it: each part capped at [`DETAIL_SAVED`] characters.
pub fn saved_detail(d: &CallDetail) -> Value {
    let cap = |v: &str| {
        let n = utf16_len(v);
        if n > DETAIL_SAVED {
            format!(
                "{}… [{} more characters not kept in saved chats]",
                js_head(v, DETAIL_SAVED),
                locale_count((n - DETAIL_SAVED) as f64)
            )
        } else {
            v.to_string()
        }
    };
    let mut m = Map::new();
    m.insert("args", cap(&d.args));
    m.insert("sent", cap(&d.sent));
    m.insert("problem", cap(&d.problem));
    m.insert("result", cap(&d.result));
    Value::Object(m)
}

/// The question a message with attachments and no text asks.
pub fn default_question(text: &str, shots: &[Shot], files: &[FileRecord]) -> String {
    let t = js_trim(text);
    if !t.is_empty() || (shots.is_empty() && files.is_empty()) {
        return t.to_string();
    }
    let images = shots.iter().filter(|s| !s.name.is_empty()).count();
    if files.is_empty() && images == 0 {
        return "Here is a screenshot of what I am looking at.".into();
    }
    if !files.is_empty() && shots.is_empty() {
        return if files.len() == 1 {
            "Please look at the attached file."
        } else {
            "Please look at the attached files."
        }
        .into();
    }
    if images > 0 && files.is_empty() && images == shots.len() {
        return if images == 1 { "Please look at the attached image." } else { "Please look at the attached images." }
            .into();
    }
    "Please look at what I attached.".into()
}

/// The session.
pub struct Session {
    pub ctx: ContextManager,
    /// The live settings (what the user saved; tools turned on during a question change them).
    pub settings: Settings,
    /// The application's tools (contracts; the [`ToolRunner`] runs them).
    pub tools: Vec<ToolContract>,
    pub agents: Vec<Agent>,
    pub skills: Vec<Skill>,
    /// Toolset name → the names of its tools.
    pub toolsets: BTreeMap<String, Vec<String>>,
    /// The application's permission rules.
    pub policy: Policy,
    pub default_agent: String,
    /// The implicit agent's title.
    pub title: String,
    /// The application's system prompt (what an empty settings prompt means).
    pub default_prompt: String,
    pub memory: Option<MemoryStore>,
    pub screen: Option<Box<dyn ScreenSource>>,
    /// The user can attach images (the model then sees images even without screenshots).
    pub attachments: bool,
    pub files: Option<Box<dyn FileSource>>,
    pub chats: Option<Box<dyn ChatStore>>,
    /// Milliseconds since the epoch (tests fix it).
    pub clock: Box<dyn Fn() -> f64 + Send>,
    pub conv: Conversation,
    /// The active agent ('' = the default one).
    pub agent_name: String,
    /// The user pressed Re-read: the next question carries the page.
    pub force_reread: bool,
    /// Write tools the user allowed "for this chat".
    pub allowed_tools: BTreeSet<String>,
    /// Provider+model pairs that refused native tool calls (auto mode).
    pub text_tools: BTreeSet<String>,
    builtins: Vec<Builtin>,
    events: Vec<SessionEvent>,
}

impl Session {
    /// A session over this context and these settings; the rest is set on the fields, then [`Session::init`].
    pub fn new(ctx: ContextManager, settings: Settings) -> Session {
        Session {
            ctx,
            settings,
            tools: Vec::new(),
            agents: Vec::new(),
            skills: Vec::new(),
            toolsets: BTreeMap::new(),
            policy: Policy::default(),
            default_agent: String::new(),
            title: "AI agent".into(),
            default_prompt: crate::prompt::DEFAULT_SYSTEM_PROMPT.into(),
            memory: None,
            screen: None,
            attachments: false,
            files: None,
            chats: None,
            clock: Box::new(crate::time::now_millis),
            conv: Conversation::default(),
            agent_name: String::new(),
            force_reread: false,
            allowed_tools: BTreeSet::new(),
            text_tools: BTreeSet::new(),
            builtins: Vec::new(),
            events: Vec::new(),
        }
    }

    /// Define the built-in tools for what the session has now (memory, a screen source). Call it after setting them.
    pub fn init(&mut self) {
        let mut list = Vec::new();
        if let Some(mem) = &self.memory {
            let remember = format!(
                r#"{{"name":"remember","title":"Remember","description":"Save a note to your long-term memory so that you still know it in future conversations. Use it when the user asks you to remember, note or keep something.","parameters":{{"text":{{"type":"string","required":true,"maxLength":{},"description":"One short, self-contained sentence: what to remember and when it applies."}},"id":{{"type":"string","description":"Only to correct an existing memory: its id (for example m3)."}}}},"effect":"read"}}"#,
                mem.max_chars
            );
            list.push(Builtin { kind: BuiltinKind::Remember, contract: builtin_contract(&remember, "memory") });
            list.push(Builtin {
                kind: BuiltinKind::Forget,
                contract: builtin_contract(
                    r#"{"name":"forget","title":"Forget","description":"Delete one note from your long-term memory. Use it when the user asks you to forget something you saved.","parameters":{"id":{"type":"string","required":true,"description":"The id of the memory to delete (for example m3)."}},"effect":"write"}"#,
                    "memory",
                ),
            });
        }
        if self.screen.is_some() {
            list.push(Builtin {
                kind: BuiltinKind::TakeScreenshot,
                contract: builtin_contract(
                    r#"{"name":"take_screenshot","title":"Take screenshot","description":"Look at the user's screen: takes a screenshot of what they are looking at right now and shows it to you. Use it when the page snapshot text is not enough: layout, colours, charts, images, canvas or 3D views, visual problems.","parameters":{},"effect":"read"}"#,
                    "vision",
                ),
            });
        }
        list.push(Builtin {
            kind: BuiltinKind::UseSkill,
            contract: builtin_contract(
                r#"{"name":"use_skill","title":"Use skill","description":"Load a skill: the instructions for a kind of work, listed under SKILLS. Call it before doing work a skill describes; the skill then stays active for this conversation.","parameters":{"name":{"type":"string","required":true,"maxLength":64,"description":"The skill's name, as listed under SKILLS."}},"effect":"read"}"#,
                "skills",
            ),
        });
        list.push(Builtin {
            kind: BuiltinKind::ReadSkillFile,
            contract: builtin_contract(
                r#"{"name":"read_skill_file","title":"Read skill file","description":"Read a file that a skill's instructions point to (a path relative to the skill's folder, such as references/guide.md).","parameters":{"skill":{"type":"string","required":true,"maxLength":64,"description":"The skill's name."},"path":{"type":"string","required":true,"maxLength":300,"description":"The file, relative to the skill's folder."},"offset":{"type":"integer","min":0,"description":"Start at this character, to read a long file in parts (default 0)."}},"effect":"read"}"#,
                "skills",
            ),
        });
        self.builtins = list;
    }

    fn now(&self) -> f64 {
        (self.clock)()
    }

    fn emit(&mut self, e: SessionEvent) {
        self.events.push(e);
    }

    /// The events since the last call.
    pub fn take_events(&mut self) -> Vec<SessionEvent> {
        std::mem::take(&mut self.events)
    }

    /* -------------------------------------------------------------------------------------- agents */

    /// The agents the user can pick from (the implicit agent when the application defines none).
    pub fn agent_list(&self) -> Vec<Agent> {
        if self.agents.is_empty() {
            vec![implicit_agent(&self.title)]
        } else {
            self.agents.clone()
        }
    }

    /// The active agent.
    pub fn agent(&self) -> Agent {
        let list = self.agent_list();
        list.iter()
            .find(|a| a.name == self.agent_name)
            .or_else(|| pick_agent(&list, &self.default_agent))
            .cloned()
            .unwrap_or_else(|| implicit_agent(&self.title))
    }

    /// The permission rules in force: the application's, plus the active agent's.
    pub fn current_policy(&self) -> Policy {
        let agent = self.agent();
        merge_policies([&self.policy, &agent.permissions])
    }

    fn denied(&self, t: &ToolContract, policy: &Policy) -> bool {
        decide_permission(&Subject::from(t), policy) == Some(Decision::Deny)
    }

    /// The application's tools the active agent may use (its tools and toolsets, all when it names none), not denied.
    pub fn agent_tools(&self) -> Vec<&ToolContract> {
        let agent = self.agent();
        let names: Vec<String> = self.tools.iter().map(|t| t.name.clone()).collect();
        let (allowed, _) = agent_tool_names(&agent, &names, &self.toolsets);
        let policy = self.current_policy();
        self.tools
            .iter()
            .filter(|t| allowed.as_ref().map_or(true, |n| n.contains(&t.name)) && !self.denied(t, &policy))
            .collect()
    }

    pub fn agent_has_tool(&self, name: &str) -> bool {
        self.agent_tools().iter().any(|t| t.name == name)
    }

    /// The skills the active agent may use.
    pub fn skills_for_agent(&self) -> Vec<&Skill> {
        let app: Vec<String> = self.skills.iter().filter(|s| s.source != "builtin").map(|s| s.name.clone()).collect();
        let (names, _) = agent_skill_names(&self.agent(), &app);
        self.skills
            .iter()
            .filter(|s| s.source == "builtin" || names.as_ref().map_or(true, |n| n.contains(&s.name)))
            .collect()
    }

    /// What the active agent may do with memory.
    pub fn memory_mode(&self) -> MemoryMode {
        if self.memory.is_some() {
            self.agent().memory
        } else {
            MemoryMode::Off
        }
    }

    /// Is screen content shared with the active agent (the user's switch and the agent's context layers)?
    pub fn share_on(&self, s: &Settings) -> bool {
        s.share_screen && self.agent().context.screen
    }

    /// The context texts the active agent receives: (app, page, view, share).
    pub fn context_texts(&self, s: &Settings) -> (String, String, String, bool) {
        let layers = self.agent().context;
        let share = self.share_on(s);
        let app = if layers.app { self.ctx.app_text() } else { String::new() };
        let page = if layers.page { self.ctx.page_text() } else { String::new() };
        let view = if share && layers.view { self.ctx.view_text() } else { String::new() };
        (app, page, view, share)
    }

    pub fn effective_prompt(&self, s: &Settings) -> String {
        if js_trim(&s.system_prompt).is_empty() {
            self.default_prompt.clone()
        } else {
            s.system_prompt.clone()
        }
    }

    /// What every request of a question shares: the editable prompt, the agent, the context, attached files.
    pub fn base_system(&self, s: &Settings) -> PromptParts {
        let (app_text, page_text, _, share) = self.context_texts(s);
        PromptParts {
            base: self.effective_prompt(s),
            agent_text: build_agent_prompt(&self.agent()),
            app_text,
            page_text,
            share,
            files: has_files(&self.conv.messages, s.history_messages),
            ..PromptParts::default()
        }
    }

    /// The whole system prompt for the next request.
    pub fn system_prompt(&self, s: &Settings) -> String {
        let mode = self.tool_mode(s);
        let mut p = self.base_system(s);
        p.tools_text = self.tool_prompt(s);
        p.memory_text = self.memory_prompt(s);
        p.skills_text = self.skills_prompt(mode != ToolMode::Off);
        p.vision = self.sees_images(s);
        p.screenshots = self.vision_on(s);
        build_system_prompt(&p)
    }

    pub fn skills_prompt(&self, can_load: bool) -> String {
        build_skills_prompt(&self.skills_for_agent(), &self.conv.skills, can_load)
    }

    /* -------------------------------------------------------------------------------------- skills */

    /// Make a skill active in this conversation. None when the agent has no such skill.
    pub fn activate_skill(&mut self, name: &str, via: &str) -> Option<Skill> {
        let skill = self.skills_for_agent().into_iter().find(|s| s.name == name).cloned()?;
        if !self.conv.skills.iter().any(|s| s == name) {
            self.conv.skills.push(name.to_string());
        }
        self.emit(SessionEvent::Skill { name: name.to_string(), via: via.to_string() });
        Some(skill)
    }

    /// How a tool a skill names stands for the active agent.
    pub fn tool_state_for(&self, name: &str) -> ToolState {
        let Some(tool) = self.tools.iter().find(|t| t.name == name) else {
            return if self.builtins_for(&self.settings).iter().any(|b| b.contract.name == name) {
                ToolState::On
            } else {
                ToolState::Missing
            };
        };
        if !self.agent_has_tool(name) {
            return ToolState::Denied;
        }
        if tool_enabled(tool, &self.settings.tool_states) {
            ToolState::On
        } else {
            ToolState::Off
        }
    }

    /* --------------------------------------------------------------------------------------- tools */

    fn builtin_exists(&self, b: &Builtin, s: &Settings) -> bool {
        match b.kind {
            BuiltinKind::Remember | BuiltinKind::Forget => self.memory_writable(s),
            BuiltinKind::TakeScreenshot => s.vision && self.screen.as_ref().is_some_and(|sc| sc.method() != "none"),
            BuiltinKind::UseSkill | BuiltinKind::ReadSkillFile => !self.skills_for_agent().is_empty(),
        }
    }

    fn builtin_enabled(&self, b: &Builtin, s: &Settings) -> bool {
        match b.kind {
            BuiltinKind::TakeScreenshot => s.screenshot_auto,
            _ => self.builtin_exists(b, s),
        }
    }

    fn memory_writable(&self, s: &Settings) -> bool {
        s.memory_enabled && s.memory_write && self.memory_mode() == MemoryMode::On
    }

    /// The built-ins that exist with these settings and are not denied.
    fn builtins_for(&self, s: &Settings) -> Vec<&Builtin> {
        let policy = self.current_policy();
        self.builtins.iter().filter(|b| self.builtin_exists(b, s) && !self.denied(&b.contract, &policy)).collect()
    }

    fn builtin_by_name(&self, name: &str) -> Option<&Builtin> {
        self.builtins.iter().find(|b| b.contract.name == name)
    }

    /// Is this tool turned on? Built-ins follow their own settings.
    fn enabled(&self, t: &ToolContract, s: &Settings) -> bool {
        match self.builtin_by_name(&t.name).filter(|b| t.builtin.is_some() && b.contract.name == t.name) {
            Some(b) => self.builtin_enabled(b, s),
            None => tool_enabled(t, &s.tool_states),
        }
    }

    /// Off (no tools), native tool calls, or text blocks.
    pub fn tool_mode(&self, s: &Settings) -> ToolMode {
        if self.agent_tools().is_empty() && self.builtins_for(s).is_empty() {
            return ToolMode::Off;
        }
        match s.tool_mode.as_str() {
            "native" => ToolMode::Native,
            "text" => ToolMode::Text,
            _ if self.text_tools.contains(&self.model_key(s)) => ToolMode::Text,
            _ => ToolMode::Native,
        }
    }

    pub fn model_key(&self, s: &Settings) -> String {
        let model = s.profiles.get(&s.provider).and_then(|p| p.model.clone()).unwrap_or_default();
        format!("{}|{}|{model}", s.transport, s.provider)
    }

    /// The tools of one request: (classes, active, app_off).
    fn tool_set<'a>(&'a self, s: &Settings) -> (ToolClasses<'a>, bool, bool) {
        let app = self.agent_tools();
        let page = self.ctx.page.as_ref().map(|p| p.id.clone()).unwrap_or_default();
        let builtins: Vec<&Builtin> =
            self.builtins_for(s).into_iter().filter(|b| !app.iter().any(|t| t.name == b.contract.name)).collect();
        if builtins.is_empty() {
            return (classify_tools(app, &s.tool_states, &page), s.tools_enabled, false);
        }
        let app_off = !s.tools_enabled && !app.is_empty();
        let mut classes =
            if s.tools_enabled { classify_tools(app, &s.tool_states, &page) } else { ToolClasses::default() };
        for b in builtins {
            if self.builtin_enabled(b, s) {
                classes.callable.push(&b.contract);
            } else if b.kind == BuiltinKind::TakeScreenshot {
                classes.off.push(&b.contract);
            }
        }
        (classes, true, app_off)
    }

    /// The TOOLS section, as it would be sent now.
    pub fn tool_prompt(&self, s: &Settings) -> String {
        let mode = self.tool_mode(s);
        if mode == ToolMode::Off {
            return String::new();
        }
        let (classes, active, app_off) = self.tool_set(s);
        build_tool_prompt(&ToolPrompt { classes: &classes, mode, enabled: active, app_off, page_label: None })
    }

    /// A tool the active agent may use (app tools it may see, then built-ins), with its built-in kind.
    fn find_tool(&self, name: &str, s: &Settings) -> Option<(ToolContract, Option<BuiltinKind>)> {
        if let Some(t) = self.agent_tools().into_iter().find(|t| t.name == name) {
            return Some((t.clone(), None));
        }
        self.builtins_for(s).into_iter().find(|b| b.contract.name == name).map(|b| (b.contract.clone(), Some(b.kind)))
    }

    /// Why a tool by this name is not usable here ('' when there is no such tool at all).
    fn hidden_reason(&self, name: &str) -> String {
        let tool =
            self.tools.iter().find(|t| t.name == name).or_else(|| self.builtin_by_name(name).map(|b| &b.contract));
        let Some(tool) = tool else { return String::new() };
        let agent = self.agent();
        if self.denied(tool, &self.current_policy()) {
            let who = if agent.implicit { String::new() } else { format!(" for the {} agent", agent.title) };
            return format!(
                "{name} is not permitted in this application{who}. Do not call it; tell the user if it matters."
            );
        }
        if tool.builtin.is_none() {
            return format!("{name} is not one of the {} agent's tools. Use the tools you were given.", agent.title);
        }
        String::new()
    }

    /// Turn a tool on or off for this user (a built-in follows its own setting).
    pub fn set_tool_enabled(&mut self, name: &str, on: bool) {
        let builtin =
            if self.tools.iter().any(|t| t.name == name) { None } else { self.builtin_by_name(name).map(|b| b.kind) };
        match builtin {
            Some(BuiltinKind::TakeScreenshot) => self.settings.screenshot_auto = on,
            Some(_) => return,
            None => {
                self.settings.tool_states.insert(name.to_string(), on);
            }
        }
        self.emit(SessionEvent::Settings);
        self.emit(SessionEvent::ToolState { name: name.to_string(), enabled: on });
    }

    /// The MEMORY section, as it would be sent now.
    pub fn memory_prompt(&self, s: &Settings) -> String {
        let mode = self.memory_mode();
        let Some(mem) = &self.memory else { return String::new() };
        if mode == MemoryMode::Off {
            return String::new();
        }
        build_memory_prompt(&mem.list(), s.memory_enabled, mode == MemoryMode::On && s.memory_enabled && s.memory_write)
    }

    /// Can screenshots be taken and shown to the model?
    pub fn vision_on(&self, s: &Settings) -> bool {
        s.vision && self.screen.as_ref().is_some_and(|sc| sc.method() != "none")
    }

    /// Can the model be shown images at all?
    pub fn sees_images(&self, s: &Settings) -> bool {
        s.vision && (self.vision_on(s) || self.attachments)
    }

    /// Undo what a built-in did (the row's Undo button).
    pub fn undo(&mut self, undo: &Undo) -> Result<(), String> {
        let Some(mem) = self.memory.as_mut() else { return Err("There is no memory.".into()) };
        match undo {
            Undo::RestoreNote { id, text } => {
                mem.update(id, text)?;
            }
            Undo::RemoveNote { id } => {
                mem.remove(id);
            }
            Undo::ReinsertNote(note) => {
                let mut all: Vec<Value> = mem.list().iter().map(MemoryNote::to_value).collect();
                all.push(note.to_value());
                mem.replace_all(&all, "user");
            }
        }
        self.emit(SessionEvent::Memory);
        Ok(())
    }

    /* --------------------------------------------------------------------------------- the tool loop */

    fn decide(&mut self, view: &mut dyn View, row: RowId, card: &Card, cancel: &CancelToken) -> String {
        view.row_status(row, "waiting", "");
        view.decide(row, card, cancel)
    }

    /// The agent looks at the screen (asking first when it is not freed to).
    fn agent_screenshot(&mut self, view: &mut dyn View, row: RowId, cancel: &CancelToken, allowed: bool) -> BuiltinOut {
        if !allowed {
            let choice = self.decide(view, row, &Card::new(CardKind::Look), cancel);
            match choice.as_str() {
                "cancel" => return BuiltinOut { status: Some("skipped"), content: Some("Stopped by the user.".into()), ..Default::default() },
                "no" => {
                    return BuiltinOut {
                        status: Some("declined"),
                        content: Some("The user did not allow a screenshot. Do not ask again for this request: work from the page snapshot, or ask the user to describe what they see.".into()),
                        ..Default::default()
                    }
                }
                "always" => self.set_tool_enabled("take_screenshot", true),
                _ => {}
            }
        } else if self.screen.as_ref().is_some_and(|s| s.needs_gesture()) {
            let choice = self.decide(view, row, &Card::new(CardKind::ShareTab), cancel);
            if choice == "cancel" {
                return BuiltinOut {
                    status: Some("skipped"),
                    content: Some("Stopped by the user.".into()),
                    ..Default::default()
                };
            }
            if choice != "share" {
                return BuiltinOut {
                    status: Some("declined"),
                    content: Some("The user did not share the screen. Do not ask again for this request: work from the page snapshot.".into()),
                    ..Default::default()
                };
            }
        }
        view.row_status(row, "running", "");
        let shot = match self.screen.as_mut().map(|s| s.take()) {
            Some(Ok(s)) => s,
            Some(Err(e)) => {
                return BuiltinOut {
                    status: Some("error"),
                    content: Some(format!("Error: {e}")),
                    summary: Some(e),
                    ..Default::default()
                }
            }
            None => return BuiltinOut::error("Error: Screenshots are switched off for this application.".into()),
        };
        BuiltinOut {
            content: Some(format!(
                "Screenshot taken ({} × {} px). It is attached: use what you see in it.",
                shot.width, shot.height
            )),
            summary: Some(format!("{} × {} px", shot.width, shot.height)),
            images: vec![Image { mime: shot.mime, data: shot.data }],
            thumb: shot.thumb,
            shot: shot.id,
            ..Default::default()
        }
    }

    fn named_skill(&self, name: &str) -> Option<Skill> {
        let n = js_trim(name);
        self.skills_for_agent().into_iter().find(|s| s.name == n).cloned()
    }

    fn skill_list(&self) -> String {
        let names: Vec<String> = self.skills_for_agent().iter().map(|s| s.name.clone()).collect();
        if names.is_empty() {
            "none".into()
        } else {
            names.join(", ")
        }
    }

    /// Run a built-in (after its checks and confirmation).
    fn run_builtin(
        &mut self,
        kind: BuiltinKind,
        args: &Map,
        view: &mut dyn View,
        row: RowId,
        cancel: &CancelToken,
    ) -> BuiltinOut {
        let s = |k: &str| args.get(k).filter(|v| !v.is_null()).map(|v| v.js_string());
        match kind {
            BuiltinKind::Remember => {
                let text = s("text").unwrap_or_default();
                let id = s("id").filter(|i| !i.is_empty());
                let Some(mem) = self.memory.as_mut() else {
                    return BuiltinOut::error("Error: There is no memory.".into());
                };
                let out = if let Some(id) = id {
                    let Some(prev) = mem.get(&id) else {
                        return BuiltinOut::error(format!(
                            "There is no memory with the id \"{id}\". Call remember without an id to add a new one."
                        ));
                    };
                    match mem.update(&id, &text) {
                        Ok(Some(m)) => BuiltinOut {
                            content: Some(format!("Updated memory {}.", m.id)),
                            summary: Some(m.text),
                            undo: Some(Undo::RestoreNote { id: id.clone(), text: prev.text }),
                            label: Some("Updated"),
                            ..Default::default()
                        },
                        Ok(None) => BuiltinOut::error(format!(
                            "There is no memory with the id \"{id}\". Call remember without an id to add a new one."
                        )),
                        Err(e) => BuiltinOut {
                            status: Some("error"),
                            content: Some(format!("Error: {e}")),
                            summary: Some(e),
                            ..Default::default()
                        },
                    }
                } else {
                    let count = mem.list().len();
                    match mem.add(&text, "agent") {
                        Ok(m) => {
                            let added = mem.list().len() > count;
                            BuiltinOut {
                                content: Some(if added {
                                    format!("Saved as memory {}.", m.id)
                                } else {
                                    format!("That is already saved (memory {}).", m.id)
                                }),
                                summary: Some(m.text.clone()),
                                undo: added.then(|| Undo::RemoveNote { id: m.id.clone() }),
                                label: Some(if added { "Saved" } else { "Already saved" }),
                                ..Default::default()
                            }
                        }
                        Err(e) => BuiltinOut {
                            status: Some("error"),
                            content: Some(format!("Error: {e}")),
                            summary: Some(e),
                            ..Default::default()
                        },
                    }
                };
                self.emit(SessionEvent::Memory);
                out
            }
            BuiltinKind::Forget => {
                let id = s("id").unwrap_or_default();
                let Some(mem) = self.memory.as_mut() else {
                    return BuiltinOut::error("Error: There is no memory.".into());
                };
                match mem.remove(&id) {
                    None => BuiltinOut::error(format!("There is no memory with the id \"{id}\".")),
                    Some(m) => {
                        self.emit(SessionEvent::Memory);
                        BuiltinOut {
                            content: Some(format!("Deleted memory {id}.")),
                            summary: Some(m.text.clone()),
                            undo: Some(Undo::ReinsertNote(m)),
                            label: Some("Deleted"),
                            ..Default::default()
                        }
                    }
                }
            }
            BuiltinKind::TakeScreenshot => self.agent_screenshot(view, row, cancel, true),
            BuiltinKind::UseSkill => {
                let name = s("name").unwrap_or_default();
                let Some(skill) = self.named_skill(&name) else {
                    return BuiltinOut::error(format!(
                        "There is no skill named \"{name}\". The skills you can use: {}.",
                        self.skill_list()
                    ));
                };
                let already = self.conv.skills.contains(&skill.name);
                self.activate_skill(&skill.name, "tool");
                let content = skill_loaded_message(&skill, |t| self.tool_state_for(t));
                BuiltinOut {
                    content: Some(content),
                    summary: Some(skill.description.clone()),
                    label: Some(if already { "Already active" } else { "Loaded" }),
                    ..Default::default()
                }
            }
            BuiltinKind::ReadSkillFile => {
                let name = s("skill").unwrap_or_default();
                let path = s("path").unwrap_or_default();
                let offset = args.get("offset").and_then(|v| v.as_f64()).unwrap_or(0.0).max(0.0) as usize;
                let Some(skill) = self.named_skill(&name) else {
                    return BuiltinOut::error(format!(
                        "There is no skill named \"{name}\". The skills you can use: {}.",
                        self.skill_list()
                    ));
                };
                if skill.base.is_empty() {
                    return BuiltinOut::error(format!(
                        "The skill \"{}\" has no folder, so it has no files to read.",
                        skill.name
                    ));
                }
                let rel = match skill_file_path(&path) {
                    Ok(r) => r,
                    Err(e) => return BuiltinOut::error(e),
                };
                let read = match &self.files {
                    Some(f) => f.read(&format!("{}{rel}", skill.base)),
                    None => Err("no files".to_string()),
                };
                let text = match read {
                    Ok(t) => t,
                    Err(e) => {
                        return BuiltinOut::error(format!(
                            "{rel} could not be read ({e}). Check the path against the skill's instructions."
                        ));
                    }
                };
                let len = utf16_len(&text);
                let start = offset.min(len);
                let end = len.min(start + SKILL_FILE_MAX);
                let more = if end < len {
                    format!("\n\n[{} more characters: call again with offset {end}.]", locale_count((len - end) as f64))
                } else {
                    String::new()
                };
                let showing = if start > 0 || !more.is_empty() {
                    format!(", showing {}–{}", locale_count(start as f64), locale_count(end as f64))
                } else {
                    String::new()
                };
                BuiltinOut {
                    content: Some(format!(
                        "{}/{rel} ({} characters{showing}):\n\n{}{more}",
                        skill.name,
                        locale_count(len as f64),
                        js_slice(&text, start as i64, Some(end as i64))
                    )),
                    summary: Some(format!("{}/{rel}", skill.name)),
                    ..Default::default()
                }
            }
        }
    }

    /// Turn what a built-in returned into the row, the action record and the result for the model.
    #[allow(clippy::too_many_arguments)]
    fn finish_builtin(
        &mut self,
        tool: &ToolContract,
        out: BuiltinOut,
        view: &mut dyn View,
        row: RowId,
        actions: &mut Vec<Action>,
        call_id: &str,
        call_name: &str,
        args: &Map,
    ) -> CallOutcome {
        let status = out.status.unwrap_or("ok");
        let content = out.content.clone().unwrap_or_else(|| "Done.".to_string());
        let summary = js_head(&out.summary.clone().or(out.content.clone()).unwrap_or_default(), 200).to_string();
        view.row_status(row, status, &summary);
        if let (Some(label), "ok") = (out.label, status) {
            view.row_label(row, label);
        }
        if !out.thumb.is_empty() {
            view.row_shot(row, &out.shot, &out.thumb);
        }
        if let (Some(undo), "ok") = (&out.undo, status) {
            view.row_undo(row, undo);
        }
        let as_call = ToolCall { arguments: args.clone(), ..Default::default() };
        let detail = call_detail(&as_call, args, "", &content);
        view.row_detail(
            row,
            &RowDetail {
                name: tool.name.clone(),
                title: tool.title.clone(),
                status: status.into(),
                detail: detail.clone(),
            },
        );
        actions.push(Action {
            call: format_call(&tool.name, args),
            title: tool.title.clone(),
            status: status.into(),
            summary: Some(summary.clone()),
            name: tool.name.clone(),
            detail: Some(saved_detail(&detail)),
            thumb: out.thumb.clone(),
            shot: if out.thumb.is_empty() { String::new() } else { out.shot.clone() },
        });
        self.emit(SessionEvent::Tool {
            name: tool.name.clone(),
            args: args.clone(),
            status: status.into(),
            result: summary,
        });
        CallOutcome { id: call_id.into(), name: call_name.into(), content, changed: false, images: out.images }
    }

    /// Run one tool call from the model, in this order: request_tool; unknown or denied; the master switch; the tool is
    /// off; not on this page; unreadable arguments; invalid arguments; the confirmation; the run.
    #[allow(clippy::too_many_arguments)]
    pub fn execute_tool(
        &mut self,
        call: &ToolCall,
        reply: &mut Reply,
        view: &mut dyn View,
        runner: &mut dyn ToolRunner,
        actions: &mut Vec<Action>,
        cancel: &CancelToken,
    ) -> CallOutcome {
        let outcome = |content: String, changed: bool| CallOutcome {
            id: call.id.clone(),
            name: call.name.clone(),
            content,
            changed,
            images: Vec::new(),
        };

        if call.name == REQUEST_TOOL {
            let raw_name = call.arguments.get("name");
            let wanted = raw_name.filter(|v| v.truthy()).map(|v| v.js_string()).unwrap_or_default();
            let settings = self.settings.clone();
            let Some((want, kind)) = self.find_tool(&wanted, &settings) else {
                let shown = raw_name.map(|v| v.js_string()).unwrap_or_else(|| "undefined".into());
                return outcome(format!("There is no tool named \"{shown}\"."), false);
            };
            if self.enabled(&want, &settings) {
                return outcome(format!("{} is already turned on: call it.", want.name), false);
            }
            if kind == Some(BuiltinKind::TakeScreenshot) {
                let row = view.row(&want.title, &format_call(&want.name, &Map::new()));
                let out = self.agent_screenshot(view, row, cancel, false);
                return self.finish_builtin(&want, out, view, row, actions, &call.id, &call.name, &Map::new());
            }
            let title = format!("Turn on {}", want.title);
            let row = view.row(&title, &format_call(&want.name, &Map::new()));
            let reason = call
                .arguments
                .get("reason")
                .filter(|v| v.truthy())
                .map(|v| js_head(&v.js_string(), 300).to_string())
                .unwrap_or_default();
            let mut card = Card::new(CardKind::Request);
            card.tool = Some(CardTool { name: want.name.clone(), title: want.title.clone(), effect: want.effect });
            card.reason = reason;
            let choice = self.decide(view, row, &card, cancel);
            let on = choice == "on";
            let (status, content) = if on {
                self.set_tool_enabled(&want.name, true);
                view.row_status(row, "ok", "Turned on");
                view.row_label(row, "Turned on");
                ("ok", format!("The user turned on {}. You can call it now.", want.name))
            } else {
                view.row_status(row, "off", "");
                (
                    "declined",
                    format!("The user kept {} turned off. Do not call it; explain how they can do it themselves, or that they can turn it on in Settings > Tools.", want.name),
                )
            };
            let detail = call_detail(call, &call.arguments, "", &content);
            view.row_detail(
                row,
                &RowDetail {
                    name: REQUEST_TOOL.into(),
                    title: title.clone(),
                    status: status.into(),
                    detail: detail.clone(),
                },
            );
            actions.push(Action {
                call: format!("turn on {}", want.name),
                title,
                status: status.into(),
                summary: None,
                name: REQUEST_TOOL.into(),
                detail: Some(saved_detail(&detail)),
                ..Default::default()
            });
            return outcome(content, false);
        }

        let settings = self.settings.clone();
        let Some((tool, kind)) = self.find_tool(&call.name, &settings) else {
            let why = self.hidden_reason(&call.name);
            let content = if why.is_empty() {
                format!("There is no tool named \"{}\". Use only the tools you were given.", call.name)
            } else {
                why
            };
            return outcome(content, false);
        };
        let decision = decide_permission(&Subject::from(&tool), &self.current_policy());
        let v = validate_args(&tool, &Value::Object(call.arguments.clone()));
        let row = view.row(&tool.title, &format_call(&tool.name, &v.args));
        let entry: Option<usize>;
        let mut problem = String::new();

        // Every way out after the row exists records the action and rolls the row down to what the model was told.
        macro_rules! record {
            ($status:expr, $summary:expr) => {{
                let status: &str = $status;
                let summary: String = $summary;
                view.row_status(row, status, &summary);
                actions.push(Action {
                    call: format_call(&tool.name, &v.args),
                    title: tool.title.clone(),
                    status: status.into(),
                    summary: Some(js_head(&summary, 200).to_string()),
                    ..Default::default()
                });
                entry = Some(actions.len() - 1);
                self.emit(SessionEvent::Tool {
                    name: tool.name.clone(),
                    args: v.args.clone(),
                    status: status.into(),
                    result: summary,
                });
            }};
        }
        macro_rules! reply_with {
            ($content:expr, $changed:expr) => {{
                let content: String = $content;
                let detail = call_detail(call, &v.args, &problem, &content);
                let status = entry.map(|i| actions[i].status.clone()).unwrap_or_else(|| "error".into());
                view.row_detail(
                    row,
                    &RowDetail { name: tool.name.clone(), title: tool.title.clone(), status, detail: detail.clone() },
                );
                if let Some(i) = entry {
                    actions[i].name = tool.name.clone();
                    actions[i].detail = Some(saved_detail(&detail));
                }
                return outcome(content, $changed);
            }};
        }

        if !settings.tools_enabled && kind.is_none() {
            record!("off", String::new());
            reply_with!("Tools are switched off by the user (Settings > Tools).".into(), false);
        }
        let mut consented = false;
        let on = self.enabled(&tool, &settings);
        if kind == Some(BuiltinKind::TakeScreenshot) && !on {
            let out = self.agent_screenshot(view, row, cancel, false);
            return self.finish_builtin(&tool, out, view, row, actions, &call.id, &call.name, &v.args);
        }
        if kind.is_some() && !on {
            record!("off", String::new());
            let msg = match kind {
                Some(BuiltinKind::UseSkill | BuiltinKind::ReadSkillFile) => "This agent has no skills.",
                _ => "Saving memories is switched off by the user (Settings > Memory).",
            };
            reply_with!(msg.into(), false);
        }
        if !on {
            let mut card = Card::new(CardKind::TurnOn);
            card.tool = Some(CardTool { name: tool.name.clone(), title: tool.title.clone(), effect: tool.effect });
            let choice = self.decide(view, row, &card, cancel);
            if choice != "on" {
                record!("off", String::new());
                reply_with!(
                    format!("{} is turned off and the user kept it off. Do not call it again.", tool.name),
                    false
                );
            }
            self.set_tool_enabled(&tool.name, true);
            consented = true;
        }
        let page = self.ctx.page.as_ref().map(|p| p.id.clone()).unwrap_or_default();
        if !tool_available(&tool, &page) {
            record!("skipped", "Not available on this screen".to_string());
            let on_pages = if tool.pages.is_empty() {
                String::new()
            } else {
                format!(" (it works on: {})", tool.pages.join(", "))
            };
            reply_with!(format!("{} is not available on this screen{on_pages}.", tool.name), false);
        }
        if call.args_error.is_some() {
            // The arguments could not be read (often: the reply hit Max reply tokens mid-call). Never guess them.
            let p = arguments_problem(call, call.cut_off, Some(settings.max_output_tokens as f64));
            problem = p.summary.clone();
            record!("error", p.summary.clone());
            reply_with!(p.content, false);
        }
        if !v.ok {
            let errors = v.errors.join("; ");
            problem = format!("Invalid arguments: {errors}.");
            record!("error", errors.clone());
            reply_with!(format!("Invalid arguments: {errors}. Check the tool's parameters and try again."), false);
        }
        let confirm_settings = ConfirmSettings {
            confirm_writes: settings.confirm_writes,
            confirm_destructive: settings.confirm_destructive,
        };
        let ask = needs_confirmation(tool.effect, decision, confirm_settings, self.allowed_tools.contains(&tool.name));
        if ask && (!consented || tool.effect == Effect::System) {
            let mut card = Card::new(CardKind::Confirm);
            card.buttons = confirm_buttons(tool.effect, decision);
            card.tool = Some(CardTool { name: tool.name.clone(), title: tool.title.clone(), effect: tool.effect });
            card.decision = decision;
            if kind == Some(BuiltinKind::Forget) {
                let id = v.args.get("id").map(|x| x.js_string()).unwrap_or_default();
                card.confirm = self
                    .memory
                    .as_ref()
                    .and_then(|m| m.get(&id))
                    .map(|m| Confirm::MemoryDelete { id: id.clone(), text: m.text });
            }
            let choice = self.decide(view, row, &card, cancel);
            match choice.as_str() {
                "cancel" => {
                    record!("skipped", String::new());
                    reply_with!("Stopped by the user.".into(), false);
                }
                "skip" => {
                    record!("declined", String::new());
                    reply_with!(
                        format!("The user declined to run {}. Do not call it again for this request.", tool.name),
                        false
                    );
                }
                "chat" => {
                    self.allowed_tools.insert(tool.name.clone());
                }
                _ => {}
            }
        }

        view.row_status(row, "running", "");
        reply.status = format!("Running {}…", tool.title);
        view.paint(reply, false);
        if let Some(k) = kind {
            let out = self.run_builtin(k, &v.args, view, row, cancel);
            return self.finish_builtin(&tool, out, view, row, actions, &call.id, &call.name, &v.args);
        }
        let changed = tool.effect != Effect::Read;
        match runner.run(&tool, &v.args, &call.id, cancel) {
            Ok(value) => {
                let content = serialize_result(Some(&value), RESULT_MAX_CHARS);
                record!("ok", content.clone());
                reply_with!(content, changed);
            }
            Err(msg) => {
                record!("error", msg.clone());
                reply_with!(format!("Error: {msg}"), changed);
            }
        }
    }

    /// One question to the model, with tool rounds: stream a reply; if it asks for tools, run them, send the results
    /// back, and repeat until it answers or the step limit is reached. Returns the last step's result.
    #[allow(clippy::too_many_arguments)]
    pub fn run_model(
        &mut self,
        settings: &Settings,
        turn: &Turn,
        reply: &mut Reply,
        model: &mut dyn ChatModel,
        view: &mut dyn View,
        runner: &mut dyn ToolRunner,
        actions: &mut Vec<Action>,
        cancel: &CancelToken,
    ) -> Result<ChatResult, AiError> {
        let turn_id = uid("t", self.now());
        let mut tool_turns: Vec<ToolTurn> = Vec::new();
        let mut last_hash = turn.snapshot_hash.clone();
        let mut mode = self.tool_mode(settings);
        let mut result = ChatResult::default();
        let mut step: i64 = 0;
        while step < MAX_ROUNDS {
            let s = self.settings.clone();
            let (request, active) = {
                let (classes, active, app_off) =
                    if mode == ToolMode::Off { (ToolClasses::default(), false, false) } else { self.tool_set(&s) };
                let tools_text = if mode == ToolMode::Off {
                    String::new()
                } else {
                    build_tool_prompt(&ToolPrompt {
                        classes: &classes,
                        mode,
                        enabled: active,
                        app_off,
                        page_label: None,
                    })
                };
                let native = mode == ToolMode::Native && active;
                let mut specs = Vec::new();
                if native {
                    specs = tool_specs(&classes.callable);
                    if !classes.off.is_empty() {
                        specs.push(request_tool_spec(&classes.off));
                    }
                }
                let mut parts = turn.base.clone();
                parts.tools_text = tools_text;
                parts.memory_text = self.memory_prompt(&s);
                parts.skills_text = self.skills_prompt(mode != ToolMode::Off);
                parts.vision = self.sees_images(&s);
                parts.screenshots = self.vision_on(&s);
                let mut messages = turn.messages.clone();
                if mode == ToolMode::Text {
                    messages.extend(text_turn_messages(&tool_turns));
                }
                let request = ChatRequest {
                    system: build_system_prompt(&parts),
                    messages,
                    tools: specs,
                    tool_turns: if native { tool_turns.clone() } else { Vec::new() },
                    turn_id: turn_id.clone(),
                };
                (request, active)
            };
            let native = mode == ToolMode::Native && active;
            let step_start = reply.raw.len();
            let native_start = reply.native.len();
            let outcome = {
                let mut on_event = |e: StreamEvent| {
                    match e.kind {
                        EventKind::Text => reply.raw.push_str(&e.text),
                        EventKind::Reasoning => reply.native.push_str(&e.text),
                        EventKind::Notice => view.notice(&e.text),
                        EventKind::Status => reply.status = e.text.clone(),
                    }
                    view.paint(reply, false);
                };
                model.chat(&request, settings, &mut on_event, cancel)
            };
            result = match outcome {
                Ok(r) => r,
                Err(e) => {
                    // Auto mode: a model or server that refuses tool definitions gets the text protocol instead.
                    let refuses_tools = native
                        && s.tool_mode == "auto"
                        && !request.tools.is_empty()
                        && reply.raw.len() == step_start
                        && !matches!(
                            e.code,
                            ErrorCode::Cancelled | ErrorCode::Auth | ErrorCode::Network | ErrorCode::Timeout
                        )
                        && {
                            let m = e.message.to_lowercase();
                            m.contains("tool") || m.contains("function")
                        };
                    if refuses_tools {
                        self.text_tools.insert(self.model_key(settings));
                        mode = ToolMode::Text;
                        view.notice(
                            "This model does not accept tool calls, so tools are described to it as text blocks.",
                        );
                        continue;
                    }
                    return Err(e);
                }
            };

            let step_raw = reply.raw[step_start..].to_string();
            let mut step_text = js_trim(&split_reasoning(&step_raw).text).to_string();
            let mut calls = result.tool_calls.clone();
            if mode == ToolMode::Text && active {
                let parsed = parse_text_tool_calls(&step_text, &format!("t{step}_"), result.truncated);
                calls = parsed.calls;
                if !calls.is_empty() {
                    reply.raw.truncate(step_start);
                    reply.raw.push_str(&parsed.text);
                    step_text = parsed.text;
                    view.paint(reply, true);
                }
            }
            // A reply that hit Max reply tokens while writing a call: that call's arguments are cut off.
            if result.truncated {
                if let Some(last) = calls.last_mut().filter(|c| c.args_error.is_some()) {
                    last.cut_off = true;
                }
            }
            if calls.is_empty() || mode == ToolMode::Off {
                if result.truncated {
                    view.notice(&format!(
                        "The reply reached Max reply tokens ({}) and was cut off. Raise it in Settings > Agent, or ask for less at once.",
                        locale_count(settings.max_output_tokens as f64)
                    ));
                }
                // After tool rounds, some local models put their whole closing answer in the reasoning channel.
                if !tool_turns.is_empty() && step_text.is_empty() {
                    let native_part = reply.native[native_start..].to_string();
                    let inline = split_reasoning(&step_raw).reasoning;
                    let said = js_trim(if native_part.is_empty() { &inline } else { &native_part }).to_string();
                    if !said.is_empty() {
                        reply.native.truncate(native_start);
                        reply.raw.truncate(step_start);
                        reply.raw.push_str(&said);
                        view.paint(reply, true);
                    }
                }
                break;
            }
            let agent = self.agent();
            let max_steps = match agent.max_tool_steps {
                Some(a) if a > 0 => s.max_tool_steps.min(a as i64),
                _ => s.max_tool_steps,
            };
            if step >= max_steps {
                view.notice(&if max_steps < s.max_tool_steps {
                    format!("Stopped after {max_steps} tool steps (the {} agent's limit).", agent.title)
                } else {
                    format!("Stopped after {max_steps} tool steps (Settings > Tools > Max tool steps).")
                });
                break;
            }

            let mut results: Vec<CallOutcome> = Vec::new();
            let mut changed = false;
            for call in &calls {
                if cancel.is_cancelled() {
                    break;
                }
                let r = self.execute_tool(call, reply, view, runner, actions, cancel);
                changed = changed || r.changed;
                results.push(r);
            }
            if cancel.is_cancelled() {
                return Err(AiError::cancelled());
            }
            // Send the screen back with the results when the actions changed it.
            if changed && self.share_on(settings) && self.ctx.has_content() && !results.is_empty() {
                runner.refresh_context(&mut self.ctx);
                let snap = self.ctx.snapshot_at(crate::time::iso_from_millis(self.now()).unwrap_or_default());
                if !snap.empty && last_hash.as_deref() != Some(snap.hash.as_str()) {
                    if let Some(last) = results.last_mut() {
                        last.content.push_str(&format!(
                            "\n\n[The screen after these actions]\n{}",
                            snapshot_block(&SnapshotRecord::from(&snap))
                        ));
                    }
                    last_hash = Some(snap.hash.clone());
                }
            }
            tool_turns.push(ToolTurn {
                text: step_text,
                calls: calls.iter().map(wire_call).collect(),
                results: results
                    .into_iter()
                    .map(|r| ToolResult { id: r.id, name: r.name, content: r.content, images: r.images })
                    .collect(),
            });
            if !reply.raw.is_empty() && !reply.raw.ends_with("\n\n") {
                reply.raw.push_str("\n\n");
            }
            reply.status = "Continuing…".into();
            view.paint(reply, false);
            step += 1;
        }
        Ok(result)
    }

    /* ---------------------------------------------------------------------------------- a question */

    /// A question starts: the snapshot plan, the user's message (added to the transcript; `/skill …` activates that
    /// skill), and what its first request sends. `image_for` gives an image's data while it is in memory.
    pub fn begin_turn(
        &mut self,
        text: &str,
        shots: &[Shot],
        files: &[FileRecord],
        image_for: Option<ImageFor<'_>>,
    ) -> Turn {
        let s = self.settings.clone();
        let vision = self.sees_images(&s);
        let share = self.share_on(&s);
        let now = self.now();
        let snapshot = (share && self.ctx.has_content())
            .then(|| self.ctx.snapshot_at(crate::time::iso_from_millis(now).unwrap_or_default()));
        let plan = plan_turn(&TurnInput {
            messages: &self.conv.messages,
            screen: snapshot.as_ref().map(Screen::from),
            history_messages: s.history_messages,
            share,
            force: self.force_reread,
        });
        let mut user = Message {
            id: uid("u", now),
            role: Role::User,
            content: text.to_string(),
            at: Some(now),
            sync: if plan.attach { "attached".into() } else { plan.reason.as_str().into() },
            ..Default::default()
        };
        let names: Vec<String> = self.skills_for_agent().iter().map(|sk| sk.name.clone()).collect();
        if let Some(slash) = parse_slash_command(text, &names) {
            if self.activate_skill(&slash.name, "slash").is_some() {
                user.skill = slash.name;
            }
        }
        if let Some(snap) = snapshot.as_ref() {
            if plan.attach {
                user.snapshot = Some(SnapshotRecord::from(snap));
            } else if plan.reason == SyncReason::Unchanged {
                user.snapshot_ref = snap.hash.clone();
            }
        }
        if vision && !shots.is_empty() {
            user.shots = shots
                .iter()
                .map(|sh| Shot {
                    id: sh.id.clone(),
                    thumb: sh.thumb.clone(),
                    width: sh.width,
                    height: sh.height,
                    kind: if sh.name.is_empty() { String::new() } else { "image".into() },
                    name: sh.name.clone(),
                })
                .collect();
        }
        user.files = files.to_vec();
        self.force_reread = false;
        let user_id = user.id.clone();
        self.conv.messages.push(user);

        let (_, _, view_text, _) = self.context_texts(&s);
        let base = self.base_system(&s);
        let unchanged =
            (plan.reason == SyncReason::Unchanged).then(|| snapshot.as_ref().map(|x| x.hash.clone())).flatten();
        let messages = build_request_messages(&RequestInput {
            messages: &self.conv.messages,
            history_messages: s.history_messages,
            view_text: &view_text,
            unchanged_hash: unchanged.as_deref(),
            image_for: if vision { image_for } else { None },
            image_messages: DEFAULT_IMAGE_MESSAGES,
        });
        let snapshot_hash = snapshot.as_ref().map(|x| x.hash.clone());
        self.emit(SessionEvent::Send {
            text: text.to_string(),
            attached: plan.attach,
            reason: plan.reason.as_str().into(),
            hash: snapshot_hash.clone(),
        });
        Turn { user_id, plan, snapshot_hash, vision, base, messages }
    }

    /// A question ended. With an answer or actions taken, the reply joins the transcript; with neither, the question
    /// is taken out again, so the transcript and the sync state are as they were.
    pub fn end_turn(
        &mut self,
        turn: &Turn,
        reply: &Reply,
        actions: Vec<Action>,
        result: Option<&ChatResult>,
        error: Option<&AiError>,
        loaded: &str,
    ) -> TurnEnd {
        let answer = js_trim(&split_reasoning(&reply.raw).text).to_string();
        let cancelled = error.is_some_and(|e| e.code == ErrorCode::Cancelled);
        if !answer.is_empty() || !actions.is_empty() {
            let who = match result {
                Some(r) => {
                    let tail = if !r.model.is_empty() {
                        format!(" · {}", r.model)
                    } else if !loaded.is_empty() {
                        format!(" · {loaded}")
                    } else {
                        String::new()
                    };
                    format!("{}{tail}", r.label)
                }
                None => String::new(),
            };
            let meta = if cancelled { "Stopped".to_string() } else { who };
            let content =
                if cancelled && !answer.is_empty() { format!("{answer}\n\n_(stopped)_") } else { answer.clone() };
            let now = self.now();
            self.conv.messages.push(Message {
                id: uid("a", now),
                role: Role::Assistant,
                content,
                at: Some(now),
                meta: meta.clone(),
                actions,
                ..Default::default()
            });
            self.emit(SessionEvent::Reply {
                text: answer.clone(),
                provider: result.map(|r| r.provider.clone()).unwrap_or_default(),
                model: result.map(|r| r.model.clone()).unwrap_or_default(),
                stopped: cancelled,
            });
            return TurnEnd { kept: true, answer, cancelled, meta };
        }
        self.conv.messages.retain(|m| m.id != turn.user_id);
        if let Some(e) = error.filter(|_| !cancelled) {
            self.emit(SessionEvent::Error { code: e.code, message: e.message.clone() });
        }
        TurnEnd { kept: false, answer, cancelled, meta: String::new() }
    }

    /// A whole question, start to end: begin, run, end. Returns how it ended, the error, and the reply.
    #[allow(clippy::too_many_arguments)]
    pub fn ask(
        &mut self,
        text: &str,
        model: &mut dyn ChatModel,
        view: &mut dyn View,
        runner: &mut dyn ToolRunner,
        cancel: &CancelToken,
    ) -> (TurnEnd, Option<AiError>, Reply) {
        let settings = self.settings.clone();
        let turn = self.begin_turn(text, &[], &[], None);
        let mut reply = Reply::default();
        let mut actions = Vec::new();
        let outcome = self.run_model(&settings, &turn, &mut reply, model, view, runner, &mut actions, cancel);
        let (result, error) = match outcome {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e)),
        };
        let end = self.end_turn(&turn, &reply, actions, result.as_ref(), error.as_ref(), "");
        (end, error, reply)
    }

    /* -------------------------------------------------------------------------------- context flag */

    /// The sync flag for the screen as it is now. No side effects.
    pub fn compute_status(&self) -> Status {
        let s = &self.settings;
        let share = self.share_on(s);
        let snap = (share && self.ctx.has_content()).then(|| self.ctx.snapshot_at(String::new()));
        let st = context_state(&TurnInput {
            messages: &self.conv.messages,
            screen: if self.ctx.has_content() { snap.as_ref().map(Screen::from) } else { None },
            history_messages: s.history_messages,
            share,
            force: self.force_reread,
        });
        Status {
            state: st.state,
            pending: st.pending,
            reason: st.reason,
            hash: st.current_hash,
            synced_hash: st.synced_hash,
            page_id: snap
                .as_ref()
                .map(|x| x.page_id.clone())
                .or_else(|| self.ctx.page.as_ref().map(|p| p.id.clone()))
                .unwrap_or_default(),
            title: snap.as_ref().map(|x| x.page_title.clone()).unwrap_or_default(),
            chars: snap.as_ref().map(|x| x.total_chars).unwrap_or(0),
            truncated: snap.as_ref().is_some_and(|x| x.truncated),
            forced: self.force_reread,
        }
    }

    /* --------------------------------------------------------------------------------- saved chats */

    /// The saved chats (newest first), as the JS runtime stores them.
    pub fn read_chats(&self) -> Vec<Value> {
        let Some(text) = self.chats.as_ref().and_then(|c| c.read()) else { return Vec::new() };
        Value::parse(&text)
            .ok()
            .and_then(|v| v.as_array().cloned())
            .map(|l| l.into_iter().filter(|c| c.get("id").is_some_and(|i| i.truthy())).collect())
            .unwrap_or_default()
    }

    fn write_chats(&mut self, list: Vec<Value>) -> bool {
        let current = self.conv.id.clone();
        let Some(store) = self.chats.as_mut() else { return false };
        if store.write(&crate::json::to_json(&Value::Array(list.clone()))) {
            return true;
        }
        // Over the quota: the text of files in the other chats goes first, then the oldest chats.
        let lean: Vec<Value> = list
            .into_iter()
            .map(|mut c| {
                if c.get("id").and_then(|i| i.as_str()) != current.as_deref() {
                    if let Some(msgs) = c.as_object_mut().and_then(|o| o.get_mut("messages")).and_then(|m| match m {
                        Value::Array(a) => Some(a),
                        _ => None,
                    }) {
                        for m in msgs.iter_mut() {
                            if let Some(Value::Array(files)) = m.as_object_mut().and_then(|o| o.get_mut("files")) {
                                for f in files.iter_mut() {
                                    if let Some(o) = f.as_object_mut() {
                                        o.remove("text");
                                    }
                                }
                            }
                        }
                    }
                }
                c
            })
            .collect();
        let mut n = lean.len();
        while n > 0 {
            if store.write(&crate::json::to_json(&Value::Array(lean[..n].to_vec()))) {
                return true;
            }
            n = (n as f64 * 0.7).floor() as usize;
        }
        false
    }

    fn derive_title(&self) -> String {
        let first = self.conv.messages.iter().find(|m| m.role == Role::User).map(|m| m.content.as_str()).unwrap_or("");
        let t = first.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.is_empty() {
            "New chat".into()
        } else if utf16_len(&t) > 48 {
            format!("{}…", js_trim(js_head(&t, 47)))
        } else {
            t
        }
    }

    /// Save the transcript (without snapshot text). Returns the chat's id, or None when nothing was saved.
    pub fn persist(&mut self) -> Option<String> {
        if self.conv.messages.is_empty() || self.chats.is_none() {
            return None;
        }
        let now = self.now();
        let id = self.conv.id.get_or_insert_with(|| uid("c", now)).clone();
        let start = self.conv.messages.len().saturating_sub(MESSAGE_LIMIT);
        let messages: Vec<Value> = self.conv.messages[start..]
            .iter()
            .map(|m| {
                let mut m = m.clone();
                m.id.clear();
                if let Some(s) = m.snapshot.as_mut() {
                    s.text = None;
                }
                crate::json::to_value(&m)
            })
            .collect();
        let mut list = self.read_chats();
        let title = self.derive_title();
        let page = self.ctx.page.as_ref().map(|p| p.id.clone()).unwrap_or_default();
        let skills = Value::from(self.conv.skills.clone());
        if let Some(existing) = list.iter_mut().find(|c| c.get("id").and_then(|i| i.as_str()) == Some(id.as_str())) {
            if let Some(o) = existing.as_object_mut() {
                o.insert("title", title);
                o.insert("updatedAt", now);
                o.insert("messages", Value::Array(messages));
                o.insert("agent", self.agent_name.as_str());
                o.insert("skills", skills);
            }
        } else {
            let mut o = Map::new();
            o.insert("id", id.as_str());
            o.insert("title", title);
            o.insert("createdAt", now);
            o.insert("updatedAt", now);
            o.insert("messages", Value::Array(messages));
            o.insert("page", page);
            o.insert("agent", self.agent_name.as_str());
            o.insert("skills", skills);
            list.push(Value::Object(o));
        }
        let at = |c: &Value| c.get("updatedAt").and_then(|v| v.as_f64()).unwrap_or(0.0);
        list.sort_by(|a, b| at(b).partial_cmp(&at(a)).unwrap_or(std::cmp::Ordering::Equal));
        list.truncate(CHAT_LIMIT);
        self.write_chats(list);
        Some(id)
    }

    /// Make a saved chat the conversation (with its agent, when that still exists). Returns whether the agent
    /// changed, or None when there is no such chat.
    pub fn load_chat(&mut self, id: &str) -> Option<bool> {
        let record = self.read_chats().into_iter().find(|c| c.get("id").and_then(|i| i.as_str()) == Some(id))?;
        self.allowed_tools.clear();
        let now = self.now();
        let messages: Vec<Message> = record
            .get("messages")
            .and_then(|m| m.as_array())
            .map(|l| {
                l.iter()
                    .filter_map(|m| crate::json::from_value::<Message>(m).ok())
                    .map(|mut m| {
                        m.id = uid(if m.role == Role::User { "u" } else { "a" }, now);
                        m
                    })
                    .collect()
            })
            .unwrap_or_default();
        let skills = record
            .get("skills")
            .and_then(|s| s.as_array())
            .map(|a| a.iter().map(|x| x.js_string()).collect())
            .unwrap_or_default();
        self.conv = Conversation { id: Some(id.to_string()), messages, skills };
        let mut changed = false;
        if let Some(agent) = record.get("agent").and_then(|a| a.as_str()) {
            if agent != self.agent_name && (agent.is_empty() || self.agent_list().iter().any(|a| a.name == agent)) {
                self.agent_name = agent.to_string();
                changed = true;
            }
        }
        Some(changed)
    }

    /// Delete a saved chat (the open conversation goes on, unsaved, when it is the one deleted).
    pub fn delete_chat(&mut self, id: &str) {
        let list = self.read_chats().into_iter().filter(|c| c.get("id").and_then(|i| i.as_str()) != Some(id)).collect();
        self.write_chats(list);
        if self.conv.id.as_deref() == Some(id) {
            self.conv.id = None;
        }
    }

    /// Start an empty conversation (saving the current one first unless `save` is false).
    pub fn new_conversation(&mut self, save: bool) {
        self.allowed_tools.clear();
        if save {
            self.persist();
        }
        self.conv = Conversation::default();
        self.force_reread = false;
    }

    /// Switch agent; a new conversation starts (save the current one first). None when there is no such agent;
    /// otherwise whether it changed.
    pub fn switch_agent(&mut self, name: &str) -> Option<bool> {
        let target = self.agent_list().into_iter().find(|a| a.name == name)?;
        if target.name == self.agent().name {
            return Some(false);
        }
        self.agent_name = if target.implicit { String::new() } else { target.name };
        self.new_conversation(false);
        Some(true)
    }

    /// Lock / sign-out: drop the transcript, the decisions taken in it, and the remembered fallbacks. Nothing is
    /// saved. (A question in progress is stopped by its cancel token.)
    pub fn reset(&mut self) {
        self.conv = Conversation::default();
        self.force_reread = false;
        self.allowed_tools.clear();
        self.text_tools.clear();
        self.events.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{Page, Source};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct MemChats(Arc<Mutex<Option<String>>>);
    impl ChatStore for MemChats {
        fn read(&self) -> Option<String> {
            self.0.lock().unwrap().clone()
        }
        fn write(&mut self, json: &str) -> bool {
            *self.0.lock().unwrap() = Some(json.to_string());
            true
        }
    }

    struct NoView;
    impl View for NoView {
        fn row(&mut self, _: &str, _: &str) -> RowId {
            0
        }
        fn decide(&mut self, _: RowId, card: &Card, _: &CancelToken) -> String {
            card.decline.to_string()
        }
    }

    struct NoTools(bool);
    impl ToolRunner for NoTools {
        fn run(&mut self, _: &ToolContract, _: &Map, _: &str, _: &CancelToken) -> Result<Value, String> {
            self.0 = true;
            Ok(Value::Null)
        }
    }

    struct Answer(&'static str, Vec<ToolCall>);
    impl ChatModel for Answer {
        fn chat(
            &mut self,
            _: &ChatRequest,
            _: &Settings,
            on_event: &mut dyn FnMut(StreamEvent),
            _: &CancelToken,
        ) -> Result<ChatResult, AiError> {
            on_event(StreamEvent::new(EventKind::Text, self.0));
            Ok(ChatResult {
                tool_calls: std::mem::take(&mut self.1),
                label: "L".into(),
                model: "m".into(),
                ..Default::default()
            })
        }
    }

    fn session(agents: &[&str]) -> (Session, Arc<Mutex<Option<String>>>) {
        let mut ctx = ContextManager::new();
        ctx.set_page(Some(Page::new("editor", "Editor").content(Source::Value("Hello.".into()))));
        let mut s = Session::new(ctx, Settings::default());
        s.agents = agents
            .iter()
            .enumerate()
            .filter_map(|(i, t)| crate::agents::parse_agent(t, &format!("agents/a{i}.md")).agent)
            .collect();
        let store = Arc::new(Mutex::new(None));
        s.chats = Some(Box::new(MemChats(store.clone())));
        s.clock = Box::new(|| 1_000.0);
        s.init();
        (s, store)
    }

    const A: &str = "---\nname: a\ndescription: A.\n---\nA.";
    const B: &str = "---\nname: b\ndescription: B.\n---\nB.";

    /// A native drawer runs a question on a worker thread: the session must be able to move there.
    #[test]
    fn the_session_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Session>();
    }

    #[test]
    fn default_questions() {
        assert_eq!(default_question("  Hi ", &[], &[]), "Hi");
        assert_eq!(default_question("", &[Shot::default()], &[]), "Here is a screenshot of what I am looking at.");
        let img = Shot { name: "x.png".into(), ..Default::default() };
        assert_eq!(default_question("", std::slice::from_ref(&img), &[]), "Please look at the attached image.");
        assert_eq!(default_question("", &[img, Shot::default()], &[]), "Please look at what I attached.");
    }

    #[test]
    fn no_view_declines_every_card() {
        let (mut s, _) = session(&[]);
        s.tools.push(
            normalize_tool(
                &Value::parse(r#"{"name":"set_x","description":"d","effect":"write","enabled":true}"#).unwrap(),
            )
            .unwrap(),
        );
        let mut runner = NoTools(false);
        let call = crate::tools::tool_call("c", "set_x", &Value::object());
        let r = s.execute_tool(
            &call,
            &mut Reply::default(),
            &mut NoView,
            &mut runner,
            &mut Vec::new(),
            &CancelToken::new(),
        );
        assert!(!runner.0, "a write tool never runs unattended");
        assert!(r.content.contains("declined to run set_x"));
    }

    #[test]
    fn saved_chats_round_trip() {
        let (mut s, store) = session(&[A, B]);
        s.agent_name = "b".into();
        let (end, err, _) = s.ask(
            "First question",
            &mut Answer("Answer.", vec![]),
            &mut NoView,
            &mut NoTools(false),
            &CancelToken::new(),
        );
        assert!(end.kept && err.is_none());
        s.conv.skills = vec!["proofreading".into()];
        let id = s.persist().expect("saved");
        let saved = Value::parse(store.lock().unwrap().as_deref().unwrap()).unwrap();
        let chat = &saved.as_array().unwrap()[0];
        assert_eq!(chat.at("title").as_str(), Some("First question"));
        assert_eq!(chat.at("agent").as_str(), Some("b"));
        assert!(
            chat.at("messages").as_array().unwrap()[0].at("snapshot").get("text").is_none(),
            "no snapshot text in saved chats"
        );

        s.agent_name = "a".into();
        s.new_conversation(false);
        assert_eq!(s.load_chat(&id), Some(true));
        assert_eq!(s.agent_name, "b");
        assert_eq!(s.conv.skills, vec!["proofreading".to_string()]);
        assert_eq!(s.conv.messages.len(), 2);
        assert_eq!(s.compute_status().state, FlagState::Unread, "a reopened chat re-reads the page");
        s.delete_chat(&id);
        assert!(s.read_chats().is_empty());
        assert_eq!(s.conv.id, None);
    }

    #[test]
    fn switch_and_reset() {
        let (mut s, _) = session(&[A, B]);
        s.conv.messages.push(Message::user("x"));
        s.allowed_tools.insert("t".into());
        assert_eq!(s.switch_agent("a"), Some(false));
        assert_eq!(s.switch_agent("b"), Some(true));
        assert!(s.conv.messages.is_empty() && s.allowed_tools.is_empty());
        assert_eq!(s.switch_agent("zzz"), None);
        s.conv.messages.push(Message::user("y"));
        s.text_tools.insert("k".into());
        s.force_reread = true;
        s.reset();
        assert!(s.conv.messages.is_empty() && s.text_tools.is_empty() && !s.force_reread);
    }

    #[test]
    fn a_failed_question_leaves_no_trace() {
        struct Fails;
        impl ChatModel for Fails {
            fn chat(
                &mut self,
                _: &ChatRequest,
                _: &Settings,
                _: &mut dyn FnMut(StreamEvent),
                _: &CancelToken,
            ) -> Result<ChatResult, AiError> {
                Err(AiError::new(ErrorCode::Network, "down"))
            }
        }
        let (mut s, _) = session(&[]);
        let (end, err, _) = s.ask("Hello?", &mut Fails, &mut NoView, &mut NoTools(false), &CancelToken::new());
        assert!(!end.kept);
        assert_eq!(err.map(|e| e.code), Some(ErrorCode::Network));
        assert!(s.conv.messages.is_empty());
        assert!(s.take_events().iter().any(|e| matches!(e, SessionEvent::Error { .. })));
    }
}
