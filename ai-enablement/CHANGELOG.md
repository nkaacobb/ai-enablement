# Changelog

All notable changes to the ai-enablement skill (named add-ai-skill up to 1.6, ai-agent-drawer before 1.1). It
installs, upgrades and extends an application's AI layer: the agent drawer runtime and the app's capability folder. The
skill's version is in `package.json`; the runtime (`VERSION` in `assets/ai-agent/ai-agent.js`) and the relays
(`AIA_RELAY_VERSION` / `RELAY_VERSION`) carry their own, which only change when their code does.
`scripts/release-hashes.json` fingerprints every released runtime and relay.

## 3.1.0 — the session, and the Rust core (runtime 1.7.0; relays unchanged at 1.4.0)

Phase B of the multi-stack plan (`docs/ai-enablement-design.md`, §7). The JS runtime's tool loop is now a module
without a UI, and a Rust runtime ports it. The two are held together by 8 new conformance suites, 24 in all.

### Runtime 1.7.0 (additive)

- **New `core/session.js`** (`AgentSession`): the agent without its UI. It owns:
  - the active agent, its tools, skills, permissions and context layers;
  - the system prompt and the tools of each request;
  - the tool loop (`runModel`, `executeTool`) and the built-in tools;
  - a question's start and end (`beginTurn`, `endTurn`);
  - the context flag and saved chats.

  It shows things through view hooks and runs headless.
- **`ui/drawer.js` is a view over the session.** It renders the rows and cards the session asks for. A card is data,
  `{ kind, buttons, tool, reason?, decision?, confirm? }`, and the drawer writes its question: the same text as
  before, including the diff shown before a capability file is replaced. Nothing looks or behaves differently, and
  the browser tests pass unchanged.
- `ContextManager` takes an optional clock (`now`), so tests can fix the snapshot time.
- **One visible difference:** the numbers in the agent's own notices and tool messages (the "Max reply tokens" notice,
  `read_skill_file`'s character counts) are written as en-US (`4,096`) on every machine, as `core/tools.js` already
  did. They used the machine's locale before.
- The public API (`createAiAgent`, `ai-agent.d.ts`) is unchanged.

### Conformance: 8 new suites, 24 in all

- `request-messages`: the transcript as a request's messages.
- `tool-set`: what one request offers, and the TOOLS text.
- `text-tools`: text-mode tool blocks.
- `capability-prompts`: the AGENT and SKILLS texts, and an agent's tools and skills.
- `adapter-requests`: what each protocol sends.
- `adapter-streams`: what each protocol reads back, with SSE edge cases and HTTP errors.
- `execute-tool`: one call through every check and card.
- `tool-loop`: one question with its tool rounds against a scripted model. It records every request exactly as sent.

`execute-tool` and `tool-loop` drive the session through a scripted harness, which their descriptions specify so
another runtime builds the same one. The suites live in `scripts/lib/conformance.mjs` (the pure functions) and the new
`scripts/lib/conformance-runtime.mjs` (requests, adapters, the session). `tests/session.test.mjs` covers what the
vectors do not: saved chats, switching agents, reset.

### The Rust core: `assets/rust/ai-enablement-core` 0.1.0

- **The agent without a UI**, a port of the JS core, passing all 24 suites. See `assets/rust/README.md`.
- **Its session** runs the tool loop over two traits:
  - `View`: rows, notices, `decide`;
  - `ToolRunner`: the app's tools, run where the host is.

  A native drawer runs a question on a worker thread and answers both from the UI thread.
- **Host rules**: `ProviderPolicy` (any, loopback only, local only, or listed providers) and a `before_send` gate (for
  example consent), enforced by `GuardedModel` before a request leaves the app.
- **The capability folder** is embedded in the binary (`embedded_files!`) or read from disk.
- **HTTP** comes from an optional ureq client.
  - `http` serves plain http, for local servers.
  - `tls` adds https with Mozilla's roots.
  - `tls-platform` adds https with the operating system's certificate store, needed behind TLS-inspecting
    antivirus.
  - Cancelling stops a request within 50 ms, mid-stream too, and shuts the connection so the server stops
    generating.
- **Tested end to end**: with `--features http`, one question runs through the session, the OpenAI-compatible adapter
  and ureq against the fake upstream. It covers a tool round, a denied tool that is never offered, and a
  loopback-only policy that refuses a cloud provider before any request. `tests/coverage.rs` fails when a vector file
  has no Rust runner.
- **Dependencies** are caret ranges (serde, serde_json, regex, ryu-js). The crate never enables serde_json's
  `preserve_order`: a host app's JSON key order stays its own.
- `tests/fixtures/fake-upstream-cli.mjs` starts the fake upstream for a test in another language (the Rust end-to-end
  question).
- The `rust-egui` stack stays **planned** until the egui drawer ships (phase C). `detect.mjs` still stops before
  installing.

### Upgrading from 3.0.0

1. Web apps: replace the runtime folder (it is unchanged in every app the skill installed). Nothing else changes.
2. Release fingerprints: `scripts/release-hashes.json` has 1.7.0.

## 3.0.0 — one skill, several stacks (runtime unchanged at 1.6.1; relays unchanged at 1.4.0)

The skill was written for web apps. Run on a native app, it said "INSTALL — follow the normal workflow" and would
have copied a browser runtime into an egui or WinForms window, which can't run it. 3.0 makes the tech stack the
first thing every run decides. The skill is split into three parts:

- **the portable contract** (`spec/`), the same on every stack;
- **one recipe per stack** (`stacks/<id>/STACK.md`);
- **one runtime per stack**.

