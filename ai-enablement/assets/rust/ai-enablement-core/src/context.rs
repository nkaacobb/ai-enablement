//! What the agent knows about the application, and what is on screen right now (`core/context.js`).
//!
//! Four layers, each given by the host application:
//!
//! - `app`: what the application is and does. Goes into the system prompt on every request.
//! - `page`: which page or view the user is on, and what it is for. Goes into the system prompt too.
//! - `content`: the live content of the screen. Fingerprinted: the snapshot is re-sent only when the hash changes.
//! - `view`: small, volatile UI state (cursor, selection, scroll). Sent with every question, never hashed.
//!
//! Each layer is a value, or a hook the runtime calls when it needs the layer ([`Source`]).

use std::fmt;
use std::sync::{Arc, LazyLock};

use regex::Regex;

use crate::hash::hash_text;
use crate::json::{stable_stringify, Map, Value};
use crate::text::{js_head, js_trim, locale_count, upper_first, utf16_len};

pub const DEFAULT_MAX_CONTEXT_CHARS: usize = 24_000;
/// The view state is clipped at this many characters.
pub const VIEW_MAX_CHARS: usize = 2_000;

/// A context hook: computes the layer when the runtime needs it. An `Err` is reported to the model as text.
pub type Hook = Arc<dyn Fn() -> Result<Value, String> + Send + Sync>;

/// A context layer: a value, or a hook that computes it.
#[derive(Clone)]
pub enum Source {
    Value(Value),
    Hook(Hook),
}

impl Source {
    pub fn hook(f: impl Fn() -> Result<Value, String> + Send + Sync + 'static) -> Self {
        Source::Hook(Arc::new(f))
    }

    pub fn resolve(&self) -> Result<Value, String> {
        match self {
            Source::Value(v) => Ok(v.clone()),
            Source::Hook(f) => f(),
        }
    }
}

impl fmt::Debug for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Source::Value(v) => write!(f, "Source::Value({v})"),
            Source::Hook(_) => f.write_str("Source::Hook(..)"),
        }
    }
}

impl<T: Into<Value>> From<T> for Source {
    fn from(v: T) -> Self {
        Source::Value(v.into())
    }
}

static CAMEL: LazyLock<Regex> = LazyLock::new(|| Regex::new("([a-z0-9])([A-Z])").expect("regex"));
static DASHES: LazyLock<Regex> = LazyLock::new(|| Regex::new("[_-]+").expect("regex"));

/// `openDialog` → `Open Dialog`, `max_items` → `Max items`.
pub fn humanize(key: &str) -> String {
    let s = CAMEL.replace_all(key, "$1 $2");
    let s = DASHES.replace_all(&s, " ");
    upper_first(js_trim(&s))
}

/// An app or page descriptor as prompt text: strings as written (trimmed); objects as `Humanized Key: value` lines,
/// lists as `- ` bullets under their key, nested objects as indented JSON; empty values left out.
pub fn describe(value: &Value) -> String {
    describe_skip(value, &[])
}

/// [`describe`], leaving out the keys in `skip`.
pub fn describe_skip(value: &Value, skip: &[&str]) -> String {
    let entries: Vec<(String, &Value)> = match value {
        Value::Null => return String::new(),
        Value::String(s) => return js_trim(s).to_string(),
        Value::Object(m) => m.iter().map(|(k, v)| (k.clone(), v)).collect(),
        Value::Array(items) => items.iter().enumerate().map(|(k, v)| (k.to_string(), v)).collect(),
        other => return other.js_string(),
    };
    let mut lines = Vec::new();
    for (key, v) in entries {
        if skip.contains(&key.as_str()) || v.is_null() || v.as_str() == Some("") {
            continue;
        }
        match v {
            Value::Array(items) => {
                if items.is_empty() {
                    continue;
                }
                lines.push(format!("{}:", humanize(&key)));
                for item in items {
                    let text = if item.type_of() == "object" { stable_stringify(item, 0) } else { item.js_string() };
                    lines.push(format!("- {text}"));
                }
            }
            Value::Object(_) => lines.push(format!("{}:\n{}", humanize(&key), stable_stringify(v, 2))),
            _ => lines.push(format!("{}: {}", humanize(&key), js_trim(&v.js_string()))),
        }
    }
    lines.join("\n")
}

