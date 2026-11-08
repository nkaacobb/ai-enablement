//! The context-sync protocol (`core/conversation.js`): for each question, does the model already have the screen?
//!
//! Chat APIs are stateless: every request resends the conversation. So "the model has the page" means "a snapshot
//! with the same fingerprint as the screen now is inside the part of the transcript that will be sent". Before each
//! question the runtime fingerprints the screen, looks for the newest snapshot inside the history window, and
//! attaches a new one only when the hashes differ. Older snapshots become one-line stubs when the request is built.
//!
//! The transcript record ([`Message`]) serialises exactly as the JS runtime saves chats, so a saved chat moves
//! between runtimes.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::{NoExpand, Regex};
use serde::{Deserialize, Serialize};

use crate::context::Snapshot;
use crate::files::{file_block, file_stub, FileRecord, StubReason};
use crate::hash::short_hash;
use crate::json::{number_to_string, Value};
use crate::text::{js_head, upper_first};

pub const DEFAULT_HISTORY_MESSAGES: i64 = 20;
/// Screenshots are large: only the newest questions that carry one are sent with their image.
pub const DEFAULT_IMAGE_MESSAGES: usize = 2;

/// Who wrote a transcript entry. Anything that is not `user` is read as `assistant`, as in JS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    #[default]
    User,
    #[serde(other)]
    Assistant,
}

/// A snapshot as a transcript keeps it. Saved chats drop `text`, which makes the snapshot a stub.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SnapshotRecord {
    pub hash: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub page_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub page_title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chars: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_chars: Option<f64>,
    pub truncated: bool,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub at: String,
}

impl From<&Snapshot> for SnapshotRecord {
    fn from(s: &Snapshot) -> Self {
        SnapshotRecord {
            hash: s.hash.clone(),
            text: Some(s.text.clone()),
            page_id: s.page_id.clone(),
            page_title: s.page_title.clone(),
            chars: Some(s.chars as f64),
            total_chars: Some(s.total_chars as f64),
            truncated: s.truncated,
            at: s.at.clone(),
        }
    }
}

/// An image attached to a question: a screenshot, or an image file (`kind: "image"`, with its name).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Shot {
    pub id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub thumb: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub kind: String,
}

impl Shot {
    fn is_image_file(&self) -> bool {
        self.kind == "image" || !self.name.is_empty()
    }
}

/// An image as it goes to a provider: MIME type and base64 data.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    pub mime: String,
    pub data: String,
}

/// A tool call recorded with an assistant turn (what `[Actions taken: …]` lists).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Action {
    pub call: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub title: String,
    pub status: String,
    /// Absent on some records (a request_tool row), present and maybe empty on others, as in JS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub thumb: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub shot: String,
}

/// One transcript entry, as the JS runtime saves it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Message {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub at: Option<f64>,
    /// `attached | unchanged | off | empty | …` (the plan's reason when nothing was attached).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub sync: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<SnapshotRecord>,
    /// The hash of the snapshot the screen still matched (`sync: "unchanged"`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub snapshot_ref: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub shots: Vec<Shot>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<FileRecord>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    /// The skill this message activated (`/name`).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub skill: String,
    /// The reply's footer (provider and model, or "Stopped").
    #[serde(skip_serializing_if = "String::is_empty")]
    pub meta: String,
}

impl Message {
    pub fn user(content: impl Into<String>) -> Self {
        Message { role: Role::User, content: content.into(), ..Default::default() }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Message { role: Role::Assistant, content: content.into(), ..Default::default() }
    }
}

/// Index of the first transcript entry that will still be sent, for a transcript of `length` entries.
pub fn window_start(length: usize, history_messages: i64) -> usize {
    let keep = history_messages.max(2);
    (length as i64 - keep).max(0) as usize
}

/// The newest snapshot (with its text still available) at or after `from`: (index, snapshot).
pub fn latest_snapshot(messages: &[Message], from: usize) -> Option<(usize, &SnapshotRecord)> {
    (from..messages.len())
        .rev()
        .find_map(|i| messages[i].snapshot.as_ref().filter(|s| s.text.is_some() && !s.hash.is_empty()).map(|s| (i, s)))
}

/// Why a question does or does not carry a snapshot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncReason {
    Off,
    Empty,
    Forced,
    Unread,
    Trimmed,
    Changed,
    Unchanged,
}

