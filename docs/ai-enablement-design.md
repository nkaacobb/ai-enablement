# AI Enablement — design notes and plan

This is the maintainers' record of how `add-ai-skill` became `ai-enablement`: what the old skill did, which of its
mechanisms were kept and generalised, the decisions taken, and what is left for later phases. The skill itself lives in
[`ai-enablement/`](../ai-enablement/) (its `SKILL.md` is what coding agents read); this file is not installed with it.

Direction: [the AI Enablement directional spec](#the-directional-spec-in-brief) — evolve the skill that builds one AI
agent drawer into an app into a framework manager for an application's whole AI layer (tools, toolsets, skills,
agents, context, memory, permissions), idempotent and upgradeable, standards-aligned (Agent Skills, MCP, Claude
Code / Copilot-style agent files), without breaking apps that already have the drawer.

---

## 1. What `add-ai-skill` 1.6 was (the survey)

### What it installed into an app

| Piece | Where in the app | Who owns it | How it got there |
| --- | --- | --- | --- |
| Runtime (`ai-agent/`: `ai-agent.js`, `.css`, `.d.ts`, `core/`, `adapters/`, `ui/`) | next to static assets (`public/ai-agent/`, `assets/ai-agent/`, `src/lib/ai-agent/`) | **framework** | copied unchanged; configured only through options |
| Relay (`relay.php` or `relay.mjs`) | next to the app's endpoints | **framework** | copied unchanged |
| Relay config (`relay.config.php` / `.json` / `.mjs`) | outside the web root (`AIA_RELAY_DIR`) or beside the relay | app | written from `relay.config.example.php` |
| Integration module (`ai-agent-setup.js`) | the app's JS | app | written per app: `createAiAgent({...})` once, loaded with `import()` |
| Content builders (`ai-content.js`) | the app's JS | app | written per app: pure, unit-tested |
| Tools module (`ai-tools.js`) | the app's JS | app | written per app over the app's own functions |
| Tool defaults (`ai-tools.json`) | beside the tools module | app | written per app (the format Settings > Tools exports) |
| Starting memories (`ai-memory.json`) | beside the tools module | app | written per app (the format Settings > Memory exports) |
| Theme / layout CSS (`--aia-*`, `html.aia-drawer-open`) | the app's CSS | app | written per app |
| Integration record (`ai-agent.integration.json`) | app root | app (written by the skill) | written at the end of every run |

### What it detected (`scripts/detect.mjs`, read-only)

- Runtime copies (`ai-agent.js` with `export const VERSION` and `createAiAgent`), their version, and whether each file
  is unchanged since that release — by comparing normalised sha256 fingerprints with `scripts/release-hashes.json`.
- Relays (by the `ai-agent-drawer` marker), their version, 1.0-style edits inside the file, config files (names only).
- `createAiAgent({...})` call sites, their `appId` and top-level options.
- The record, `ai-tools.json`, `ai-memory.json`.
- Per feature (tools, memory, vision, attachments): is it in the installed runtime, and does the integration use it.
- Workarounds a newer runtime makes redundant; host requirements (canvas/WebGL, CSP, Permissions-Policy).
- A status: `none` → build, `upgrade`, `current`, `integration-without-runtime`.

### What it updated and preserved (`references/upgrading.md`)

- **Replaced wholesale**: the runtime folder and the relay file — only when detect says they are unchanged copies; an
  edited copy is diffed against the app's history and the edit turned into an option or kept as a documented patch.
