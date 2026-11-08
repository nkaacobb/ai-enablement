//! Markdown files with YAML frontmatter (`core/frontmatter.js`): SKILL.md (Agent Skills) and agent files.
//!
//! It reads the subset of YAML these files use: mappings, block and flow sequences, nested mappings, plain /
//! single- / double-quoted scalars, `|` and `>` block scalars, plain scalars continued on more indented lines, and
//! comments. What it cannot read is reported, never guessed. Anchors, tags and multi-document streams are not
//! supported. The parse is a line-by-line port of the JS, so both runtimes read a file the same way.

use crate::json::{string_to_number, Map, Value};
use crate::text::{is_js_space, js_head, js_trim};

/// A file split into its frontmatter data, its body, and what could not be read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Frontmatter {
    pub data: Map,
    pub body: String,
    pub errors: Vec<String>,
}

impl Frontmatter {
    pub fn to_value(&self) -> Value {
        let mut m = Map::new();
        m.insert("data", self.data.clone());
        m.insert("body", self.body.as_str());
        m.insert("errors", self.errors.clone());
        Value::Object(m)
    }
}

const PLAIN_START: [char; 16] = ['#', '\'', '"', '{', '[', ']', '}', ',', '&', '*', '!', '|', '>', '%', '@', '`'];

fn is_blank(c: Option<&char>) -> bool {
    matches!(c, Some(' ' | '\t'))
}

fn s(chars: &[char]) -> String {
    chars.iter().collect()
}

fn trim(chars: &[char]) -> &[char] {
    let start = chars.iter().position(|c| !is_js_space(*c)).unwrap_or(chars.len());
    let end = chars.iter().rposition(|c| !is_js_space(*c)).map_or(start, |e| e + 1);
    &chars[start..end.max(start)]
}

fn trim_end(chars: &[char]) -> &[char] {
    let end = chars.iter().rposition(|c| !is_js_space(*c)).map_or(0, |e| e + 1);
    &chars[..end]
}

fn starts_with(chars: &[char], prefix: &str) -> bool {
    let p: Vec<char> = prefix.chars().collect();
    chars.len() >= p.len() && chars[..p.len()] == p[..]
}

fn closing_double(t: &[char]) -> Option<usize> {
    let mut k = 1;
    while k < t.len() {
        if t[k] == '\\' {
            k += 2;
            continue;
        }
        if t[k] == '"' {
            return Some(k);
        }
        k += 1;
    }
    None
}

fn closing_single(t: &[char]) -> Option<usize> {
    let mut k = 1;
    while k < t.len() {
        if t[k] != '\'' {
            k += 1;
            continue;
        }
        if t.get(k + 1) == Some(&'\'') {
            k += 2;
            continue;
        }
        return Some(k);
    }
    None
}

fn closed_quote(text: &[char]) -> bool {
    let t = trim(text);
    match t.first() {
        Some('"') => closing_double(t).is_some_and(|k| k > 0),
        Some('\'') => closing_single(t).is_some_and(|k| k > 0),
        _ => true,
    }
}

fn balanced(t: &[char]) -> bool {
    let mut depth: i64 = 0;
    let mut quote: Option<char> = None;
    let mut k = 0;
    while k < t.len() {
        let ch = t[k];
        if let Some(q) = quote {
            if q == '"' && ch == '\\' {
                k += 2;
                continue;
            }
            if ch == q {
                quote = None;
            }
            k += 1;
            continue;
        }
        match ch {
            '"' | '\'' => quote = Some(ch),
            '[' | '{' => depth += 1,
            ']' | '}' => depth -= 1,
            _ => {}
        }
        k += 1;
    }
    depth <= 0 && quote.is_none()
}

fn split_flow(t: &[char]) -> Vec<Vec<char>> {
    let mut out = Vec::new();
    let mut depth: i64 = 0;
    let mut quote: Option<char> = None;
    let mut cur: Vec<char> = Vec::new();
    let mut k = 0;
    while k < t.len() {
        let ch = t[k];
        if let Some(q) = quote {
            cur.push(ch);
            if q == '"' && ch == '\\' {
                if let Some(next) = t.get(k + 1) {
                    cur.push(*next);
                }
                k += 2;
                continue;
            }
            if ch == q {
                quote = None;
            }
            k += 1;
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            cur.push(ch);
            k += 1;
            continue;
        }
        if ch == '[' || ch == '{' {
            depth += 1;
        }
        if ch == ']' || ch == '}' {
            depth -= 1;
        }
        if ch == ',' && depth == 0 {
            out.push(std::mem::take(&mut cur));
            k += 1;
            continue;
        }
        cur.push(ch);
        k += 1;
    }
    out.push(cur);
    out
}

