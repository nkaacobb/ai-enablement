//! JSON with JavaScript semantics: the data model of the whole crate.
//!
//! The JS runtime is the reference, and several of its outputs depend on the order in which an object's keys were
//! written (prompt text from context objects, the order of a tool's parameters, `required` lists, a call shown as
//! `name(a: 1, b: 2)`). `serde_json::Value` sorts keys unless its `preserve_order` feature is on, and that feature
//! must never be switched on here: Cargo unifies features across a workspace, so it would change the key order of
//! every `serde_json::Map` in the host application. So this module has its own value:
//!
//! - objects keep their keys in insertion order, as JS objects do (keys that are array indices first, ascending);
//! - numbers are `f64`, as in JS;
//! - parsing goes through serde (any serde format works) and keeps the document order;
//! - [`to_json`] / [`to_json_indent`] write what `JSON.stringify` writes, and [`stable_stringify`] what the JS
//!   runtime's `stableStringify` writes (keys sorted by UTF-16 code units).

use std::fmt;

use serde::de::{self, Deserialize, Deserializer, MapAccess, SeqAccess, Visitor};
use serde::ser::{Serialize, SerializeMap, SerializeSeq, Serializer};

use crate::text::{is_js_space, utf16_cmp};

/// A JSON value with JavaScript semantics (see the module documentation).
#[derive(Clone, Debug, Default)]
pub enum Value {
    #[default]
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(Map),
}

/// An object: keys in insertion order, unique. Assigning to an existing key keeps its place (as in JS).
#[derive(Clone, Debug, Default)]
pub struct Map {
    entries: Vec<(String, Value)>,
}

/// JS orders keys that are array indices ("0", "17") before all other keys, ascending.
fn array_index(key: &str) -> Option<u32> {
    if key.is_empty() || key.len() > 10 || (key.len() > 1 && key.starts_with('0')) {
        return None;
    }
    if !key.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: u64 = key.parse().ok()?;
    (n < u64::from(u32::MAX)).then_some(n as u32)
}

impl Map {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.entries.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.entries.iter_mut().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.iter().any(|(k, _)| k == key)
    }

    /// `obj[key] = value`: replaces in place, or adds the key where JS would.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<Value>) {
        let key = key.into();
        let value = value.into();
        if let Some(slot) = self.get_mut(&key) {
            *slot = value;
            return;
        }
        match array_index(&key) {
            Some(n) => {
                let at = self
                    .entries
                    .iter()
                    .position(|(k, _)| array_index(k).map_or(true, |m| m > n))
                    .unwrap_or(self.entries.len());
                self.entries.insert(at, (key, value));
            }
            None => self.entries.push((key, value)),
        }
    }

    /// `delete obj[key]`: the other keys keep their order.
    pub fn remove(&mut self, key: &str) -> Option<Value> {
        let at = self.entries.iter().position(|(k, _)| k == key)?;
        Some(self.entries.remove(at).1)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&String, &Value)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &Value> {
        self.entries.iter().map(|(_, v)| v)
    }
}

impl IntoIterator for Map {
    type Item = (String, Value);
    type IntoIter = std::vec::IntoIter<(String, Value)>;
    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<K: Into<String>, V: Into<Value>> FromIterator<(K, V)> for Map {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut m = Map::new();
        for (k, v) in iter {
            m.insert(k, v);
        }
        m
    }
}

/// Deep equality as the conformance vectors define it: key order ignored, array order kept.
impl PartialEq for Map {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len() && self.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Null, Value::Null) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Number(a), Value::Number(b)) => a == b,
            (Value::String(a), Value::String(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => a == b,
            (Value::Object(a), Value::Object(b)) => a == b,
            _ => false,
        }
    }
}

impl Value {
    /// Parse JSON text (`JSON.parse`), keeping the document's key order.
    pub fn parse(text: &str) -> Result<Value, String> {
        serde_json::from_str(text).map_err(|e| e.to_string())
    }

    pub fn object() -> Value {
        Value::Object(Map::new())
    }

    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&Map> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    pub fn as_object_mut(&mut self) -> Option<&mut Map> {
        match self {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    /// `value[key]` for an object (None for anything else, or a missing key).
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_object().and_then(|m| m.get(key))
    }

    /// `value[key]`, with a missing key or a non-object read as `null` (JS `undefined` and `null` alike).
    pub fn at(&self, key: &str) -> &Value {
        static NULL: Value = Value::Null;
        self.get(key).unwrap_or(&NULL)
    }

    /// JS `typeof`: "object" for null, arrays and objects.
    pub fn type_of(&self) -> &'static str {
        match self {
            Value::Null | Value::Array(_) | Value::Object(_) => "object",
            Value::Bool(_) => "boolean",
            Value::Number(_) => "number",
            Value::String(_) => "string",
        }
    }

