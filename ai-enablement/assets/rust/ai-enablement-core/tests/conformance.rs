//! The conformance vectors (`spec/conformance/*.json`): the same input → output pairs the JS reference runtime
//! produces, run against this crate. A failing case is a behaviour difference; the vectors are the contract.
//! This file runs the pure functions; `conformance_requests.rs` runs what a request carries and what each provider
//! protocol sends and reads back. Where the vectors are found: `common/mod.rs`.

mod common;

use common::{assert_suites_exist, bool_or, obj, opt, run_suite, str_of};

use ai_enablement_core::agents::{parse_agent, pick_agent, Agent};
use ai_enablement_core::context::{describe, ContextManager, Page};
use ai_enablement_core::conversation::{
    context_state, plan_turn, snapshot_block, superseded_stub, window_start, Message, Screen, SnapshotRecord, TurnInput,
};
use ai_enablement_core::frontmatter::parse_frontmatter;
use ai_enablement_core::hash::{hash_text, short_hash};
use ai_enablement_core::json::{stable_stringify, Value};
use ai_enablement_core::memory::{
    build_memory_prompt, memory_text, parse_memory_file, MemoryNote, NoteOptions, MEMORY_CHARS, MEMORY_MAX,
};
use ai_enablement_core::permissions::{
    decide_permission, merge_policies, needs_confirmation, normalize_policy, ConfirmSettings, Decision, Subject,
};
use ai_enablement_core::prompt::{build_system_prompt, PromptParts};
use ai_enablement_core::schema::{annotations_for, effect_from_annotations, Effect};
use ai_enablement_core::skills::{parse_skill, parse_slash_command, skill_file_path};
use ai_enablement_core::time::parse_date;
use ai_enablement_core::tools::{
    normalize_tool, read_arguments, serialize_result, to_mcp_tool, validate_args, RESULT_MAX_CHARS,
};

/// The suites this runner covers. A suite file not listed here is not run (yet).
const SUITES: [&str; 16] = [
    "hash",
    "stable-stringify",
    "describe",
    "context-snapshot",
    "plan-turn",
    "snapshot-wire",
    "system-prompt",
    "tool-contract",
    "tool-arguments",
    "tool-results",
    "permissions",
    "effects-annotations",
    "frontmatter",
    "skills",
    "agents",
    "memory",
];

#[test]
fn suites_table_names_existing_files() {
    assert_suites_exist(&SUITES);
}

#[test]
fn hash() {
    run_suite("hash", |i| {
        let h = hash_text(&str_of(i.at("text")));
        obj(vec![("short", short_hash(&h).into()), ("hash", h.into())])
    });
}

#[test]
fn stable_stringify_suite() {
    run_suite("stable-stringify", |i| {
        stable_stringify(i.at("value"), i.at("indent").as_f64().unwrap_or(2.0) as usize).into()
    });
}

#[test]
fn describe_suite() {
    run_suite("describe", |i| describe(i.at("value")).into());
}

#[test]
fn context_snapshot() {
    run_suite("context-snapshot", |i| {
        let ctx = ContextManager {
            page: Some(Page::from_value(i.at("page"))),
            max_chars: i.at("maxChars").as_f64().unwrap_or(24_000.0) as usize,
            ..Default::default()
        };
        let s = ctx.snapshot_at(String::new());
        obj(vec![
            ("pageId", s.page_id.into()),
            ("pageTitle", s.page_title.into()),
            ("text", s.text.into()),
            ("chars", s.chars.into()),
            ("totalChars", s.total_chars.into()),
            ("truncated", s.truncated.into()),
            ("hash", s.hash.into()),
            ("empty", s.empty.into()),
        ])
    });
}

