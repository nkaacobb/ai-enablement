# Upgrading an app that already has the agent

Use this when `scripts/detect.mjs` reports **UPGRADE** or **CURRENT**, or the user asks to update the agent or add a
feature to an app that already has it — including apps built with **add-ai-skill 1.x**, the skill's earlier name:
their runtime, relay, record and options are all recognised, and they upgrade like any other. **Never build a second
agent**: one `createAiAgent()` per app, and the same `appId` forever (it namespaces every user's settings, API keys
and saved chats in their browser).

The rule throughout: **framework-owned** files (the runtime folder, the relay file) are replaced when they are
unchanged copies and reconciled when they were edited; **app-owned** files (the integration, content builders, the
capability folder, relay config, CSS) are only ever added to or edited in place.

The user's words set the scope: "just update the runtime", "only add tools", "add memory and screenshots", "upgrade
everything". Without a scope, propose the full upgrade (U3) and proceed unless they object.

**An upgrade adds only what is missing.** What the app already has — its hooks, prompts, tools, tool config,
customisations, users' settings and saved chats — stays. The runtime folder and the relay file are replaced as a whole
(they were copied unchanged); everything else is added piece by piece, and only the pieces detect reports as not there.

## U1. Find what is there

```bash
node <skill>/scripts/detect.mjs <app-root>          # --json for machine-readable output
```

It reports the runtime copy (version, and whether it is unchanged since that release), the relays (version, 1.0-style
edits), relay config files (names only), the `createAiAgent()` call(s) with their `appId` and options, the manifest
(`ai-enablement.json`) or the 1.x record (`ai-agent.integration.json`), capability folders and the paths they name
that are missing, development-time folders (`.claude/` and the like — never the app's), tool config and memory files,
app code that looks like a workaround a newer runtime covers, and the **Layout guards** of each runtime copy's
stylesheet (below). Its **Features** lines are the to-do list of the upgrade:

```
Features  (in the installed runtime? · used by the integration?)
  tools       in the runtime · used: tools, toolsConfig, src/ai/ai-tools.json
  memory      NOT in the runtime (arrives with 1.3.0) · to add: comes with the runtime; seed ai-memory.json
  vision      NOT in the runtime (arrives with 1.3.0) · to add: comes with the runtime; decide on a `screenshot` hook
  attachments NOT in the runtime (arrives with 1.5.0) · to add: comes with the runtime (the + button); a `readFile` hook only for the app's own file formats
  capabilities NOT in the runtime (arrives with 1.6.0) · not adopted — offer the capability folder (references/upgrading.md, "Adopting the framework")
  agents       NOT in the runtime (arrives with 1.6.0) · none — the app has its one implicit agent; offer agents where users do distinct kinds of work
  skills       NOT in the runtime (arrives with 1.6.0) · none — offer skills for multi-step work the app's users repeat
  workspace    NOT in the runtime (arrives with 1.6.0) · off — development only (references/in-app-authoring.md)
```

Here the tools are done (leave them), and the upgrade is: the runtime, then the memory file and the screenshot
decision; attachments come with the runtime. "Check for memory and vision" lists canvases/WebGL views and policy headers that matter for screenshots. Without Node, search for `ai-agent.js` containing `export const VERSION`, for `createAiAgent(`, and
for `ai-enablement.json` (or the 1.x `ai-agent.integration.json`) and an `index.json` with `"format": "ai-enablement/1"`.

Then read, before changing anything:

- **The manifest** (`ai-enablement.json`) or the 1.x record (`ai-agent.integration.json`), if there is one: features
  in use, the context and tool plans, what the user declined.
- **The capability folder**, if there is one: `node <skill>/scripts/validate.mjs <app-root>` before the upgrade, so
  what was already broken is not blamed on it.
- **The integration module** (the file that calls `createAiAgent`), the content builders and hooks, custom CSS for the
  drawer, and the relay config — enough to restate the current context plan in a few lines.
- **Edited copies.** Detect says whether the runtime or relay files differ from their release. An edited copy must not
  be overwritten blindly: find the change in the app's history (`git log --follow -p -- <runtime dir>/<file>`), then
  turn it into an option or config setting of the new version, or keep it as a documented patch and tell the user
  (so it can be folded back into the skill).
- **More than one** runtime copy or `createAiAgent()` call: find which one the app really loads; plan to remove the rest.

## Layout guards (every run, whatever the task)

Since runtime 1.6.1 the runtime's stylesheet carries two **layout guards** for the settings dialog
(`references/settings-layout.md`): **pinned chrome** — its header, tab strip and footer never shrink, only the body
scrolls (before, a tall tab such as Tools clipped the tab strip behind the body) — and **host isolation** — the page's
own element rules (`label { display: flex; justify-content: space-between }`, `button { … }`, `p`, `h2`,
`body { text-align: center }`…) no longer reach the drawer or the dialog (before, they centred the labels, shrank the
input rows and pushed checkbox text away from its box).

Every time the skill runs on an app that already has the agent — install, upgrade, "add a tool", anything — check
detect's `Layout guards` lines (`layoutGuards` in `--json`; `node <skill>/scripts/guards.mjs <app-root>` prints only
these):

