//! One whole question over real HTTP: the session, the OpenAI-compatible adapter and the ureq client against the skill's
//! fake upstream (`tests/fixtures/fake-upstream-cli.mjs`, an OpenAI-compatible server that streams scripted replies and
//! records what it receives), behind a loopback-only policy. The model calls a tool, gets its result, and answers.
//!
//! Needs the `http` feature and Node; a vendored copy without the skill's fixtures says so and passes.
#![cfg(feature = "http")]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;

use ai_enablement_core::client::HttpModel;
use ai_enablement_core::context::{ContextManager, Page, Source};
use ai_enablement_core::http::UreqClient;
use ai_enablement_core::json::{Map, Value};
use ai_enablement_core::permissions::normalize_policy;
use ai_enablement_core::policy::{GuardedModel, ProviderPolicy};
use ai_enablement_core::session::{Card, RowId, Session, ToolRunner, View};
use ai_enablement_core::settings::Settings;
use ai_enablement_core::tools::{normalize_tool, ToolContract};
use ai_enablement_core::transport::{CancelToken, ErrorCode};

fn fixture() -> Option<PathBuf> {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../tests/fixtures/fake-upstream-cli.mjs");
    p.is_file().then_some(p)
}

/// The fake upstream: its URL, and a way to stop it and read every request it received.
struct Upstream {
    child: std::process::Child,
    lines: std::io::Lines<BufReader<std::process::ChildStdout>>,
    url: String,
}

impl Upstream {
    fn start(plan: &str) -> Option<Upstream> {
        let script = fixture()?;
        let mut child = Command::new("node")
            .arg(script)
            .arg(plan)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| eprintln!("fake upstream: skipped (Node could not be started: {e})"))
            .ok()?;
        let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
        let url = lines.next()?.ok()?;
        Some(Upstream { child, lines, url })
    }

    /// Stop it; the POST requests it received (their JSON bodies).
    fn stop(mut self) -> Vec<Value> {
        let _ = self.child.stdin.take().map(|mut s| s.flush());
        let all = self.lines.next().and_then(|l| l.ok()).and_then(|l| Value::parse(&l).ok()).unwrap_or(Value::Null);
        let _ = self.child.wait();
        all.as_array()
            .map(|a| {
                a.iter().filter(|r| r.at("method").as_str() == Some("POST")).map(|r| r.at("body").clone()).collect()
            })
            .unwrap_or_default()
    }
}

struct Approve(Vec<String>);
impl View for Approve {
    fn row(&mut self, title: &str, _call: &str) -> RowId {
        self.0.push(title.to_string());
        self.0.len() - 1
    }
    fn decide(&mut self, _row: RowId, _card: &Card, _cancel: &CancelToken) -> String {
        "run".into()
    }
}

struct Tools(Vec<String>);
impl ToolRunner for Tools {
    fn run(&mut self, tool: &ToolContract, args: &Map, _id: &str, _cancel: &CancelToken) -> Result<Value, String> {
        self.0.push(format!("{}({})", tool.name, Value::Object(args.clone())));
        Ok(Value::from("Found 2 matches."))
    }
}

fn session(url: &str) -> Session {
    let mut ctx = ContextManager::new();
    ctx.set_page(Some(Page::new("editor", "Editor").content(Source::Value("The quick brown fox.".into()))));
    let settings = Settings::from_json(
        &Value::parse(&format!(
            r#"{{"provider":"lmstudio","profiles":{{"lmstudio":{{"baseUrl":"{url}","model":"fake-model"}}}}}}"#
        ))
        .unwrap(),
    );
    let mut s = Session::new(ctx, settings);
    for def in [
        r#"{"name":"find_text","description":"Find text.","effect":"read","enabled":true,"parameters":{"query":{"type":"string","required":true}}}"#,
        r#"{"name":"delete_doc","description":"Delete the document.","effect":"destructive","enabled":true}"#,
    ] {
        s.tools.push(normalize_tool(&Value::parse(def).unwrap()).unwrap());
    }
    s.policy = normalize_policy(&Value::parse(r#"{"deny":["delete_*"]}"#).unwrap(), &mut Vec::new());
    s.init();
    s
}

#[test]
fn a_question_with_a_tool_round_over_http() {
    let plan = r#"{"replies":[{"toolCalls":[{"id":"c1","name":"find_text","arguments":{"query":"fox"}}]},{"text":["Two"," matches."]}]}"#;
    let Some(upstream) = Upstream::start(plan) else {
        eprintln!("fake upstream: skipped (no tests/fixtures/fake-upstream-cli.mjs or no Node)");
        return;
    };
    let mut s = session(&upstream.url);
    let mut model =
        GuardedModel::new(HttpModel::new(Arc::new(UreqClient::new()), |_| String::new()), ProviderPolicy::LoopbackOnly);
    let mut view = Approve(Vec::new());
    let mut tools = Tools(Vec::new());
    let (end, error, reply) = s.ask("Find fox.", &mut model, &mut view, &mut tools, &CancelToken::new());
    let requests = upstream.stop();

    assert!(error.is_none(), "{error:?}");
    assert!(end.kept);
    assert_eq!(end.answer, "Two matches.");
    assert_eq!(reply.raw, "Two matches.");
    assert_eq!(tools.0, [r#"find_text({"query":"fox"})"#]);
    assert_eq!(end.meta, "LM Studio (local) · fake-model");
    let last = s.conv.messages.last().unwrap();
    assert_eq!(last.actions.len(), 1);
    assert_eq!(last.actions[0].status, "ok");

    assert_eq!(requests.len(), 2, "two requests: the question, then the tool result");
    let first = &requests[0];
    assert_eq!(first.at("model").as_str(), Some("fake-model"));
    let offered: Vec<&str> =
        first.at("tools").as_array().unwrap().iter().filter_map(|t| t.at("function").at("name").as_str()).collect();
    assert_eq!(offered, ["find_text"], "a denied tool never reaches the request");
    let question = first.at("messages").as_array().unwrap().last().unwrap().at("content").as_str().unwrap().to_string();
    assert!(question.contains("<page_snapshot page=\"editor\"") && question.ends_with("Find fox."), "{question}");
    let second = requests[1].at("messages").as_array().unwrap();
    let tool_msg = second.iter().find(|m| m.at("role").as_str() == Some("tool")).expect("the tool result");
    assert_eq!(tool_msg.at("tool_call_id").as_str(), Some("c1"));
    assert_eq!(tool_msg.at("content").as_str(), Some("Found 2 matches."));
}

#[test]
fn a_cloud_provider_is_refused_before_any_request() {
    let mut s = session("http://127.0.0.1:9");
    s.settings.provider = "openai".into();
    let mut model = GuardedModel::new(
        HttpModel::new(Arc::new(UreqClient::new()), |_| "sk-test-key-123456".into()),
        ProviderPolicy::LoopbackOnly,
    );
    let (end, error, _) =
        s.ask("Hello?", &mut model, &mut Approve(Vec::new()), &mut Tools(Vec::new()), &CancelToken::new());
    assert!(!end.kept);
    assert_eq!(error.map(|e| e.code), Some(ErrorCode::Refused));
    assert!(s.conv.messages.is_empty(), "the refused question leaves no trace");
}