/// Screen content as text: a string as written; `{ text }` alone as its text; anything else as stable JSON.
pub fn serialize_content(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        Value::Object(m) if m.len() == 1 && m.get("text").is_some_and(|t| t.as_str().is_some()) => {
            m.get("text").and_then(Value::as_str).unwrap_or_default().to_string()
        }
        other => stable_stringify(other, 2),
    }
}

/// Text cut to fit, saying so: `max_chars - 160` characters kept plus the truncation note. 0 = no limit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Clipped {
    pub text: String,
    pub truncated: bool,
}

pub fn clip(text: &str, max_chars: usize) -> Clipped {
    let len = utf16_len(text);
    if max_chars == 0 || len <= max_chars {
        return Clipped { text: text.to_string(), truncated: false };
    }
    let keep = max_chars.saturating_sub(160);
    Clipped {
        text: format!(
            "{}\n…[truncated: the first {} of {} characters are shown]",
            js_head(text, keep),
            locale_count(keep as f64),
            locale_count(len as f64)
        ),
        truncated: true,
    }
}

/// The screen content captured for one question (`ContextManager.snapshot()`).
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub page_id: String,
    pub page_title: String,
    /// What the model reads (clipped).
    pub text: String,
    /// Length of `text` (UTF-16 code units).
    pub chars: usize,
    /// Length of the full content before clipping.
    pub total_chars: usize,
    pub truncated: bool,
    /// cyrb53 of `pageId\npageTitle\nFULL content`.
    pub hash: String,
    /// The content is blank.
    pub empty: bool,
    /// When it was taken (ISO 8601).
    pub at: String,
    /// The content hook failed with this message.
    pub error: Option<String>,
}

impl Snapshot {
    /// Capture content: serialised, clipped at `max_chars`, fingerprinted over the full text before clipping.
    pub fn capture(
        page_id: &str,
        page_title: &str,
        content: Result<Value, String>,
        max_chars: usize,
        at: String,
    ) -> Self {
        let (full, error) = match content {
            Ok(v) => (serialize_content(&v), None),
            Err(e) => (format!("[The screen content could not be read: {e}]"), Some(e)),
        };
        let clipped = clip(&full, max_chars);
        Snapshot {
            page_id: page_id.to_string(),
            page_title: page_title.to_string(),
            chars: utf16_len(&clipped.text),
            total_chars: utf16_len(&full),
            truncated: clipped.truncated,
            text: clipped.text,
            hash: hash_text(&format!("{page_id}\n{page_title}\n{full}")),
            empty: js_trim(&full).is_empty(),
            at,
            error,
        }
    }

    /// As the JS runtime writes it (`{ pageId, pageTitle, text, chars, totalChars, truncated, hash, empty, at }`).
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("pageId", self.page_id.as_str());
        m.insert("pageTitle", self.page_title.as_str());
        m.insert("text", self.text.as_str());
        m.insert("chars", self.chars);
        m.insert("totalChars", self.total_chars);
        m.insert("truncated", self.truncated);
        m.insert("hash", self.hash.as_str());
        m.insert("empty", self.empty);
        m.insert("at", self.at.as_str());
        if let Some(e) = &self.error {
            m.insert("error", e.as_str());
        }
        Value::Object(m)
    }
}

/// The page the user is on: `id`, `title`, other descriptive keys (`purpose`, …) in order, and the content and view
/// layers.
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub id: String,
    pub title: Option<String>,
    /// Descriptive keys other than id and title, in the order the app gives them.
    pub meta: Map,
    pub content: Option<Source>,
    pub view: Option<Source>,
}

