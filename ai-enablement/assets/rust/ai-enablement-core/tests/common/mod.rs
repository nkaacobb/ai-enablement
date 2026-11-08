//! What the conformance runners share: where the vectors are, running a suite, and small JSON helpers.
//!
//! The vectors are read from `AI_ENABLEMENT_SPEC` (a path to `spec/conformance`), else from the skill's own folder
//! next to this crate. A vendored copy inside an app has neither: the tests then say so and pass.

// Each test file uses its own subset.
#![allow(dead_code)]

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use ai_enablement_core::json::{Map, Value};

pub fn spec_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("AI_ENABLEMENT_SPEC") {
        let p = PathBuf::from(dir);
        return p.is_dir().then_some(p);
    }
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../spec/conformance");
    p.is_dir().then_some(p)
}

/// Every suite a runner lists has its vector file.
pub fn assert_suites_exist(suites: &[&str]) {
    let Some(dir) = spec_dir() else { return };
    for s in suites {
        assert!(dir.join(format!("{s}.json")).is_file(), "spec/conformance/{s}.json is missing");
    }
}

/// Run every case of a suite; fail once, listing every case that differs.
pub fn run_suite(suite: &str, run: impl Fn(&Value) -> Value) {
    let Some(dir) = spec_dir() else {
        eprintln!("conformance: skipped ({suite}): no spec/conformance folder (set AI_ENABLEMENT_SPEC to the skill's spec/conformance).");
        return;
    };
    let path = dir.join(format!("{suite}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let file = Value::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert_eq!(file.at("format").as_str(), Some("ai-enablement/conformance/1"), "{suite}: unknown vector format");
    let cases = file.at("cases").as_array().cloned().unwrap_or_default();
    assert!(!cases.is_empty(), "{suite}: no cases");
    let mut failures = Vec::new();
    for c in &cases {
        let name = c.at("name").as_str().unwrap_or("?").to_string();
        let expected = c.at("output");
        match catch_unwind(AssertUnwindSafe(|| run(c.at("input")))) {
            Ok(actual) if &actual == expected => {}
            Ok(actual) => failures.push(format!("- {name}\n    expected: {expected}\n    actual:   {actual}")),
            Err(_) => failures.push(format!("- {name}: panicked")),
        }
    }
    assert!(
        failures.is_empty(),
        "{suite}: {} of {} cases differ\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    Value::Object(pairs.into_iter().collect::<Map>())
}

pub fn str_of(v: &Value) -> String {
    v.as_str().unwrap_or_default().to_string()
}

pub fn bool_or(v: &Value, default: bool) -> bool {
    v.as_bool().unwrap_or(default)
}

pub fn opt<T: Into<Value>>(v: Option<T>) -> Value {
    v.map_or(Value::Null, Into::into)
}
