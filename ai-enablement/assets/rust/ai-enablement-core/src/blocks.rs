//! Reading values a model wrote loosely (`core/blocks.js`): a flat object from JSON or `key: value` lines, and one
//! value coerced and clamped to a field spec. Tool arguments go through the same code.
//!
//! Small local models are loose with formats: they add comments or trailing commas, quote numbers, or write
//! `key: value` lines. Parsing is linear (no regular expressions that could backtrack on model output).

use crate::json::{js_round, parse_float, string_to_number, to_fixed, to_json, Map, Value};
use crate::text::{js_head, js_trim, utf16_len};

const MAX_BODY: usize = 20_000;
const TRUE: [&str; 7] = ["true", "yes", "on", "1", "enabled", "enable", "y"];
const FALSE: [&str; 7] = ["false", "no", "off", "0", "disabled", "disable", "n"];
/// A string field without `maxLength` is limited to this many characters.
pub const STRING_LIMIT: usize = 500;

/// Lower-case, letters and digits only: "Wind speed", "wind_speed" and "windSpeed" all become "windspeed".
pub fn normalize_key(key: &str) -> String {
    key.to_lowercase().chars().filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit()).collect()
}

fn unquote(s: &[char]) -> String {
    let t = trim_chars(s);
    if t.len() >= 2 {
        let (a, b) = (t[0], t[t.len() - 1]);
        if matches!(a, '"' | '\'' | '`') && a == b {
            return t[1..t.len() - 1].iter().collect();
        }
    }
    t.iter().collect()
}

