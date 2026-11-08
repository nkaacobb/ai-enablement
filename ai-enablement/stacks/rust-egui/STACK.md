# Stack recipe: Rust with egui / eframe

Status: **planned**. The native Rust runtime (`assets/rust/`: `ai-enablement-core` and `ai-enablement-egui`) is not
in the skill yet, so detect.mjs says STOP for this stack. Tauri apps are not this stack: their UI is a webview, so
they use `stacks/web/STACK.md`, "Tauri".

## What to do today

- **Never** copy the web runtime (`assets/ai-agent/`) into an egui app, or hand-write a drawer, a provider client or a
  tool loop "in the spirit of" the skill. Either one is an unmaintained second implementation that no lifecycle step
  can upgrade.
- Tell the user the Rust runtime is planned, and offer what works now:
  1. **Survey and plan** (SKILL.md steps 1–2, with §1 below). Write the plan into the app's docs so the install can
     follow it later.
  2. **The `ai/` folder's data**: skills (`skills/*/SKILL.md`), agents (`agents/*.md`), toolsets, permissions and
     starting memories. They follow `spec/framework.md` and every runtime reads them unchanged. Put them where §4 says.
     `validate.mjs` checks them; tool references wait for the catalog (§9).
- The rest of this file is the design the runtime is being built to. Treat the API names as provisional until the
  runtime ships.

## 1. Survey: what to look for in an egui app

In addition to the portable survey in SKILL.md:

- **The frame**: the `eframe::App` impl and its draw function, and the panel order. The order matters because the
  drawer is an `egui::Panel::right` drawn **before** the `CentralPanel`. Also note: the window's minimum size, the
  fixed-width columns in the central panel, and anything anchored to the right edge (toasts).
- **Pages**: usually an enum (`Screen`, `Tab`, `Route`) plus sub-tab enums and selection fields. A page id is the
  path: `session/clinical`, `library/packs`.
- **`host`**: the struct the UI already calls for data and actions: a services object, an `Arc<App>`, a store. Note
  which state is UI-only (navigation, selection, open dialogs): tools that change it need `&mut` access to the UI
  struct, which the drawer provides on the UI thread (§5).
- **The action surface**: the public service functions, classified read / write / destructive / external / system.
  Flag the ones that must **never** be tools, such as approvals, sending, exporting and deleting, when the app's spec
  reserves them for a person.
- **Policy constraints** (the portable survey item, often decisive here). Look for:
  - loopback-only rules for model servers;
  - whether model tool calling is allowed at all;
  - consent gates before data reaches a model;
  - where data may persist (an encrypted store, nothing in plaintext files);
  - what happens on lock or sign-out.
- **Existing AI code**: a model client (`/v1/chat/completions`), an MCP tool layer (`tools/list`). Reuse their provider
  settings and their tool descriptors' shape; never build a second provider settings screen.
- **Threading**: background jobs, channels, `request_repaint`, and whether model calls must queue behind other jobs.
- **Theme**: palette and light/dark tokens the drawer must use. **Accessibility names** on widgets (what tests drive).
- **Tests**: `egui_kittest` harness (size, renderer), mock model servers, screenshot tests.

## 3. Install the runtime

Copy `assets/rust/ai-enablement-core/` and `assets/rust/ai-enablement-egui/` unchanged into the app's workspace,
`crates/ai-enablement-core/` and `crates/ai-enablement-egui/` (framework-owned, fingerprinted like the web runtime).

- **Workspace app**: add both to `[workspace] members`. In a workspace they take shared dependencies with
  `{ workspace = true }`, so egui is always the app's own version. Add any version they need to
  `[workspace.dependencies]`, pinned the way the app pins its dependencies.
- **Single crate**: use path dependencies.

detect.mjs recognises the installed copy by the crate name `ai-enablement-core` and its `pub const VERSION`.

## 4. The capability folder