/// A trailing ` # comment` outside quotes (YAML: a # after whitespace starts a comment).
fn strip_comment(t: &[char]) -> Vec<char> {
    let mut quote: Option<char> = None;
    let mut k = 0;
    while k < t.len() {
        let ch = t[k];
        if let Some(q) = quote {
            if q == '"' && ch == '\\' {
                k += 2;
                continue;
            }
            if ch == q {
                quote = None;
            }
            k += 1;
            continue;
        }
        if ch == '"' || ch == '\'' {
            if k == 0 || is_js_space(t[k - 1]) || matches!(t[k - 1], '[' | '{' | ',' | ':') {
                quote = Some(ch);
            }
            k += 1;
            continue;
        }
        if ch == '#' && (k == 0 || t[k - 1] == ' ' || t[k - 1] == '\t') {
            return trim_end(&t[..k]).to_vec();
        }
        k += 1;
    }
    t.to_vec()
}

fn unescape_double(t: &[char]) -> String {
    let mut out = String::new();
    let hex = |from: usize, n: usize| -> Option<u32> {
        let part = t.get(from..from + n)?;
        if !part.iter().all(char::is_ascii_hexdigit) {
            return None;
        }
        u32::from_str_radix(&s(part), 16).ok()
    };
    let mut k = 0;
    while k < t.len() {
        let ch = t[k];
        if ch != '\\' {
            out.push(ch);
            k += 1;
            continue;
        }
        let n = t.get(k + 1).copied();
        if n == Some('u') {
            if let Some(code) = hex(k + 2, 4) {
                // A surrogate pair written as two escapes is one character, as in JS.
                if (0xD800..0xDC00).contains(&code) && t.get(k + 6) == Some(&'\\') && t.get(k + 7) == Some(&'u') {
                    if let Some(low) = hex(k + 8, 4).filter(|l| (0xDC00..0xE000).contains(l)) {
                        let c = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                        out.push(char::from_u32(c).unwrap_or('\u{FFFD}'));
                        k += 12;
                        continue;
                    }
                }
                out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                k += 6;
                continue;
            }
        }
        if n == Some('x') {
            if let Some(code) = hex(k + 2, 2) {
                out.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                k += 4;
                continue;
            }
        }
        match n {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('0') => out.push('\0'),
            Some('b') => out.push('\u{08}'),
            Some('f') => out.push('\u{0C}'),
            Some('e') => out.push('\u{1B}'),
            Some(c) => out.push(c),
            None => {}
        }
        k += 2;
    }
    out
}

/// `key: value` → (key as written, still quoted if it was; the rest trimmed), or None. The key ends at the first
/// `:` followed by a space or the end of the line (inside flow `{…}`: at the first `:`), outside quotes.
fn split_key(t: &[char], flow: bool) -> Option<(Vec<char>, Vec<char>)> {
    let ends = |k: usize| flow || k + 1 >= t.len() || is_blank(t.get(k + 1));
    let first = *t.first()?;
    if first == '"' || first == '\'' {
        let end = if first == '"' { closing_double(t) } else { closing_single(t) }?;
        let mut k = end + 1;
        while is_blank(t.get(k)) {
            k += 1;
        }
        if t.get(k) != Some(&':') || !ends(k) {
            return None;
        }
        return Some((t[..=end].to_vec(), trim(&t[k + 1..]).to_vec()));
    }
    if PLAIN_START.contains(&first) || (first == '-' && (t.len() == 1 || is_blank(t.get(1)))) {
        return None;
    }
    for k in 0..t.len() {
        if t[k] == '#' && k > 0 && is_blank(t.get(k - 1)) {
            return None;
        }
        if t[k] == ':' && ends(k) {
            let key = trim_end(&t[..k]);
            return if key.is_empty() { None } else { Some((key.to_vec(), trim(&t[k + 1..]).to_vec())) };
        }
    }
    None
}

