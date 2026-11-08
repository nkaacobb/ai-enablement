//! Permissions (`core/permissions.js`): which tools the agent may use without asking, must ask about, or may not
//! use at all.
//!
//! A policy is three rule lists, set by the application and by the active agent. A rule is a tool name, a name
//! with `*` wildcards, `toolset:<glob>` or `effect:<glob>`. Precedence: deny > ask > allow > the user's
//! confirmation settings. `deny` hides the tool from the model; `ask` always confirms; `allow` runs it without the
//! card. A `system` tool always confirms: no setting and no allow rule switches that off.
//!
//! These rules are a UI-side guard, not access control: the application's own checks stay authoritative.

use serde::{Deserialize, Serialize};

use crate::json::Value;
use crate::schema::Effect;
use crate::text::{is_js_space, js_trim};
use crate::tools::ToolContract;

/// The rule lists, in the contract's order.
pub const PERMISSION_KEYS: [&str; 3] = ["allow", "ask", "deny"];

/// A policy: clean, de-duplicated rule lists.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    pub allow: Vec<String>,
    pub ask: Vec<String>,
    pub deny: Vec<String>,
}

impl Policy {
    fn list_mut(&mut self, key: &str) -> Option<&mut Vec<String>> {
        match key {
            "allow" => Some(&mut self.allow),
            "ask" => Some(&mut self.ask),
            "deny" => Some(&mut self.deny),
            _ => None,
        }
    }

    pub fn list(&self, decision: Decision) -> &[String] {
        match decision {
            Decision::Allow => &self.allow,
            Decision::Ask => &self.ask,
            Decision::Deny => &self.deny,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.allow.is_empty() && self.ask.is_empty() && self.deny.is_empty()
    }

    pub fn to_value(&self) -> Value {
        Value::Object(
            [("allow", self.allow.clone()), ("ask", self.ask.clone()), ("deny", self.deny.clone())]
                .into_iter()
                .collect(),
        )
    }
}

/// `^(?:(toolset|effect):)?[\w*-]{1,64}$`
fn valid_rule(rule: &str) -> bool {
    let body = rule.strip_prefix("toolset:").or_else(|| rule.strip_prefix("effect:")).unwrap_or(rule);
    !body.is_empty()
        && body.len() <= 64
        && body.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'*' || b == b'-')
}

/// A policy from JSON (`{ allow, ask, deny }`, each a list or a comma/space-separated string). Invalid rules are
/// left out and reported in `problems`.
pub fn normalize_policy(value: &Value, problems: &mut Vec<String>) -> Policy {
    let mut out = Policy::default();
    let Some(m) = value.as_object() else {
        if !value.is_null() {
            problems.push("permissions must be an object with allow / ask / deny lists.".into());
        }
        return out;
    };
    for (key, list) in m.iter() {
        let Some(target) = out.list_mut(key) else {
            problems.push(format!("permissions: unknown list \"{key}\" (use allow, ask or deny)."));
            continue;
        };
        let items: Vec<String> = match list {
            Value::Array(items) => {
                items.iter().map(|v| if v.is_null() { String::new() } else { v.js_string() }).collect()
            }
            Value::Null => vec![],
            other => other.js_string().split(|c: char| c == ',' || is_js_space(c)).map(str::to_string).collect(),
        };
        for raw in items {
            let rule = js_trim(&raw);
            if rule.is_empty() {
                continue;
            }
            if !valid_rule(rule) {
                problems.push(format!("permissions.{key}: \"{rule}\" is not a rule (a tool name, name_*, toolset:<name> or effect:<effect>)."));
                continue;
            }
            if !target.iter().any(|r| r == rule) {
                target.push(rule.to_string());
            }
        }
    }
    out
}