Web apps are unaffected: the runtime, the relays and every API are unchanged. The native runtimes (Rust/egui,
C#/WinForms) are planned. Until they ship, detect says so and stops before installing.

### Stacks

- **`scripts/detect.mjs` names the stack** on its first line. It reads where the stack came from in the files
  (`Cargo.toml` dependencies, `.csproj` UI flags, `tauri.conf.json`, `package.json`, PHP or HTML files) and reports
  whether the skill supports it:

  | Stack | What it covers | Status |
  | --- | --- | --- |
  | `web` (hosts `static`, `php`, `node`, `tauri`, `aspnet`) | HTML pages, including Tauri webviews and ASP.NET Core / Blazor | supported |
  | `rust-egui` | Rust with egui / eframe | planned |
  | `dotnet-winforms` | C# with Windows Forms | planned |
  | `dotnet-wpf`, `dotnet-winui`, `dotnet-maui`, `rust-other` | recognised | no recipe |
- **What "No AI agent found" leads to now depends on the stack.**
  - A supported stack gets INSTALL with its recipe.
  - A planned or unsupported stack gets STOP: never copy the web runtime into a native app, never hand-write a
    drawer, and offer what works on any stack (the survey and plan, and the `ai/` folder's skills, agents, toolsets
    and permissions).
  - An unrecognised folder gets "ask the user".
- **Flagged**: two native UIs in one folder, and a manifest whose `"stack"` disagrees with the files. The manifest's
  stack wins.
- **Build output is skipped** beside the project file that makes it: Cargo's `target/`, and .NET's `bin/` and `obj/`.
  A built Rust workspace used to fill detect's 20,000-file walk. Also skipped: `.vs/`, `.idea/`.
- New `scripts/lib/stacks.mjs` (shared by detect, validate and scaffold).

### SKILL.md and the recipes

- **`SKILL.md` is a stack-neutral router.** Step 0 reads the Stack line first. Steps 1–15 state each step's portable
  rule; *how* is in `stacks/<id>/STACK.md` under the same number. The survey gains a portable **policy constraints**
  item: what the app allows an AI to see and do (consent, local-only models, where data may persist, actions
  reserved for a person). A plan that would break one stops and asks.
- **`stacks/web/STACK.md`**: the web steps that used to be in SKILL.md (code samples, the Quick API), plus new
  **Tauri** (Rust commands as `host` through `invoke`, CSP, a Rust command instead of the relay) and **ASP.NET Core /
  Blazor** (wwwroot, JS interop, the relay alternatives) sections.
- **`stacks/rust-egui/STACK.md`** and **`stacks/dotnet-winforms/STACK.md`** (planned): what to do today, and the
  integration design the native runtimes are being built to, step by step.

### The portable contract: `spec/`

- `references/framework.md` → **`spec/framework.md`**; `references/context-sync.md` → **`spec/context-sync.md`**
  (content unchanged).
- **`spec/behaviour.md`** (new): everything a runtime does, with its limits. Every number was checked against the
  JS runtime.
- **`spec/tool-catalog.md`** (new): the tools of a compiled app as data. A test in the app generates
  `ai/tools.catalog.json` and fails when it is stale; the index names it as `"toolCatalog"`.
- **`spec/conformance/`** (new): 16 suites of frozen input → output vectors from the JS runtime. They cover hash,
  stable JSON, context description, snapshots, sync planning, the snapshot wire format, the system prompt, tool
  contracts, tool arguments, tool results, permissions, effects ↔ MCP annotations, frontmatter, skills, agents and
  memory. `tests/conformance.test.mjs` holds the JS runtime to them, and the Rust and .NET runtimes will run the same
  files. `scripts/conformance-export.mjs [--write]` regenerates them, on purpose only.

### Lifecycle scripts

- **validate.mjs**:
  - Reads the tool catalog and checks against it what it checks for JS modules: contracts, duplicates, toolsets,
    agents, `allowed-tools`, `ai-tools.json`.
  - On a compiled stack without a catalog, references to tools are *unchecked* warnings, not errors, and a warning
    asks for the catalog.
  - Flags stack warnings, and notes a manifest without `"stack"`.
- **scaffold.mjs**:
  - `--stack`, or the nearest manifest's `"stack"`.
  - On a compiled stack, `init` writes an index without tool modules, and `tool` is refused with where tools go
    instead (code, the recipe's §9). Skills, agents and toolsets scaffold as before.
- **Manifest**: `"stack": { "id", "host"?, "ui"? }` (template, Hello World, `references/upgrading.md`).
- **Installers**: `target/`, `bin/` and `obj/` are not copied (the native runtimes' build output). `.gitignore` the
  same.

### Upgrading from 2.0.1

1. Nothing to do in apps: the runtime and relays are unchanged. On the next run, add `"stack"` to
   `ai-enablement.json` (validate notes it).
2. Links into the skill: `references/framework.md` and `references/context-sync.md` are now under `spec/`.

## 2.0.1 — the settings dialog's layout guards (runtime 1.6.1; relays unchanged at 1.4.0)

Two layout bugs in the settings dialog, fixed in the runtime and kept fixed in every app the skill touches. Found in a
real app (an electromagnetic-field teaching lab). All 1.6 options and methods keep working.

- **Clipped tabs.** The dialog's card is a flex column (header, tab strip, body, footer). The tab strip scrolls
  sideways, so its automatic minimum height is 0, and on a tall tab the layout took height from it as well as from
  the body: at 1920×910 the 45px strip was 23px on Tools, 29px on Context, 35px on Memory, the tabs clipped behind the
  body and hard to click. It happened on any page, host CSS or not.
- **Host CSS in the dialog.** The page's global `label { display: flex; justify-content: space-between; … }` centred
  the uppercase labels, shrank each input + button row to its content (the model input cut to "Leave empty to use
  whate…") and pushed "Remember API keys" away from its checkbox. Bare `button`, `input`, `select`, `p`, `h2`,
  `body { text-align: center }`, `* { box-sizing: content-box }` rules did the same kind of damage.

### Runtime 1.6.1

- `ai-agent.css`, `/* aia-guard: pinned-chrome */`: the dialog's header, tab strip and footer never shrink
  (`flex-shrink: 0`); the body takes what is left (`min-height: 0`) and is the only part that scrolls. The drawer's
  own flex columns (header, context bar, saved chats, messages, composer, the file viewer) state the same.
- `ai-agent.css`, `/* aia-guard: host-isolation */`, right after the theme tokens: inside `.aia-scope`, every element
  the runtime renders is reverted to the browser's own styles (`all: revert`, at specificity (0,1,0)), so the page's
  rules on bare elements no longer apply; `svg`/`img` are left out (their presentation attributes) and the icons'
  paint is restated. Custom properties (the theme) and `[hidden]` are unaffected.
- Every other runtime rule is now `.aia-scope .aia-x` — (0,2,0) or more, with their relative order unchanged — so a
  host rule with a class, attribute or state (`input[type=number]`, `button:hover { transform }`, `.dark label`)
  does not win either, and the rules state the layout properties they depend on (`.aia-field`: `align-items:
  stretch; justify-content: flex-start; text-align: left`; every button: margin, font, transform, shadow…). No new
  `!important`; no Shadow DOM.
- The layout spec is written above the modal rules and in `references/settings-layout.md`.
- Nothing else looks different. On a page without host CSS, every element of the drawer (messages, Markdown, tool
  rows, the + menu, saved chats) has the same geometry as in 1.6.0, and its computed styles differ only in equivalent
  keywords (`normal` → `stretch`); in the dialog, the tab strip keeps its full height (what is below it moves down
  accordingly) and a checkbox's text box spans its row (the text stays where it was). Button font sizes and weights
  that a `font: inherit` rule had always overridden were removed rather than switched on, so buttons keep their look.
- Dev-time (`devWarnings`, `ui/layout-check.js`): when the settings dialog opens, and on each tab, its computed styles
  are compared with the spec (`.aia-field` `align-items` not `stretch`, `.aia-check` `justify-content` not
  `flex-start`, a squeezed tab strip…), and a console warning names the element and says that host CSS is leaking in.
  It never changes the page.

### Skill

- **Every run on an app that has the agent checks the guards** (SKILL.md, step 0; `references/upgrading.md`, "Layout
  guards"): a missing guard is fixed in that run, whatever the task — an unchanged runtime is replaced (U4), an
  edited one gets the guard blocks in place with the new `scripts/guards.mjs --apply` (its other edits kept, the
  patch recorded under `patches` in the manifest) — and the user is told in one line.
- `scripts/detect.mjs`: `layoutGuards` per runtime copy (`pinnedChrome` — read from the stylesheet's rules with their
  specificity, so a hand-made fix counts; `hostIsolation`), with what to do; the host's own global element rules
  (`label`, `input`, `select`, `textarea`, `button`, `p`, `h1`–`h6`, `body` text-align) from its CSS files and inline
  `<style>` blocks, for information (`hostRules`); app CSS that the guards make redundant among the hints. In the
  summary and in `--json`.
- `scripts/verify.mjs`: a `settings:<tab>` check per visible tab at 1920×1080 and 1280×600 (`--settings-sizes`) — the
  tab strip whole and inside the card, the footer and Save inside the card, labels at their card's left edge, input
  rows full width with the input at least half, checkbox text within 12px of its box, nothing centred — naming the
  element and the host rule that decides the value; screenshots `settings-<tab>.png`.
- `references/settings-layout.md` (new): the spec, how the CSS holds it, restyling from the app, how it is checked.
  Updated: `upgrading.md` (Layout guards, U4, U5, the manifest's `patches`), `checklist.md`, `architecture.md`,
  `api.md`.
- Tests: the settings dialog under `tests/fixtures/hostile-host/hostile.css` (every rule above and more) with 29
  tools, 30 memories and a long screen, every tab at both sizes, identical geometry with and without the host
  stylesheet, and a deliberate leak the check and the dev warning must catch (`tests/browser.test.mjs`); detect on a
  runtime with both guards, with neither and with the hand-made fix, and the host rules (`tests/detect.test.mjs`);
  the stylesheet reader and `--apply` (`tests/guards.test.mjs`). `tests/fixtures/hostile-host/index.html` is the same
  page for `verify.mjs` (`?rules=off` for the control run).

### Upgrading from 2.0.0 / runtime 1.6.0

1. Re-copy `assets/ai-agent/` (an edited copy: `node <skill>/scripts/guards.mjs <app-root> --apply`). The relays are
   unchanged. Bump `?v=` in the asset URLs.
2. Behaviour to know about (`references/upgrading.md`, U4): app CSS that restyles a runtime class with a single class
   (`.aia-btn-primary { … }`) no longer wins by load order — write `.aia-scope .aia-btn-primary`, or use the
   `--aia-*` variables; the page's element rules no longer style the agent's UI.
3. Remove app-side workarounds the guards cover (U5): `flex-shrink` on `.aia-tabs`, `!important` on `.aia-field`,
   `.aia-check`, `.aia-row`… once `verify.mjs` passes without them.

## 2.0.0 — AI Enablement: tools, toolsets, skills, agents, permissions, lifecycle, in-app authoring (runtime 1.6.0; relays unchanged at 1.4.0)

The skill is now **ai-enablement**. It still builds the agent drawer into an app, and it now manages the app's whole
AI layer: first-class tools and toolsets (MCP-compatible), Agent Skills, agents, permissions, a capability folder the
runtime loads, a lifecycle (install → upgrade → validate → add → reconcile) with a manifest, and a development
workspace in which the app's own agent writes new tools. Every 1.5 option and method keeps working; apps built with
add-ai-skill are recognised and upgrade normally. Design notes: `docs/ai-enablement-design.md` in the repository.

### Rename

- The skill folder, its name and its installed copies are `ai-enablement`. The installer no longer removes other
  installed skills: `add-ai-skill` copies stay until removed on purpose (`-RemoveLegacy` / `--remove-legacy`).
- Unchanged on purpose, so installed apps and relays keep working: `ai-agent.js`, `createAiAgent`, the `aia-` CSS
  prefix, `ai-tools.json`, `ai-memory.json`, and the relay protocol (`X-Requested-With: ai-agent-drawer`,
  `"relay": "ai-agent-drawer"`).

### Runtime 1.6.0

- **Capability folder**: `createAiAgent({ capabilities: 'ai/index.json', host })` loads an index naming tool modules,
  toolsets, skills, agents, permissions, the tool config and the memory file (`core/capabilities.js`).
  `agent.capabilities.reload()` loads it again (fresh copies of every module and file, the tool config re-applied).
  Problems are collected — console, `agent.capabilities.problems()` — never thrown.
- **Tools**: a JSON Schema `inputSchema` (MCP) as an alternative to `parameters` (the enforceable subset; the rest is
  refused with the reason — `core/schema.js`); `minLength`, `pattern`, `minItems` enforced; MCP `annotations`
  accepted and derived; `agent.tools.mcp()` / `toMcpTool()` export MCP descriptors; `run(args, { host, … })` gets the
  integration's `host`; new effects `external` and `system`.
- **Toolsets**: modules export `{ name, title, description, tools }`; JSON toolsets group tools defined elsewhere;
  Settings > Tools groups by them; `agent.tools.toolsets()`.
- **Permissions** (`core/permissions.js`): `{ allow, ask, deny }` rules (names, `*`, `toolset:`, `effect:`) from the
  index, the `permissions` option and the active agent; deny > ask > allow > the user's settings; `system` tools always
  confirm. Denied tools never reach the model; Settings > Tools shows *blocked*, *always asks*, *no confirmation*.
- **Skills** (`core/skills.js`, Agent Skills format): listed in the system prompt; the model loads one with
  `use_skill` (then it stays in the system prompt for the conversation, saved with the chat); `/name` in the composer;
  `read_skill_file` for the files a skill points to. `agent.skills.list()`, `activate()`, `active()`.
- **Agents** (`core/agents.js`): Markdown files with YAML frontmatter composing instructions, tools, toolsets, skills,
  permissions, context layers, memory policy and step limits. A picker replaces the drawer title when there are two
  or more; switching starts a new chat; saved chats remember their agent. `agent.agents.list()`, `current()`, `use()`.
- **In-app authoring** (`workspace` option, `core/workspace.js`): when the skill's dev server answers, the agent gets
  `describe_host`, `list_source_files`, `read_source_file`, `search_source`, `write_ai_file` (effect `system`: always
  confirmed, shown whole or as a diff; creating an existing file is refused; a module without `export default` or with
  `run(args, host)` is refused with the fix), `reload_capabilities` (reports unregistered tool files and misnamed tool
  config keys) and the built-in skill `create-tool` (`skills/create-tool/SKILL.md`, shipped in the runtime).
- Tools may refuse a call before the confirmation card (`precheck`); `confirmHtml` may be async.
- New exports: `toMcpTool`, `fromJsonSchema`, `loadCapabilities`, `parseSkill`, `parseAgent`, `parseFrontmatter`.
  New events: `agent`, `skill`, `capabilities`, `workspace`.
- `agent.ready` also waits for the capability index and the workspace check.

### Skill

- `SKILL.md` is a lifecycle router: detect → install / upgrade / validate → add a capability.
- New references: `framework.md` (concepts, formats, ownership, permissions, the manifest), `capabilities.md`
  (create a tool / toolset / skill / agent, context, memory), `in-app-authoring.md`. Updated: `tools.md` (toolset
  modules, `host`, `inputSchema`, effects), `upgrading.md` (adopting the framework, the 1.6 changes, the manifest),
  `api.md`, `architecture.md`, `checklist.md`, `frameworks.md` (where the capability folder goes per stack),
  `memory-and-vision.md`.
- **Manifest** `ai-enablement.json` replaces `ai-agent.integration.json` (still read): `framework` (runtime and relay
  — framework-owned) and `capabilities` sections.
- Scripts: `detect.mjs` (capability folders and missing paths, the manifest or the 1.x record, dev-time folders kept
  apart, the new features, a relay compared with the skill's own relay version — a 1.4 relay is no longer reported as
  an upgrade because the runtime is 1.5, — `createAiAgent` mentioned in comments no longer counted); new
  `validate.mjs`, `scaffold.mjs` (templates in `assets/templates/`), `workspace.mjs`; `verify.mjs` has a
  *capabilities* check.
- Hello World runs on the framework: `ai/index.json`, three toolset modules (`document`, `editor` — its tool written
  as JSON Schema — `file`), skills `proofreading` (with a reference file) and `summarize`, agents `writer` and
  `proofreader`, an `ask` rule for the file toolset, `ai-enablement.json`; `ai-tools.js` became the toolset modules.
- Tests: `framework`, `framework-browser` (agents, skills, permissions, saved chats; in-app authoring end to end with a
  scripted model), `workspace`, `lifecycle`; `detect` and `example` extended. Also tried live with LM Studio and
  `qwen/qwen3.5-9b`: the in-app agent added working tools from a plain request.

### Upgrading from 1.6 (add-ai-skill) / runtime 1.5

1. Re-copy `assets/ai-agent/` (new files in `core/` and `skills/`). The relays are unchanged.
2. Behaviour to know about (`references/upgrading.md`, U4): `host` in tool contexts; the AGENT and SKILLS prompt
   sections (only with agents or skills); permission badges; the agent picker; `agent` and `skills` in saved chats;
   the new `$comment` of exported tool configs.
3. Write `ai-enablement.json` from `ai-agent.integration.json` (`references/upgrading.md`, "The manifest").
4. Offer, do not force, the capability folder (`references/upgrading.md`, "Adopting the framework") — and with it
   skills, agents, permissions and in-app authoring where they fit.

## 1.6.0 — attachments: the + button (runtime 1.5.0; relays unchanged at 1.4.0)

The user can give the agent an image or a document with the question. A **+** button on the left of the message field
opens a menu: **Attach an image** and **Upload a file**. Drag and drop onto the drawer and pasting attach the same way.
All 1.5 options and methods keep working; the relays need no change.

### Images

- An attached image goes to the model **exactly like a screenshot**: the same pipeline (`imageFromFile()` in
  `ui/capture.js`) makes a JPEG of at most `screenshotMaxEdge` px and a thumbnail; it waits in the composer, shows on the
  question (click to enlarge), counts toward the 3 images per question, and follows the screenshot rules (the two
  newest questions with images send them; saved chats keep thumbnails only).
- The model is told which images are screenshots and which are files, by name (`[Attached to this message: a
  screenshot of the user's screen (…) and the image file "diagram.png".]`). Questions with screenshots only keep the
  1.4 wording.
- Offered while "This model can see images" is on; otherwise *Attach an image* is greyed out with the reason, and an
  image dropped or picked anyway gets a message saying how to enable it (an SVG is then read as text).

### Files

- Read **in the browser, without dependencies**, and sent as text inside `<attached_file name="…" type="…"
  chars="…">` blocks (a closing tag inside the file is neutralised):
  - **PDF** (`core/pdf.js`): page tree, fonts with ToUnicode maps or encodings (WinAnsi, MacRoman, Differences with
    glyph names), Flate/ASCII filters and PNG predictors, object streams, form XObjects, inline images skipped; line
    breaks and spaces from text positions; pages marked `--- Page N ---`. Checked on PDFs from Word-style tools,
    LaTeX-style manuals, CJK licence texts and the browser's own print-to-PDF. Encrypted and scanned (text-less) PDFs
    are refused with a sentence that says so.
  - **Word, Excel, PowerPoint, OpenDocument, RTF** (`core/office.js` over a small ZIP reader in `core/bytes.js`):
    headings and list items, tables as rows, footnotes, text boxes once (not their fallback copy); one CSV section per
    sheet with dates from their number format; slides in presentation order with speaker notes; RTF in its code pages.
  - **Text** of any kind (code, CSV, JSON, Markdown, logs, config): UTF-8, UTF-16 with a BOM, else Windows-1252; files
    with unknown extensions are read when their content is text.
  - Refused with what to do instead: old binary Office files, iWork, archives, audio/video, other binaries, empty
    files, files over 25 MB.
- Each file's text is cut to **Max file content** (Settings > Agent; setting `maxFileChars`, 40,000 by default) and
  the model is told. 5 files per question. A file stays in the conversation while its question is in the history
  window; the same content attached again is sent once. Saved chats keep a file's text (dropped from older chats
  first when storage runs out).
- The chip on the question opens **the text the agent received**.
- The system prompt gains an *Attached files* paragraph only while the conversation carries files.
- The PDF and office readers load with `import()` on first use, so start-up cost does not grow.

### API

- Options `attachments` (default `true`; `false` removes the + button, drop and paste) and `readFile(file, { kind,
  name })` — the app's own reader, tried first (text, `{ text, label }`, or `null` for the built-in readers).
- `agent.attach(files)` → one `{ kind, name, size, … }` (or `null`) per file; event `attach`; `send` carries `images`
  and `files`. Setting `maxFileChars`.
- `agent.ask(text)` and the welcome suggestions take what waits in the composer along, as Send does.
- New modules: `core/files.js`, `core/bytes.js`, `core/pdf.js`, `core/office.js`.

### Skill workflow

- `SKILL.md`: the feature, an *Attachments* line in the context plan, step 9 "Memory, vision and attachments",
  verification and report. `references/memory-and-vision.md` has an *Attachments* section (formats, limits, what to
  do in an integration, `readFile`, CSP `blob:` for SVGs, relays); `api.md`, `upgrading.md` (U4–U6, the record's
  `attachments` feature), `checklist.md` (checks and troubleshooting), `architecture.md`, `providers.md`.
- `scripts/detect.mjs` reports the `attachments` feature, and flags app code that reads files for the agent itself.
  `scripts/verify.mjs` has an `attachments` check (the menu, one attached text file).
- The skill description now mentions attachments and is shorter than before.
- Tests: `tests/files.test.mjs` with documents built in memory (`tests/fixtures/documents.mjs`: DOCX, XLSX, PPTX, ODF,
  RTF, PDFs with WinAnsi/Differences and Type0/ToUnicode fonts, object streams, forms); in a real browser the + menu
  with the keyboard, an image and a Word file through the pickers and what the model receives, the file viewer, saved
  chats, drag and drop, paste, a text-only model, `attach()` + `ask()`, Send while a file is read, `attachments:
  false`, and a PDF printed by the browser itself.

### Upgrading from 1.5

1. Re-copy `assets/ai-agent/` (four new files in `core/`). The relays are unchanged.
2. Behaviour changes to know about (`references/upgrading.md`, U4):
   - The composer has a + button (`.aia-plus`) left of the field; drops on the drawer no longer reach host `drop`
     listeners; pasted files are attached (a paste that carries text pastes the text).
   - `agent.ask()` takes the composer's attachments along.
   - The system prompt's image paragraph starts "Images:" (was "Screenshots:"); tests matching the old text change.
   - `screenshots: false` keeps Settings > Vision while attachments are on.
3. Set `defaults.maxFileChars` lower for a default model with a small context; remove app-side "send a file to the
   agent" code (detect flags it), keeping a reader for the app's own formats as `readFile`.

## 1.5.0 — tool-call diagnostics (runtime and relays 1.4.0)

When a tool call fails, the user can now see why, and the model is told what really went wrong. Found with LM Studio
and `qwen/qwen3.5-9b` (a model that thinks first, at the default *Max reply tokens* of 4,096) writing a long note list
in one JSON-string argument: the reply was cut off mid-arguments, the lenient parser made up a value from the broken
JSON, the model was told *missing required argument "notes"*, and it "fixed" its quoting over and over. All 1.4 options
and methods keep working.

### Cut-off replies

- Every adapter reports `truncated: true` when the reply stopped at the max-tokens limit: OpenAI-compatible
  `finish_reason: "length"` (streamed or not), Anthropic `stop_reason: "max_tokens"`, Gemini `finishReason:
  "MAX_TOKENS"`, the relay's `done.truncated`. `streamChat()` passes it on.
- A cut-off call is **not run**. Its row says *Cut off: the reply reached Max reply tokens (4,096 tokens) after N
  characters of arguments. Raise it in Settings > Agent, or ask for less per step.*; the model is told to do the work
  in several smaller calls. A cut-off reply without tool calls shows a notice. In text mode an unclosed ```` ```tool ````
  block becomes that cut-off call, but only when the reply was truncated.

### Arguments

- `readArguments(value) → { args, error }` (`core/tools.js`): JSON first (trailing commas forgiven), `key: value` lines
  only when the result is not garbled, and JSON that stops before it is closed is reported as cut off. A call whose
  arguments cannot be read is reported to the model, never run with guessed values. `parseArguments()` keeps its
  contract (an object; `{}` for unreadable text).
- Tool calls carry `raw` (the argument text as the model wrote it) and `argsError`; `wireCall()` strips both before
  calls go back to the provider.
- Text over a string parameter's `maxLength` (default 500) is an error with both sizes ("`notes` is 12,981 characters
  long, over its limit of 12,000: send less in one call"), never cut into broken JSON. Tool arguments only:
  `parseBlockValues` and fenced blocks still cut, as before.

### Tool rows roll down

- The row's line is a button: once the call is over, clicking it rolls down the problem (if any), *Arguments the tool
  received*, *As the model sent them* (only when different), *Returned to the model* / *Error returned to the model*,
  and a **Copy** button. Every finished row does, including the built-in tools and the *Turn on* request.
- **Copy tool log** under a reply with tool rows copies every row as text.
- Saved chats keep the detail (each part capped at 2,000 characters), so reopened chats roll down too. Saved action
  records gain `name` and `detail: { args, sent, problem, result }`.
- New `core/tools.js` exports: `readArguments`, `jsonUnclosed`, `toolCall`, `wireCall`, `argumentsProblem`,
  `callDetail`, `callReport`, `STATUS_WORDS`; `parseTextToolCalls(text, idPrefix, { cutOff })`.

### Relays (1.4.0)

- `tool_call` events carry `raw` (the arguments as the model wrote them); `done` carries `truncated: true` when the
  reply hit the max-tokens limit. `relay.php` tracks the finish reason of all three protocols. An older relay keeps
  working: a cut-off call then reads as "could not be read" rather than "cut off".

### Skill workflow

- `references/tools.md`: the `maxLength` rule, rows that roll down, cut-off detection, and tools that take large
  payloads in sections (`offset` / `part`) so one call fits in one reply. Step 8 of `SKILL.md` says the same.
- Tests: `tests/tool-calls.test.mjs` (argument reading, limits, cut-off detection per provider and through the relay
  adapter, text mode, row detail), relay tests for `raw` / `truncated` (Node + PHP), and in a real browser a cut-off
  call rolled down, copied and reopened from a saved chat.

### Upgrading from 1.4

1. Re-copy `assets/ai-agent/` and the relay.
2. Behaviour changes to know about (`references/upgrading.md`, U4):
   - A string tool parameter without `maxLength` now refuses text over 500 characters instead of cutting it. Give
     every parameter that takes long text an explicit `maxLength`.
   - Unreadable arguments are an error the model sees, not a best guess.
   - Tool calls from the adapters and `streamChat()` carry `raw` (and `argsError`).
   - The tool row's line is a `<button>` (it was a `div`): check app CSS that styles `.aia-tool-line`.
3. For a model that thinks before it answers, raise *Max reply tokens* (`defaults.maxOutputTokens`) if its tool calls
   carry a lot of data.

## 1.4.0 — memory and vision (runtime and relays 1.3.0)

The agent remembers what it is asked to remember, and can look at the screen. Both are part of the runtime: an app
gets them by replacing its runtime copy, and the skill adds the app-specific part. All 1.2 options and methods keep
working.

### Memory

- Notes kept between conversations: the user says "remember that…", the model calls the built-in `remember` tool, the
  chat shows a chip with **Undo**; `forget` deletes one after the user confirms. The notes are a `== MEMORY ==`
  section of every system prompt, marked as notes that lose against the screen.
- **Settings > Memory**: the list (edit, delete, add), *Download ai-memory.json*, *Copy JSON*, *Import a file…*,
  *Delete all*, and two switches (use memories; let the agent save them).
- `memoryFile` (`ai-memory.json`): the app's starting notes. A browser stores only what differs from the file (its own
  notes, edits, deletions), so a later file still reaches users. `memorySave(file)` lets an app's backend keep them.
- `agent.memory` (`list`, `add`, `update`, `remove`, `clear`, `export`, `import`), the `memory` event, the settings
  `memoryEnabled` / `memoryWrite`, `memory: false` to remove it. Limits: 100 notes of 500 characters.

### Vision

- A **camera button** beside Send attaches a screenshot of what the user is looking at (a thumbnail in the composer,
  then on the question); the model receives it as an image.
- The agent's own `take_screenshot` tool, callable only when the user frees it (Settings > Vision: untick "Only take
  a screenshot when I press the camera button"). Otherwise the agent can ask, and the user gets *Allow once* /
  *Always allow* / *No*. Every screenshot the agent takes shows as a thumbnail on its chip.
- **Settings > Vision**: "This model can see images" (off: no camera button, no screenshots), the camera-only switch,
  how screenshots are taken here, *Stop sharing this tab*; with LM Studio, what the server says about the loaded model.
- Two ways to take the picture: the browser's screen capture of the tab (default: the whole page as displayed, drawer
  cropped off; the browser asks the user) or the app's `screenshot` hook (a canvas, image, video frame, Blob…; no
  prompt; a WebGL canvas returned right after a render is read before its buffer is cleared).
- JPEG scaled to `screenshotMaxEdge` (1280 px); only the two newest questions with screenshots send their images;
  saved chats keep thumbnails only. `agent.screenshot()`, the `screenshot` event, the settings `vision` /
  `screenshotAuto`, `screenshots: false` to remove it.
- Images in all three provider formats (OpenAI-compatible, Anthropic, Gemini) and in text-mode tool results. Checked
  live with a vision model on LM Studio; Anthropic and Gemini follow their documented formats (unit-tested, not run live).

### Relays (1.3.0)

- `relay.php` and `relay.mjs` check and pass `images` on user messages and tool results: `limits.maxImages` (public 4,
  local 16; 0 refuses images) and `maxImageBytes` (public 1.5 MB, local 4 MB, as base64), on top of `maxBodyBytes`,
  which stays the cap for the text. The GET reply has `images` and, from the preset, `vision`.
- The drawer asks a relay once before the first request with an image, and refuses the question with a clear message
  when the relay is older than 1.3 (it would drop the image silently). With `relayProbe`, such a relay switches vision off.
- `relay.php` answers 413 with the reason when a body is larger than PHP's `post_max_size` (PHP discards such bodies).

### Skill workflow

- New step 9, **Memory and vision**, and `references/memory-and-vision.md`: collecting the seed notes, `ai-memory.json`,
  where users' notes live, choosing the capture method, hook recipes (2D, WebGL, layered canvases), host requirements
  (secure context, `Permissions-Policy`, CSP), relay limits, privacy.
- `scripts/detect.mjs` reports, per feature (tools, memory, vision), whether the installed runtime has it and whether
  the integration uses it, plus canvases/WebGL views and policy headers that matter for screenshots — so an upgrade
  adds only what is missing. `references/upgrading.md` has the steps for an app at 1.2 or older.
- `scripts/verify.mjs`: `memory` and `vision` checks (it takes one screenshot the way the camera button does).
- Hello World ships `ai-memory.json`.
- Tests: the memory store and prompt, images per provider and in the conversation, relay image limits (Node + PHP),
  and in a real browser: remember / forget / Undo / Settings > Memory, a WebGL `screenshot` hook, the agent asking to
  look, and the browser's screen capture with the drawer cropped off.

### Upgrading from 1.2

1. Re-copy `assets/ai-agent/` and the relay. Memory and vision are then on with their defaults.
2. Add what the app lacks (`references/upgrading.md`, "Adding memory and vision"): `ai-memory.json` + `memoryFile`,
   and a `screenshot` hook where the view is a canvas. Set `defaults: { vision: false }` for a text-only default model.
3. Behaviour changes to know about:
   - Requests now carry tool definitions even in apps without tools (`remember`, `forget`, and `request_tool` while
     the agent may not look on its own). A model server without tool calling gets them as text blocks automatically.
     `memory: false` and `screenshots: false` give exactly the 1.2 requests.
   - The default composer placeholder is shorter ("Ask about what is on screen…").
   - A relay must be 1.3 for screenshots; its body cap for text is unchanged, images come on top. Raise
     `client_max_body_size` / `post_max_size` in front of it.
   - A page with a `Content-Security-Policy` needs `img-src data:` for the thumbnails.

## 1.3.0 — upgrade mode (runtime and relays unchanged: 1.2.0)

Running the skill on an app that already has the agent now upgrades it instead of building a second one.

- **Step 0** in SKILL.md: run `scripts/detect.mjs` first. It finds the runtime copy and its version, says whether the
  copy is unchanged since its release (fingerprints for 1.0.0, 1.1.0 and 1.2.0), finds relays (with 1.0-style edits:
  constants, keys or auth code inside the file — never printing keys), relay config files (names only), every
  `createAiAgent()` call with its `appId` and options, the integration record, tool configs, and app code that looks
  like a workaround a newer runtime covers. It ends with BUILD, UPGRADE or CURRENT.
- **`references/upgrading.md`**: the upgrade workflow — read the record and the CHANGELOG since the installed
  version, propose the upgrade, replace the runtime and relay wholesale (migrating 1.0 relay constants to
  `relay.config.php`), remove redundant workarounds (table), offer the new features that fit (tools first), verify,
  and record. Keeps the `appId`, hooks, prompts and customisations.
- **`ai-agent.integration.json`**: every integration and upgrade leaves a record in the app (versions, files,
  features, plans, declined features), so the next run knows exactly what is there.
- `scripts/release-hashes.mjs` records the fingerprints of a release (maintainers: run it for every release).
- Tests: `tests/detect.test.mjs`.

## 1.2.0 — the agent can act: tools

The agent no longer only explains and suggests: it calls the application's own functions. All 1.1 options and
methods keep working; an app without `tools` behaves exactly as before.

### Skill workflow

- Survey step: the app's **action surface** (API clients, store actions, services, handlers, controls with ranges).
- Context plan: a **tool plan** per page (what each tool wraps, read / write / destructive, parameters, what is left
  out), with the user choosing which tools start on.
- New step "Build the tools" and `references/tools.md`: one tools module over the app's own functions, `ai-tools.json`
  for the default selection, unit tests, and checks with a model.

### Runtime

- `tools` option, `page.tools`, `toolsConfig` (e.g. `'ai-tools.json'`), and `agent.tools` (`list`, `register`,
  `unregister`, `setEnabled`, `run`, `exportConfig`); events `tool` and `tool-state`.
- The tool loop: native tool calls for OpenAI-compatible servers (LM Studio, Ollama, OpenAI, DeepSeek, OpenRouter,
  custom), Anthropic and Gemini; a text-block protocol for models without tool calling (`toolMode`); arguments
  validated and clamped; results (and the screen, if it changed) sent back; up to `maxToolSteps` rounds.
- Confirmations: write tools ask (*Run* / *Allow for this chat* / *Skip*), destructive tools always ask — both
  switchable (`confirmWrites`, `confirmDestructive`). Reading tools never ask.
- Turned-off tools stay known to the model: it can say so and ask with the built-in `request_tool`, which shows a
  *Turn on* button; turning a tool on saves the choice.
- **Settings > Tools**: a checkbox per tool (grouped, with its effect and availability), the switches, and
  "Download ai-tools.json" to make the selection the app's default. Only choices that differ from the app's defaults
  are stored.
- Chips for every call in the reply; actions kept in saved chats and summarised in later requests.
- Settings > Context counts the tool definitions in the size estimate; the Context tab's system prompt includes the
  TOOLS section.
- Some local models (seen with LM Studio + Qwen 3.5 9B) answer the step after a tool round only in their reasoning
  channel: that reasoning is shown as the answer.

### Relays (1.2.0)

- `relay.php` and `relay.mjs` pass `tools` and `toolTurns` to the provider (all three protocols) and stream
  `tool_call` events. Public mode counts one question per chain of tool steps (`turnId`) up to `limits.maxToolSteps`;
  a forged continuation is counted. New limits `maxTools` (64) and `maxToolSteps` (public 10, local 30).

### Tooling

- `scripts/verify.mjs` reports the tool catalog and runs the reading tools that need no arguments.
- Hello World: eight tools over the editor's functions (`ai-tools.js`) and `ai-tools.json`.
- Tests: tool unit tests, adapter tool formats, relay tool pass-through (Node + PHP), the full drawer tool loop in a
  real browser (confirmation, turn-on, text mode), Hello World's tools.

### Upgrading from 1.1

Re-copy `assets/ai-agent/` and the relay. Nothing else changes until you add `tools`. A 1.1 relay ignores tools, so
upgrade the relay too if the app uses one.

## 1.1.0 — lessons from the first production integration

The skill is now named **add-ai-skill** (was `ai-agent-drawer`): install it with the repository's
`install-skill.ps1` / `install-skill.sh`, which also remove copies installed under the old name. What it puts into
apps keeps its names: the `ai-agent/` runtime folder, the `aia-` CSS prefix, and the relay protocol identifiers
(`X-Requested-With: ai-agent-drawer`, `"relay": "ai-agent-drawer"`), so existing integrations and relays keep working.

A WebGL simulation fed by a Web Worker (~10 state updates/s), native `<dialog>` modals, a dense fixed-width header,
developed on XAMPP and deployed on Nginx + PHP-FPM. Everything it needed app-side workarounds for is now built in or
documented. All 1.0 options and methods keep working.

### Runtime

- **`contextChanged()` no longer starves under continuous updates**: a trailing debounce with a max wait
  (`debounceMaxMs`, default 1000). Before, an app changing state every ~100 ms never let the 300 ms debounce fire.
- **Key isolation** (`isolateKeys`, default on): key events that start inside the drawer or the settings modal do not
  reach the host's bubbling listeners, so host shortcuts (Space = play, letters, arrows) cannot swallow typing.
  Ctrl/Cmd application shortcuts still reach the host; the hotkey and Escape work from inside the drawer.
- **Native modal dialogs** (`dialogs: 'dock'`, opt-in): dialogs are shown non-modally beside the open drawer and
  become modal again when it closes, without `close`/`beforetoggle`/`toggle` events reaching the host.
- **Dev-time warnings** (`devWarnings`, default on for local hosts): the pushed layout overflows or hides elements
  (including the toggle) under the drawer; a modal dialog makes the drawer inert.
- **Theming**: the theme rules are wrapped in `:where()`, so host `.aia-scope { --aia-… }` overrides win in dark
  mode too. New `--aia-badge-synced/-dirty/-unread/-ring/-size/-top/-right` variables for the toggle's context dot
  (defaults unchanged). New `--aia-push-width` host hook. `html.aia-drawer-open` and `--aia-drawer-width` are now
  documented, supported hooks.
- **Code actions that survive small models**: `parseBlockValues(block, { tags, schema })` (custom tag, or `json` when
  every key is known; JSON or `key: value` lines; linear-time parsing; clamping) and `setControlValue(el, value)`
  (applies through the app's real controls: native setter + `input`/`change`).
- **Relay auto-detection**: `relayProbe` (GET the relay at startup, use it when `available`, with its preset) and
  async `defaults` (promise or async function). New `agent.ready`, `agent.relayInfo()`, `relay` event. Saved settings
  that no longer fit the relay are adjusted when read, without touching storage.
- **Resume**: a drawer reopened during `createAiAgent()` replays its `open` event (`{ resumed: true }`) to listeners
  registered in the same tick. The composer no longer takes focus from a settings modal opened right after the drawer.
- **Context size**: Settings > Context shows the estimated tokens of the first request and warns (local providers)
  above `contextWarnTokens` (3000).
- Relay errors keep their own code and message in the drawer (a refused origin is no longer shown as "check your
  API key"); `detail` is appended when the relay sends one.

### Relays

- `relay.php` and `relay.mjs` are configured by a **config file** (`relay.config.example.php`; `.json`/`.mjs` for
  Node), looked up in `AIA_RELAY_CONFIG`, then `AIA_RELAY_DIR` (default: `ai-agent-relay` next to the web root), then
  next to the relay. Apps copy the relay unchanged.
- **Public mode**: only the configured preset (provider, models, server key); visitor keys never forwarded;
  same-origin enforced (`X-Requested-With`, `Origin`, `Sec-Fetch-Site`, no CORS headers); per-visitor per-minute and
  per-day limits plus a site-wide daily cap (salted, daily-rotating hashes; IPv6 by /64; no addresses or text stored);
  body, message-count and reply-token caps.
- Streams start with `: open` and send `: keepalive` comments every ~15 s while the model is silent (Nginx
  `fastcgi_read_timeout`; also detects a visitor's Stop). `X-Accel-Buffering: no`, `apache_setenv('no-gzip')`.
- Typed errors with HTTP status and drawer error code; misconfiguration details only for requests from the server
  itself. `GET` always answers 200 (`available: false` instead of 403). Keys from `getenv()` and `$_SERVER`
  (`fastcgi_param`), then the config. `caBundle: 'native'` for Windows certificate stores (TLS-scanning antivirus).
- `relay.mjs` exports `createRelay`, `loadConfig`, `createStaticHandler`; its static server never serves
  `relay.config.*` or dotfiles.

### Tooling and docs

- `scripts/verify.mjs`: automated integration check in headless Edge/Chrome (no dependencies; Node 22+).
- Tests: `tests/relay.test.mjs` (both relays, fake upstream), `tests/browser.test.mjs` (real browser),
  `tests/example.test.mjs`; new unit tests in `tests/runtime.test.mjs`.
- Hello World now loads its integration with `import()` (`ai-agent-setup.js`), builds its content in a pure tested
  module (`content.js`), and has an "Apply editor settings" code action using the new helpers.
- References: real-time apps, content builders, keyboard shortcuts, modal dialogs, layout recipe, change signals,
  small-model code actions, stale caches, local model context size, relay modes/config, Nginx + PHP-FPM and
  Apache/XAMPP deployment, TLS trust.

### Upgrading an app from 1.0

1. Re-copy `assets/ai-agent/` (unchanged, as always).
2. Re-copy `relay.php` / `relay.mjs`. If you had edited constants in 1.0's `relay.php`, move them to a
   `relay.config.php`: `AIA_ALLOW_REMOTE` → `'allowRemote'`, `AIA_ALLOW_ANY_UPSTREAM` → `'allowAnyUpstream'`,
   `AIA_KEYS` → `'keys'`, `AIA_TIMEOUT` → `'timeout'`, `AIA_MAX_BODY` → `'limits' => ['maxBodyBytes' => …]`; an auth
   check you added in the file goes into `'authorize'`.
3. Behaviour changes to know about: typing in the drawer no longer reaches host key handlers (`isolateKeys: false`
   restores 1.0); the flag refreshes during continuous updates (`debounceMaxMs: 0` restores 1.0); dev warnings appear
   in the console on local hosts (`devWarnings: false`); the relay's GET answers 200 with `available: false` for
   clients it will not serve (was 403); in local mode, requests that came through a proxy (`X-Forwarded-For`…) are no
   longer treated as local — a local dev proxy in front of the relay needs `allowRemote`.
4. Remove app-side workarounds the runtime now covers: your own `contextChanged` throttle, dialog/`showModal`
   switching code (use `dialogs: 'dock'`), keydown filters for the drawer, a pre-mount relay probe (use `relayProbe`).

## 1.0.0

First version: drawer, context layers and hash-based sync with the flag, rich Markdown, saved chats, settings for
eight providers, editable system prompt, PHP and Node relays (local only), Hello World example, unit tests.