impl Page {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Page { id: id.into(), title: Some(title.into()), ..Default::default() }
    }

    /// A page object as the JS runtime takes it (`{ id, title, purpose, content, view, … }`). `tools` is left out:
    /// a page's tools go to the model as tools.
    pub fn from_value(v: &Value) -> Self {
        let mut page = Page { id: "page".into(), ..Default::default() };
        let Some(m) = v.as_object() else { return page };
        for (k, val) in m.iter() {
            match k.as_str() {
                "id" if !val.is_null() => page.id = val.js_string(),
                "title" if !val.is_null() => page.title = Some(val.js_string()),
                "content" => page.content = (!val.is_null()).then(|| Source::Value(val.clone())),
                "view" => page.view = (!val.is_null()).then(|| Source::Value(val.clone())),
                "tools" | "id" | "title" => {}
                _ => page.meta.insert(k.clone(), val.clone()),
            }
        }
        page
    }

    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.meta.insert(key, value);
        self
    }

    pub fn content(mut self, content: impl Into<Source>) -> Self {
        self.content = Some(content.into());
        self
    }

    pub fn view(mut self, view: impl Into<Source>) -> Self {
        self.view = Some(view.into());
        self
    }

    /// The title shown and fingerprinted: the title, else the id.
    pub fn title_or_id(&self) -> &str {
        self.title.as_deref().unwrap_or(&self.id)
    }
}

/// The context layers of one agent, as `ContextManager` in the JS runtime.
#[derive(Clone, Debug)]
pub struct ContextManager {
    pub app: Option<Source>,
    pub page: Option<Page>,
    pub max_chars: usize,
}

impl Default for ContextManager {
    fn default() -> Self {
        ContextManager { app: None, page: None, max_chars: DEFAULT_MAX_CONTEXT_CHARS }
    }
}

impl ContextManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_app(&mut self, app: impl Into<Source>) {
        self.app = Some(app.into());
    }

    pub fn set_page(&mut self, page: Option<Page>) {
        self.page = page;
    }

    /// Replace only the content of the current page.
    pub fn set_content(&mut self, content: impl Into<Source>) {
        self.page.get_or_insert_with(|| Page::new("page", "Page")).content = Some(content.into());
    }

    /// Replace only the view state of the current page.
    pub fn set_view(&mut self, view: impl Into<Source>) {
        self.page.get_or_insert_with(|| Page::new("page", "Page")).view = Some(view.into());
    }

    pub fn set_max_chars(&mut self, n: usize) {
        if n > 0 {
            self.max_chars = n;
        }
    }

    /// The page shares content (it may still be empty).
    pub fn has_content(&self) -> bool {
        self.page.as_ref().is_some_and(|p| p.content.is_some())
    }

    /// The `== APPLICATION ==` text.
    pub fn app_text(&self) -> String {
        match self.app.as_ref().map(Source::resolve) {
            None => String::new(),
            Some(Ok(v)) => describe(&v),
            Some(Err(e)) => format!("[app context hook failed: {e}]"),
        }
    }

    /// The `== CURRENT PAGE ==` text: id, title and the descriptive keys (not the content or view).
    pub fn page_text(&self) -> String {
        let Some(p) = &self.page else { return String::new() };
        let mut m = Map::new();
        m.insert("id", p.id.as_str());
        if let Some(t) = &p.title {
            m.insert("title", t.as_str());
        }
        for (k, v) in p.meta.iter() {
            m.insert(k.clone(), v.clone());
        }
        describe(&Value::Object(m))
    }

    /// Capture the screen content now.
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot_at(crate::time::iso_now())
    }

    /// [`ContextManager::snapshot`] with the capture time given (tests, replays).
    pub fn snapshot_at(&self, at: String) -> Snapshot {
        let (id, title) = match &self.page {
            Some(p) => (p.id.clone(), p.title_or_id().to_string()),
            None => ("page".to_string(), "page".to_string()),
        };
        let content = match self.page.as_ref().and_then(|p| p.content.as_ref()) {
            Some(src) => src.resolve(),
            None => Ok(Value::Null),
        };
        Snapshot::capture(&id, &title, content, self.max_chars, at)
    }

    /// The volatile view state, clipped at [`VIEW_MAX_CHARS`]; never hashed.
    pub fn view_text(&self) -> String {
        let Some(src) = self.page.as_ref().and_then(|p| p.view.as_ref()) else { return String::new() };
        match src.resolve() {
            Ok(Value::Null) => String::new(),
            Ok(Value::String(s)) if s.is_empty() => String::new(),
            Ok(Value::String(s)) => clip(&s, VIEW_MAX_CHARS).text,
            Ok(v) => clip(&describe(&v), VIEW_MAX_CHARS).text,
            Err(e) => format!("[view state hook failed: {e}]"),
        }
    }
}
