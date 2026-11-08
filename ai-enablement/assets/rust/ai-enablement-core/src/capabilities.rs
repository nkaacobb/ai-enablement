//! The application's capability folder (`core/capabilities.js`, `spec/framework.md`): `index.json` names the agents,
//! skills, toolsets, the tool config, the starting memories and the permission rules; paths are relative to it. A
//! compiled app defines its tools in code, so `tools` (JS modules) is not read here; `toolCatalog` is the generated
//! catalog that `validate.mjs` reads, not something the runtime needs.
//!
//! The folder is read through a [`FileSource`]: embedded in the binary ([`EmbeddedFiles`], with the
//! [`embedded_files!`](crate::embedded_files) macro) or a folder on disk ([`DiskFiles`], e.g. in debug builds through
//! an environment variable, so skills and agents can be edited without rebuilding). Problems are collected, never
//! fatal: one broken skill must not take the agent down.

use crate::agents::{parse_agent, Agent};
use crate::json::{Map, Value};
use crate::memory::MemoryStore;
use crate::permissions::{normalize_policy, Policy};
use crate::session::{FileSource, Session};
use crate::settings::Settings;
use crate::skills::{parse_skill, Skill};
use crate::tools::tools_config_patch;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

pub const INDEX_FORMAT: &str = "ai-enablement/1";
const INDEX_KEYS: [&str; 12] = [
    "$schema",
    "$comment",
    "format",
    "agents",
    "skills",
    "tools",
    "toolsets",
    "toolsConfig",
    "memory",
    "permissions",
    "defaultAgent",
    "toolCatalog",
];
const LATER: [&str; 3] = ["prompts", "resources", "context"];

/// Files compiled into the binary: `(path relative to the capability folder, text)`.
#[derive(Clone, Copy, Debug)]
pub struct EmbeddedFiles(pub &'static [(&'static str, &'static str)]);

impl FileSource for EmbeddedFiles {
    fn read(&self, path: &str) -> Result<String, String> {
        let want = normalize(path).ok_or_else(|| format!("{path} is outside the capability folder"))?;
        self.0
            .iter()
            .find(|(p, _)| normalize(p).as_deref() == Some(want.as_str()))
            .map(|(_, t)| t.to_string())
            .ok_or_else(|| "not found".to_string())
    }
}

/// A capability folder on disk.
#[derive(Clone, Debug)]
pub struct DiskFiles(pub PathBuf);

impl FileSource for DiskFiles {
    fn read(&self, path: &str) -> Result<String, String> {
        let rel = normalize(path).ok_or_else(|| format!("{path} is outside the capability folder"))?;
        std::fs::read_to_string(self.0.join(rel)).map_err(|e| e.to_string())
    }
}

/// Embed a capability folder's files: `embedded_files!("resources/ai": "index.json", "agents/writer.md", …)`, the
/// folder relative to the crate's `Cargo.toml`.
#[macro_export]
macro_rules! embedded_files {
    ($root:literal : $($path:literal),* $(,)?) => {
        $crate::capabilities::EmbeddedFiles(&[$(($path, include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $root, "/", $path)))),*])
    };
}

/// The folder on disk named by an environment variable (debug builds), else the embedded files.
pub fn source_from_env(var: &str, embedded: EmbeddedFiles) -> Box<dyn FileSource> {
    match std::env::var(var) {
        Ok(dir) if !dir.is_empty() && Path::new(&dir).is_dir() => Box::new(DiskFiles(PathBuf::from(dir))),
        _ => Box::new(embedded),
    }
}