#[derive(Clone)]
struct Line {
    raw: Vec<char>,
    n: usize,
    indent: usize,
    text: Vec<char>,
}

impl Line {
    fn is_content(&self) -> bool {
        !self.text.is_empty() && self.text[0] != '#'
    }

    fn is_item(&self) -> bool {
        self.text == ['-'] || starts_with(&self.text, "- ")
    }
}

struct Parser {
    lines: Vec<Line>,
    i: usize,
    errors: Vec<String>,
}

impl Parser {
    fn skip(&mut self) {
        while self.i < self.lines.len() && !self.lines[self.i].is_content() {
            self.i += 1;
        }
    }

    fn block_scalar(&mut self, header: &[char], parent_indent: usize) -> Value {
        let literal = header[0] == '|';
        let chomp = if header.contains(&'-') {
            "strip"
        } else if header.contains(&'+') {
            "keep"
        } else {
            "clip"
        };
        let mut body: Vec<Vec<char>> = Vec::new();
        let mut indent: Option<usize> = None;
        while self.i < self.lines.len() {
            let l = &self.lines[self.i];
            if l.text.is_empty() {
                body.push(vec![]);
                self.i += 1;
                continue;
            }
            if l.indent <= parent_indent {
                break;
            }
            let ind = *indent.get_or_insert(l.indent);
            if l.indent < ind {
                break;
            }
            body.push(l.raw[ind.min(l.raw.len())..].to_vec());
            self.i += 1;
        }
        let mut trailing = 0;
        while body.last().is_some_and(Vec::is_empty) {
            body.pop();
            trailing += 1;
        }
        if body.is_empty() {
            return Value::from("");
        }
        let out = if literal {
            body.iter().map(|l| s(l)).collect::<Vec<_>>().join("\n")
        } else {
            // Folding: lines join with a space, an empty line is a line break, more-indented lines keep their breaks.
            let starts_space = |l: &[char]| l.first().is_some_and(|c| is_js_space(*c));
            let mut out = String::new();
            for k in 0..body.len() {
                let line = &body[k];
                if k == 0 {
                    out = s(line);
                } else if line.is_empty() {
                    out.push('\n');
                } else if body[k - 1].is_empty() {
                    out.push_str(&s(line));
                } else if starts_space(line) || starts_space(&body[k - 1]) {
                    out.push('\n');
                    out.push_str(&s(line));
                } else {
                    out.push(' ');
                    out.push_str(&s(line));
                }
            }
            out
        };
        Value::String(match chomp {
            "strip" => out,
            "keep" => out + &"\n".repeat(trailing + 1),
            _ => out + "\n",
        })
    }

    /// `^[|>][+-]?\d*\s*(#.*)?$`
    fn is_block_header(r: &[char]) -> bool {
        let mut k = 0;
        if !matches!(r.first(), Some('|' | '>')) {
            return false;
        }
        k += 1;
        if matches!(r.get(k), Some('+' | '-')) {
            k += 1;
        }
        while r.get(k).is_some_and(char::is_ascii_digit) {
            k += 1;
        }
        while r.get(k).is_some_and(|c| is_js_space(*c)) {
            k += 1;
        }
        k == r.len() || r[k] == '#'
    }

    /// The value after `key:` (or `- `) on line `at`, plus any continuation lines.
    fn inline_value(&mut self, rest: &[char], at: usize, parent_indent: usize) -> Value {
        let r = trim(rest).to_vec();
        if Self::is_block_header(&r) {
            return self.block_scalar(&r, parent_indent);
        }
        match r.first() {
            Some('"' | '\'') => {
                let mut v = r.clone();
                // A quoted scalar may continue on the next lines until its closing quote.
                while !closed_quote(&v) && self.i < self.lines.len() && self.lines[self.i].indent > parent_indent {
                    v.push(' ');
                    v.extend_from_slice(&self.lines[self.i].text.clone());
                    self.i += 1;
                }
                self.scalar(&v, at)
            }
            Some('[' | '{') => {
                let mut v = r.clone();
                while !balanced(&v) && self.i < self.lines.len() && self.lines[self.i].indent > parent_indent {
                    v.push(' ');
                    v.extend_from_slice(&self.lines[self.i].text.clone());
                    self.i += 1;
                }
                self.flow(&v, at)
            }
            _ => {
                let mut v = strip_comment(&r);
                // A plain scalar continues on more-indented lines (folded with spaces).
                while self.i < self.lines.len()
                    && self.lines[self.i].is_content()
                    && self.lines[self.i].indent > parent_indent
                {
                    v.push(' ');
                    v.extend(strip_comment(&self.lines[self.i].text));
                    self.i += 1;
                }
                self.scalar(&v, at)
            }
        }
    }

