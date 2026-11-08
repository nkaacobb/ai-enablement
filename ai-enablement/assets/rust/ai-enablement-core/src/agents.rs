//! Agents (`core/agents.js`): configured AI workers. An agent composes what the application already has (tools,
//! toolsets, skills, context layers, memory, permissions, limits) and never defines tools or skills itself. One
//! Markdown file per agent with YAML frontmatter; the body is its instructions.

use std::collections::BTreeMap;

use crate::frontmatter::{list_value, parse_frontmatter};
use crate::json::{Map, Value};
use crate::permissions::{normalize_policy, Policy};
use crate::skills::valid_skill_name;
use crate::text::js_trim;

/// The context layers an agent can receive.
pub const CONTEXT_LAYERS: [&str; 4] = ["app", "page", "screen", "view"];
/// What an agent may do with memory.
pub const MEMORY_MODES: [&str; 3] = ["on", "read", "off"];
const KNOWN: [&str; 19] = [
    "name",
    "description",
    "title",
    "tools",
    "toolsets",
    "skills",
    "permissions",
    "context",
    "memory",
    "maxToolSteps",
    "model",
    "welcome",
    "suggestions",
    "default",
    "instructions",
    "body",
    "text",
    "source",
    "file",
];

/// Which context layers an agent receives. Without `screen` there are no snapshots and the flag shows "off".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContextLayers {
    pub app: bool,
    pub page: bool,
    pub screen: bool,
    pub view: bool,
}

impl Default for ContextLayers {
    fn default() -> Self {
        ContextLayers { app: true, page: true, screen: true, view: true }
    }
}

/// `on` (sees and saves notes), `read` (sees them, cannot save), `off`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MemoryMode {
    #[default]
    On,
    Read,
    Off,
}

impl MemoryMode {
    pub fn as_str(self) -> &'static str {
        match self {
            MemoryMode::On => "on",
            MemoryMode::Read => "read",
            MemoryMode::Off => "off",
        }
    }
}

/// An agent.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Agent {
    pub name: String,
    pub title: String,
    pub description: String,
    pub instructions: String,
    /// None = every tool (with `toolsets` also None).
    pub tools: Option<Vec<String>>,
    pub toolsets: Option<Vec<String>>,
    /// None = every skill.
    pub skills: Option<Vec<String>>,
    pub permissions: Policy,
    pub context: ContextLayers,
    pub memory: MemoryMode,
    /// At most this many tool rounds per question (1–30).
    pub max_tool_steps: Option<u32>,
    /// A preference shown to the user; not applied automatically.
    pub model: String,
    pub welcome: String,
    pub suggestions: Vec<String>,
    pub default: bool,
    pub file: Option<String>,
    /// The application's implicit agent (it defines no agent files).
    pub implicit: bool,
}

impl Agent {
    pub fn to_value(&self) -> Value {
        let list = |l: &Option<Vec<String>>| l.clone().map_or(Value::Null, Value::from);
        let mut ctx = Map::new();
        ctx.insert("app", self.context.app);
        ctx.insert("page", self.context.page);
        ctx.insert("screen", self.context.screen);
        ctx.insert("view", self.context.view);
        let mut m = Map::new();
        m.insert("name", self.name.as_str());
        m.insert("title", self.title.as_str());
        m.insert("description", self.description.as_str());
        m.insert("instructions", self.instructions.as_str());
        if self.implicit {
            m.insert("implicit", true);
        }
        m.insert("tools", list(&self.tools));
        m.insert("toolsets", list(&self.toolsets));
        m.insert("skills", list(&self.skills));
        m.insert("permissions", self.permissions.to_value());
        m.insert("context", ctx);
        m.insert("memory", self.memory.as_str());
        m.insert("maxToolSteps", self.max_tool_steps.map_or(Value::Null, Value::from));
        m.insert("model", self.model.as_str());
        m.insert("welcome", self.welcome.as_str());
        m.insert("suggestions", self.suggestions.clone());
        m.insert("default", self.default);
        if let Some(f) = &self.file {
            m.insert("file", f.as_str());
        }
        Value::Object(m)
    }
}

/// An agent read: the agent (None when it cannot be used) and the problems found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedAgent {
    pub agent: Option<Agent>,
    pub problems: Vec<String>,
}

impl ParsedAgent {
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("agent", self.agent.as_ref().map_or(Value::Null, Agent::to_value));
        m.insert("problems", self.problems.clone());
        Value::Object(m)
    }
}

fn humanize(name: &str) -> String {
    let mut out = String::new();
    let mut in_run = false;
    for c in name.chars() {
        if c == '-' || c == '_' {
            if !in_run {
                out.push(' ');
            }
            in_run = true;
        } else {
            out.push(c);
            in_run = false;
        }
    }
    let mut chars = out.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => c.to_ascii_uppercase().to_string() + chars.as_str(),
        Some(c) => c.to_string() + chars.as_str(),
        None => out,
    }
}

