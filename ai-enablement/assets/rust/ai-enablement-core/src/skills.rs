//! Skills (`core/skills.js`): reusable instructions for a kind of work, in the open Agent Skills format.
//!
//! Progressive disclosure, as the format intends: the system prompt lists each skill's name and description; the
//! model calls `use_skill` when a request matches one; from then on the skill's instructions are part of the system
//! prompt for the rest of the conversation; `read_skill_file` reads a file the instructions point to. A message
//! starting with `/skill-name` activates it too. A skill's `scripts/` are never run.

use std::sync::LazyLock;

use regex::Regex;

use crate::frontmatter::{list_value, parse_frontmatter};
use crate::json::{quote, Map, Value};
use crate::text::{js_head, js_trim, locale_count, utf16_len, JS_SPACE_CLASS};

/// The body of an active skill is clipped at this many characters in the prompt.
pub const SKILL_BODY_MAX: usize = 16_000;
/// `read_skill_file` returns at most this many characters per call.
pub const SKILL_FILE_MAX: usize = 20_000;
/// The file types `read_skill_file` reads.
pub const SKILL_FILE_TYPES: [&str; 16] =
    ["md", "markdown", "txt", "json", "csv", "tsv", "yaml", "yml", "xml", "html", "js", "mjs", "ts", "py", "sh", "sql"];

/// `^(?=.{1,64}$)[a-z0-9]+(?:-[a-z0-9]+)*$`: the names of skills and agents.
pub fn valid_skill_name(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty()
        && b.len() <= 64
        && b[0] != b'-'
        && b[b.len() - 1] != b'-'
        && !name.contains("--")
        && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// A skill, ready to list and activate.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Skill {
    pub name: String,
    pub description: String,
    /// The instructions (the Markdown after the frontmatter), trimmed.
    pub body: String,
    /// Where its folder is (a URL or path ending with "/"), for `read_skill_file`; empty = no files.
    pub base: String,
    /// `app` or `builtin`.
    pub source: String,
    pub license: Option<String>,
    pub compatibility: Option<String>,
    pub metadata: Option<Map>,
    /// Information shown to the model, never a pre-approval.
    pub allowed_tools: Vec<String>,
}

impl Skill {
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("name", self.name.as_str());
        m.insert("description", self.description.as_str());
        m.insert("body", self.body.as_str());
        m.insert("base", self.base.as_str());
        m.insert("source", self.source.as_str());
        if let Some(l) = &self.license {
            m.insert("license", l.as_str());
        }
        if let Some(c) = &self.compatibility {
            m.insert("compatibility", c.as_str());
        }
        if let Some(md) = &self.metadata {
            m.insert("metadata", md.clone());
        }
        m.insert("allowedTools", self.allowed_tools.clone());
        Value::Object(m)
    }
}

/// A skill file read: the skill (None when it cannot be used) and the problems found.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedSkill {
    pub skill: Option<Skill>,
    pub problems: Vec<String>,
}

/// A SKILL.md file → the skill and its problems. `base`: the skill's folder; `folder`: the folder's name (the
/// format wants it to equal `name`); `source`: `app` or `builtin`.
pub fn parse_skill(text: &str, base: &str, folder: &str, source: &str) -> ParsedSkill {
    let fm = parse_frontmatter(text);
    let data = &fm.data;
    let mut problems: Vec<String> = fm.errors.iter().map(|e| format!("frontmatter: {e}")).collect();
    let text_field = |k: &str| data.get(k).and_then(Value::as_str).map(|s| js_trim(s).to_string()).unwrap_or_default();
    let name = text_field("name");
    let description = text_field("description");
    if name.is_empty() {
        problems.push("name is missing (frontmatter \"name:\").".into());
    } else if !valid_skill_name(&name) {
        problems.push(format!(
            "name \"{name}\" must be 1-64 lowercase letters, digits and single hyphens (not at the start or end)."
        ));
    }
    if description.is_empty() {
        problems.push("description is missing: it is what the model reads to decide when to use the skill.".into());
    } else if utf16_len(&description) > 1024 {
        problems.push(format!("description is {} characters long; the format allows 1024.", utf16_len(&description)));
    }
    if !folder.is_empty() && !name.is_empty() && folder != name {
        problems.push(format!("the folder is \"{folder}\" but the name is \"{name}\"; the format wants them equal."));
    }
    if let Some(c) = data.get("compatibility") {
        if utf16_len(&c.js_string()) > 500 {
            problems.push("compatibility is longer than 500 characters.".into());
        }
    }
    if let Some(m) = data.get("metadata") {
        if !matches!(m, Value::Object(_)) {
            problems.push("metadata must be a mapping of keys to values.".into());
        }
    }
    if js_trim(&fm.body).is_empty() {
        problems.push("the instructions (the Markdown after the frontmatter) are empty.".into());
    }
    if name.is_empty() || !valid_skill_name(&name) || description.is_empty() {
        return ParsedSkill { skill: None, problems };
    }
    let truthy = |k: &str| data.get(k).filter(|v| v.truthy()).map(Value::js_string);
    ParsedSkill {
        skill: Some(Skill {
            description: js_head(&description, 1024).to_string(),
            body: js_trim(&fm.body).to_string(),
            base: base.to_string(),
            source: source.to_string(),
            license: truthy("license"),
            compatibility: truthy("compatibility"),
            metadata: data.get("metadata").and_then(Value::as_object).cloned(),
            allowed_tools: list_value(data.get("allowed-tools").unwrap_or(&Value::Null)),
            name,
        }),
        problems,
    }
}