/// A relative path with `.` and `..` resolved, `/`-separated; None when it leaves the folder or is absolute.
fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for c in Path::new(&path.replace('\\', "/")).components() {
        match c {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

fn join(dir: &str, entry: &str) -> String {
    if dir.is_empty() {
        entry.to_string()
    } else {
        format!("{dir}/{entry}")
    }
}

/// A toolset: tools defined elsewhere, grouped.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Toolset {
    pub name: String,
    pub title: String,
    pub description: String,
    pub tools: Vec<String>,
}

fn humanize_name(name: &str) -> String {
    let spaced: String = name.chars().map(|c| if c == '_' || c == '-' { ' ' } else { c }).collect();
    let collapsed = spaced.split(' ').filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" ");
    let mut chars = collapsed.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// A toolset definition (`{ name, title?, description?, tools: [names] }`), or the problem with it.
pub fn normalize_toolset(def: &Value) -> Result<Toolset, String> {
    let name = def
        .get("name")
        .filter(|v| !v.is_null())
        .map(|v| crate::text::js_trim(&v.js_string()).to_string())
        .unwrap_or_default();
    if !crate::tools::valid_tool_name(&name) {
        return Err(format!("toolset name \"{name}\" is not valid (letters, digits, _ or -, starting with a letter)."));
    }
    let mut tools: Vec<String> = Vec::new();
    for t in def.get("tools").and_then(|t| t.as_array()).cloned().unwrap_or_default() {
        match t.as_str() {
            Some(s) if !tools.iter().any(|x| x == s) => tools.push(s.to_string()),
            Some(_) => {}
            None => {
                return Err(format!(
                    "toolset \"{name}\": an entry that is not a tool name (tools are defined in code)."
                ))
            }
        }
    }
    let title = def.get("title").filter(|v| v.truthy()).map(|v| v.js_string()).unwrap_or_else(|| humanize_name(&name));
    let description = def.get("description").filter(|v| v.truthy()).map(|v| v.js_string()).unwrap_or_default();
    Ok(Toolset { name, title, description, tools })
}

/// What a capability index holds.
#[derive(Clone, Debug, Default)]
pub struct Capabilities {
    pub agents: Vec<Agent>,
    pub skills: Vec<Skill>,
    pub toolsets: Vec<Toolset>,
    pub permissions: Policy,
    pub default_agent: String,
    /// The tool config file (`ai-tools.json`) as JSON, when the index names one.
    pub tools_config: Option<Value>,
    /// The starting memories (`ai-memory.json`) as JSON, when the index names them.
    pub memory: Option<Value>,
    pub problems: Vec<String>,
}

impl Capabilities {
    /// Toolset name → its tools, as the session takes them.
    pub fn toolset_map(&self) -> BTreeMap<String, Vec<String>> {
        let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for ts in &self.toolsets {
            let e = out.entry(ts.name.clone()).or_default();
            for t in &ts.tools {
                if !e.contains(t) {
                    e.push(t.clone());
                }
            }
        }
        out
    }

    /// The settings defaults the tool config gives (`{ toolStates, toolsEnabled, … }`), to lay under what the user
    /// saved: `Settings::sanitize(&saved, &Settings::sanitize(&app_defaults, &Settings::default()))`.
    pub fn settings_defaults(&self) -> Value {
        let mut m = Map::new();
        let Some(cfg) = &self.tools_config else { return Value::Object(m) };
        let p = tools_config_patch(cfg);
        if !p.tool_states.is_empty() {
            m.insert("toolStates", p.tool_states.iter().map(|(k, v)| (k.clone(), Value::Bool(*v))).collect::<Map>());
        }
        for (k, v) in [
            ("toolsEnabled", p.tools_enabled),
            ("confirmWrites", p.confirm_writes),
            ("confirmDestructive", p.confirm_destructive),
        ] {
            if let Some(b) = v {
                m.insert(k, b);
            }
        }
        if let Some(mode) = p.tool_mode {
            m.insert("toolMode", mode);
        }
        if let Some(n) = p.max_tool_steps {
            m.insert("maxToolSteps", n as f64);
        }
        Value::Object(m)
    }

    /// Give a session these capabilities: agents, skills (their files read from `files`), toolsets, the rules, the
    /// default agent, and the starting memories under the user's own.
    pub fn apply(&self, session: &mut Session, files: Option<Box<dyn FileSource>>) {
        session.agents = self.agents.clone();
        session.skills = self.skills.clone();
        session.toolsets = self.toolset_map();
        session.policy = self.permissions.clone();
        session.default_agent = self.default_agent.clone();
        if let (Some(mem), Some(file)) = (session.memory.as_mut(), self.memory.as_ref()) {
            mem.set_base(file);
        }
        if files.is_some() {
            session.files = files;
        }
        session.init();
    }
}

/// Load a capability index (`index_path` inside `files`, e.g. `index.json`).
pub fn load_capabilities(files: &dyn FileSource, index_path: &str) -> Capabilities {
    let mut out = Capabilities::default();
    let problem = |out: &mut Capabilities, at: &str, msg: String| {
        out.problems.push(if at.is_empty() { msg } else { format!("{at}: {msg}") })
    };
    let index = match files.read(index_path).map_err(|e| e.to_string()).and_then(|t| Value::parse(&t)) {
        Ok(v) => v,
        Err(e) => {
            problem(&mut out, index_path, format!("the capability index could not be loaded ({e})."));
            return out;
        }
    };
    let Some(obj) = index.as_object() else {
        problem(&mut out, "index", "the capability index must be a JSON object.".into());
        return out;
    };
    let dir = normalize(index_path).and_then(|p| p.rsplit_once('/').map(|(d, _)| d.to_string())).unwrap_or_default();
    if let Some(f) = obj.get("format").filter(|f| f.as_str() != Some(INDEX_FORMAT)) {
        problem(
            &mut out,
            "index",
            format!("format \"{}\" is not \"{INDEX_FORMAT}\"; reading it anyway.", f.js_string()),
        );
    }
    for k in obj.keys() {
        if LATER.contains(&k.as_str()) {
            problem(&mut out, "index", format!("\"{k}\" is reserved for a later version and is ignored."));
        } else if !INDEX_KEYS.contains(&k.as_str()) {
            problem(&mut out, "index", format!("\"{k}\" is not an index key; it is ignored."));
        }
    }
    let list = |out: &mut Capabilities, k: &str| -> Vec<Value> {
        match obj.get(k) {
            None => Vec::new(),
            Some(Value::Array(a)) => a.clone(),
            Some(_) => {
                out.problems.push(format!("index: \"{k}\" must be a list."));
                Vec::new()
            }
        }
    };
    let at = |entry: &Value| match entry.as_str() {
        Some(s) => s.to_string(),
        None => {
            entry.get("name").map(|n| format!("{} (inline)", n.js_string())).unwrap_or_else(|| "inline entry".into())
        }
    };

    if !list(&mut out, "tools").is_empty() {
        problem(&mut out, "index", "\"tools\" names JS tool modules, which a compiled app does not load: its tools are defined in code (and listed in the tool catalog).".into());
    }
    for entry in list(&mut out, "toolsets") {
        let def = match entry.as_str() {
            Some(path) => match files.read(&join(&dir, path)).and_then(|t| Value::parse(&t)) {
                Ok(v) => v,
                Err(e) => {
                    problem(&mut out, &at(&entry), format!("could not be loaded ({e})."));
                    continue;
                }
            },
            None => entry.clone(),
        };
        match normalize_toolset(&def) {
            Ok(ts) => out.toolsets.push(ts),
            Err(e) => problem(&mut out, &at(&entry), e),
        }
    }
    for entry in list(&mut out, "skills") {
        let Some(path) = entry.as_str() else {
            let r = crate::skills::normalize_skill(&entry, "app");
            out.problems.extend(r.problems.iter().map(|p| format!("{}: {p}", at(&entry))));
            out.skills.extend(r.skill);
            continue;
        };
        let trimmed = path.trim_end_matches('/');
        let md = if trimmed.to_ascii_lowercase().ends_with(".md") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/SKILL.md")
        };
        let file = join(&dir, &md);
        let folder_path = file.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_default();
        let folder = folder_path.rsplit('/').next().unwrap_or("").to_string();
        match files.read(&file) {
            Ok(text) => {
                let r = parse_skill(&text, &format!("{folder_path}/"), &folder, "app");
                out.problems.extend(r.problems.iter().map(|p| format!("{path}: {p}")));
                out.skills.extend(r.skill);
            }
            Err(e) => problem(&mut out, path, format!("SKILL.md could not be loaded ({e}).")),
        }
    }
    for entry in list(&mut out, "agents") {
        let r = match entry.as_str() {
            Some(path) => match files.read(&join(&dir, path)) {
                Ok(text) => parse_agent(&text, path),
                Err(e) => {
                    problem(&mut out, path, format!("could not be loaded ({e})."));
                    continue;
                }
            },
            None => crate::agents::normalize_agent(&entry),
        };
        out.problems.extend(r.problems.iter().map(|p| format!("{}: {p}", at(&entry))));
        out.agents.extend(r.agent);
    }
    let json_file = |out: &mut Capabilities, key: &str| -> Option<Value> {
        match obj.get(key)? {
            Value::String(path) => match files.read(&join(&dir, path)).and_then(|t| Value::parse(&t)) {
                Ok(v) => Some(v),
                Err(e) => {
                    out.problems.push(format!("{path}: could not be loaded ({e})."));
                    None
                }
            },
            v @ Value::Object(_) => Some(v.clone()),
            _ => None,
        }
    };
    out.tools_config = json_file(&mut out, "toolsConfig");
    out.memory = json_file(&mut out, "memory");
    let mut policy_problems = Vec::new();
    out.permissions = normalize_policy(obj.get("permissions").unwrap_or(&Value::Null), &mut policy_problems);
    out.problems.extend(policy_problems.into_iter().map(|p| format!("index: {p}")));
    out.default_agent = obj.get("defaultAgent").and_then(|v| v.as_str()).unwrap_or("").to_string();
    out
}

