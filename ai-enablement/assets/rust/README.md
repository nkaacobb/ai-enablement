# The Rust runtime

`ai-enablement-core` is the agent without a UI, for native Rust apps (egui/eframe). It is the same agent as the JS
runtime (`assets/ai-agent/`): it passes the conformance vectors in `spec/conformance/` and follows
`spec/behaviour.md`. Where the two disagree, the JS runtime is right; fix the Rust.

The egui drawer (`ai-enablement-egui`) comes next. Until it ships, the `rust-egui` stack stays *planned* and
`detect.mjs` stops before installing.

## Modules

| Module | JS counterpart | What it does |
| --- | --- | --- |
| `json`, `text`, `time`, `hash` | `core/hash.js` | Ordered JSON with JS semantics, UTF-16 lengths, ISO dates, cyrb53. |
| `context`, `conversation` | `core/context.js`, `core/conversation.js` | Snapshots and fingerprints, the sync plan and the flag, request messages, the transcript format. |
| `prompt` | `core/prompt.js` | The system prompt and its protocol texts. |
| `tools`, `schema`, `blocks` | `core/tools.js`, `core/schema.js`, `core/blocks.js` | Tool contracts, arguments, the tools of a request, text mode, results, MCP descriptors. |
| `permissions` | `core/permissions.js` | Allow, ask and deny rules, and confirmations. |
| `frontmatter`, `skills`, `agents`, `memory` | the same files | Formats, prompts and the memory store. |
| `capabilities` | `core/capabilities.js` | `index.json` from embedded files (`embedded_files!`) or a folder (`DiskFiles`). |
| `settings`, `providers` | `core/settings.js`, `core/providers.js` | Settings and the provider catalog. |
| `chat`, `reasoning`, `sse`, `transport` | `core/messages.js`, `core/reasoning.js`, `core/transport.js` | The neutral request, `<think>` splitting, SSE, errors, redaction, `HttpClient`. |
| `adapters`, `client`, `http` | `adapters/*.js`, `core/client.js` | The four protocols, the fallback provider, `HttpModel`, and an optional ureq client. |
| `session` | `core/session.js` | The active agent, the request, the tool loop with its cards, the built-ins, saved chats. |
| `policy` | — | Host rules: `ProviderPolicy` (for example loopback only) and a `before_send` gate, via `GuardedModel`. |

## How a native drawer drives the session

The tool loop is straight-line code, as in JS. It calls two traits, and either may block.

- `View`: rows, notices, repainting, and `decide(card)` (ask the user).
- `ToolRunner`: run an application tool with `&mut Host`.

A drawer runs a question on a worker thread (`Session::ask`, or `begin_turn` / `run_model` / `end_turn`) and answers
`decide` and `run` from the UI thread over channels, where the host is. The model streams on that worker. Tool
handlers are synchronous and must return quickly: there is no timeout race as in JS.

Built-in tools run inside the session: `remember`, `forget`, `take_screenshot` (through a `ScreenSource`), `use_skill`
and `read_skill_file` (through a `FileSource`). The development-workspace tools do not exist on native stacks.

## Features

| Feature | What it adds |
| --- | --- |
| (none) | Everything but an HTTP client. Bring your own `transport::HttpClient`. |
| `http` | `http::UreqClient` over plain http, for local servers such as LM Studio and Ollama. |
| `tls` | https, trusting Mozilla's roots (webpki). |
| `tls-platform` | https, trusting the operating system's certificate store. Use it behind antivirus or proxies that inspect TLS: with `tls` alone, cloud calls then fail with `UnknownIssuer`. |

- **MSRV**: 1.80 without features, 1.85 with `http` (ureq's).
- **Timeouts**: ureq applies the timeout to each phase before the response arrives, and as an idle timeout between
  reads of the body, so a reply that keeps streaming is never cut off.
- **Cancelling**: a cancel takes effect within 50 ms, while waiting for the response or mid-stream. The connection
  is then shut down, so the server stops generating.

## Testing

```bash
cd assets/rust
cargo test                    # unit tests, and every conformance suite
cargo test --features http    # also the HTTP client, and the end-to-end question against the fake upstream (Node)
cargo clippy --all-targets --all-features -- -D warnings
cargo fmt --check
```

- `rust-toolchain.toml` pins the toolchain this workspace is developed with. The crate's MSRV is in its
  `Cargo.toml`.
- The vectors are read from `AI_ENABLEMENT_SPEC`, or else from `../../../spec/conformance`. A vendored copy inside
  an app has neither, so its conformance tests print a skip notice and pass.
- The end-to-end test starts `tests/fixtures/fake-upstream-cli.mjs` with Node.

## Vendoring rules

- **Copy `ai-enablement-core/` unchanged** into the app's workspace. It is framework-owned, like `assets/ai-agent/`.
- **Dependencies use caret ranges**, so the copy resolves to the versions the app already locks.
- **Never enable serde_json's `preserve_order` feature**, not even in dev-dependencies. Feature unification would
  change the key order of every `serde_json::Map` in the host app, including JSON it hashes. Key order is kept by
  this crate's own `json::Value`.
- **The crate's `rustfmt.toml`** keeps the app's `cargo fmt --all` from rewriting the copy, so its fingerprint
  survives.