impl SyncReason {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncReason::Off => "off",
            SyncReason::Empty => "empty",
            SyncReason::Forced => "forced",
            SyncReason::Unread => "unread",
            SyncReason::Trimmed => "trimmed",
            SyncReason::Changed => "changed",
            SyncReason::Unchanged => "unchanged",
        }
    }
}

/// The screen as the plan needs it: its fingerprint, and whether it is blank.
#[derive(Clone, Copy, Debug)]
pub struct Screen<'a> {
    pub hash: &'a str,
    pub empty: bool,
}

impl<'a> From<&'a Snapshot> for Screen<'a> {
    fn from(s: &'a Snapshot) -> Self {
        Screen { hash: &s.hash, empty: s.empty }
    }
}

/// What [`plan_turn`] decides.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub attach: bool,
    pub reason: SyncReason,
    /// The hash of the newest snapshot inside the window.
    pub synced_hash: Option<String>,
}

/// The inputs of [`plan_turn`] and [`context_state`].
#[derive(Clone, Copy, Debug)]
pub struct TurnInput<'a> {
    /// The transcript BEFORE the new question.
    pub messages: &'a [Message],
    /// The screen now (None: the page shares no content).
    pub screen: Option<Screen<'a>>,
    pub history_messages: i64,
    /// The user allows sharing the screen (and the agent receives it).
    pub share: bool,
    /// The user pressed "Re-read".
    pub force: bool,
}

impl<'a> TurnInput<'a> {
    pub fn new(messages: &'a [Message], screen: Option<Screen<'a>>) -> Self {
        TurnInput { messages, screen, history_messages: DEFAULT_HISTORY_MESSAGES, share: true, force: false }
    }
}

/// Must the next question carry a snapshot? `off → empty → forced → unread / trimmed → changed → unchanged`.
pub fn plan_turn(input: &TurnInput) -> Plan {
    if !input.share {
        return Plan { attach: false, reason: SyncReason::Off, synced_hash: None };
    }
    let Some(screen) = input.screen.filter(|s| !s.empty) else {
        return Plan { attach: false, reason: SyncReason::Empty, synced_hash: None };
    };
    let messages = input.messages;
    let latest = latest_snapshot(messages, window_start(messages.len() + 1, input.history_messages));
    let synced_hash = latest.map(|(_, s)| s.hash.clone());
    if input.force {
        return Plan { attach: true, reason: SyncReason::Forced, synced_hash };
    }
    match latest {
        None => {
            let reason =
                if messages.iter().any(|m| m.snapshot.is_some()) { SyncReason::Trimmed } else { SyncReason::Unread };
            Plan { attach: true, reason, synced_hash }
        }
        Some((_, s)) if s.hash != screen.hash => Plan { attach: true, reason: SyncReason::Changed, synced_hash },
        Some(_) => Plan { attach: false, reason: SyncReason::Unchanged, synced_hash },
    }
}

/// The flag shown to the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlagState {
    /// The model has exactly what is on screen.
    Synced,
    /// The screen changed since the model last saw it.
    Dirty,
    /// The model has not seen this page in this conversation yet.
    Unread,
    /// The page provides no content (or it is empty).
    None,
    /// Sharing is switched off.
    Off,
}

impl FlagState {
    pub fn as_str(self) -> &'static str {
        match self {
            FlagState::Synced => "synced",
            FlagState::Dirty => "dirty",
            FlagState::Unread => "unread",
            FlagState::None => "none",
            FlagState::Off => "off",
        }
    }
}

/// [`context_state`]: the flag, and whether the next question will carry a fresh snapshot (`pending`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextState {
    pub state: FlagState,
    pub pending: bool,
    pub reason: SyncReason,
    pub current_hash: Option<String>,
    pub synced_hash: Option<String>,
}

pub fn context_state(input: &TurnInput) -> ContextState {
    let plan = plan_turn(input);
    let current = input.screen.map(|s| s.hash).filter(|h| !h.is_empty());
    let state = match plan.reason {
        SyncReason::Off => FlagState::Off,
        SyncReason::Empty => FlagState::None,
        SyncReason::Unchanged => FlagState::Synced,
        SyncReason::Forced => {
            if plan.synced_hash.as_deref() == input.screen.map(|s| s.hash) {
                FlagState::Synced
            } else {
                FlagState::Dirty
            }
        }
        SyncReason::Changed => FlagState::Dirty,
        SyncReason::Unread | SyncReason::Trimmed => FlagState::Unread,
    };
    ContextState {
        state,
        pending: plan.attach,
        reason: plan.reason,
        current_hash: current.map(str::to_string),
        synced_hash: plan.synced_hash,
    }
}

