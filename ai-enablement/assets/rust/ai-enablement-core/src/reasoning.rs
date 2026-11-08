//! Separating a model's thinking from its answer (`core/reasoning.js`). Providers that report reasoning in its own
//! field are handled by the adapters; this covers models that write it inline as `<think>…</think>`. It runs over the
//! whole accumulated text, so it is correct mid-stream: an unclosed block is "still thinking", and a stray closing tag
//! (templates that put the opening tag in the prompt) means everything before it was reasoning.

use regex::Regex;
use std::sync::OnceLock;

/// What [`split_reasoning`] returns.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Split {
    /// The reasoning parts, trimmed, joined by a blank line.
    pub reasoning: String,
    /// The answer, with leading white space removed.
    pub text: String,
    /// An opening tag is still unclosed: the model is thinking.
    pub thinking: bool,
}

fn open_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)<(think|thinking)\b[^>]*>").expect("open tag"))
}

fn close_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)</(think|thinking)\s*>").expect("close tag"))
}

/// The end of the first `>` at or after `from` (JS `indexOf('>', from) + 1`; 0 when there is none).
fn after_gt(s: &str, from: usize) -> usize {
    s[from..].find('>').map(|i| from + i + 1).unwrap_or(0)
}

/// `<think>…</think>` blocks, with the closing tag the same as the opening one (case-insensitive), removed in one
/// left-to-right pass as JS `replace(/<(think|thinking)\b[^>]*>([\s\S]*?)<\/\1\s*>/gi)` does.
fn take_blocks(s: &str, parts: &mut Vec<String>) -> String {
    let mut out = String::new();
    let mut pos = 0;
    while let Some(m) = open_re().find_at(s, pos) {
        let caps = open_re().captures_at(s, m.start()).expect("same match");
        let tag = regex::escape(&caps[1]);
        let close = Regex::new(&format!(r"(?i)</{tag}\s*>")).expect("close tag");
        match close.find_at(s, m.end()) {
            Some(c) => {
                parts.push(s[m.end()..c.start()].to_string());
                out.push_str(&s[pos..m.start()]);
                pos = c.end();
            }
            None => {
                // No closing tag for this one: the scan goes on from the next character.
                out.push_str(&s[pos..m.start() + 1]);
                pos = m.start() + 1;
            }
        }
    }
    out.push_str(&s[pos..]);
    out
}

/// Split inline reasoning from the answer.
pub fn split_reasoning(raw: &str) -> Split {
    let mut s = raw.to_string();
    let mut parts: Vec<String> = Vec::new();

    let first_open = open_re().find(&s).map(|m| m.start());
    let first_close = close_re().find(&s).map(|m| m.start());
    if let Some(fc) = first_close {
        if first_open.map_or(true, |fo| fc < fo) {
            parts.push(s[..fc].to_string());
            s = s[after_gt(&s, fc)..].to_string();
        }
    }

    s = take_blocks(&s, &mut parts);

    let mut thinking = false;
    if let Some(m) = open_re().find(&s) {
        let open = m.start();
        parts.push(s[after_gt(&s, open)..].to_string());
        s.truncate(open);
        thinking = true;
    }

    let reasoning =
        parts.iter().map(|p| crate::text::js_trim(p)).filter(|p| !p.is_empty()).collect::<Vec<_>>().join("\n\n");
    Split { reasoning, text: crate::text::js_trim_start(&s).to_string(), thinking }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_blocks() {
        let r = split_reasoning("<think>plan</think>\n\nAnswer.");
        assert_eq!((r.reasoning.as_str(), r.text.as_str(), r.thinking), ("plan", "Answer.", false));
        let r = split_reasoning("<THINK a=1>x</think>A<thinking>y</thinking>B");
        assert_eq!((r.reasoning.as_str(), r.text.as_str()), ("x\n\ny", "AB"));
    }

    #[test]
    fn unclosed_and_stray() {
        let r = split_reasoning("Hi <think>still going");
        assert_eq!((r.reasoning.as_str(), r.text.as_str(), r.thinking), ("still going", "Hi ", true));
        let r = split_reasoning("thought first</think> The answer.");
        assert_eq!((r.reasoning.as_str(), r.text.as_str(), r.thinking), ("thought first", "The answer.", false));
        // A <think> without its own closing tag does not swallow a later <thinking> block.
        let r = split_reasoning("<think>a<thinking>b</thinking>c");
        assert_eq!((r.reasoning.as_str(), r.text.as_str(), r.thinking), ("b\n\nac", "", true));
    }
}