#[test]
fn plan_turn_suite() {
    run_suite("plan-turn", |i| {
        let messages: Vec<Message> =
            serde_json::from_str(&i.get("messages").cloned().unwrap_or(Value::Array(vec![])).to_json())
                .expect("messages");
        let snap = i.at("snapshot");
        let hash = str_of(snap.at("hash"));
        let screen = (!snap.is_null()).then(|| Screen { hash: &hash, empty: bool_or(snap.at("empty"), false) });
        let history = i.at("historyMessages").as_f64().unwrap_or(20.0) as i64;
        let input = TurnInput {
            messages: &messages,
            screen,
            history_messages: history,
            share: bool_or(i.at("share"), true),
            force: bool_or(i.at("force"), false),
        };
        let plan = plan_turn(&input);
        let state = context_state(&input);
        obj(vec![
            (
                "plan",
                obj(vec![
                    ("attach", plan.attach.into()),
                    ("reason", plan.reason.as_str().into()),
                    ("syncedHash", opt(plan.synced_hash)),
                ]),
            ),
            (
                "state",
                obj(vec![
                    ("currentHash", opt(state.current_hash)),
                    ("syncedHash", opt(state.synced_hash)),
                    ("pending", state.pending.into()),
                    ("reason", state.reason.as_str().into()),
                    ("state", state.state.as_str().into()),
                ]),
            ),
            ("windowStart", window_start(messages.len() + 1, history).into()),
        ])
    });
}

#[test]
fn snapshot_wire() {
    run_suite("snapshot-wire", |i| {
        let s: SnapshotRecord = serde_json::from_str(&i.at("snapshot").to_json()).expect("snapshot");
        obj(vec![("block", snapshot_block(&s).into()), ("stub", superseded_stub(&s).into())])
    });
}

#[test]
fn system_prompt() {
    run_suite("system-prompt", |i| {
        let t = |k: &str| str_of(i.at(k));
        let p = PromptParts {
            base: t("base"),
            agent_text: t("agentText"),
            app_text: t("appText"),
            page_text: t("pageText"),
            share: bool_or(i.at("share"), true),
            files: bool_or(i.at("files"), false),
            vision: bool_or(i.at("vision"), false),
            screenshots: bool_or(i.at("screenshots"), true),
            memory_text: t("memoryText"),
            skills_text: t("skillsText"),
            tools_text: t("toolsText"),
        };
        build_system_prompt(&p).into()
    });
}

#[test]
fn tool_contract() {
    run_suite("tool-contract", |i| match normalize_tool(i.at("tool")) {
        Ok(t) => obj(vec![("tool", t.to_value()), ("mcp", to_mcp_tool(&t))]),
        Err(e) => obj(vec![("error", e.into())]),
    });
}

#[test]
fn tool_arguments() {
    run_suite("tool-arguments", |i| {
        let raw = i.at("raw");
        let r = read_arguments(raw);
        let tool = normalize_tool(i.at("tool")).expect("tool");
        let v = validate_args(&tool, raw);
        obj(vec![
            ("read", obj(vec![("args", r.args.into()), ("error", r.error.into())])),
            ("validated", obj(vec![("ok", v.ok.into()), ("args", v.args.into()), ("errors", v.errors.into())])),
        ])
    });
}

#[test]
fn tool_results() {
    run_suite("tool-results", |i| {
        let max = i.at("max").as_f64().map_or(RESULT_MAX_CHARS, |m| m as usize);
        serialize_result(i.get("value"), max).into()
    });
}

#[test]
fn permissions() {
    run_suite("permissions", |i| {
        let t = i.at("tool");
        let name = str_of(t.at("name"));
        let toolset = str_of(t.at("toolset"));
        let effect = Effect::parse(t.at("effect").as_str().unwrap_or("")).expect("effect");
        let subject =
            Subject { name: &name, effect, toolsets: if toolset.is_empty() { vec![] } else { vec![toolset.as_str()] } };
        let mut problems = Vec::new();
        let policies: Vec<_> = i
            .at("policies")
            .as_array()
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|p| normalize_policy(p, &mut problems))
            .collect();
        let policy = merge_policies(&policies);
        let decision = decide_permission(&subject, &policy);
        let s = i.at("settings");
        let settings = ConfirmSettings {
            confirm_writes: s.at("confirmWrites") != &Value::Bool(false),
            confirm_destructive: s.at("confirmDestructive") != &Value::Bool(false),
        };
        let confirm = match decision {
            Some(Decision::Deny) => Value::Null,
            d => needs_confirmation(effect, d, settings, bool_or(i.at("allowedForChat"), false)).into(),
        };
        obj(vec![("decision", decision.map_or("", Decision::as_str).into()), ("confirm", confirm)])
    });
}

