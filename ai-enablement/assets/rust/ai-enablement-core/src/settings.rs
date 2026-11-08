//! The agent's settings (`core/settings.js`): validated like a schema — unknown keys dropped, an invalid value keeps
//! the base value, strings bounded, URLs http(s) without credentials. Layering: built-in defaults < the app's
//! defaults < what the user saved. API keys are never part of the settings (a `KeyStore` holds them).

use crate::json::{Map, Value};
use crate::providers::{provider, provider_ids};
use regex::Regex;
use std::collections::BTreeMap;
use std::sync::OnceLock;

pub const DEFAULT_HISTORY_MESSAGES: i64 = 20;
pub const DEFAULT_MAX_CONTEXT_CHARS: i64 = 24_000;
pub const DEFAULT_MAX_FILE_CHARS: i64 = 40_000;

/// Length limits of string settings.
pub const LIMIT_URL: usize = 400;
pub const LIMIT_MODEL: usize = 200;
pub const LIMIT_PROMPT: usize = 20_000;

/// A provider's remembered address and model.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Profile {
    pub base_url: Option<String>,
    pub model: Option<String>,
}

/// Every setting, sanitised.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub provider: String,
    pub profiles: BTreeMap<String, Profile>,
    /// `direct` or `relay`.
    pub transport: String,
    pub relay_url: String,
    pub fallback_provider: String,
    pub system_prompt: String,
    pub temperature: f64,
    pub max_output_tokens: i64,
    pub history_messages: i64,
    pub max_context_chars: i64,
    /// `show`, `hide` or `off`.
    pub reasoning: String,
    pub share_screen: bool,
    pub timeout_sec: i64,
    pub remember_keys: bool,
    pub tools_enabled: bool,
    /// `auto`, `native` or `text`.
    pub tool_mode: String,
    pub confirm_writes: bool,
    pub confirm_destructive: bool,
    pub max_tool_steps: i64,
    pub tool_states: BTreeMap<String, bool>,
    pub memory_enabled: bool,
    pub memory_write: bool,
    pub vision: bool,
    pub screenshot_auto: bool,
    pub max_file_chars: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            provider: "lmstudio".into(),
            profiles: BTreeMap::new(),
            transport: "direct".into(),
            relay_url: String::new(),
            fallback_provider: String::new(),
            system_prompt: String::new(),
            temperature: 0.4,
            max_output_tokens: 4096,
            history_messages: DEFAULT_HISTORY_MESSAGES,
            max_context_chars: DEFAULT_MAX_CONTEXT_CHARS,
            reasoning: "show".into(),
            share_screen: true,
            timeout_sec: 120,
            remember_keys: false,
            tools_enabled: true,
            tool_mode: "auto".into(),
            confirm_writes: true,
            confirm_destructive: true,
            max_tool_steps: 8,
            tool_states: BTreeMap::new(),
            memory_enabled: true,
            memory_write: true,
            vision: true,
            screenshot_auto: false,
            max_file_chars: DEFAULT_MAX_FILE_CHARS,
        }
    }
}

fn re(cell: &'static OnceLock<Regex>, pattern: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pattern).expect("settings pattern"))
}

/// An http(s) URL without credentials or a key in its query, at most 400 characters (`validUrl`).
pub fn valid_url(v: &str) -> bool {
    static CTRL: OnceLock<Regex> = OnceLock::new();
    static KEYQ: OnceLock<Regex> = OnceLock::new();
    if crate::text::utf16_len(v) > LIMIT_URL || re(&CTRL, r"[\s\x00-\x1f]").is_match(v) {
        return false;
    }
    let lower = v.to_ascii_lowercase();
    let rest = if let Some(r) = lower.strip_prefix("http://") {
        r
    } else if let Some(r) = lower.strip_prefix("https://") {
        r
    } else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') || crate::providers::url_host(v).is_none() {
        return false;
    }
    let search = v.split_once('?').map(|(_, q)| format!("?{}", q.split('#').next().unwrap_or(""))).unwrap_or_default();
    !re(&KEYQ, r"(?i)[?&](key|api_key|token|access_token)=").is_match(&search)
}

fn valid_endpoint(v: &str) -> bool {
    static REL: OnceLock<Regex> = OnceLock::new();
    v.is_empty()
        || valid_url(v)
        || (crate::text::utf16_len(v) <= LIMIT_URL
            && re(&REL, r"^[A-Za-z0-9_./~%-][A-Za-z0-9_./~%?=&-]*$").is_match(v)
            && !v.starts_with("//"))
}

fn valid_model(v: &str) -> bool {
    static M: OnceLock<Regex> = OnceLock::new();
    crate::text::utf16_len(v) <= LIMIT_MODEL && re(&M, r"^[A-Za-z0-9_@:./+\- ]*$").is_match(v)
}

fn valid_tool_name(v: &str) -> bool {
    static T: OnceLock<Regex> = OnceLock::new();
    re(&T, r"^[A-Za-z][A-Za-z0-9_-]{0,63}$").is_match(v)
}