    fn parse_node(&mut self, indent: usize) -> Value {
        self.skip();
        if self.i >= self.lines.len() {
            return Value::Null;
        }
        let l = &self.lines[self.i];
        if l.indent < indent {
            return Value::Null;
        }
        let ind = l.indent;
        if l.is_item() {
            self.parse_sequence(ind)
        } else {
            self.parse_mapping(ind)
        }
    }

    fn parse_sequence(&mut self, indent: usize) -> Value {
        let mut out = Vec::new();
        loop {
            self.skip();
            if self.i >= self.lines.len() {
                break;
            }
            let l = self.lines[self.i].clone();
            if l.indent != indent || !l.is_item() {
                break;
            }
            self.i += 1;
            let rest: Vec<char> = if l.text == ['-'] { vec![] } else { l.text[2..].to_vec() };
            let lead = rest.iter().take_while(|c| is_js_space(**c)).count();
            let content_indent = indent + 2 + lead;
            let rest_trim = trim(&rest).to_vec();
            if rest_trim.is_empty() {
                out.push(self.parse_node(indent + 1));
                continue;
            }
            if split_key(&rest_trim, false).is_some() {
                // "- key: value" starts a mapping whose other keys are indented to the key's column.
                let mut raw: Vec<char> = vec![' '; content_indent];
                raw.extend_from_slice(&rest_trim);
                self.lines.insert(self.i, Line { raw, n: l.n, indent: content_indent, text: rest_trim });
                out.push(self.parse_mapping(content_indent));
                continue;
            }
            out.push(self.inline_value(&rest, l.n, indent));
        }
        Value::Array(out)
    }

    fn parse_mapping(&mut self, indent: usize) -> Value {
        let mut out = Map::new();
        loop {
            self.skip();
            if self.i >= self.lines.len() {
                break;
            }
            let l = self.lines[self.i].clone();
            if l.indent < indent {
                break;
            }
            if l.indent > indent {
                self.errors.push(format!("Line {}: unexpected indentation.", l.n));
                self.i += 1;
                continue;
            }
            if l.is_item() {
                break;
            }
            let Some((key_raw, rest)) = split_key(&l.text, false) else {
                self.errors.push(format!(
                    "Line {}: expected \"key: value\", found \"{}\".",
                    l.n,
                    js_head(&s(&l.text), 60)
                ));
                self.i += 1;
                continue;
            };
            self.i += 1;
            let key = self.scalar(&key_raw, l.n).js_string();
            if out.contains_key(&key) {
                self.errors.push(format!("Line {}: \"{key}\" is set twice; the last value wins.", l.n));
            }
            let rest_t = trim(&rest);
            let value = if rest_t.is_empty() || rest_t[0] == '#' {
                self.skip();
                match self.lines.get(self.i) {
                    Some(next) if next.indent == indent && next.is_item() => self.parse_sequence(indent),
                    Some(next) if next.indent > indent => self.parse_node(indent + 1),
                    _ => Value::Null,
                }
            } else {
                self.inline_value(&rest, l.n, indent)
            };
            out.insert(key, value);
        }
        Value::Object(out)
    }