/// The agent's name from its file: `agents/writer.md` → `writer`, `x.agent.md` → `x`, `agents/x/AGENT.md` → `x`.
fn file_name(file: &str) -> String {
    let parts: Vec<&str> = file.split(['\\', '/']).collect();
    let base = parts.last().copied().unwrap_or("");
    if base.eq_ignore_ascii_case("AGENT.md") {
        return parts.len().checked_sub(2).map_or(String::new(), |k| parts[k].to_string());
    }
    let lower = base.to_ascii_lowercase();
    for suffix in [".agent.md", ".md"] {
        if lower.ends_with(suffix) {
            return base[..base.len() - suffix.len()].to_string();
        }
    }
    base.to_string()
}

/// The agent an application has when it defines none: everything (the 1.x behaviour).
pub fn implicit_agent(title: &str) -> Agent {
    Agent {
        name: "default".into(),
        title: title.into(),
        description: "The application's agent.".into(),
        implicit: true,
        default: true,
        ..Default::default()
    }
}

/// Frontmatter-style data + instructions → the agent and its problems.
pub fn agent_from_data(data: &Map, instructions: &str, file: &str) -> ParsedAgent {
    let mut problems = Vec::new();
    let name = match data.get("name") {
        Some(Value::String(s)) => js_trim(s).to_string(),
        _ if !file.is_empty() => file_name(file),
        _ => String::new(),
    };
    let description =
        data.get("description").and_then(Value::as_str).map(|s| js_trim(s).to_string()).unwrap_or_default();
    if name.is_empty() {
        problems.push("name is missing (frontmatter \"name:\").".into());
    } else if !valid_skill_name(&name) {
        problems.push(format!("name \"{name}\" must be 1-64 lowercase letters, digits and single hyphens."));
    }
    if description.is_empty() {
        problems.push("description is missing: it tells the user (and other agents) what this agent is for.".into());
    }
    for k in data.keys() {
        if !KNOWN.contains(&k.as_str()) {
            problems.push(format!("\"{k}\" is not an agent setting; it is ignored."));
        }
    }
    let list = |k: &str| data.get(k).filter(|v| !v.is_null()).map(list_value);
    let permissions = normalize_policy(data.get("permissions").unwrap_or(&Value::Null), &mut problems);
    let mut context = ContextLayers::default();
    if let Some(c) = data.get("context").filter(|v| !v.is_null()) {
        let layers = list_value(c);
        for l in &layers {
            if !CONTEXT_LAYERS.contains(&l.as_str()) {
                problems.push(format!("context: \"{l}\" is not a context layer ({}).", CONTEXT_LAYERS.join(", ")));
            }
        }
        let has = |l: &str| layers.iter().any(|x| x == l);
        context = ContextLayers { app: has("app"), page: has("page"), screen: has("screen"), view: has("view") };
    }
    let mut memory = MemoryMode::On;
    if let Some(m) = data.get("memory").filter(|v| !v.is_null()) {
        let word = match m {
            Value::Bool(true) => "on".to_string(),
            Value::Bool(false) => "off".to_string(),
            other => js_trim(&other.js_string()).to_lowercase(),
        };
        match word.as_str() {
            "on" => memory = MemoryMode::On,
            "read" => memory = MemoryMode::Read,
            "off" => memory = MemoryMode::Off,
            _ => problems.push(format!("memory: \"{}\" is not one of {}.", m.js_string(), MEMORY_MODES.join(", "))),
        }
    }
    let mut max_tool_steps = None;
    if let Some(v) = data.get("maxToolSteps").filter(|v| !v.is_null()) {
        match v.finite().filter(|n| n.fract() == 0.0 && (1.0..=30.0).contains(n)) {
            Some(n) => max_tool_steps = Some(n as u32),
            None => problems.push("maxToolSteps must be a whole number from 1 to 30.".into()),
        }
    }
    if name.is_empty() || !valid_skill_name(&name) || description.is_empty() {
        return ParsedAgent { agent: None, problems };
    }
    let text = |k: &str| data.get(k).and_then(Value::as_str);
    let suggestions = match data.get("suggestions") {
        Some(Value::Array(items)) => list_value(&Value::Array(items.clone())),
        Some(v) if v.truthy() => list_value(&Value::Array(vec![v.clone()])),
        _ => vec![],
    };
    ParsedAgent {
        agent: Some(Agent {
            title: text("title").map(js_trim).filter(|t| !t.is_empty()).map_or_else(|| humanize(&name), str::to_string),
            description,
            instructions: js_trim(instructions).to_string(),
            tools: list("tools"),
            toolsets: list("toolsets"),
            skills: list("skills"),
            permissions,
            context,
            memory,
            max_tool_steps,
            model: text("model").map(|m| js_trim(m).to_string()).unwrap_or_default(),
            welcome: text("welcome").unwrap_or_default().to_string(),
            suggestions,
            default: data.get("default") == Some(&Value::Bool(true)),
            file: (!file.is_empty()).then(|| file.to_string()),
            implicit: false,
            name,
        }),
        problems,
    }
}