fn attr(v: &str) -> String {
    v.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('\n', " ")
}

static CLOSE_SNAPSHOT: LazyLock<Regex> = LazyLock::new(|| Regex::new("(?i)</page_snapshot").expect("regex"));

/// The block the model reads. Its closing tag is neutralised inside the content so a document cannot fake it.
pub fn snapshot_block(s: &SnapshotRecord) -> String {
    let body = CLOSE_SNAPSHOT.replace_all(s.text.as_deref().unwrap_or(""), NoExpand("<\\/page_snapshot"));
    let chars = s.total_chars.or(s.chars).unwrap_or(crate::text::utf16_len(&body) as f64);
    let mut meta = vec![
        format!("page=\"{}\"", attr(&s.page_id)),
        format!("title=\"{}\"", attr(&s.page_title)),
        format!("hash=\"{}\"", short_hash(&s.hash)),
        format!("chars=\"{}\"", number_to_string(chars)),
    ];
    if s.truncated {
        meta.push("truncated=\"true\"".into());
    }
    if !s.at.is_empty() {
        meta.push(format!("captured=\"{}\"", attr(&s.at)));
    }
    format!("<page_snapshot {}>\n{body}\n</page_snapshot>", meta.join(" "))
}

/// The one line that replaces an older snapshot in a request.
pub fn superseded_stub(s: &SnapshotRecord) -> String {
    let title = if s.page_title.is_empty() { &s.page_id } else { &s.page_title };
    format!(
        "[Page snapshot {} of \"{title}\" omitted here: a newer snapshot appears later in the conversation.]",
        short_hash(&s.hash)
    )
}

/// `[Actions taken: call → done; …]` before an earlier answer that used tools ('' without actions).
pub fn actions_line(actions: &[Action]) -> String {
    if actions.is_empty() {
        return String::new();
    }
    let word = |s: &str| -> String {
        match s {
            "ok" => "done".into(),
            "error" => "failed".into(),
            "declined" => "declined by the user".into(),
            "off" => "tool turned off".into(),
            "skipped" => "skipped".into(),
            other => other.into(),
        }
    };
    let items: Vec<String> = actions
        .iter()
        .map(|a| {
            let summary = match a.summary.as_deref() {
                Some(s) if !s.is_empty() => format!(" ({})", js_head(s, 120)),
                _ => String::new(),
            };
            format!("{} → {}{summary}", a.call, word(&a.status))
        })
        .collect();
    format!("[Actions taken: {}]\n\n", items.join("; "))
}

fn quoted(name: &str) -> String {
    let n = if name.is_empty() { "image" } else { name };
    let clean: String = n.chars().map(|c| if matches!(c, '"' | '\n' | '\r') { ' ' } else { c }).collect();
    format!("\"{}\"", js_head(&clean, 120))
}

fn list_text(items: &[String]) -> String {
    match items.len() {
        0 => String::new(),
        1 | 2 => items.join(" and "),
        n => format!("{} and {}", items[..n - 1].join(", "), items[n - 1]),
    }
}

fn describe_images(shots: &[&Shot]) -> String {
    let screens = shots.iter().filter(|s| !s.is_image_file()).count();
    let mut items = Vec::new();
    if screens > 0 {
        let what = if screens == 1 { "a screenshot".to_string() } else { format!("{screens} screenshots") };
        items.push(format!("{what} of the user's screen (taken when this message was sent)"));
    }
    for s in shots.iter().filter(|s| s.is_image_file()) {
        items.push(format!("the image file {}", quoted(&s.name)));
    }
    list_text(&items)
}

/// The line that tells the model which images a question carried (`sent`: the indexes still included).
pub fn images_note(shots: &[Shot], sent: &[usize]) -> String {
    let n = shots.len();
    if !shots.iter().any(Shot::is_image_file) {
        let k = sent.len();
        return if k > 0 {
            format!(
                "[{} of the user's screen, taken when this message was sent, {} attached.]",
                if k == 1 { "A screenshot".to_string() } else { format!("{k} screenshots") },
                if k == 1 { "is" } else { "are" }
            )
        } else {
            format!(
                "[{} attached to this message; {} not included any more.]",
                if n == 1 { "A screenshot was".to_string() } else { format!("{n} screenshots were") },
                if n == 1 { "it is" } else { "they are" }
            )
        };
    }
    let included: Vec<&Shot> = sent.iter().filter_map(|i| shots.get(*i)).collect();
    let gone: Vec<&Shot> = shots.iter().enumerate().filter(|(i, _)| !sent.contains(i)).map(|(_, s)| s).collect();
    let mut parts = Vec::new();
    if !included.is_empty() {
        parts.push(format!("Attached to this message: {}.", describe_images(&included)));
    }
    if !gone.is_empty() {
        let what = describe_images(&gone);
        let one = gone.len() == 1;
        parts.push(format!(
            "{} {} attached to this message but {} not included any more.",
            upper_first(&what),
            if one { "was" } else { "were" },
            if one { "is" } else { "are" }
        ));
    }
    format!("[{}]", parts.join(" "))
}