    /// JS truthiness.
    pub fn truthy(&self) -> bool {
        match self {
            Value::Null => false,
            Value::Bool(b) => *b,
            Value::Number(n) => *n != 0.0 && !n.is_nan(),
            Value::String(s) => !s.is_empty(),
            Value::Array(_) | Value::Object(_) => true,
        }
    }

    /// JS `String(value)`.
    pub fn js_string(&self) -> String {
        match self {
            Value::Null => "null".into(),
            Value::Bool(b) => b.to_string(),
            Value::Number(n) => number_to_string(*n),
            Value::String(s) => s.clone(),
            Value::Array(items) => items
                .iter()
                .map(|v| if v.is_null() { String::new() } else { v.js_string() })
                .collect::<Vec<_>>()
                .join(","),
            Value::Object(_) => "[object Object]".into(),
        }
    }

    /// JS `Number(value)`.
    pub fn js_number(&self) -> f64 {
        match self {
            Value::Null => 0.0,
            Value::Bool(b) => f64::from(u8::from(*b)),
            Value::Number(n) => *n,
            Value::String(s) => string_to_number(s),
            Value::Array(_) => string_to_number(&self.js_string()),
            Value::Object(_) => f64::NAN,
        }
    }

    /// A finite number (JS `Number.isFinite(value)`: true only for numbers).
    pub fn finite(&self) -> Option<f64> {
        self.as_f64().filter(|n| n.is_finite())
    }

    /// Serialise as `JSON.stringify` (compact).
    pub fn to_json(&self) -> String {
        to_json(self)
    }

    /// Convert to a `serde_json::Value` (its key order is serde_json's own).
    pub fn to_serde(&self) -> serde_json::Value {
        match self {
            Value::Null => serde_json::Value::Null,
            Value::Bool(b) => serde_json::Value::Bool(*b),
            Value::Number(n) => number_to_serde(*n),
            Value::String(s) => serde_json::Value::String(s.clone()),
            Value::Array(a) => serde_json::Value::Array(a.iter().map(Value::to_serde).collect()),
            Value::Object(m) => serde_json::Value::Object(m.iter().map(|(k, v)| (k.clone(), v.to_serde())).collect()),
        }
    }
}