    fn scalar(&mut self, text: &[char], at: usize) -> Value {
        let t = trim(text);
        let Some(&first) = t.first() else { return Value::Null };
        if first == '"' {
            let Some(end) = closing_double(t) else {
                self.errors.push(format!("Line {at}: unclosed double quote."));
                return Value::String(s(&t[1..]));
            };
            let after = trim(&t[end + 1..]);
            if !after.is_empty() && after[0] != '#' {
                self.errors.push(format!("Line {at}: text after a closing quote."));
            }
            return Value::String(unescape_double(&t[1..end]));
        }
        if first == '\'' {
            let mut out = String::new();
            let mut k = 1;
            while k < t.len() {
                if t[k] == '\'' {
                    if t.get(k + 1) == Some(&'\'') {
                        out.push('\'');
                        k += 2;
                        continue;
                    }
                    break;
                }
                out.push(t[k]);
                k += 1;
            }
            if k >= t.len() {
                self.errors.push(format!("Line {at}: unclosed single quote."));
            }
            return Value::String(out);
        }
        if first == '[' || first == '{' {
            return self.flow(t, at);
        }
        let text = s(t);
        match text.as_str() {
            "true" | "True" | "TRUE" => return Value::Bool(true),
            "false" | "False" | "FALSE" => return Value::Bool(false),
            "null" | "Null" | "NULL" | "~" => return Value::Null,
            _ => {}
        }
        if is_yaml_number(&text) {
            return Value::Number(string_to_number(&text));
        }
        if matches!(first, '&' | '*' | '!' | '%' | '@' | '`') {
            self.errors
                .push(format!("Line {at}: \"{first}\" (anchors, aliases, tags) is not supported; quote the value."));
        }
        Value::String(text)
    }

    /// `[a, "b, c", {k: v}]` or `{a: 1, b: [x]}`.
    fn flow(&mut self, text: &[char], at: usize) -> Value {
        let t = strip_comment(trim(text));
        let open = t.first().copied().unwrap_or('[');
        let close = if open == '[' { ']' } else { '}' };
        if t.last() != Some(&close) || t.len() < 2 {
            self.errors.push(format!("Line {at}: unclosed {open}."));
            return if open == '[' { Value::Array(vec![]) } else { Value::object() };
        }
        let items = split_flow(&t[1..t.len() - 1]);
        let nested = |x: &[char]| matches!(trim(x).first(), Some('[' | '{'));
        if open == '[' {
            let mut out = Vec::new();
            for x in items.iter().filter(|x| !trim(x).is_empty()) {
                out.push(if nested(x) { self.flow(x, at) } else { self.scalar(x, at) });
            }
            return Value::Array(out);
        }
        let mut out = Map::new();
        for item in &items {
            let it = trim(item);
            if it.is_empty() {
                continue;
            }
            let Some((k, v)) = split_key(it, true) else {
                self.errors.push(format!("Line {at}: expected \"key: value\" inside {{…}}."));
                continue;
            };
            let key = self.scalar(&k, at).js_string();
            let value = if matches!(v.first(), Some('[' | '{')) { self.flow(&v, at) } else { self.scalar(&v, at) };
            out.insert(key, value);
        }
        Value::Object(out)
    }
}

/// `^[-+]?\d+$` or `^[-+]?(\d+\.\d*|\.\d+|\d+)([eE][-+]?\d+)?$`
fn is_yaml_number(t: &str) -> bool {
    let b = t.as_bytes();
    let mut k = usize::from(matches!(b.first(), Some(b'-' | b'+')));
    let int_start = k;
    while b.get(k).is_some_and(u8::is_ascii_digit) {
        k += 1;
    }
    let int_digits = k - int_start;
    if b.get(k) == Some(&b'.') {
        k += 1;
        let frac_start = k;
        while b.get(k).is_some_and(u8::is_ascii_digit) {
            k += 1;
        }
        if int_digits == 0 && k == frac_start {
            return false;
        }
    } else if int_digits == 0 {
        return false;
    }
    if matches!(b.get(k), Some(b'e' | b'E')) {
        k += 1;
        if matches!(b.get(k), Some(b'-' | b'+')) {
            k += 1;
        }
        let exp_start = k;
        while b.get(k).is_some_and(u8::is_ascii_digit) {
            k += 1;
        }
        if k == exp_start {
            return false;
        }
    }
    k == b.len()
}

/// Parse a small YAML document: the value (null for an empty one), and what could not be read.
pub fn parse_yaml(text: &str) -> (Value, Vec<String>) {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines = normalized
        .split('\n')
        .enumerate()
        .map(|(n, raw)| {
            let raw: Vec<char> = raw.chars().collect();
            let indent = raw.iter().take_while(|c| is_js_space(**c)).count();
            let text = trim(&raw).to_vec();
            Line { raw, n: n + 1, indent, text }
        })
        .collect();
    let mut p = Parser { lines, i: 0, errors: Vec::new() };
    let value = p.parse_node(0);
    p.skip();
    if p.i < p.lines.len() {
        let l = &p.lines[p.i];
        let msg = format!("Line {}: could not be read (\"{}\").", l.n, js_head(&s(&l.text), 60));
        p.errors.push(msg);
    }
    (value, p.errors)
}