/// Does the part of the transcript that will be sent carry attached files? (The system prompt then explains them.)
pub fn has_files(messages: &[Message], history_messages: i64) -> bool {
    messages[window_start(messages.len(), history_messages)..]
        .iter()
        .any(|m| m.role == Role::User && !m.files.is_empty())
}

/// A provider-neutral message of a request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestMessage {
    pub role: Role,
    pub content: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Image>,
}

/// Gives the image of a shot while it is still in memory.
pub type ImageFor<'a> = &'a dyn Fn(&Shot) -> Option<Image>;

/// The inputs of [`build_request_messages`].
pub struct RequestInput<'a> {
    /// The transcript INCLUDING the new question as the last entry.
    pub messages: &'a [Message],
    pub history_messages: i64,
    /// The current view state, attached to the new question only.
    pub view_text: &'a str,
    /// Set when the screen matches an earlier snapshot, to say so explicitly.
    pub unchanged_hash: Option<&'a str>,
    /// The image of a screenshot (or attached image file) while it is still in memory; None when the model cannot
    /// see images.
    pub image_for: Option<ImageFor<'a>>,
    /// Only the newest this many questions with images carry them.
    pub image_messages: usize,
}

impl<'a> RequestInput<'a> {
    pub fn new(messages: &'a [Message]) -> Self {
        RequestInput {
            messages,
            history_messages: DEFAULT_HISTORY_MESSAGES,
            view_text: "",
            unchanged_hash: None,
            image_for: None,
            image_messages: DEFAULT_IMAGE_MESSAGES,
        }
    }
}

/// Build the provider-neutral messages for a request: the window of the transcript, the newest snapshot in full and
/// older ones as stubs, the view state and the "unchanged" line on the new question, attached files (a file attached
/// again is sent with its newest question only), and images for the newest questions that carry them.
pub fn build_request_messages(input: &RequestInput) -> Vec<RequestMessage> {
    let start = window_start(input.messages.len(), input.history_messages);
    let windowed = &input.messages[start..];
    let latest = latest_snapshot(windowed, 0).map(|(i, _)| i);
    let last = windowed.len().checked_sub(1);

    // Which questions still send their images: the newest ones whose images are available.
    let mut images_at: HashMap<usize, Vec<(usize, Image)>> = HashMap::new();
    if let Some(image_for) = input.image_for {
        for (i, m) in windowed.iter().enumerate().rev() {
            if images_at.len() >= input.image_messages {
                break;
            }
            if m.role != Role::User || m.shots.is_empty() {
                continue;
            }
            let sent: Vec<(usize, Image)> =
                m.shots.iter().enumerate().filter_map(|(k, s)| image_for(s).map(|img| (k, img))).collect();
            if !sent.is_empty() {
                images_at.insert(i, sent);
            }
        }
    }
    // A file attached more than once (same content) is sent with its newest question only.
    let mut last_file_at: HashMap<&str, usize> = HashMap::new();
    for (i, m) in windowed.iter().enumerate() {
        if m.role == Role::User {
            for f in &m.files {
                if !f.hash.is_empty() {
                    last_file_at.insert(&f.hash, i);
                }
            }
        }
    }

    windowed
        .iter()
        .enumerate()
        .map(|(i, m)| {
            if m.role != Role::User {
                return RequestMessage {
                    role: Role::Assistant,
                    content: format!("{}{}", actions_line(&m.actions), m.content),
                    images: vec![],
                };
            }
            let mut parts = Vec::new();
            if let Some(s) = &m.snapshot {
                parts.push(if latest == Some(i) { snapshot_block(s) } else { superseded_stub(s) });
            }
            if Some(i) == last {
                if let (None, Some(h)) = (&m.snapshot, input.unchanged_hash) {
                    parts.push(format!(
                        "[The screen is unchanged since page snapshot {}: it is still current.]",
                        short_hash(h)
                    ));
                }
                if !input.view_text.is_empty() {
                    parts.push(format!("<view_state>\n{}\n</view_state>", input.view_text));
                }
            }
            for f in &m.files {
                if f.text.is_none() {
                    parts.push(file_stub(f, StubReason::Gone));
                } else if !f.hash.is_empty() && last_file_at.get(f.hash.as_str()) != Some(&i) {
                    parts.push(file_stub(f, StubReason::Repeated));
                } else {
                    parts.push(file_block(f));
                }
            }
            let sent = images_at.remove(&i).unwrap_or_default();
            if !m.shots.is_empty() {
                parts.push(images_note(&m.shots, &sent.iter().map(|(k, _)| *k).collect::<Vec<_>>()));
            }
            parts.push(m.content.clone());
            RequestMessage {
                role: Role::User,
                content: parts.join("\n\n"),
                images: sent.into_iter().map(|(_, img)| img).collect(),
            }
        })
        .collect()
}