- **Migrated**: relay 1.0 constants → config file.
- **Removed**: workarounds the new runtime covers (a table per version).
- **Added piece by piece**: features the app does not use yet, offered as a menu; declined ones recorded.
- **Preserved**: `appId` (it namespaces users' settings, keys and saved chats), hooks, prompts, tools, tool config,
  memories, CSS customisations, users' browser data.

### What the skill itself owned

The release history (`CHANGELOG.md` with *Upgrading* notes per version), the fingerprints, the reference
integration (Hello World), the verification tooling (`verify.mjs`), and the tests.

## 2. The mechanisms worth keeping

| Mechanism | Generalised into |
| --- | --- |
| Framework-owned files are copied unchanged and fingerprinted per release | The **ownership rule** of the framework: everything inside the runtime folder and the relay files is framework-owned (replaced when unchanged, reconciled when edited); everything else is app-owned and only ever added to or edited in place. Built-in capabilities (skills, tools) ship *inside* the runtime folder so they upgrade with it. |
| `detect.mjs` statuses + per-feature "in runtime / used" lines | The **lifecycle router**: none → install · older → upgrade · current → validate · capability missing → add · edited framework file → reconcile. Detect now also reports the capability folder, the manifest and framework adoption. |
| The integration record | The **manifest** `ai-enablement.json` (same role, wider: runtime, relay, capability folder, integration files, features, declined). The old record is still read, and is renamed on upgrade. |
| The tool registry (`core/tools.js`, `toolRegistry` in `ai-agent.js`) | **Registries** for tools, toolsets, skills and agents, filled from options and from a capability index the runtime loads. |
| Built-in tools (`remember`, `forget`, `take_screenshot`) with their own settings | The pattern for built-in **capabilities**: `use_skill`, `read_skill_file`, and the workspace tools are built-ins too. |
| `effect: read / write / destructive` + confirmation settings | The **permission categories** (adds `external`, `system`) and an allow / ask / deny **policy** per app and per agent. |
| `ai-tools.json`, `ai-memory.json` | Unchanged formats; in new installs they live in the capability folder and are named by its index. |
| Context hooks (`app`, `page.content` hashed, `page.view` volatile) and the hash-based sync with its flag | Kept as they are — the proven core. Agents choose which layers they receive. Application-defined context providers are phase 2. |
| Prompt sections (`== TOOLS ==`, `== MEMORY ==`) | `== AGENT ==` and `== SKILLS ==` sections follow the same pattern. |

## 3. Directory structure: the spec's sketch vs. what was chosen

The spec sketches `ai/{agents,skills,tools,toolsets,resources,prompts,context,memory}` for *what* the app can do and
`src/ai/{runtime,providers,registries,integration}` for *how*. The existing skill already separates the two — the
runtime folder (copied, framework-owned) versus the app's integration files — so the choice was to keep that split
and give the *what* side a home:

```
<app>/
  ai-enablement.json             manifest (development-time; committed; never secrets)
  <static>/ai-agent/             runtime — "how" (framework-owned, unchanged: providers, registries, UI, built-ins)
  <static>/ai/                   capabilities — "what" (app-owned)
    index.json                   the capability index the runtime loads
    tools/*.js                   tool modules (a tool, a list, or a toolset per module)
    toolsets/*.json              optional toolsets that group tools defined elsewhere
    skills/<name>/SKILL.md       Agent Skills (+ references/, scripts/, assets/)
    agents/<name>.md             agent definitions (Markdown + YAML frontmatter)
    ai-tools.json                which tools start on (unchanged format)
    ai-memory.json               starting memories (unchanged format)
  <app js>/ai-agent-setup.js     integration — mounts the agent, passes `host` and the context hooks
```

- `ai/` must be **served** (the runtime fetches Markdown and JSON, and imports tool modules with `import()`); in
  bundled apps it goes under `public/`, or the integration imports tool modules statically and passes them inline.
- `providers/`, `registries/` and `runtime/` from the sketch are the runtime folder's `adapters/` and `core/`; they
  are not re-split, because the runtime is copied as one unit and its internal layout is not the app's concern.
- `resources/`, `prompts/`, `context/` (as files) are reserved names in the index format but not implemented in
  phase 1 (see §6). Memory stays `ai-memory.json` + `memorySave`.
- Existing apps keep their layout. Adopting `ai/` is offered, never forced; `ai-tools.js` passed inline keeps working.
- Development-time capabilities (`.claude/skills`, `.claude/agents`, `.github/agents`, `.agents/skills`) are a
  different thing from the app's `ai/skills` and `ai/agents`; detect and validate never mix them up.

## 4. Decisions

**Name and compatibility.** Skill `ai-enablement` 2.0.0 (the folder, the installed name). The runtime keeps its file
and API names (`ai-agent.js`, `createAiAgent`, `aia-` CSS, `ai-tools.json`, `ai-memory.json`) and the relay wire
identifiers (`X-Requested-With: ai-agent-drawer`, `"relay": "ai-agent-drawer"`), exactly as the 1.1 rename did, so
every installed app and relay keeps working and upgrades by copying the runtime folder. Runtime 1.6.0 is additive:
every 1.5 option and method keeps its behaviour. The installer no longer deletes other installed skills — the old
`add-ai-skill` copies stay until removed on purpose (`-RemoveLegacy`).

**Tools.** A tool stays one object: `name`, `title`, `description`, input contract, effect, handler. The input
contract is either the existing shorthand (`parameters: { q: { type: 'string', maxLength } }`) or a JSON Schema
`inputSchema` as in MCP (the supported subset converts to the shorthand; unsupported constructs are refused with the
reason, never silently loosened). `annotations` (MCP hints) are accepted and derived; `agent.tools.mcp()` exports the
catalog as MCP tool descriptors. The handler is `run(args, { host, agent, signal, call })`: `host` is the object the
integration passes (`createAiAgent({ host })`), so tool modules in `ai/tools/` need no closures over app globals and
the application keeps the business logic. Return values are structured (objects become JSON for the model).

**Toolsets.** A module may export a toolset (`{ name, title, description, tools: [...] }`), or a JSON file may group
tools defined elsewhere. Agents reference them with `toolsets:`; permission rules with `toolset:<name>`; Settings >
Tools groups by them.

**Permissions.** Effects `read`, `write`, `destructive`, `external` (sends something outside the app), `system`
(changes the app itself, e.g. writes code). A policy has `allow`, `ask`, `deny` rule lists (tool names, `*`
wildcards, `toolset:<name>`, `effect:<effect>`), from the app (index or option) and the active agent. Precedence
deny > ask > allow > the user's confirmation settings. `deny` hides a tool from the model and the user cannot turn it
on from the chat; `ask` always confirms; `allow` skips the confirmation card. `system` tools always confirm — no
setting or `allow` rule switches that off. Deliberately small: no per-argument rules yet.

**Skills.** The open Agent Skills format, unchanged: `SKILL.md` with `name` + `description` frontmatter (optional
`license`, `compatibility`, `metadata`, `allowed-tools`), body = instructions, optional `references/`, `scripts/`,
`assets/`. Progressive disclosure as the format intends: names and descriptions in the system prompt; the model calls
`use_skill` to load one, which then stays in the system prompt for the rest of that conversation (saved with the
chat); `read_skill_file` reads a file the skill points to. The user can also start a message with `/skill-name`.
`allowed-tools` is read and shown to the model, not used to skip confirmations (that would let a text file loosen
the app's safety rules). `scripts/` are never executed in the browser.

**Agents.** One Markdown file per agent with YAML frontmatter (the Claude Code / Copilot convention): `name`,
`description`, `title`, `tools`, `toolsets`, `skills`, `permissions`, `context`, `memory`, `maxToolSteps`, `model`
(a hint, not applied in phase 1), `welcome`, `suggestions`; the body is the agent's instructions. An agent composes —
it never defines tools or skills. Omitted lists mean "all". The drawer shows a picker when there are two or more;
switching starts a new chat; saved chats remember their agent. Without agent files the app has one implicit agent
(the 1.x behaviour).

**Capability index.** `ai/index.json` lists paths (or inline objects) for `agents`, `skills`, `tools`, `toolsets`,
plus `toolsConfig`, `memory`, `permissions`, `defaultAgent`. Paths keep the layout flexible (an app may keep
`agents/x/AGENT.md`). A browser cannot list folders, and an index is also what tooling (validate, a future MCP
bridge) reads without running the app.

**Manifest.** `ai-enablement.json` replaces `ai-agent.integration.json` (read when present; renamed on upgrade): the
old fields, plus `framework` (runtime and relay paths and versions — the framework-owned set) and `capabilities`
(the index path and what it holds). Fingerprints stay in the skill (`release-hashes.json`); the manifest names
paths, so reconciliation is: framework file unchanged → replace; edited → reconcile by hand; app file → never
replaced.

**In-app authoring (the motivating idea).** The in-app agent can add a tool to the application while a developer is
working on it: a dev-only *workspace* server (`scripts/workspace.mjs`, part of the skill, never copied into the app)
lets the page list and read the app's source and write files inside the capability folder only. The runtime offers
built-in tools when — and only when — that server answers (`describe_host`, `list_source_files`,
`read_source_file`, `search_source`, `write_ai_file` with effect `system`, `reload_capabilities`) and a built-in
skill, `create-tool`, that teaches the in-app agent the tool format and conventions. Every write waits for the user:
a new file is shown whole, a replacement as a diff; creating a file that exists is refused (replacing takes an
explicit `replace: true`); a module that could not load is refused with the fix; the reload reports unregistered
files and misnamed tool-config keys. The new tool is loaded without a page reload. The same operations are available to coding agents through the skill
(`references/capabilities.md`, `scripts/scaffold.mjs`).

## 5. Phase 1 (skill 2.0.0, runtime 1.6.0) — delivered

1. Rename to `ai-enablement`; installers that leave other installed skills alone.
2. Runtime: `core/frontmatter.js`, `core/schema.js` (JSON Schema ↔ fields, MCP descriptors), `core/permissions.js`,
   `core/skills.js`, `core/agents.js`, `core/capabilities.js` (index loading, registries), `core/workspace.js`;
   tools with `inputSchema` / `annotations` / `host` / new effects; the agent picker, active skills, permission
   enforcement, built-in skill and workspace tools; Settings shows agents, skills, toolsets and blocked tools.
3. Skill: `SKILL.md` as a lifecycle router; `references/framework.md` (concepts and formats),
   `references/capabilities.md` (create tool / toolset / skill / agent / context / memory),
   `references/in-app-authoring.md`; templates in `assets/templates/ai/`.
4. Scripts: `detect.mjs` (capability folder, manifest, legacy record, adoption, dev-time folders ignored, relay
   version compared with the relay's own release), `validate.mjs` (capability folder and references), `scaffold.mjs`
   (create a capability from a template and register it), `workspace.mjs` (dev server), `verify.mjs` (capabilities).
5. Hello World on the framework: `ai/index.json`, three toolset modules, two skills, two agents, permissions.
6. Tests for every new module, the browser flows (agent picker, skills, permissions, workspace authoring), detect,
   validate, scaffold and the workspace server.
7. Tried live with LM Studio and `qwen/qwen3.5-9b` (a 9B local model): the in-app agent turned "a tool that tells me
   the file name and whether there are unsaved changes" and "…where the cursor is" into working tools. The early runs
   are why `describe_host`, the create/replace distinction, the diff, the module pre-checks and the reload hints
   exist: the model guessed `host`'s shape, rewrote an existing toolset, keyed the tool config by the toolset name.

## 6. Later phases

| Phase | What | Notes |
| --- | --- | --- |
| 2 | **Context providers**: named sources registered by the app (`selection`, `document`, `user`) that feed the same hash-based sync (hashed) or the view state (volatile); agents pick them. | Generalises `page.content` / `page.view` without changing the protocol: one snapshot block, one hash. |
| 2 | **Prompts** (MCP prompts): `ai/prompts/*.md` templates shown as suggestions and `/name` commands. | Index key reserved. |
| 2 | **Model preferences**: apply an agent's `model` when the user allows it. | Currently a hint. |
| 3 | **Resources** (MCP resources): read-only data the agent can pull (`resources/list`, `resources/read`). | Index key reserved. |
| 3 | **MCP bridge**: expose the catalog through an MCP server (stdio or streamable HTTP) for external clients. | `agent.tools.mcp()` already emits the descriptors; the workspace server is the natural host. |
| 3 | **Memory providers**: memory beyond the browser and `memorySave` (per user, server-side, scoped per agent). | |
| 3 | **Multi-agent**: an agent handing a sub-task to another (`delegate` built-in). | |
| 3 | **Finer permissions**: per-argument rules, per-user policy from the app's backend. | |

## 7. Several stacks (skill 3.0)

**Why.** The skill was written for web apps. Run on a native Rust app (KAROS Therapy Session Intelligence: egui/eframe
0.37, a Cargo workspace), detect said "No AI agent found: INSTALL — follow the normal workflow". The workflow would
have copied a DOM runtime into an egui window. The same holds for a WinForms app. The audit showed the skill was
already two things that nothing named:

- **A portable framework**: the capability folder, effects and permissions, the context-sync protocol, prompt
  assembly, the lifecycle, the ownership rule, the manifest.
- **A web implementation**: the JS/DOM runtime, JS tool modules, CDP verification, CSS layout guards, the PHP/Node
  relay, and detection by JS markers.

**Decisions.**

| Decision | Choice |
| --- | --- |
| One skill or several | One skill, three layers. `spec/` is the portable contract. `stacks/<id>/STACK.md` holds one recipe per stack, numbered like SKILL.md's steps. There is one runtime per stack. Separate skills would duplicate the lifecycle, the formats and the manifest, and drift. |
| Stacks | `web` (hosts: static, php, node, **tauri**, **aspnet**), `rust-egui`, `dotnet-winforms`. Tauri and ASP.NET/Blazor render HTML, so they use the JS runtime; only their `host` binding and transport differ. WPF, WinUI, MAUI and other Rust UIs are recognised, with no recipe yet. |
| Native UI | A native drawer per language, as a no-UI core library plus a thin UI crate/project (egui panel, WinForms `UserControl`). Hosting the web drawer in a webview was rejected: it is awkward in egui, looks foreign, and fights native keyboard and DPI handling. |
| Delivery | Vendored copies (`crates/ai-enablement-*`, `src/AiEnablement.*`), framework-owned and fingerprinted like `assets/ai-agent/`. The crate takes `egui = { workspace = true }`, so it always matches the app's egui. |
| Tools in compiled apps | Code: contract and handler together, registered inline. A test in the app generates `ai/tools.catalog.json` and fails when it is stale. `validate.mjs` reads it, so references are checked without compiling (`spec/tool-catalog.md`). |
| Same product | `spec/behaviour.md`, and frozen conformance vectors (`spec/conformance/`, 16 suites) that every runtime runs. The JS runtime is the reference. |
| Policy hooks | Native runtimes take a provider policy (for example loopback only), a `before_send` gate (consent), and settings, key, memory and chat stores (or none), plus `reset()` on lock. KAROS needs all of them: its spec forbids cloud inference, requires consent before patient text reaches a model, keeps all patient data in an encrypted vault, and (today) forbids model tool calling. The portable survey gained a **policy constraints** item, and a plan that would break one stops and asks. |
| References | Only the portable docs moved (`framework.md`, `context-sync.md` → `spec/`). The web references stay in `references/`, because runtime files mention those paths, and editing a runtime file changes its release fingerprint. |

**Delivered in 3.0.0 (phase A).**

- Stack detection and routing: STOP on planned stacks, build output skipped.
- SKILL.md as a router; the web, rust-egui and dotnet-winforms recipes.
- `spec/behaviour.md`, `spec/tool-catalog.md`, the conformance vectors with their export and test.
- A tool-catalog-aware `validate`, a stack-aware `scaffold`, `"stack"` in the manifest, installers that skip build
  output.
- No runtime change.

**Delivered in 3.1.0 (phase B).**

- **The session out of the drawer.** `core/session.js` (runtime 1.7.0, additive) owns the active agent, the request,
  the tool loop, the built-ins and saved chats. The drawer is a view over it. Cards are data, `{ kind, buttons, tool,
  confirm }`, and the drawer writes their questions. The public API is unchanged, and the browser tests pass
  unchanged.
- **8 new conformance suites**, 24 in all: `request-messages`, `tool-set`, `text-tools`, `capability-prompts`,
  `adapter-requests`, `adapter-streams`, `execute-tool` and `tool-loop`. The last two drive the session through a
  scripted harness: scripted tools, model and clicks, and a fixed clock. `tool-loop` records every request exactly as
  sent, so the neutral request is the contract between a session and its adapters.
- **`assets/rust/ai-enablement-core`**, which passes all 24 suites.
  - The tool loop is straight-line code over two traits: `View` (its `decide` may block) and `ToolRunner` (runs
    the app's tools where the host is). A drawer runs a question on a worker thread and answers both from the UI
    thread, so tool handlers keep `&mut Host` without locks, and the session is not generic over the host.
  - Host rules: `ProviderPolicy` and a `before_send` gate, via `GuardedModel`.
  - The capability folder: embedded with `embedded_files!`, or read from disk.
  - HTTP behind cargo features (ureq): `http`, `tls`, and `tls-platform` for TLS-inspecting antivirus.
  - Exit met: `cargo test` runs every vector (`tests/coverage.rs` keeps it so), and with `--features http` a question
    with a tool round runs over HTTP against `tests/fixtures/fake-upstream.mjs` (through `fake-upstream-cli.mjs`).
  - The crate keeps JSON key order itself and never enables serde_json's `preserve_order`: feature unification
    would reorder the host app's JSON, and KAROS hashes canonical JSON.
- Not yet: release fingerprints for the Rust family (with the first installable release, phase C), and in-app
  authoring on native stacks (phase F).

**Next.**

| Phase | What | Exit |
| --- | --- | --- |
| B ✓ | **Rust core** (`assets/rust/ai-enablement-core`), delivered in 3.1.0: context sync, prompt, tool registry and argument validation, permissions, the capability loader (embedded or disk), skills, agents, memory, the OpenAI-compatible adapter, storage and policy traits; then Anthropic and Gemini. Starts by extracting the tool loop from `ui/drawer.js` into a DOM-free `core/session.js` (runtime 1.7.0, additive), and by adding vectors for adapters, request messages and tool-loop decisions. | `cargo test` passes every vector; the tool loop runs against `tests/fixtures/fake-upstream.mjs`. |
| C | **egui drawer** (`ai-enablement-egui`) and `examples/hello-world-egui` (the same `ai/` skills and agents, Rust tools). The rust-egui recipe goes from planned to supported (detect flips it automatically when `assets/rust/ai-enablement-egui/Cargo.toml` exists). | A kittest flow at 1920×1080; live with LM Studio. |
| D | **KAROS pilot**, after its spec decision: a context-only assistant, or an amendment allowing in-app read tools plus proposal-based writes. | `cargo test --workspace`; the drawer screenshot reviewed. |
| E | **.NET core + WinForms panel**, against the same vectors. | `dotnet test`; the example maximised at 1920×1080. |
| F | Data-only in-app authoring for native stacks; WPF; an optional relay adapter for native apps; publishing crates and packages if vendoring becomes a burden. | |

## The directional spec, in brief

Tools (machine-readable contracts, validated inputs, structured outputs, MCP/JSON Schema compatible), toolsets,
skills (Agent Skills), agents (compose instructions, skills, tools, context, memory, permissions, limits, model
preference), an `ai/` capability area separate from runtime code, development-time vs application capabilities kept
apart, a lifecycle (install → upgrade → validate → add → reconcile, preserving customisation), capability creation
("add a tool that lets the AI inspect the current track"), tools as first-class components with the framework owning
discovery, schemas, validation, invocation, errors, permissions, timeouts, logging and result formatting, permission
categories (read, write, destructive, external, system → allowed / denied / ask), context generalised, provider
independence, framework-manager upgrades with manifests, no breakage for existing apps, and a first milestone of
standardised skills + first-class tools + basic agents + registries + safe reconciliation.
