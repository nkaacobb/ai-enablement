//! Every vector file in `spec/conformance/` has a runner here, and every runner has its file: a suite added to the JS
//! export fails this test until the Rust runtime runs it too.

mod common;

/// The suites the three runner files cover: `conformance.rs` (pure functions), `conformance_requests.rs` (requests,
/// adapters), `conformance_session.rs` (the session).
const COVERED: [&str; 24] = [
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
    "request-messages",
    "tool-set",
    "text-tools",
    "capability-prompts",
    "adapter-requests",
    "adapter-streams",
    "execute-tool",
    "tool-loop",
];

#[test]
fn every_vector_file_has_a_runner() {
    let Some(dir) = common::spec_dir() else {
        eprintln!("coverage: skipped (no spec/conformance folder)");
        return;
    };
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .expect("spec/conformance")
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".json").map(str::to_string))
        .collect();
    files.sort();
    let mut covered: Vec<String> = COVERED.iter().map(|s| s.to_string()).collect();
    covered.sort();
    assert_eq!(files, covered, "spec/conformance and the Rust runners disagree");
}