`resources/ai/` (or the app's existing resources folder):

```bash
node <skill>/scripts/scaffold.mjs resources/ai init
```

It is embedded at compile time by default (the runtime's `embedded!("resources/ai")` table), so nothing new ships
beside the executable. Debug builds may load it from disk (`AI_CAPABILITIES_DIR`) so skills and agents can be edited
without rebuilding. The index names a tool catalog instead of tool modules (§9).

## 5. Mount the agent once

One integration module, app-owned, e.g. `src/ai/mod.rs`:

- **Build** the agent once: `AiAgent::builder(app_id)`.
- **Configure** it: app info, system prompt, the capability source, the tool registry (§9), and the hooks the
  survey's policy constraints need:

  | Hook | What it is for |
  | --- | --- |
  | `ProviderPolicy` | for example loopback only |
  | provider settings source | reads the app's own provider settings if it has them |
  | `before_send` | may refuse, for example on a missing consent, with a message the drawer shows |
  | `SettingsStore`, `KeyStore` | default: a JSON file in the app-data folder, and Windows Credential Manager |
  | `MemoryStore`, `ChatStore` | default: files; `None` turns persistence off |
- **Store** it in the app struct.
- **Wire** it into the frame:
  - An "Ask AI" button in the top bar calls `agent.toggle()`.
  - Ctrl+I toggles too, unless a text field has focus elsewhere.
  - In the draw function, before the `CentralPanel`: `agent.show(ui, &mut host)`. It draws the right panel only while
    open, runs pending tool calls on the UI thread with `&mut host`, and polls the model stream from its worker
    thread. Since the agent lives in the app struct, take it out for the call (`let mut ai = std::mem::take(&mut
    self.ai)` … put it back), or keep it beside the host struct.
  - Hide it where the app hides its navigation (locked screens).
  - On lock or sign-out, call `agent.reset()`.

## 6. Pages

`fn page(app: &App) -> PageContext { id, title, purpose, content, view }` per screen, built from the app's state,
never from widgets. The `content` builders are pure functions (state → `String`) with `cargo test` unit tests. They
are deterministic, keep the precision and labels the UI shows, and stay small enough for the model's context. Check
local models with 8k contexts especially.

## 7. Change signal

Call `agent.context_changed()` after commits, job completions, imports, and navigation. The runtime debounces it
(300 ms, max wait 1 s) and recomputes the flag. In immediate mode there is no subscription. Call it where the
mutation happens, or once per frame when a cheap revision counter changed.

## 8. Fit the host app

- Use the app's palette: the drawer takes theme tokens, or reads `ctx.style()`.
- Move right-anchored toasts left by the drawer's width while it is open.
- Typing in the drawer never reaches the app's shortcuts. The app's shortcut handler checks
  `agent.wants_keyboard()`.
- Confirmations use the drawer's own cards. External handoffs that the app already previews in its own modal stay
  there.

## 9. Tools

- **Shape**: one module per toolset, `src/ai/tools/<toolset>.rs`, each returning a `Toolset` of `Tool`s. A tool is the
  contract plus a handler: name, title, description, effect, parameters (shorthand builder or a JSON Schema
  `serde_json::Value`), pages, and `run(&Args, &mut ToolCtx<Host>) -> Result<Value, ToolError>`.
- **Handlers** call the app's own service functions. They never reimplement them, and never bypass the app's own
  checks.
  - **Writes**: where the app has a proposal/accept pattern, a write tool creates a proposal and the person accepts it
    in the app's UI.
  - **Never tools**: what the spec forbids. Also `deny` those in the index permissions, so a later edit cannot slip
    them in.
- **Catalog test** (`tests/ai_catalog.rs`): builds the registry with a stand-in host, exports `ai/tools.catalog.json`,
  and fails when the committed file differs. With `AI_CATALOG_UPDATE=1` it rewrites the file (`spec/tool-catalog.md`).
  `validate.mjs` reads the catalog.
- **Unit tests**: each toolset is tested over a test host (an in-memory app).

## 11. Memory, vision, attachments

All are optional, and the policy constraints decide.

- **Memory**: a file in the app-data folder, the app's encrypted store through `MemoryStore` (per user, per record), or
  off.
- **Vision**: the app's own window through `ViewportCommand::Screenshot`, cropped to the central panel, scaled to
  1280 px JPEG. It is off where screens show data that must not leave as images.
- **Attachments**: `rfd` file dialog and dropped files. Text extraction for plain text, plus whatever the app already
  reads (PDF, Office). Where the app has its own import and review flow for documents, that flow is the way in, and
  attachments stay off.

## 13. Transport

Direct to the provider. A native app has no CORS, so there is no relay. Keys go in the OS keychain, never in settings
files or logs. Local providers (LM Studio, Ollama) need no key. If the app's policy allows only local models, set
`ProviderPolicy::LoopbackOnly`: the settings screen then offers only local providers, and the runtime refuses anything
else before a request.

## 14. Verify

- `node <skill>/scripts/validate.mjs <app-root>` (the folder and catalog), then `cargo test` (content builders,
  toolsets, catalog).
- **The UI flow**: an `egui_kittest` test at **1920×1080** against a mock model server that streams text and tool
  calls. The flow:
  1. open the drawer;
  2. ask → flag *synced*;
  3. change the screen → *dirty* → ask → *Read the page*;
  4. ask again → *Page unchanged*;
  5. a write tool → confirmation card → run → result;
  6. a denied tool never reaches the request;
  7. lock → reset.

  Save a screenshot as evidence.
- **By hand**, in the release build, maximised: the same flow against the real model, plus Settings (every tab).

## 15. Manifest

`"stack": { "id": "rust-egui", "ui": "egui <version>" }`. `framework.runtime` names `crates/ai-enablement-core` and
`crates/ai-enablement-egui`. `capabilities.index` is `resources/ai/index.json`.