/// Several policies as one: each list is the union of theirs.
pub fn merge_policies<'a>(policies: impl IntoIterator<Item = &'a Policy>) -> Policy {
    let mut out = Policy::default();
    for p in policies {
        for (src, dst) in [(&p.allow, &mut out.allow), (&p.ask, &mut out.ask), (&p.deny, &mut out.deny)] {
            for rule in src {
                if !dst.contains(rule) {
                    dst.push(rule.clone());
                }
            }
        }
    }
    out
}

/// `*` wildcards; everything else literal.
pub fn glob(pattern: &str, value: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == value;
    }
    let parts: Vec<&str> = pattern.split('*').collect();
    let mut at = 0;
    for (k, part) in parts.iter().enumerate() {
        if k == 0 {
            if !value.starts_with(part) {
                return false;
            }
            at = part.len();
            continue;
        }
        if k == parts.len() - 1 {
            return value.len() - at >= part.len() && value.ends_with(part);
        }
        match value[at..].find(part) {
            Some(found) => at += found + part.len(),
            None => return false,
        }
    }
    true
}

/// What a rule is matched against: a tool's name, effect and toolsets.
#[derive(Clone, Debug)]
pub struct Subject<'a> {
    pub name: &'a str,
    pub effect: Effect,
    pub toolsets: Vec<&'a str>,
}

impl<'a> From<&'a ToolContract> for Subject<'a> {
    fn from(t: &'a ToolContract) -> Self {
        Subject { name: &t.name, effect: t.effect, toolsets: t.toolsets_of() }
    }
}

/// Does one rule cover this tool?
pub fn rule_matches(rule: &str, tool: &Subject) -> bool {
    if let Some(p) = rule.strip_prefix("effect:") {
        return glob(p, tool.effect.as_str());
    }
    if let Some(p) = rule.strip_prefix("toolset:") {
        return tool.toolsets.iter().any(|ts| glob(p, ts));
    }
    glob(rule, tool.name)
}

/// A rule list's verdict.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Deny,
    Ask,
    Allow,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Deny => "deny",
            Decision::Ask => "ask",
            Decision::Allow => "allow",
        }
    }
}

/// `deny`, `ask`, `allow`, or None (no rule: the user's settings decide).
pub fn decide_permission(tool: &Subject, policy: &Policy) -> Option<Decision> {
    [Decision::Deny, Decision::Ask, Decision::Allow]
        .into_iter()
        .find(|d| policy.list(*d).iter().any(|r| rule_matches(r, tool)))
}

/// The user's confirmation settings (Settings > Tools). Both on by default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ConfirmSettings {
    pub confirm_writes: bool,
    pub confirm_destructive: bool,
}

impl Default for ConfirmSettings {
    fn default() -> Self {
        ConfirmSettings { confirm_writes: true, confirm_destructive: true }
    }
}

/// Must the user confirm this call? The first matching line decides: system → always; ask → yes; allow → no;
/// write → while `confirm_writes`, unless allowed for this chat; destructive / external → while
/// `confirm_destructive`; read → no.
pub fn needs_confirmation(
    effect: Effect,
    decision: Option<Decision>,
    settings: ConfirmSettings,
    allowed_for_chat: bool,
) -> bool {
    if effect == Effect::System {
        return true;
    }
    match decision {
        Some(Decision::Ask) => return true,
        Some(Decision::Allow) => return false,
        _ => {}
    }
    match effect {
        Effect::Write => settings.confirm_writes && !allowed_for_chat,
        Effect::Destructive | Effect::External => settings.confirm_destructive,
        _ => false,
    }
}

/// A policy in one line, for prompts and settings (`deny: new_document · ask: toolset:file`).
pub fn describe_policy(policy: &Policy) -> String {
    [("allow", &policy.allow), ("ask", &policy.ask), ("deny", &policy.deny)]
        .into_iter()
        .filter(|(_, l)| !l.is_empty())
        .map(|(k, l)| format!("{k}: {}", l.join(", ")))
        .collect::<Vec<_>>()
        .join(" · ")
}