fn number_to_serde(n: f64) -> serde_json::Value {
    if !n.is_finite() {
        return serde_json::Value::Null;
    }
    if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_992.0 {
        return serde_json::Value::from(n as i64);
    }
    serde_json::Number::from_f64(n).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

impl From<serde_json::Value> for Value {
    fn from(v: serde_json::Value) -> Self {
        match v {
            serde_json::Value::Null => Value::Null,
            serde_json::Value::Bool(b) => Value::Bool(b),
            serde_json::Value::Number(n) => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
            serde_json::Value::String(s) => Value::String(s),
            serde_json::Value::Array(a) => Value::Array(a.into_iter().map(Value::from).collect()),
            serde_json::Value::Object(m) => Value::Object(m.into_iter().map(|(k, v)| (k, Value::from(v))).collect()),
        }
    }
}

impl From<&Value> for serde_json::Value {
    fn from(v: &Value) -> Self {
        v.to_serde()
    }
}

impl From<Value> for serde_json::Value {
    fn from(v: Value) -> Self {
        v.to_serde()
    }
}

impl From<bool> for Value {
    fn from(b: bool) -> Self {
        Value::Bool(b)
    }
}

impl From<f64> for Value {
    fn from(n: f64) -> Self {
        Value::Number(n)
    }
}

macro_rules! from_int {
    ($($t:ty),*) => {$(
        impl From<$t> for Value {
            fn from(n: $t) -> Self {
                Value::Number(n as f64)
            }
        }
    )*};
}
from_int!(i32, i64, u32, u64, usize);

impl From<&str> for Value {
    fn from(s: &str) -> Self {
        Value::String(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Self {
        Value::String(s)
    }
}

impl From<&String> for Value {
    fn from(s: &String) -> Self {
        Value::String(s.clone())
    }
}

impl From<Map> for Value {
    fn from(m: Map) -> Self {
        Value::Object(m)
    }
}

impl<T: Into<Value>> From<Vec<T>> for Value {
    fn from(v: Vec<T>) -> Self {
        Value::Array(v.into_iter().map(Into::into).collect())
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(v: Option<T>) -> Self {
        v.map_or(Value::Null, Into::into)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&to_json(self))
    }
}

/* ------------------------------------------------------------------------------------------------ serde */

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Value;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("any JSON value")
            }
            fn visit_bool<E: de::Error>(self, b: bool) -> Result<Value, E> {
                Ok(Value::Bool(b))
            }
            fn visit_i64<E: de::Error>(self, n: i64) -> Result<Value, E> {
                Ok(Value::Number(n as f64))
            }
            fn visit_u64<E: de::Error>(self, n: u64) -> Result<Value, E> {
                Ok(Value::Number(n as f64))
            }
            fn visit_f64<E: de::Error>(self, n: f64) -> Result<Value, E> {
                Ok(Value::Number(n))
            }
            fn visit_str<E: de::Error>(self, s: &str) -> Result<Value, E> {
                Ok(Value::String(s.to_string()))
            }
            fn visit_string<E: de::Error>(self, s: String) -> Result<Value, E> {
                Ok(Value::String(s))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_none<E: de::Error>(self) -> Result<Value, E> {
                Ok(Value::Null)
            }
            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
                Value::deserialize(d)
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
                let mut out = Vec::new();
                while let Some(v) = seq.next_element()? {
                    out.push(v);
                }
                Ok(Value::Array(out))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
                let mut out = Map::new();
                while let Some((k, v)) = map.next_entry::<String, Value>()? {
                    out.insert(k, v);
                }
                Ok(Value::Object(out))
            }
        }
        deserializer.deserialize_any(V)
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Value::Null => s.serialize_unit(),
            Value::Bool(b) => s.serialize_bool(*b),
            Value::Number(n) => {
                if !n.is_finite() {
                    s.serialize_unit()
                } else if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_992.0 {
                    s.serialize_i64(*n as i64)
                } else {
                    s.serialize_f64(*n)
                }
            }
            Value::String(v) => s.serialize_str(v),
            Value::Array(a) => {
                let mut seq = s.serialize_seq(Some(a.len()))?;
                for v in a {
                    seq.serialize_element(v)?;
                }
                seq.end()
            }
            Value::Object(m) => m.serialize(s),
        }
    }
}

impl Serialize for Map {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.len()))?;
        for (k, v) in self.iter() {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Map {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        match Value::deserialize(deserializer)? {
            Value::Object(m) => Ok(m),
            other => Err(de::Error::custom(format!("expected an object, found {}", other.type_of()))),
        }
    }
}

/* ------------------------------------------------------------------------------------- numbers as in JS */

/// ECMAScript `Number::toString`: `1e+21`, `0.1`, `-1.5`, `123456789012`; `-0` is `"0"`.
pub fn number_to_string(n: f64) -> String {
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    if n == 0.0 {
        return "0".into();
    }
    ryu_js::Buffer::new().format_finite(n).to_string()
}

/// ECMAScript `StringToNumber` (what `Number("…")` does): whitespace trimmed, `""` is 0, hex / octal / binary
/// prefixes, `Infinity`; anything else that is not a decimal literal is NaN.
pub fn string_to_number(s: &str) -> f64 {
    let t = s.trim_matches(is_js_space);
    if t.is_empty() {
        return 0.0;
    }
    match t {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    let lower = t.get(..2).map(str::to_ascii_lowercase);
    let radix = match lower.as_deref() {
        Some("0x") => 16,
        Some("0o") => 8,
        Some("0b") => 2,
        _ => 0,
    };
    if radix != 0 {
        let digits = &t[2..];
        if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
            return f64::NAN;
        }
        return digits.chars().fold(0.0, |acc, c| acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0)));
    }
    if decimal_literal_len(t) == t.len() {
        parse_decimal(t)
    } else {
        f64::NAN
    }
}

/// JS `parseFloat`: the longest decimal prefix (after leading whitespace), or NaN.
pub fn parse_float(s: &str) -> f64 {
    let t = s.trim_start_matches(is_js_space);
    for (lit, v) in [("Infinity", f64::INFINITY), ("+Infinity", f64::INFINITY), ("-Infinity", f64::NEG_INFINITY)] {
        if t.starts_with(lit) {
            return v;
        }
    }
    let n = decimal_literal_len(t);
    if n == 0 {
        f64::NAN
    } else {
        parse_decimal(&t[..n])
    }
}

