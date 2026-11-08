//! Tool contracts in the standard shapes (`core/schema.js`): JSON Schema for inputs (MCP's `inputSchema`) and MCP
//! tool annotations for what a tool does.
//!
//! A JSON Schema input is converted to the runtime's field shorthand, so arguments are validated the same way
//! either way. Only the subset that can be enforced is accepted: a construct that cannot be enforced (nested
//! objects, `$ref`, `oneOf`…) is refused with the reason, never silently loosened.

use std::fmt;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::json::{Map, Value};

/// What a tool can do; it decides whether the user is asked first.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    /// Looks something up; changes nothing.
    Read,
    /// Changes the application's state (undoable, or easily put right).
    #[default]
    Write,
    /// Deletes or overwrites; cannot be undone.
    Destructive,
    /// Reaches outside the application: sends, publishes, pays, calls a third party.
    External,
    /// Changes the application itself: its code, its configuration, its capabilities.
    System,
}

/// The effect names, in the contract's order.
pub const EFFECT_NAMES: [&str; 5] = ["read", "write", "destructive", "external", "system"];

impl Effect {
    pub const ALL: [Effect; 5] = [Effect::Read, Effect::Write, Effect::Destructive, Effect::External, Effect::System];

    pub fn as_str(self) -> &'static str {
        match self {
            Effect::Read => "read",
            Effect::Write => "write",
            Effect::Destructive => "destructive",
            Effect::External => "external",
            Effect::System => "system",
        }
    }

    /// An effect by name (exact, as JS's `EFFECTS.includes`).
    pub fn parse(name: &str) -> Option<Effect> {
        Effect::ALL.into_iter().find(|e| e.as_str() == name)
    }
}

impl fmt::Display for Effect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// MCP annotations → an effect (when the tool does not say its effect). None when they say nothing.
pub fn effect_from_annotations(a: Option<&Value>) -> Option<Effect> {
    let a = a?.as_object()?;
    let hint = |k: &str| a.get(k).and_then(Value::as_bool);
    if hint("readOnlyHint") == Some(true) {
        return Some(Effect::Read);
    }
    if hint("destructiveHint") == Some(true) {
        return Some(Effect::Destructive);
    }
    if hint("openWorldHint") == Some(true) {
        return Some(Effect::External);
    }
    if hint("readOnlyHint") == Some(false) || hint("destructiveHint") == Some(false) {
        return Some(Effect::Write);
    }
    None
}

/// An effect (+ the tool's own annotations) → MCP annotations. The effect wins where they disagree; a read tool
/// may still say it looks outside (`openWorldHint`, a web search).
pub fn annotations_for(effect: Effect, given: Option<&Map>) -> Map {
    let mut out = given.cloned().unwrap_or_default();
    let own_open = given.and_then(|g| g.get("openWorldHint")).and_then(Value::as_bool) == Some(true);
    out.insert("readOnlyHint", effect == Effect::Read);
    out.insert("destructiveHint", matches!(effect, Effect::Destructive | Effect::System));
    out.insert("openWorldHint", effect == Effect::External || own_open);
    if effect == Effect::Read {
        out.remove("destructiveHint");
    }
    out
}

const IGNORED: [&str; 13] = [
    "title",
    "description",
    "default",
    "examples",
    "$comment",
    "deprecated",
    "readOnly",
    "writeOnly",
    "format",
    "contentMediaType",
    "contentEncoding",
    "$schema",
    "$id",
];
const STRING_KEYS: [&str; 6] = ["type", "enum", "maxLength", "minLength", "pattern", "const"];
const NUMBER_KEYS: [&str; 8] =
    ["type", "enum", "minimum", "maximum", "exclusiveMinimum", "exclusiveMaximum", "multipleOf", "const"];
const BOOLEAN_KEYS: [&str; 2] = ["type", "const"];
const ARRAY_KEYS: [&str; 4] = ["type", "items", "maxItems", "minItems"];
const UNSUPPORTED: [&str; 7] = ["$ref", "oneOf", "anyOf", "allOf", "not", "if", "patternProperties"];