/// A JSON number that is an integer within [min, max] (`Number.isInteger` and the range).
fn int_in(v: &Value, min: i64, max: i64) -> Option<i64> {
    let n = v.as_f64()?;
    (n.is_finite() && n.fract() == 0.0 && n >= min as f64 && n <= max as f64).then_some(n as i64)
}

fn num_in(v: &Value, min: f64, max: f64) -> Option<f64> {
    let n = v.as_f64()?;
    (n.is_finite() && n >= min && n <= max).then_some(n)
}

fn one_of(v: &Value, allowed: &[&str]) -> Option<String> {
    let s = v.as_str()?;
    allowed.contains(&s).then(|| s.to_string())
}

impl Settings {
    /// Merge a JSON candidate over `base`, field by field (`sanitizeSettings(candidate, base)`).
    pub fn sanitize(candidate: &Value, base: &Settings) -> Settings {
        let empty = Map::new();
        let src = candidate.as_object().unwrap_or(&empty);
        let mut out = base.clone();
        let get = |k: &str| src.get(k);
        let ids: Vec<&str> = provider_ids().collect();
        let mut with_none = vec![""];
        with_none.extend(ids.iter().copied());

        if let Some(v) = get("provider").and_then(|v| one_of(v, &ids)) {
            out.provider = v;
        }
        if let Some(v) = get("transport").and_then(|v| one_of(v, &["direct", "relay"])) {
            out.transport = v;
        }
        if let Some(v) = get("relayUrl").and_then(|v| v.as_str()).filter(|s| valid_endpoint(s)) {
            out.relay_url = v.to_string();
        }
        if let Some(v) = get("fallbackProvider").and_then(|v| one_of(v, &with_none)) {
            out.fallback_provider = v;
        }
        if let Some(v) =
            get("systemPrompt").and_then(|v| v.as_str()).filter(|s| crate::text::utf16_len(s) <= LIMIT_PROMPT)
        {
            out.system_prompt = v.to_string();
        }
        if let Some(v) = get("temperature").and_then(|v| num_in(v, 0.0, 2.0)) {
            out.temperature = v;
        }
        let ints: [(&str, i64, i64, &mut i64); 6] = [
            ("maxOutputTokens", 64, 64_000, &mut out.max_output_tokens),
            ("historyMessages", 2, 200, &mut out.history_messages),
            ("maxContextChars", 1000, 400_000, &mut out.max_context_chars),
            ("timeoutSec", 10, 900, &mut out.timeout_sec),
            ("maxToolSteps", 1, 30, &mut out.max_tool_steps),
            ("maxFileChars", 1000, 400_000, &mut out.max_file_chars),
        ];
        for (k, min, max, slot) in ints {
            if let Some(v) = src.get(k).and_then(|v| int_in(v, min, max)) {
                *slot = v;
            }
        }
        if let Some(v) = get("reasoning").and_then(|v| one_of(v, &["show", "hide", "off"])) {
            out.reasoning = v;
        }
        if let Some(v) = get("toolMode").and_then(|v| one_of(v, &["auto", "native", "text"])) {
            out.tool_mode = v;
        }
        let bools: [(&str, &mut bool); 9] = [
            ("shareScreen", &mut out.share_screen),
            ("rememberKeys", &mut out.remember_keys),
            ("toolsEnabled", &mut out.tools_enabled),
            ("confirmWrites", &mut out.confirm_writes),
            ("confirmDestructive", &mut out.confirm_destructive),
            ("memoryEnabled", &mut out.memory_enabled),
            ("memoryWrite", &mut out.memory_write),
            ("vision", &mut out.vision),
            ("screenshotAuto", &mut out.screenshot_auto),
        ];
        for (k, slot) in bools {
            if let Some(v) = src.get(k).and_then(|v| v.as_bool()) {
                *slot = v;
            }
        }
        // Profiles: per provider, a valid address and model over the base's.
        if let Some(p) = get("profiles").and_then(|v| v.as_object()) {
            for id in &ids {
                let Some(prof) = p.get(id).and_then(|v| v.as_object()) else { continue };
                let entry = out.profiles.entry((*id).to_string()).or_default();
                if let Some(u) = prof.get("baseUrl").and_then(|v| v.as_str()).filter(|s| valid_url(s)) {
                    entry.base_url = Some(u.to_string());
                }
                if let Some(m) = prof.get("model").and_then(|v| v.as_str()).filter(|s| valid_model(s)) {
                    entry.model = Some(crate::text::js_trim(m).to_string());
                }
                if entry.base_url.is_none() && entry.model.is_none() {
                    out.profiles.remove(*id);
                }
            }
        }
        // Tool states: valid names with boolean values, over the base's.
        if let Some(t) = get("toolStates").and_then(|v| v.as_object()) {
            for (name, on) in t.iter() {
                if let (true, Some(b)) = (valid_tool_name(name), on.as_bool()) {
                    out.tool_states.insert(name.clone(), b);
                }
            }
        }
        if out.fallback_provider == out.provider {
            out.fallback_provider.clear();
        }
        out
    }