/// Settings over the app's defaults: built-in defaults < `app_defaults` (e.g. [`Capabilities::settings_defaults`])
/// < what the user saved.
pub fn layered_settings(app_defaults: &Value, saved: &Value) -> Settings {
    Settings::sanitize(saved, &Settings::sanitize(app_defaults, &Settings::default()))
}

/// A memory store whose base is the app's memory file.
pub fn memory_with_base(file: Option<&Value>) -> MemoryStore {
    let mut m = MemoryStore::new();
    if let Some(f) = file {
        m.set_base(f);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILES: EmbeddedFiles = EmbeddedFiles(&[
        (
            "ai/index.json",
            r#"{"format":"ai-enablement/1","agents":["agents/writer.md","agents/missing.md"],"skills":["skills/proofreading"],"toolsets":["toolsets/document.json",{"name":"bad name","tools":[]}],"toolsConfig":"ai-tools.json","memory":"ai-memory.json","permissions":{"deny":["delete_*"]},"defaultAgent":"writer","toolCatalog":"tools.catalog.json","prompts":[]}"#,
        ),
        ("ai/agents/writer.md", "---\nname: writer\ndescription: Writes.\ntoolsets: [document]\n---\nYou write."),
        ("ai/skills/proofreading/SKILL.md", "---\nname: proofreading\ndescription: Proofread.\n---\nRead, then fix."),
        ("ai/skills/proofreading/references/list.md", "Check commas."),
        ("ai/toolsets/document.json", r#"{"name":"document","tools":["find_text","replace_text","find_text"]}"#),
        ("ai/ai-tools.json", r#"{"tools":{"replace_text":false},"confirmWrites":false}"#),
        ("ai/ai-memory.json", r#"{"version":1,"memories":[{"id":"m1","text":"Units are mm."}]}"#),
    ]);

    #[test]
    fn loads_an_index() {
        let c = load_capabilities(&FILES, "ai/index.json");
        assert_eq!(c.agents.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["writer"]);
        assert_eq!(c.skills[0].base, "ai/skills/proofreading/");
        assert_eq!(c.toolsets[0].tools, ["find_text", "replace_text"]);
        assert_eq!(c.toolsets[0].title, "Document");
        assert_eq!(c.default_agent, "writer");
        assert_eq!(c.permissions.deny, ["delete_*"]);
        let problems = c.problems.join("\n");
        assert!(problems.contains("agents/missing.md: could not be loaded"), "{problems}");
        assert!(problems.contains("bad name (inline): toolset name"), "{problems}");
        assert!(problems.contains("\"prompts\" is reserved"), "{problems}");
        let s = layered_settings(&c.settings_defaults(), &Value::parse(r#"{"confirmWrites":true}"#).unwrap());
        assert_eq!(s.tool_states.get("replace_text"), Some(&false));
        assert!(s.confirm_writes, "what the user saved wins");
        let mut session = Session::new(crate::context::ContextManager::new(), s);
        session.memory = Some(MemoryStore::new());
        c.apply(&mut session, Some(Box::new(FILES)));
        assert_eq!(session.memory.as_ref().unwrap().list()[0].text, "Units are mm.");
        assert_eq!(
            session.files.as_ref().unwrap().read("ai/skills/proofreading/references/list.md").unwrap(),
            "Check commas."
        );
    }

    #[test]
    fn paths_stay_inside() {
        assert_eq!(normalize("a/./b/../c.md").as_deref(), Some("a/c.md"));
        assert_eq!(normalize("../secret"), None);
        assert!(FILES.read("ai/../../etc/passwd").is_err());
    }
}