fn family_keys(family: &str) -> Option<&'static [&'static str]> {
    match family {
        "string" => Some(&STRING_KEYS),
        "number" => Some(&NUMBER_KEYS),
        "boolean" => Some(&BOOLEAN_KEYS),
        "array" => Some(&ARRAY_KEYS),
        _ => None,
    }
}

/// `x > 0` as JS evaluates it for a schema keyword (strings are coerced).
fn positive(v: Option<&Value>) -> Option<f64> {
    let n = v?.js_number();
    (n > 0.0).then_some(n)
}

/// The one type of a property: (type, nullable), or the reason.
fn prop_type(key: &str, prop: &Map, at: &str) -> Result<(String, bool), String> {
    let mut nullable = false;
    let mut ty: Option<Value> = prop.get("type").cloned();
    if let Some(Value::Array(types)) = &ty {
        let real: Vec<&Value> = types.iter().filter(|t| t.as_str() != Some("null")).collect();
        nullable = real.len() < types.len();
        if real.len() != 1 {
            let list: Vec<String> = types.iter().map(Value::js_string).collect();
            return Err(format!("{at}: \"{key}\" has several types ({}); give it one.", list.join(", ")));
        }
        ty = Some(real[0].clone());
    }
    if ty.is_none() {
        if let Some(Value::Array(values)) = prop.get("enum") {
            ty = Some(Value::from(if values.iter().all(|v| matches!(v, Value::Number(_))) {
                "number"
            } else {
                "string"
            }));
        } else if let Some(c) = prop.get("const") {
            ty = Some(Value::from(c.type_of()));
        }
    }
    let ty = match ty {
        None => "undefined".to_string(),
        Some(v) => v.js_string(),
    };
    if ty == "object" {
        return Err(format!("{at}: \"{key}\" is an object. Tool arguments are flat: give each value its own parameter, or take JSON text in a string with a maxLength."));
    }
    Ok((ty, nullable))
}

fn string_field(key: &str, prop: &Map, at: &str) -> Result<Map, String> {
    let mut field = Map::new();
    field.insert("type", "string");
    for k in ["maxLength", "minLength"] {
        if positive(prop.get(k)).is_some() {
            field.insert(k, prop.get(k).cloned().unwrap_or_default());
        }
    }
    if let Some(Value::String(pattern)) = prop.get("pattern") {
        if Regex::new(pattern).is_err() {
            return Err(format!("{at}: \"{key}\" has a pattern that is not a valid regular expression."));
        }
        field.insert("pattern", pattern.as_str());
    }
    Ok(field)
}

fn number_field(key: &str, prop: &Map, at: &str, ty: &str) -> Result<Map, String> {
    let mut field = Map::new();
    field.insert("type", ty);
    let finite = |k: &str| prop.get(k).and_then(Value::finite);
    let mut min = finite("minimum");
    let mut max = finite("maximum");
    let ex_min = finite("exclusiveMinimum");
    let ex_max = finite("exclusiveMaximum");
    if ex_min.is_some() || ex_max.is_some() {
        // Exact for integers (> 0 is >= 1); a number range with an open end cannot be clamped, so it is refused.
        if ty != "integer" {
            return Err(format!("{at}: \"{key}\" uses an exclusive bound; use minimum / maximum for a number."));
        }
        if let Some(e) = ex_min {
            min = Some(min.unwrap_or(f64::NEG_INFINITY).max(e.floor() + 1.0));
        }
        if let Some(e) = ex_max {
            max = Some(max.unwrap_or(f64::INFINITY).min(e.ceil() - 1.0));
        }
    }
    if let Some(m) = min {
        field.insert("min", m);
    }
    if let Some(m) = max {
        field.insert("max", m);
    }
    if positive(prop.get("multipleOf")).is_some() {
        field.insert("step", prop.get("multipleOf").cloned().unwrap_or_default());
    }
    Ok(field)
}