fn trim_chars(s: &[char]) -> &[char] {
    let start = s.iter().position(|c| !crate::text::is_js_space(*c)).unwrap_or(s.len());
    let end = s.iter().rposition(|c| !crate::text::is_js_space(*c)).map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

/// Strip a trailing line comment that is not inside quotes (`# …` or `// …`).
fn strip_comment(line: &[char]) -> &[char] {
    let mut quote: Option<char> = None;
    for i in 0..line.len() {
        let ch = line[i];
        if let Some(q) = quote {
            if ch == q && (i == 0 || line[i - 1] != '\\') {
                quote = None;
            }
            continue;
        }
        if ch == '"' || ch == '\'' {
            quote = Some(ch);
            continue;
        }
        if ch == '#' && (i == 0 || line[i - 1] == ' ' || line[i - 1] == '\t') {
            return &line[..i];
        }
        if ch == '/' && line.get(i + 1) == Some(&'/') && (i == 0 || line[i - 1] != ':') {
            return &line[..i];
        }
    }
    line
}

/// A flat object from JSON or `key: value` / `key = value` lines (values as strings), or None.
pub fn parse_loose_object(code: &str) -> Option<Map> {
    let text = js_trim(code);
    if text.is_empty() || utf16_len(text) > MAX_BODY {
        return None;
    }
    if text.starts_with('{') {
        if let Ok(Value::Object(m)) = Value::parse(text) {
            return Some(m);
        }
    }
    let mut out = Map::new();
    let mut found = false;
    for raw in text.split('\n') {
        let chars: Vec<char> = raw.chars().collect();
        let mut line: &[char] = trim_chars(strip_comment(&chars));
        if line.len() >= 2 && line[0] == '-' && line[1] == ' ' {
            line = trim_chars(&line[2..]);
        }
        let s: String = line.iter().collect();
        if line.is_empty() || s == "{" || s == "}" || s == "}," || line[0] == '[' {
            continue;
        }
        if line[0] == '{' {
            line = trim_chars(&line[1..]);
        }
        if line.last() == Some(&'}') {
            line = trim_chars(&line[..line.len() - 1]);
        }
        if matches!(line.last(), Some(',' | ';')) {
            line = trim_chars(&line[..line.len() - 1]);
        }
        let mut sep = None;
        let mut quote: Option<char> = None;
        for (i, ch) in line.iter().enumerate() {
            if let Some(q) = quote {
                if *ch == q {
                    quote = None;
                }
                continue;
            }
            if *ch == '"' || *ch == '\'' {
                quote = Some(*ch);
                continue;
            }
            if *ch == ':' || *ch == '=' {
                sep = Some(i);
                break;
            }
        }
        let Some(sep) = sep.filter(|s| *s > 0) else { continue };
        let key = unquote(&line[..sep]);
        let value = unquote(&line[sep + 1..]);
        if key.is_empty() {
            continue;
        }
        out.insert(key, value);
        found = true;
    }
    found.then_some(out)
}

/// The field spec's `type` ("string" when absent).
pub fn spec_type(spec: &Map) -> String {
    match spec.get("type") {
        Some(t) if t.truthy() => t.js_string(),
        _ => "string".into(),
    }
}

/// A spec keyword as a finite number (`Number.isFinite(spec.min)`: numbers only).
pub fn spec_finite(spec: &Map, key: &str) -> Option<f64> {
    spec.get(key).and_then(Value::finite)
}

/// A spec keyword when `spec[key] > 0` (as JS compares: strings are coerced).
pub fn spec_positive(spec: &Map, key: &str) -> Option<f64> {
    let n = spec.get(key)?.js_number();
    (n > 0.0).then_some(n)
}

/// One value coerced to a field spec (`coerceValue`): Some((value, adjusted)), or None when it cannot be.
/// spec: `{ type: number|integer|boolean|enum|string, min?, max?, step?, values?, maxLength? }`.
pub fn coerce_value(spec: &Map, raw: &Value) -> Option<(Value, bool)> {
    let ty = spec_type(spec);
    match ty.as_str() {
        "number" | "integer" => {
            let n = match raw {
                Value::Number(n) => *n,
                Value::Null => f64::NAN,
                other => parse_float(js_trim(&other.js_string())),
            };
            if !n.is_finite() {
                return None;
            }
            let mut v = n;
            if let Some(step) = spec_positive(spec, "step") {
                let origin = spec_finite(spec, "min").unwrap_or(0.0);
                v = origin + js_round((v - origin) / step) * step;
                let step_text = spec.get("step").map(Value::js_string).unwrap_or_default();
                let decimals = step_text.split('.').nth(1).map_or(0, utf16_len).min(10);
                v = string_to_number(&to_fixed(v, decimals));
            }
            if ty == "integer" {
                v = js_round(v);
            }
            if let Some(min) = spec_finite(spec, "min") {
                v = v.max(min);
            }
            if let Some(max) = spec_finite(spec, "max") {
                v = v.min(max);
            }
            Some((Value::Number(v), v != n))
        }
        "boolean" => {
            if let Value::Bool(b) = raw {
                return Some((Value::Bool(*b), false));
            }
            let s = if raw.is_null() { String::new() } else { js_trim(&raw.js_string()).to_lowercase() };
            if TRUE.contains(&s.as_str()) {
                Some((Value::Bool(true), false))
            } else if FALSE.contains(&s.as_str()) {
                Some((Value::Bool(false), false))
            } else {
                None
            }
        }
        "enum" => {
            let values: &[Value] = spec.get("values").and_then(Value::as_array).map_or(&[], Vec::as_slice);
            let s_owned = if raw.is_null() { String::new() } else { raw.js_string() };
            let s = js_trim(&s_owned);
            let hit = values
                .iter()
                .find(|x| x.js_string() == s)
                .or_else(|| values.iter().find(|x| x.js_string().to_lowercase() == s.to_lowercase()))
                .or_else(|| values.iter().find(|x| normalize_key(&x.js_string()) == normalize_key(s)))?;
            Some((hit.clone(), hit.js_string() != s))
        }
        _ => {
            let s = match raw {
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                Value::Array(_) | Value::Object(_) => to_json(raw),
                other => other.js_string(),
            };
            let max = spec_positive(spec, "maxLength").map_or(STRING_LIMIT, |m| m.trunc() as usize);
            let adjusted = utf16_len(&s) > max;
            Some((Value::String(js_head(&s, max).to_string()), adjusted))
        }
    }
}

/// `values.join(sep)` as JS joins an array (null items are empty).
pub fn join_values(values: &[Value], sep: &str) -> String {
    values.iter().map(|v| if v.is_null() { String::new() } else { v.js_string() }).collect::<Vec<_>>().join(sep)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(s: &str) -> Map {
        Value::parse(s).unwrap().as_object().unwrap().clone()
    }

    #[test]
    fn loose_lines() {
        let m = parse_loose_object("n: 4\nmode = \"fast\" # comment\n- url: http://x // note\n").unwrap();
        assert_eq!(Value::Object(m).to_json(), r#"{"n":"4","mode":"fast","url":"http://x"}"#);
        assert!(parse_loose_object("just text").is_none());
    }

    #[test]
    fn numbers_snap_and_clamp() {
        let s = spec(r#"{"type":"number","min":0,"max":1,"step":0.25}"#);
        assert_eq!(coerce_value(&s, &Value::from(0.6)).unwrap().0, Value::from(0.5));
        assert_eq!(coerce_value(&s, &Value::from("0.9 units")).unwrap().0, Value::from(1.0));
        assert!(coerce_value(&s, &Value::from("x")).is_none());
        let s = spec(r#"{"type":"number","step":0.1}"#);
        assert_eq!(coerce_value(&s, &Value::from(0.30000000000000004)).unwrap().0, Value::from(0.3));
    }
}