    /// `sanitize` over the built-in defaults.
    pub fn from_json(candidate: &Value) -> Settings {
        Settings::sanitize(candidate, &Settings::default())
    }

    /// Apply a patch (what Settings > Save or a tool sends): `toolStates` and `profiles` merge, the rest replaces.
    pub fn save(&mut self, patch: &Value) {
        *self = Settings::sanitize(patch, self);
    }

    /// The effective address and model of a provider: its profile first, the catalog second (`profileFor`).
    pub fn profile_for(&self, id: &str) -> (String, String) {
        let p = provider(id);
        let prof = self.profiles.get(id);
        let base =
            prof.and_then(|x| x.base_url.clone()).filter(|s| !s.is_empty()).unwrap_or_else(|| p.base_url.to_string());
        let model = prof.and_then(|x| x.model.clone()).unwrap_or_default();
        (base, model)
    }

    /// The settings as the JS runtime stores them (camelCase keys).
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("provider", self.provider.as_str());
        let mut profiles = Map::new();
        for (id, p) in &self.profiles {
            let mut e = Map::new();
            if let Some(u) = &p.base_url {
                e.insert("baseUrl", u.as_str());
            }
            if let Some(md) = &p.model {
                e.insert("model", md.as_str());
            }
            profiles.insert(id.as_str(), e);
        }
        m.insert("profiles", profiles);
        m.insert("transport", self.transport.as_str());
        m.insert("relayUrl", self.relay_url.as_str());
        m.insert("fallbackProvider", self.fallback_provider.as_str());
        m.insert("systemPrompt", self.system_prompt.as_str());
        m.insert("temperature", self.temperature);
        m.insert("maxOutputTokens", self.max_output_tokens as f64);
        m.insert("historyMessages", self.history_messages as f64);
        m.insert("maxContextChars", self.max_context_chars as f64);
        m.insert("reasoning", self.reasoning.as_str());
        m.insert("shareScreen", self.share_screen);
        m.insert("timeoutSec", self.timeout_sec as f64);
        m.insert("rememberKeys", self.remember_keys);
        m.insert("toolsEnabled", self.tools_enabled);
        m.insert("toolMode", self.tool_mode.as_str());
        m.insert("confirmWrites", self.confirm_writes);
        m.insert("confirmDestructive", self.confirm_destructive);
        m.insert("maxToolSteps", self.max_tool_steps as f64);
        let mut states = Map::new();
        for (k, v) in &self.tool_states {
            states.insert(k.as_str(), *v);
        }
        m.insert("toolStates", states);
        m.insert("memoryEnabled", self.memory_enabled);
        m.insert("memoryWrite", self.memory_write);
        m.insert("vision", self.vision);
        m.insert("screenshotAuto", self.screenshot_auto);
        m.insert("maxFileChars", self.max_file_chars as f64);
        Value::Object(m)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value {
        Value::parse(s).unwrap()
    }

    #[test]
    fn invalid_values_keep_the_base() {
        let s = Settings::from_json(&v(
            r#"{"provider":"nope","maxOutputTokens":12.5,"temperature":3,"toolMode":"text","vision":"yes","maxToolSteps":30}"#,
        ));
        assert_eq!(s.provider, "lmstudio");
        assert_eq!(s.max_output_tokens, 4096);
        assert_eq!(s.temperature, 0.4);
        assert_eq!(s.tool_mode, "text");
        assert!(s.vision);
        assert_eq!(s.max_tool_steps, 30);
    }

    #[test]
    fn profiles_tool_states_and_save() {
        let mut s = Settings::from_json(&v(
            r#"{"provider":"openai","fallbackProvider":"openai","profiles":{"openai":{"baseUrl":"https://api.openai.com/v1/","model":" gpt-5-mini "},"google":{"baseUrl":"https://u:p@x.com"}},"toolStates":{"a":true,"9x":true,"b":1}}"#,
        ));
        assert_eq!(s.fallback_provider, "");
        assert_eq!(s.profile_for("openai"), ("https://api.openai.com/v1/".to_string(), "gpt-5-mini".to_string()));
        assert!(!s.profiles.contains_key("google"));
        assert_eq!(s.tool_states.len(), 1);
        s.save(&v(r#"{"toolStates":{"b":false}}"#));
        assert_eq!(s.tool_states.get("a"), Some(&true));
        assert_eq!(s.tool_states.get("b"), Some(&false));
        assert_eq!(s.profile_for("lmstudio").0, "http://127.0.0.1:9000");
    }

    #[test]
    fn urls() {
        assert!(valid_url("http://127.0.0.1:9000"));
        assert!(!valid_url("https://x.com/?api_key=1"));
        assert!(!valid_url("ftp://x.com"));
        assert!(valid_endpoint("/ai-relay"));
        assert!(valid_endpoint("api/relay.php"));
        assert!(!valid_endpoint("//evil.com"));
    }
}