/// Length in bytes of the decimal literal at the start of `t` (`[+-]? digits [. digits] [e [+-] digits]`), or 0.
fn decimal_literal_len(t: &str) -> usize {
    let b = t.as_bytes();
    let mut i = 0;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - int_start;
    if i < b.len() && b[i] == b'.' {
        let frac_start = i + 1;
        let mut j = frac_start;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if digits > 0 || j > frac_start {
            digits += j - frac_start;
            i = j;
        }
    }
    if digits == 0 {
        return 0;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    i
}

fn parse_decimal(t: &str) -> f64 {
    // Rust's parser wants digits on both sides of the point.
    let mut s = t.to_string();
    if let Some(p) = s.find('.') {
        let after = s[p + 1..].chars().next();
        if !after.is_some_and(|c| c.is_ascii_digit()) {
            s.insert(p + 1, '0');
        }
        let before = p.checked_sub(1).and_then(|q| s[q..].chars().next());
        if !before.is_some_and(|c| c.is_ascii_digit()) {
            s.insert(p, '0');
        }
    }
    s.parse().unwrap_or(f64::NAN)
}

/// JS `Math.round`: halves round up (towards +∞).
pub fn js_round(x: f64) -> f64 {
    if !x.is_finite() {
        return x;
    }
    let f = x.floor();
    if x - f >= 0.5 {
        f + 1.0
    } else {
        f
    }
}

/// JS `Number.prototype.toFixed(digits)`: exact decimal rounding, ties away from zero.
pub fn to_fixed(x: f64, digits: usize) -> String {
    if !x.is_finite() || x.abs() >= 1e21 {
        return number_to_string(x);
    }
    let negative = x < 0.0;
    // Rust prints the exact binary value when asked for enough digits; round that decimal string by hand.
    let exact = format!("{:.1100}", x.abs());
    let (int_part, frac_part) = exact.split_once('.').unwrap_or((&exact, ""));
    let mut digits_vec: Vec<u8> = int_part.bytes().chain(frac_part.bytes().take(digits)).map(|b| b - b'0').collect();
    let next = frac_part.as_bytes().get(digits).copied().unwrap_or(b'0');
    if next >= b'5' {
        let mut k = digits_vec.len();
        loop {
            if k == 0 {
                digits_vec.insert(0, 1);
                break;
            }
            k -= 1;
            if digits_vec[k] == 9 {
                digits_vec[k] = 0;
            } else {
                digits_vec[k] += 1;
                break;
            }
        }
    }
    let int_len = digits_vec.len() - digits;
    // As in JS, any negative number keeps its sign: (-0.001).toFixed(2) is "-0.00".
    let mut out = String::from(if negative { "-" } else { "" });
    for (k, d) in digits_vec.iter().enumerate() {
        if k == int_len {
            out.push('.');
        }
        out.push(char::from(b'0' + d));
    }
    out
}

/* ------------------------------------------------------------------------------------------ stringify */

/// `JSON.stringify(value)`: compact, keys in insertion order.
pub fn to_json(v: &Value) -> String {
    let mut out = String::new();
    write_value(v, &mut out, "", "", false);
    out
}

/// `JSON.stringify(value, null, indent)` (indent capped at 10, as in JS; 0 is compact).
pub fn to_json_indent(v: &Value, indent: usize) -> String {
    let gap = " ".repeat(indent.min(10));
    let mut out = String::new();
    write_value(v, &mut out, &gap, "", false);
    out
}

/// A serde type as a [`Value`], its fields in declaration order (written as JSON text and read back, which keeps the
/// order; `serde_json::to_value` would sort the keys).
pub fn to_value<T: Serialize + ?Sized>(t: &T) -> Value {
    serde_json::to_string(t).ok().and_then(|s| Value::parse(&s).ok()).unwrap_or(Value::Null)
}

/// A [`Value`] as a serde type.
pub fn from_value<T: serde::de::DeserializeOwned>(v: &Value) -> Result<T, String> {
    serde_json::from_str(&to_json(v)).map_err(|e| e.to_string())
}

/// The JS runtime's `stableStringify`: object keys sorted (UTF-16 code-unit order) at every level, arrays kept in
/// order, non-finite numbers as null. Content builders, tool results and fingerprints rely on it.
pub fn stable_stringify(v: &Value, indent: usize) -> String {
    let gap = " ".repeat(indent.min(10));
    let mut out = String::new();
    write_value(v, &mut out, &gap, "", true);
    out
}

fn write_value(v: &Value, out: &mut String, gap: &str, current: &str, sorted: bool) {
    match v {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => {
            if n.is_finite() {
                out.push_str(&number_to_string(*n));
            } else {
                out.push_str("null");
            }
        }
        Value::String(s) => quote_into(s, out),
        Value::Array(items) => {
            if items.is_empty() {
                out.push_str("[]");
                return;
            }
            let inner = format!("{current}{gap}");
            out.push('[');
            for (k, item) in items.iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                if !gap.is_empty() {
                    out.push('\n');
                    out.push_str(&inner);
                }
                write_value(item, out, gap, &inner, sorted);
            }
            if !gap.is_empty() {
                out.push('\n');
                out.push_str(current);
            }
            out.push(']');
        }
        Value::Object(m) => {
            if m.is_empty() {
                out.push_str("{}");
                return;
            }
            let mut entries: Vec<(&String, &Value)> = m.iter().collect();
            if sorted {
                entries.sort_by(|a, b| utf16_cmp(a.0, b.0));
            }
            let inner = format!("{current}{gap}");
            out.push('{');
            for (k, (key, value)) in entries.into_iter().enumerate() {
                if k > 0 {
                    out.push(',');
                }
                if !gap.is_empty() {
                    out.push('\n');
                    out.push_str(&inner);
                }
                quote_into(key, out);
                out.push(':');
                if !gap.is_empty() {
                    out.push(' ');
                }
                write_value(value, out, gap, &inner, sorted);
            }
            if !gap.is_empty() {
                out.push('\n');
                out.push_str(current);
            }
            out.push('}');
        }
    }
}