#[test]
fn effects_annotations() {
    run_suite("effects-annotations", |i| match i.at("effect").as_str() {
        Some(e) => obj(vec![("annotations", annotations_for(Effect::parse(e).expect("effect"), None).into())]),
        None => obj(vec![("effect", effect_from_annotations(i.get("annotations")).map_or("", Effect::as_str).into())]),
    });
}

#[test]
fn frontmatter() {
    run_suite("frontmatter", |i| parse_frontmatter(&str_of(i.at("text"))).to_value());
}

#[test]
fn skills() {
    run_suite("skills", |i| {
        if let Some(paths) = i.at("paths").as_array() {
            return Value::Array(
                paths
                    .iter()
                    .map(|p| {
                        let path = str_of(p);
                        match skill_file_path(&path) {
                            Ok(ok) => obj(vec![("path", path.into()), ("ok", ok.into())]),
                            Err(e) => obj(vec![("path", path.into()), ("error", e.into())]),
                        }
                    })
                    .collect(),
            );
        }
        if let Some(commands) = i.at("commands").as_array() {
            let names: Vec<String> =
                i.at("names").as_array().map(|n| n.iter().map(str_of).collect()).unwrap_or_default();
            return Value::Array(
                commands
                    .iter()
                    .map(|c| {
                        let command = parse_slash_command(&str_of(c), &names)
                            .map_or(Value::Null, |s| obj(vec![("name", s.name.into()), ("rest", s.rest.into())]));
                        obj(vec![("text", c.clone()), ("command", command)])
                    })
                    .collect(),
            );
        }
        let r = parse_skill(&str_of(i.at("text")), "", &str_of(i.at("folder")), "app");
        let skill = r.skill.map_or(Value::Null, |s| {
            let mut v = s.to_value();
            if let Some(m) = v.as_object_mut() {
                m.remove("base");
            }
            v
        });
        obj(vec![("skill", skill), ("problems", r.problems.into())])
    });
}

#[test]
fn agents() {
    run_suite("agents", |i| {
        if let Some(picks) = i.at("pick").as_array() {
            return Value::Array(
                picks
                    .iter()
                    .map(|p| {
                        let agents: Vec<Agent> = p
                            .at("agents")
                            .as_array()
                            .map_or(&[][..], Vec::as_slice)
                            .iter()
                            .map(|n| {
                                let n = str_of(n);
                                Agent { name: n.replace('*', ""), default: n.ends_with('*'), ..Default::default() }
                            })
                            .collect();
                        opt(pick_agent(&agents, &str_of(p.at("wanted"))).map(|a| a.name.clone()))
                    })
                    .collect(),
            );
        }
        parse_agent(&str_of(i.at("text")), &str_of(i.at("file"))).to_value()
    });
}

#[test]
fn memory() {
    run_suite("memory", |i| {
        if let Some(p) = i.get("prompt") {
            let items: Vec<MemoryNote> = p
                .at("items")
                .as_array()
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|m| MemoryNote { id: str_of(m.at("id")), text: str_of(m.at("text")), ..Default::default() })
                .collect();
            return build_memory_prompt(&items, bool_or(p.at("enabled"), true), bool_or(p.at("canWrite"), false))
                .into();
        }
        if let Some(t) = i.get("text") {
            return memory_text(&t.js_string(), MEMORY_CHARS).into();
        }
        let now = || parse_date("2026-06-01T00:00:00.000Z").unwrap_or(0.0);
        let notes = parse_memory_file(
            i.at("json"),
            MEMORY_MAX,
            NoteOptions { max_chars: MEMORY_CHARS, source: "app", now: &now },
        );
        Value::Array(notes.iter().map(MemoryNote::to_value).collect())
    });
}