/// The plan's reason as the transcript records it on a question that carried no snapshot.
pub fn sync_word(plan: &Plan) -> &'static str {
    if plan.attach {
        "attached"
    } else {
        plan.reason.as_str()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(hash: &str) -> Option<SnapshotRecord> {
        Some(SnapshotRecord {
            hash: hash.into(),
            text: Some("page".into()),
            page_id: "p".into(),
            page_title: "P".into(),
            ..Default::default()
        })
    }

    #[test]
    fn request_messages_stub_older_snapshots_and_add_view() {
        let msgs = vec![
            Message { snapshot: snap("aaaaaaaaaaaaaa"), ..Message::user("one") },
            Message {
                actions: vec![Action { call: "f()".into(), status: "ok".into(), ..Default::default() }],
                ..Message::assistant("done")
            },
            Message { snapshot: snap("bbbbbbbbbbbbbb"), ..Message::user("two") },
        ];
        let input = RequestInput { view_text: "cursor 3", ..RequestInput::new(&msgs) };
        let out = build_request_messages(&input);
        assert_eq!(out.len(), 3);
        assert!(out[0].content.starts_with("[Page snapshot aaaaaaa of \"P\" omitted here"));
        assert_eq!(out[1].content, "[Actions taken: f() → done]\n\ndone");
        assert!(out[2].content.starts_with("<page_snapshot page=\"p\" title=\"P\" hash=\"bbbbbbb\""));
        assert!(out[2].content.ends_with("<view_state>\ncursor 3\n</view_state>\n\ntwo"));
    }

    #[test]
    fn unchanged_line_and_images() {
        let msgs = vec![Message {
            shots: vec![
                Shot { id: "s1".into(), ..Default::default() },
                Shot { id: "s2".into(), name: "x.png".into(), kind: "image".into(), ..Default::default() },
            ],
            ..Message::user("look")
        }];
        let image_for = |s: &Shot| (s.id == "s1").then(|| Image { mime: "image/jpeg".into(), data: "AA".into() });
        let input = RequestInput {
            unchanged_hash: Some("0123456789abcd"),
            image_for: Some(&image_for),
            ..RequestInput::new(&msgs)
        };
        let out = build_request_messages(&input);
        assert_eq!(out[0].images.len(), 1);
        assert_eq!(
            out[0].content,
            "[The screen is unchanged since page snapshot 0123456: it is still current.]\n\n[Attached to this message: a screenshot of the user's screen (taken when this message was sent). The image file \"x.png\" was attached to this message but is not included any more.]\n\nlook"
        );
    }

    #[test]
    fn repeated_file_sent_once() {
        let f = FileRecord {
            name: "a.txt".into(),
            label: "Text".into(),
            hash: "h1".into(),
            text: Some("x".into()),
            chars: 1.0,
            ..Default::default()
        };
        let msgs = vec![
            Message { files: vec![f.clone()], ..Message::user("1") },
            Message::assistant("ok"),
            Message { files: vec![f], ..Message::user("2") },
        ];
        let out = build_request_messages(&RequestInput::new(&msgs));
        assert!(out[0].content.starts_with("[File \"a.txt\" (Text) omitted here"));
        assert!(out[2]
            .content
            .starts_with("<attached_file name=\"a.txt\" type=\"Text\" chars=\"1\" hash=\"h1\">\nx\n</attached_file>"));
        assert!(has_files(&msgs, 20));
    }
}