/// A skill given inline (`{ name, description, body | instructions, allowed-tools?, base? }`, or `{ text }`).
pub fn normalize_skill(def: &Value, source: &str) -> ParsedSkill {
    let Some(d) = def.as_object() else {
        return ParsedSkill { skill: None, problems: vec!["a skill must be an object.".into()] };
    };
    let base = d.get("base").filter(|b| b.truthy()).map(Value::js_string).unwrap_or_default();
    if let Some(Value::String(text)) = d.get("text") {
        return parse_skill(text, &base, "", source);
    }
    let field = |k: &str| d.get(k).filter(|v| !v.is_null());
    let text_of = |v: Option<&Value>| v.map(Value::js_string).unwrap_or_default();
    let body = text_of(field("body").or_else(|| field("instructions")));
    let fm = [
        "---".to_string(),
        format!("name: {}", quote(&text_of(field("name")))),
        format!("description: {}", quote(&text_of(field("description")))),
        "---".to_string(),
        body,
    ]
    .join("\n");
    let mut r = parse_skill(&fm, &base, "", source);
    if let Some(skill) = &mut r.skill {
        if let Some(v) = d.get("allowed-tools") {
            skill.allowed_tools = list_value(v);
        }
        if let Some(v) = d.get("allowedTools") {
            skill.allowed_tools = list_value(v);
        }
    }
    r
}

/// A path the model asked to read inside a skill's folder → the clean relative path, or the reason it is refused.
/// Relative paths only, no `..`, no hidden files, text types only.
pub fn skill_file_path(path: &str) -> Result<String, String> {
    let t = js_trim(path);
    let p = match t.strip_prefix("./") {
        Some(rest) => rest.trim_start_matches('/'),
        None => t,
    };
    if p.is_empty() {
        return Err("Give the path of a file inside the skill, for example references/guide.md.".into());
    }
    let scheme = {
        let b = p.as_bytes();
        let mut k = 1;
        while k < b.len() && (b[k].is_ascii_alphanumeric() || matches!(b[k], b'+' | b'.' | b'-')) {
            k += 1;
        }
        b[0].is_ascii_alphabetic() && b.get(k) == Some(&b':')
    };
    if scheme || p.starts_with('/') || p.contains('\\') {
        return Err(format!(
            "\"{p}\" is not a path inside the skill: use a relative path such as references/guide.md."
        ));
    }
    if p.split('/').any(|s| s.is_empty() || s == "." || s == ".." || s.starts_with('.')) {
        return Err(format!("\"{p}\" leaves the skill's folder or names a hidden file."));
    }
    let ext = p
        .rsplit_once('.')
        .map(|(_, e)| e)
        .filter(|e| !e.is_empty() && e.bytes().all(|c| c.is_ascii_alphanumeric()))
        .unwrap_or("")
        .to_ascii_lowercase();
    if !SKILL_FILE_TYPES.contains(&ext.as_str()) {
        return Err(format!("\"{p}\" is not a text file the agent can read ({}).", SKILL_FILE_TYPES.join(", ")));
    }
    if p.to_uppercase() == "SKILL.MD" {
        return Err("SKILL.md is the skill itself: call use_skill instead.".into());
    }
    Ok(p.to_string())
}

/// `/proofreading fix the second paragraph` → the skill's name and the rest of the message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashCommand {
    pub name: String,
    pub rest: String,
}

static SLASH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!("^/([a-z0-9]+(?:-[a-z0-9]+)*)(?:[{JS_SPACE_CLASS}]+((?s:.*)))?$")).expect("regex")
});