fn is_fence(line: &str) -> bool {
    let rest = line.strip_prefix("---").or_else(|| line.strip_prefix("..."));
    rest.is_some_and(|r| r.chars().all(|c| c == ' ' || c == '\t'))
}

/// Split a file into its frontmatter data, body and errors. A file without frontmatter is all body.
pub fn parse_frontmatter(text: &str) -> Frontmatter {
    let src = text.strip_prefix('\u{FEFF}').unwrap_or(text).replace("\r\n", "\n").replace('\r', "\n");
    let opens = src.strip_prefix("---").is_some_and(|r| {
        let r = r.trim_start_matches([' ', '\t']);
        r.starts_with('\n')
    });
    if !opens {
        return Frontmatter { data: Map::new(), body: src, errors: vec![] };
    }
    let lines: Vec<&str> = src.split('\n').collect();
    let Some(end) = (1..lines.len()).find(|i| is_fence(lines[*i])) else {
        return Frontmatter {
            data: Map::new(),
            body: src.clone(),
            errors: vec!["The frontmatter has no closing --- line.".into()],
        };
    };
    let (value, mut errors) = parse_yaml(&lines[1..end].join("\n"));
    let data = match value {
        Value::Object(m) => m,
        Value::Null => Map::new(),
        _ => {
            errors.push("The frontmatter is not a mapping of keys to values.".into());
            Map::new()
        }
    };
    let body = lines[end + 1..].join("\n");
    Frontmatter { data, body: body.trim_start_matches('\n').to_string(), errors }
}

/// A list from a frontmatter value: a YAML list, or a comma- or space-separated string (Agent Skills'
/// `allowed-tools`).
pub fn list_value(v: &Value) -> Vec<String> {
    match v {
        Value::Null => vec![],
        Value::String(s) if s.is_empty() => vec![],
        Value::Array(items) => items
            .iter()
            .map(|x| if x.is_null() { String::new() } else { js_trim(&x.js_string()).to_string() })
            .filter(|x| !x.is_empty())
            .collect(),
        other => other
            .js_string()
            .split(|c: char| c == ',' || is_js_space(c))
            .map(|x| js_trim(x).to_string())
            .filter(|x| !x.is_empty())
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(text: &str) -> String {
        Value::Object(parse_frontmatter(text).data).to_json()
    }

    #[test]
    fn sequences_of_mappings_and_block_scalars() {
        assert_eq!(
            data("---\nitems:\n  - name: a\n    n: 1\n  - b\nlit: |\n  one\n  two\n\nkeep: >+\n  x\n\n---\n"),
            r#"{"items":[{"name":"a","n":1},"b"],"lit":"one\ntwo\n","keep":"x\n\n"}"#
        );
    }

    #[test]
    fn quoted_scalars_and_escapes() {
        assert_eq!(
            data("---\na: \"tab\\tA\\u00e9\\uD83D\\uDE42\"\nb: 'multi\n  line'\nc: plain\n  continued # c\n---\n"),
            r#"{"a":"tab\tAé🙂","b":"multi line","c":"plain continued"}"#
        );
    }

    #[test]
    fn flow_collections() {
        assert_eq!(data("---\nf: {a: 1, b: [x, \"y, z\"]}\ne: []\n---\n"), r#"{"f":{"a":1,"b":["x","y, z"]},"e":[]}"#);
    }

    #[test]
    fn reports_what_it_cannot_read() {
        let f = parse_frontmatter("---\na: 1\na: 2\nb: &anchor x\n  - stray\n---\n");
        assert_eq!(Value::Object(f.data).to_json(), r#"{"a":2,"b":"&anchor x - stray"}"#);
        assert_eq!(
            f.errors,
            vec![
                "Line 2: \"a\" is set twice; the last value wins.",
                "Line 3: \"&\" (anchors, aliases, tags) is not supported; quote the value."
            ]
        );
        assert_eq!(parse_frontmatter("---\na: 1\n").errors, vec!["The frontmatter has no closing --- line."]);
        assert_eq!(
            parse_frontmatter("---\n- a\n---\nB").errors,
            vec!["The frontmatter is not a mapping of keys to values."]
        );
    }
}