/// An agent file (Markdown with YAML frontmatter) → the agent and its problems.
pub fn parse_agent(text: &str, file: &str) -> ParsedAgent {
    let fm = parse_frontmatter(text);
    let mut r = agent_from_data(&fm.data, &fm.body, file);
    let mut problems: Vec<String> = fm.errors.iter().map(|e| format!("frontmatter: {e}")).collect();
    problems.append(&mut r.problems);
    r.problems = problems;
    r
}

/// An agent given inline (`{ name, description, instructions, tools, … }`) or as file text (`{ text, file? }`).
pub fn normalize_agent(def: &Value) -> ParsedAgent {
    let Some(d) = def.as_object() else {
        return ParsedAgent { agent: None, problems: vec!["an agent must be an object.".into()] };
    };
    let file = d.get("file").filter(|f| f.truthy()).map(Value::js_string).unwrap_or_default();
    if let Some(Value::String(text)) = d.get("text") {
        return parse_agent(text, &file);
    }
    let instructions =
        d.get("instructions").filter(|v| !v.is_null()).or_else(|| d.get("body").filter(|v| !v.is_null()));
    agent_from_data(d, &instructions.map(Value::js_string).unwrap_or_default(), &file)
}

/// Adds a name once (a JS `Set` keeps insertion order).
fn add_once(names: &mut Vec<String>, name: &str) {
    if !names.iter().any(|n| n == name) {
        names.push(name.to_string());
    }
}

/// Which tools an agent may use: Some(names), or None for "all of them". Names in insertion order, as the JS `Set`
/// keeps them: the agent's tools, then its toolsets' tools. Problems name tools and toolsets that do not exist.
pub fn agent_tool_names(
    agent: &Agent,
    tools: &[String],
    toolsets: &BTreeMap<String, Vec<String>>,
) -> (Option<Vec<String>>, Vec<String>) {
    let mut problems = Vec::new();
    if agent.tools.is_none() && agent.toolsets.is_none() {
        return (None, problems);
    }
    let mut names = Vec::new();
    for t in agent.tools.iter().flatten() {
        if tools.contains(t) {
            add_once(&mut names, t);
        } else {
            problems.push(format!("agent \"{}\": tool \"{t}\" does not exist.", agent.name));
        }
    }
    for ts in agent.toolsets.iter().flatten() {
        let Some(set) = toolsets.get(ts) else {
            problems.push(format!("agent \"{}\": toolset \"{ts}\" does not exist.", agent.name));
            continue;
        };
        for t in set {
            if tools.contains(t) {
                add_once(&mut names, t);
            }
        }
    }
    (Some(names), problems)
}

/// Which skills an agent may use: Some(names, in the agent's order), or None for "all of them".
pub fn agent_skill_names(agent: &Agent, skills: &[String]) -> (Option<Vec<String>>, Vec<String>) {
    let mut problems = Vec::new();
    let Some(list) = &agent.skills else { return (None, problems) };
    let mut names = Vec::new();
    for s in list {
        if skills.contains(s) {
            add_once(&mut names, s);
        } else {
            problems.push(format!("agent \"{}\": skill \"{s}\" does not exist.", agent.name));
        }
    }
    (Some(names), problems)
}

/// The AGENT section of the system prompt ('' for the implicit agent or one without instructions).
pub fn build_agent_prompt(agent: &Agent) -> String {
    if agent.implicit || agent.instructions.is_empty() {
        return String::new();
    }
    format!(
        "== AGENT: {0} ==\nYou are working as the \"{0}\" agent of this application. {1}\n\n{2}",
        agent.title, agent.description, agent.instructions
    )
}

/// The agent to start with: the one named, else the one marked default, else the first.
pub fn pick_agent<'a>(agents: &'a [Agent], wanted: &str) -> Option<&'a Agent> {
    agents.iter().find(|a| a.name == wanted).or_else(|| agents.iter().find(|a| a.default)).or_else(|| agents.first())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_files() {
        assert_eq!(file_name("agents/writer.md"), "writer");
        assert_eq!(file_name("x/Helper.Agent.MD"), "Helper");
        assert_eq!(file_name("agents\\pro\\AGENT.md"), "pro");
        assert_eq!(humanize("code-review_bot"), "Code review bot");
    }

    #[test]
    fn tool_and_skill_names() {
        let a = parse_agent(
            "---\nname: a\ndescription: d\ntools: [x, nope]\ntoolsets: [doc, gone]\nskills: [s]\n---\nI.",
            "",
        )
        .agent
        .unwrap();
        let toolsets: BTreeMap<String, Vec<String>> =
            [("doc".to_string(), vec!["y".to_string(), "z".to_string()])].into_iter().collect();
        let (names, problems) = agent_tool_names(&a, &["x".into(), "y".into()], &toolsets);
        assert_eq!(names.unwrap(), vec!["x", "y"]);
        assert_eq!(
            problems,
            vec!["agent \"a\": tool \"nope\" does not exist.", "agent \"a\": toolset \"gone\" does not exist."]
        );
        assert_eq!(
            build_agent_prompt(&a),
            "== AGENT: A ==\nYou are working as the \"A\" agent of this application. d\n\nI."
        );
        assert!(build_agent_prompt(&implicit_agent("X")).is_empty());
    }
}