/// A message starting with `/name` of a known skill, or None.
pub fn parse_slash_command<S: AsRef<str>>(text: &str, names: &[S]) -> Option<SlashCommand> {
    let c = SLASH.captures(js_trim(text))?;
    let name = c.get(1)?.as_str();
    if !names.iter().any(|n| n.as_ref() == name) {
        return None;
    }
    Some(SlashCommand { name: name.to_string(), rest: js_trim(c.get(2).map_or("", |m| m.as_str())).to_string() })
}

/// Text cut to `max` characters, saying so.
pub fn clip_text(text: &str, max: usize, what: &str) -> String {
    let len = utf16_len(text);
    if len <= max {
        return text.to_string();
    }
    let keep = max.saturating_sub(120);
    format!(
        "{}\n…[{what} cut: the first {} of {} characters are shown]",
        js_head(text, keep),
        locale_count(keep as f64),
        locale_count(len as f64)
    )
}

static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("[{JS_SPACE_CLASS}]+")).expect("regex"));

/// The SKILLS section of the system prompt: the skills this agent may use (name and description), then the
/// instructions of each active one. '' when there are none. `can_load`: the model can call `use_skill`.
pub fn build_skills_prompt(skills: &[&Skill], active: &[String], can_load: bool) -> String {
    if skills.is_empty() {
        return String::new();
    }
    let on = |s: &Skill| active.contains(&s.name);
    let mut lines = vec!["== SKILLS ==".to_string()];
    lines.push(if can_load {
        "Skills are instructions for particular kinds of work in this application. When the user's request matches a skill below, call `use_skill` with its name before you start, then follow it; it stays active for the rest of the conversation. Do not guess what a skill says. A message starting with /skill-name activates that skill.".into()
    } else {
        "Skills are instructions for particular kinds of work in this application. The active ones are below; follow them when they apply.".into()
    });
    for s in skills {
        lines.push(format!(
            "- {}{} — {}",
            s.name,
            if on(s) { " (active)" } else { "" },
            SPACES.replace_all(&s.description, " ")
        ));
    }
    for s in skills.iter().filter(|s| on(s)) {
        lines.push(String::new());
        lines.push(format!("== SKILL: {} (active) ==", s.name));
        lines.push(clip_text(&s.body, SKILL_BODY_MAX, "skill"));
        if can_load {
            lines.push(format!("(Files this skill points to — relative to its folder — can be read with read_skill_file, skill \"{}\".)", s.name));
        }
    }
    lines.join("\n")
}

/// How a tool a skill names stands for the active agent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    On,
    Off,
    Missing,
    Denied,
}

/// What `use_skill` tells the model: the skill is active, and how the tools it names stand right now.
pub fn skill_loaded_message(skill: &Skill, tool_state: impl Fn(&str) -> ToolState) -> String {
    let mut parts = vec![format!(
        "The skill \"{0}\" is active: its instructions are now in your system prompt, under == SKILL: {0} (active) ==. Follow them.",
        skill.name
    )];
    if !skill.allowed_tools.is_empty() {
        let word = |s: ToolState| match s {
            ToolState::On => "on",
            ToolState::Off => "turned off — ask with request_tool if you need it",
            ToolState::Missing => "not in this application",
            ToolState::Denied => "not permitted",
        };
        let list: Vec<String> = skill.allowed_tools.iter().map(|t| format!("{t} ({})", word(tool_state(t)))).collect();
        parts.push(format!("Tools it uses: {}.", list.join(", ")));
    }
    parts.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_and_loaded_message() {
        let a = Skill {
            name: "proofreading".into(),
            description: "Check\n  spelling.".into(),
            body: "1. Read.".into(),
            allowed_tools: vec!["find".into(), "gone".into()],
            ..Default::default()
        };
        let b =
            Skill { name: "summarize".into(), description: "Sum up.".into(), body: "B".into(), ..Default::default() };
        let text = build_skills_prompt(&[&a, &b], &["proofreading".into()], true);
        assert!(text.contains("\n- proofreading (active) — Check spelling.\n- summarize — Sum up.\n\n== SKILL: proofreading (active) ==\n1. Read.\n(Files this skill points to"));
        let msg = skill_loaded_message(&a, |t| if t == "find" { ToolState::On } else { ToolState::Missing });
        assert!(msg.ends_with("Tools it uses: find (on), gone (not in this application)."));
    }

    #[test]
    fn inline_skill() {
        let r = normalize_skill(
            &Value::parse(r#"{"name":"x","description":"Say \"hi\".","instructions":"Do it.","allowedTools":"a, b"}"#)
                .unwrap(),
            "app",
        );
        let s = r.skill.unwrap();
        assert_eq!((s.description.as_str(), s.body.as_str(), s.allowed_tools.len()), ("Say \"hi\".", "Do it.", 2));
    }
}