```
Layout guards  (ai-agent.css, runtime 1.6.1+: the settings dialog keeps its tabs and ignores host element CSS)
  public/ai-agent/ai-agent.css  pinned chrome: MISSING · host isolation: MISSING — FIX IN THIS RUN: replace the runtime (unchanged 1.6.0 copy; references/upgrading.md, U4)
```

A guard that is MISSING is fixed **in this run**, as part of whatever the user asked for:

- **Runtime unedited since a release** (detect: "unchanged copy", `fix: "replace"`): replace the runtime as in U4. That
  is the whole runtime upgrade to the skill's version: read its CHANGELOG entries and U4's behaviour notes as for any
  upgrade.
- **Runtime edited** (detect: "EDITED", or an unknown release; `fix: "patch"`): apply the guard blocks to the app's
  `ai-agent.css` in place and keep the app's other edits:

  ```bash
  node <skill>/scripts/guards.mjs <app-root> --apply
  ```

  It inserts `/* aia-guard: host-isolation */` after the theme tokens (it must come before every component rule) and
  appends `/* aia-guard: pinned-chrome */`, only the ones missing, and records the patch in the manifest
  (`ai-enablement.json`, or the 1.x `ai-agent.integration.json`) under `"patches"`. If the app's only edits are
  what the guards cover (the "head, tabs and foot never shrink" rules some apps added by hand), replace the runtime
  instead — nothing is lost. An in-place patch stops element rules and the squeezed tab strip; host rules with an
  attribute or state (`input[type=checkbox] { margin }`, `button:hover { transform }`) are only beaten by the
  current runtime's own rules, so reconcile the app's edits onto the new runtime when you can (U1, "Edited copies").