fn array_field(key: &str, prop: &Map, at: &str) -> Result<Map, String> {
    let default_items = Value::Object([("type", "string")].into_iter().collect());
    let items = match prop.get("items") {
        None | Some(Value::Null) => &default_items,
        Some(v) => v,
    };
    if matches!(items, Value::Array(_)) {
        return Err(format!("{at}: \"{key}\" has a tuple of item schemas; give one schema for every item."));
    }
    let mut item = field_from_schema(&format!("{key}[]"), items, at)?;
    if item.get("type").and_then(Value::as_str) == Some("array") {
        return Err(format!("{at}: \"{key}\" is a list of lists, which tool arguments do not support."));
    }
    item.remove("description");
    item.remove("default");
    item.remove("nullable");
    let mut field = Map::new();
    field.insert("type", "array");
    field.insert("items", item);
    if positive(prop.get("maxItems")).is_some() {
        field.insert("maxItems", prop.get("maxItems").cloned().unwrap_or_default());
    }
    if positive(prop.get("minItems")).is_some() {
        field.insert("minItems", prop.get("minItems").cloned().unwrap_or_default());
    }
    Ok(field)
}

/// One property of a JSON Schema object → a field spec (or the reason it cannot be one).
fn field_from_schema(key: &str, prop: &Value, at: &str) -> Result<Map, String> {
    let Some(prop) = prop.as_object() else {
        return Err(format!("{at}: \"{key}\" must be a schema object."));
    };
    if let Some(bad) = UNSUPPORTED.iter().find(|k| prop.contains_key(k)) {
        return Err(format!("{at}: \"{key}\" uses {bad}, which tool arguments do not support. Describe it with one type, or with an enum."));
    }
    let (ty, nullable) = prop_type(key, prop, at)?;
    let family = if ty == "integer" { "number" } else { ty.as_str() };
    let Some(allowed) = family_keys(family) else {
        return Err(format!("{at}: \"{key}\" has an unsupported type \"{ty}\"."));
    };
    if let Some(unknown) = prop.keys().find(|k| !allowed.contains(&k.as_str()) && !IGNORED.contains(&k.as_str())) {
        return Err(format!("{at}: \"{key}\" uses \"{unknown}\", which cannot be enforced for a {ty}."));
    }
    if family == "boolean" {
        if let Some(c) = prop.get("const") {
            return Err(format!("{at}: \"{key}\" can only be {}: that is not a parameter.", c.js_string()));
        }
    }
    let values: Option<Vec<Value>> = match prop.get("enum") {
        Some(Value::Array(list)) => Some(list.iter().filter(|v| !v.is_null()).cloned().collect()),
        _ => None,
    };
    let mut field = if (values.is_some() || prop.contains_key("const")) && family != "boolean" && family != "array" {
        let list = values.unwrap_or_else(|| vec![prop.get("const").cloned().unwrap_or_default()]);
        if list.is_empty() {
            return Err(format!("{at}: \"{key}\" has an empty enum."));
        }
        let mut f = Map::new();
        f.insert("type", "enum");
        f.insert("values", list);
        f
    } else {
        match family {
            "string" => string_field(key, prop, at)?,
            "number" => number_field(key, prop, at, &ty)?,
            "boolean" => [("type", "boolean")].into_iter().collect(),
            _ => array_field(key, prop, at)?,
        }
    };
    if let Some(d) = prop.get("description").filter(|d| d.truthy()) {
        field.insert("description", d.js_string());
    }
    if let Some(d) = prop.get("default") {
        field.insert("default", d.clone());
    }
    if nullable {
        field.insert("nullable", true);
    }
    Ok(field)
}