/// A string as `JSON.stringify` quotes it.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    quote_into(s, &mut out);
    out
}

fn quote_into(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_document_order_and_js_index_order() {
        let v = Value::parse(r#"{"b":1,"a":2,"10":3,"2":4,"b":5}"#).unwrap();
        assert_eq!(to_json(&v), r#"{"2":4,"10":3,"b":5,"a":2}"#);
    }

    #[test]
    fn numbers_as_js_writes_them() {
        for (n, s) in [
            (0.0, "0"),
            (-0.0, "0"),
            (1e21, "1e+21"),
            (-1.5, "-1.5"),
            (123456789012.0, "123456789012"),
            (0.1, "0.1"),
            (1e-7, "1e-7"),
        ] {
            assert_eq!(number_to_string(n), s);
        }
    }

    #[test]
    fn string_to_number_like_js() {
        assert_eq!(string_to_number(" 12 "), 12.0);
        assert_eq!(string_to_number(""), 0.0);
        assert_eq!(string_to_number("0x10"), 16.0);
        assert_eq!(string_to_number("1."), 1.0);
        assert_eq!(string_to_number(".5"), 0.5);
        assert_eq!(string_to_number("+5"), 5.0);
        assert!(string_to_number("5px").is_nan());
        assert!(string_to_number("inf").is_nan());
        assert_eq!(parse_float("4abc"), 4.0);
        assert_eq!(parse_float("  3.5e2x"), 350.0);
        assert_eq!(parse_float("1e"), 1.0);
        assert!(parse_float("abc").is_nan());
    }

    #[test]
    fn to_fixed_rounds_ties_away_from_zero() {
        assert_eq!(to_fixed(2.5, 0), "3");
        assert_eq!(to_fixed(0.125, 2), "0.13");
        assert_eq!(to_fixed(-0.125, 2), "-0.13");
        assert_eq!(to_fixed(0.5000000000000001, 2), "0.50");
        assert_eq!(to_fixed(1.005, 2), "1.00");
        assert_eq!(to_fixed(-0.001, 2), "-0.00");
        assert_eq!(to_fixed(9.995, 1), "10.0");
    }

    #[test]
    fn js_round_halves_up() {
        assert_eq!(js_round(-2.5), -2.0);
        assert_eq!(js_round(2.5), 3.0);
        assert_eq!(js_round(0.49999999999999994), 0.0);
    }

    #[test]
    fn stringify_indent_and_escapes() {
        let v = Value::parse(r#"{"q":"a\"b\n\u0001","e":{},"l":[]}"#).unwrap();
        assert_eq!(to_json(&v), r#"{"q":"a\"b\n\u0001","e":{},"l":[]}"#);
        assert_eq!(to_json_indent(&v, 2), "{\n  \"q\": \"a\\\"b\\n\\u0001\",\n  \"e\": {},\n  \"l\": []\n}");
    }
}