Then tell the user in one line ("Also fixed the settings dialog: its tabs no longer clip and the page's own CSS no
longer leaks in."), check it (`verify.mjs` → `settings:<tab>` lines; by hand, every Settings tab in the real app), and
remove the app-side workarounds it makes redundant (U5). A fix made by hand in the stylesheet counts: detect reads its
rules (with their specificity), not only the marker comments.

## U2. Read what changed

Read `CHANGELOG.md` from the installed version up to the current one: new features, behaviour changes, and each
version's **Upgrading** notes. They say what to migrate and which app-side workarounds the new runtime makes redundant.

## U3. Propose the upgrade (a few lines to the user)

- Replace the runtime `X → Y` (and the relay `X → Y`), with the relay migration if any.
- Workarounds to remove (from detect's hints and your reading).
- New features that fit **this** app, as a short menu with one line each (U6) — the user picks.
- What stays as it is: the `appId`, the context hooks, actions, prompts, the app's CSS for the drawer.

## U4. Replace the runtime and the relay

- **Runtime**: replace the contents of the app's runtime folder with the skill's `assets/ai-agent/` — delete the old
  files first so files removed in newer versions do not linger. Same folder, so import paths and the stylesheet link do
  not change. Bundled apps: rebuild.
- **Relay**: copy `relay.php` / `relay.mjs` unchanged over the old file. Keep the existing config file where it is.
  From a 1.0 relay, move edits made inside the file into a `relay.config.php` (outside the web root, or next to the
  relay in development — see `providers.md`):

  | 1.0 (edited in relay.php) | 1.1+ (relay.config.php) |
  | --- | --- |
  | `AIA_ALLOW_REMOTE = true` | `'allowRemote' => true` — and add an `'authorize'` check |
  | `AIA_ALLOW_ANY_UPSTREAM = true` | `'allowAnyUpstream' => true` |
  | `AIA_KEYS = ['openai' => '…']` | `'keys' => [...]` or environment variables (never print the values) |
  | `AIA_TIMEOUT`, `AIA_MAX_BODY` | `'timeout'`, `'limits' => ['maxBodyBytes' => …]` |
  | sign-in/CSRF code at "enforce it here" | `'authorize' => static function (): bool\|string { … }` |

  An embedded Node relay: update its imports to the new `relay.mjs` exports (`createRelay`, `loadConfig`).
- **Capability folder** (if the app has one): never replaced. Validate it after the runtime is in.
- **Framework behaviour from runtime 1.6.1** (check the app against it; every option keeps working):
  - Every runtime CSS rule is `.aia-scope .aia-x` (specificity (0,2,0) or more). App CSS that restyles a runtime
    class with a single class (`.aia-btn-primary { … }`) no longer wins by load order: write it
    `.aia-scope .aia-btn-primary` (loaded after `ai-agent.css`), or better use the `--aia-*` variables. Theme
    variables on `.aia-scope` work as before.
  - The page's element rules (`label`, `button`, `input`, `select`, `p`, `h1`–`h6`, `section`, `header`…) no longer
    reach the drawer or the settings dialog (`references/settings-layout.md`). An app that styled the agent through
    them loses that styling.
  - The settings dialog's tab strip keeps its full height on tall tabs (it used to be squeezed and clipped).
  - `devWarnings` also checks the settings dialog against its layout spec when it opens, and warns when host CSS leaks
    in.
- **Framework behaviour from runtime 1.6** (check the app against it; every 1.5 option keeps working):
  - Effects `external` and `system` exist; `toolSpecs` describe them. An unknown effect still means `write` (asks
    first), as before; `validate.mjs` now reports it.
  - Tools receive `host` in their context (`run(args, { host, agent, signal, call })`) — `undefined` unless the app
    passes `host`, so existing tools are unaffected.
  - New system prompt sections: `== AGENT: … ==` (only with agent files) and `== SKILLS ==` (only with skills).
    Without either, the prompt is as in 1.5.
  - Settings > Tools shows permission badges; Settings > Agent lists agents and skills when there are any.
  - The drawer title becomes an agent picker when there are two or more agents (`.aia-agent-pick`).
  - Saved chats keep `agent` and `skills` (older chats load as before).
  - `exportToolsConfig()` / "Download ai-tools.json" writes a new `$comment` text (the format is unchanged).
- **Caches**: bump the version in the asset URLs (`?v=1.6.1`) or rely on `no-cache`, so browsers load the new files.
  A bundled app also gets new runtime files (`core/files.js`, `bytes.js`, `pdf.js`, `office.js`); the PDF and office
  readers are loaded with `import()`, which every bundler splits into its own chunk.
- **Attachment behaviour from runtime 1.5** (check the app against it):
  - The composer has a **+** button left of the message field (`.aia-plus`), and the drawer takes drops and pasted
    files: a drop on the drawer no longer reaches the host page's own `drop` listeners. App CSS that styles
    `.aia-composer > *` or assumes the textarea is the composer's first control needs a look.
  - `agent.ask(text)` (and the welcome suggestions) now take what waits in the composer along, as Send does.
  - The system prompt's paragraph on images starts "Images:" (it was "Screenshots:") and also covers image files;
    an *Attached files* paragraph is added while a conversation carries files. App tests that match the old wording
    need updating.
  - `screenshots: false` keeps Settings > Vision while attachments are on ("This model can see images" also governs
    attached images); `attachments: false` as well removes it.
  - New setting `maxFileChars` (Settings > Agent > *Max file content*, 40,000 by default): set a lower default for a
    local model with a small context.
- **Web server limits** when the app uses a relay and screenshots: request bodies grow (see `providers.md`).
- **Tool-call behaviour from runtime 1.4** (check the app's tools against it):
  - Text over a string parameter's `maxLength` — **500 when unset** — is an error the model is told about, no longer
    cut. Give every parameter that takes long text (documents, note lists, JSON) an explicit `maxLength`.
  - Arguments that cannot be read (broken or cut-off JSON) are reported to the model and the call is not run.
    `parseArguments()` (`core/tools.js`) returns `{}` for such text instead of a guess from the `key: value` parser.
  - Tool calls from the adapters and `streamChat()` carry `raw` (the model's argument text) and, when unreadable,
    `argsError`; app code that compares whole call objects must allow for them. Calls sent back to the provider are
    stripped to the wire fields. A 1.4 relay sends `tool_call.raw` and `done.truncated`; older relays still work.
  - A tool row's line is now a `<button class="aia-tool-line">` (it was a `div`) with a caret, and finished rows roll
    down. Check any app CSS that styles `.aia-tool-line` or `.aia-tool`. The reply action id `copy-tools` is taken by
    the built-in *Copy tool log*.

## U5. Remove workarounds the new runtime covers

Remove each only after checking the new behaviour covers what the app needed (for the layout entries: `verify.mjs`'s
`settings:<tab>` checks pass with the workaround taken out):

| Workaround in the app | Since | Replace with |
| --- | --- | --- |
| Own throttle/interval around `contextChanged()` | 1.1 | Call `contextChanged()` on every update; `debounceMaxMs` paces the flag. |
| `showModal` overrides, switching dialogs to non-modal and back, swallowing `close` events | 1.1 | `dialogs: 'dock'` (keep including the dialog's contents in the page content). |
| Key filters so host shortcuts ignore the drawer | 1.1 | `isolateKeys` (default). Keep `closest('.aia-scope')` checks in **capture-phase** host handlers. |
| Probing the relay before `createAiAgent` | 1.1 | `relayProbe: true` + `defaults.relayUrl`. |
| Theme overrides with `[data-aia-theme]` / `:not(...)` selectors | 1.1 | A plain `.aia-scope { --aia-… }` rule (wins in light and dark). |
| Badge colours via `::after` overrides | 1.1 | `--aia-badge-*` variables. |
| Strict `when: (b) => b.language === 'x'` on value-applying code actions | 1.1 | `parseBlockValues` + `setControlValue`. |
| Code actions that are really app operations ("Apply filter") | 1.2 | A tool (the model calls it; the user confirms), optionally keeping the button. |
| "Things to remember" pasted into the system prompt or the app context; an app-side notes list for the agent | 1.3 | `ai-memory.json` + `memoryFile` (users edit them in Settings > Memory). Keep in the app context what describes the app for everyone. |
| An app-side "send a screenshot" button, or `canvas.toDataURL()` pasted into questions | 1.3 | The camera button; the app's capture code becomes the `screenshot` hook. |
| An app-side "send a file to the agent" button, `FileReader` code that pastes file text into `agent.ask()`, a drop handler on the drawer | 1.5 | The + button (and drag and drop, paste). A reader for a format the runtime does not read (or pdf.js the app already ships) becomes the `readFile` hook. |
| App CSS that pins the settings dialog's frame: `flex-shrink` / `flex` on `.aia-tabs`, `.aia-modal-head`, `.aia-modal-foot`; `min-height` on `.aia-modal-body` | 1.6.1 | Nothing: the pinned-chrome guard (detect lists such rules under "Check"). |
| App CSS that fights the host's own element rules inside the agent: `!important` on `.aia-field`, `.aia-check`, `.aia-row`, `.aia-label`, `.aia-section`, `.aia-btn`…; resets like `.aia-scope label { display: block }` | 1.6.1 | Nothing: the host-isolation guard. Restyling that is a real design choice stays, written `.aia-scope .aia-x` (U4). |

Keep the layout CSS under `html.aia-drawer-open` — that is still the recipe.

## U6. Offer the new features

Offer only what fits the app; record what the user declines (U7) so the next upgrade does not ask again.

| Feature | Since | Fits when | What it takes |
| --- | --- | --- | --- |
| **Tools** — the agent acts in the app | 1.2 | Users do things on the screens (almost every app) | The tool plan (SKILL.md steps 1–2), tool modules over the app's own functions, `ai-tools.json` with the user's choice of what is on, tests (`tools.md`, `capabilities.md`). |
| **Capability folder** — tools, toolsets, permissions in `ai/` | 1.6 | Before adding skills, agents or in-app authoring; or to organise many tools | "Adopting the framework" below. |
| **Skills** — written procedures the agent loads | 1.6 | Users repeat multi-step work (proofread, reconcile, design) | `scaffold.mjs … skill`; steps naming the tools (`capabilities.md`). |
| **Agents** — roles the user picks | 1.6 | Distinct kinds of work needing their own instructions, tools or permissions | `scaffold.mjs … agent`; compose existing tools, toolsets, skills. |
| **Permissions** — allow / ask / deny | 1.6 | Something must always confirm, or never run, whatever users switch | `permissions` in the index or an agent (`spec/framework.md`). |
| **In-app authoring** — the app's AI adds tools | 1.6 | Developers want to grow the tool set from inside the app | `host`, `workspace: true` in development, `scripts/workspace.mjs` (`in-app-authoring.md`). |
| **Memory** — notes kept between conversations | 1.3 | Always (it is on once the runtime is replaced) | Seed notes the screen does not show — ask the user — in `ai-memory.json`, loaded with `memoryFile`; `memorySave` if the app's backend should keep users' notes (`memory-and-vision.md`). |
| **Vision** — screenshots for models that see images | 1.3 | Always (on once the runtime is replaced); most useful where the screen is visual: canvases, WebGL, charts, layouts | Decide how the picture is taken: the browser's screen capture (nothing to write) or a `screenshot` hook for a canvas view; `defaults.vision` for the app's default model; host requirements; relay 1.3 (`memory-and-vision.md`). |
| **Attachments** — the + button: images and files with a question | 1.5 | Always (on once the runtime is replaced) | Nothing, usually. `defaults.maxFileChars` for a small-context model; `readFile` for the app's own file formats; `attachments: false` where users' documents must not reach the model provider (`memory-and-vision.md`, "Attachments"). |
| Dialog docking | 1.1 | The app uses `dialog.showModal()` | `dialogs: 'dock'`. |
| Relay auto-detection | 1.1 | The app runs with and without its relay | `relayProbe`. |
| Relay public mode | 1.1 | Deployed for visitors, server-held key | `relay.config.php` with `'mode' => 'public'`, a preset and limits. |
| Layout check / recipe | 1.1 | Fixed-width shell, wide header | Fix what the dev warning reports (`frameworks.md`, "Layout"). |
| Value code actions for small models | 1.1 | Actions that apply settings/values | `parseBlockValues` / `setControlValue`. |

Automatic with the new runtime (mention, nothing to do): key isolation, the max-wait debounce, theming fixes, the
context-size estimate, resume replay, relay error messages — and, from 1.3, the Memory and Vision tabs, the camera
button, and the built-in `remember` / `forget` / `take_screenshot` tools; from 1.5, the + button (attach images and
files), drag and drop and paste onto the drawer; from 1.6, permission badges in Settings > Tools, MCP export
(`agent.tools.mcp()`), and `inputSchema` / `annotations` on tools. Tell the user they are there even if nothing app-specific is added,
and how to switch them off (`memory: false`, `screenshots: false`, `attachments: false`, or in Settings).

**Adding memory and vision to an app at 1.2 or older** (what "only the pieces it does not have" comes to):

1. Replace the runtime (U4). Memory and vision now work with their defaults.
2. Memory: collect the seed notes with the user, write `ai-memory.json` beside the tool config, add
   `memoryFile: 'ai-memory.json'` to the existing `createAiAgent()` call. Move "remember this" text out of the system
   prompt into it (U5).
3. Vision: look at detect's canvas/WebGL list and the app's main view; keep the screen capture or add a `screenshot`
   hook; set `defaults.vision` to match the default model.
4. Relay in use: replace it too (1.3 passes images; an older one is refused with a message when a question carries a
   screenshot) and raise the web server's body limit.
5. Verify (`verify.mjs` reports `memory` and `vision`), and record the features (U7).

## Adopting the framework (an app with the agent but no capability folder)

Runtime 1.6 loads tools, toolsets, skills and agents from a capability folder. An app that passes its tools inline
(`tools: appTools`) keeps working unchanged: adoption is **offered, never forced** — propose it when the user wants
skills or agents, asks for a new tool, or wants the app's AI to add tools itself (`in-app-authoring.md`).

1. `node <skill>/scripts/scaffold.mjs <served-dir>/ai init` — next to the app's static files (`spec/framework.md`,
   "Bundled apps" for apps built from `src/`).
2. **Move, do not rewrite, the tools.** Give the integration a `host` (the object the tools close over: the store, a
   service layer, the app instance). Move each tools module into `ai/tools/` as a toolset module whose tools take
   `host` from `run(args, { host })` instead of a closure — the bodies stay the same. A factory like
   `export function appTools(app) { return [...] }` becomes `export default (host) => [...]` with no other change.
3. Move `ai-tools.json` and `ai-memory.json` into `ai/` (keep their content) and name them in the index
   (`"toolsConfig"`, `"memory"`); add the modules to `"tools"`.
4. In the integration: `capabilities: '<URL of ai/index.json>', host: …` in place of `tools`, `toolsConfig`,
   `memoryFile`. Same `appId`.
5. `validate.mjs`, the app's tool tests (with `host` mocked), `verify.mjs`: the *tools* line lists the same tools,
   the *capabilities* line the index.

Hello World went through exactly this (`examples/hello-world/`: `ai-tools.js` → `ai/tools/{document,editor,file}.js`).

## U7. Verify, record, report

1. Run the app's own tests, `node <skill>/scripts/validate.mjs <app-root>` (if there is a capability folder), then
   `node <skill>/scripts/verify.mjs <url> [--change …]` (its `settings:<tab>` lines check the dialog's layout on every
   tab), and check by hand what was added (SKILL.md step 14). Confirm the saved chats and settings of the old version
   are still there (same `appId`).
2. Write or update **`ai-enablement.json`** (below). An app with the 1.x record `ai-agent.integration.json`: write the
   manifest from it (keep every field the record had) and delete the record — `git` shows it as a rename.
3. Report: versions before → after, what was replaced, migrated and removed, the features added and declined, and
   anything you could not verify.

## The manifest: `ai-enablement.json`

Written at the end of every install, upgrade and capability change, at the app's repository root, and committed with
the app. It tells the next run exactly what is there and who owns it. Never secrets: it is committed (validate
refuses one that looks like it holds a key).

```json
{
  "skill": "ai-enablement",
  "skillVersion": "2.0.1",
  "runtimeVersion": "1.6.1",
  "updated": "2026-10-02",
  "appId": "inventory",
  "stack": { "id": "web", "host": "php" },
  "framework": {
    "runtime": "public/ai-agent",
    "relay": { "file": "public/api/relay.php", "version": "1.4.0", "mode": "public", "config": "AIA_RELAY_DIR (outside the web root)" }
  },
  "capabilities": {
    "index": "public/ai/index.json",
    "agents": ["stock-clerk", "buyer"],
    "skills": ["reorder-check"],
    "toolsets": ["orders", "items"],
    "tools": 9,
    "permissions": "ask: toolset:orders · buyer denies cancel_order"
  },
  "integration": ["public/js/ai-agent-setup.js", "public/js/ai-content.js"],
  "toolsConfig": "public/ai/ai-tools.json",
  "memoryFile": "public/ai/ai-memory.json",
  "features": ["tools", "memory", "vision:hook", "attachments", "capabilities", "agents", "skills", "dialogs:dock", "relayProbe", "codeActions"],
  "memory": { "seeded": 4, "userNotes": "browser (no memorySave)" },
  "vision": { "capture": "screenshot hook: the WebGL view (js/view.js)", "defaultModelSees": true, "agentMayLook": false },
  "declined": ["relay public mode"],
  "pages": [
    { "id": "orders", "content": "visible rows with status and totals", "tools": ["filter_orders", "open_order", "cancel_order"] }
  ],
  "customizations": ["theme via css/ai-agent-theme.css", "layout fix for .app-shell under html.aia-drawer-open"],
  "patches": ["public/ai-agent/ai-agent.css: aia-guard host-isolation from ai-enablement 2.0.1 (runtime 1.6.1) applied in place to an edited copy (2026-10-03)"],
  "notes": "Anything the next person should know."
}
```

- `stack` (skill 3.0) pins the app's tech stack for every later run: `id` is `web`, `rust-egui` or `dotnet-winforms`
  (the recipes in `stacks/`), plus `host` for web apps (`static`, `php`, `node`, `tauri`, `aspnet`) and optionally
  `ui` (`"egui 0.37"`). detect.mjs reports the stack it sees; the manifest's wins, and a disagreement is flagged. An
  older manifest without it gets a note from validate: add it.
- `framework` names the framework-owned files: with the skill's release fingerprints they tell an unchanged copy
  (replace it) from an edited one (reconcile it). Everything else listed is app-owned.
- `features` uses these names so the next run can tell what is there: `tools`, `memory`, `vision:screen` (the
  browser's screen capture) or `vision:hook`, `attachments` (or `attachments:readFile`), `capabilities`, `agents`,
  `skills`, `workspace`, `dialogs:dock`, `relayProbe`, `codeActions`, `replyActions`. A feature the user switched off
  goes into `declined` (`"memory (memory: false)"`), so the next run does not offer it again.
- `patches`: changes made inside a framework-owned file that is kept as an edited copy — what, from which skill
  version, when (`scripts/guards.mjs --apply` writes its own entry). The next upgrade reads them before replacing the
  file.
- The 1.x record had the same fields with `"skill": "add-ai-skill"`, `"runtime"` and `"relay"` at the top level
  instead of under `framework`, and no `capabilities`; detect and validate read both.