/// A JSON Schema object (a tool's `inputSchema`) → the runtime's field specs (a tool's `parameters`), in the
/// order of `properties`. `at` names the tool in messages (`Tool "find_text"`).
pub fn from_json_schema(schema: &Value, at: &str) -> Result<Map, String> {
    let schema = match schema {
        Value::Null => return Ok(Map::new()),
        Value::Object(m) => m,
        _ => return Err(format!("{at}: the input schema must be an object.")),
    };
    if let Some(t) = schema.get("type") {
        if t.as_str() != Some("object") {
            return Err(format!("{at}: the input schema must have type \"object\"."));
        }
    }
    for bad in ["$ref", "oneOf", "anyOf", "allOf", "patternProperties"] {
        if schema.contains_key(bad) {
            return Err(format!("{at}: the input schema uses {bad}, which tool arguments do not support."));
        }
    }
    let mut required: Vec<String> = Vec::new();
    if let Some(Value::Array(list)) = schema.get("required") {
        for r in list {
            let r = r.js_string();
            if !required.contains(&r) {
                required.push(r);
            }
        }
    }
    let mut out = Map::new();
    if let Some(Value::Object(props)) = schema.get("properties") {
        for (key, prop) in props.iter() {
            let mut field = field_from_schema(key, prop, at)?;
            if required.contains(key) {
                field.insert("required", true);
            }
            out.insert(key.clone(), field);
        }
    }
    if let Some(missing) = required.iter().find(|k| !out.contains_key(k)) {
        return Err(format!("{at}: \"{missing}\" is required but not among the properties."));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conv(s: &str) -> Result<Map, String> {
        from_json_schema(&Value::parse(s).unwrap(), "Tool \"t\"")
    }

    #[test]
    fn converts_the_supported_subset() {
        let p = conv(r#"{"type":"object","properties":{"n":{"type":"integer","exclusiveMinimum":0,"maximum":9},"tags":{"type":"array","items":{"type":"string","maxLength":10},"maxItems":3},"m":{"enum":["a","b",null]},"x":{"type":["string","null"]}},"required":["n"]}"#).unwrap();
        assert_eq!(
            Value::Object(p).to_json(),
            r#"{"n":{"type":"integer","min":1,"max":9,"required":true},"tags":{"type":"array","items":{"type":"string","maxLength":10},"maxItems":3},"m":{"type":"enum","values":["a","b"]},"x":{"type":"string","nullable":true}}"#
        );
    }

    #[test]
    fn refuses_with_the_reason() {
        assert_eq!(conv(r#"{"properties":{"o":{"type":"object"}}}"#).unwrap_err(), "Tool \"t\": \"o\" is an object. Tool arguments are flat: give each value its own parameter, or take JSON text in a string with a maxLength.");
        assert_eq!(
            conv(r#"{"properties":{"n":{"type":"number","exclusiveMinimum":0}}}"#).unwrap_err(),
            "Tool \"t\": \"n\" uses an exclusive bound; use minimum / maximum for a number."
        );
        assert_eq!(
            conv(r#"{"properties":{"a":{"type":"array","items":{"type":"array"}}}}"#).unwrap_err(),
            "Tool \"t\": \"a\" is a list of lists, which tool arguments do not support."
        );
        assert_eq!(
            conv(r#"{"properties":{"u":{"type":["string","number"]}}}"#).unwrap_err(),
            "Tool \"t\": \"u\" has several types (string, number); give it one."
        );
        assert_eq!(
            conv(r#"{"properties":{},"required":["z"]}"#).unwrap_err(),
            "Tool \"t\": \"z\" is required but not among the properties."
        );
        assert_eq!(
            conv(r#"{"properties":{"s":{"type":"string","format":"date","foo":1}}}"#).unwrap_err(),
            "Tool \"t\": \"s\" uses \"foo\", which cannot be enforced for a string."
        );
    }

    #[test]
    fn effects_and_annotations() {
        let given: Map = [("title", Value::from("T")), ("openWorldHint", Value::from(true))].into_iter().collect();
        assert_eq!(
            Value::Object(annotations_for(Effect::Read, Some(&given))).to_json(),
            r#"{"title":"T","openWorldHint":true,"readOnlyHint":true}"#
        );
    }
}
