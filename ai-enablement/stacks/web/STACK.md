# Stack recipe: web

Status: **supported**. Runtime: `assets/ai-agent/` (JavaScript, vanilla ES modules, no build step). For any app whose
UI is an HTML page: plain HTML, PHP, React/Vite, Next.js, Vue, Svelte, Angular, and also **Tauri** (a webview over a
Rust backend) and **ASP.NET Core / Blazor** (HTML from a C# backend). detect.mjs reports the host: `static`, `php`,
`node`, `tauri` or `aspnet`.

The section numbers below are the steps in `SKILL.md`; each says *how* on the web. The portable rules stay in
SKILL.md and `spec/`. Web references: `references/frameworks.md` (per framework), `references/api.md` (every option),
`references/tools.md`, `references/memory-and-vision.md`, `references/providers.md` (relays), `references/settings-layout.md`
(host CSS), `references/checklist.md`, `references/upgrading.md`.

## 1. Survey: what to look for in a web app

In addition to the portable survey in SKILL.md:

- **Entry points**: where the root layout is rendered, whether code is bundled from `src/` or served as is.
- **Production**: web server (Apache, Nginx, Node, static hosting), PHP or Node, long-running processes allowed, how
  development differs. This decides the relay (§13). Never plan on `.htaccess` or rewrites.
- **Static assets**: where they are served from (`public/`, `static/`, `assets/`), how caches are busted.
- **The action surface**: API clients, store actions (Redux, Pinia, Zustand), services, handlers behind buttons,
  menus and forms, routers, controls with their real ranges. **One object** that reaches them becomes `host`.
- **Layout**: the header/toolbar for an "Ask AI" button; the main content container that makes room for the drawer;
  fixed-width grids, wide toolbars, fixed elements.
- **Global keyboard handlers** and **modal dialogs** (`showModal()`, focus traps).
- **How the view is drawn** (DOM, canvas, WebGL), iframes, phones, `Permissions-Policy`, `Content-Security-Policy`.
- **Backend**, auth/CSRF, deployment beyond localhost. **Theme**.

## 3. Install the runtime

Copy the whole `assets/ai-agent/` folder into the app, unchanged, next to the static assets (`assets/ai-agent/`,
`public/ai-agent/`), or to `src/lib/ai-agent/` for bundled apps. It is framework-owned: configure it through
options, never edit it. If a change is truly needed, make it generic and tell the user so it can be folded back into
the skill. Version the asset URLs or serve them `no-cache`.

## 4. The capability folder

```bash
node <skill>/scripts/scaffold.mjs <served-dir>/ai init       # index.json, ai-tools.json, ai-memory.json
```

It must be **served** next to the app, for example `public/ai/`, because the runtime fetches its Markdown and JSON and
imports tool modules. Bundled apps whose modules live in `src/` keep the folder under `public/` for skills and agents.
Their tool modules are then either served too, or imported statically and passed as `tools` (`spec/framework.md`,
"Bundled apps").

## 5. Mount the agent once

One integration module (e.g. `assets/js/ai-agent-setup.js`, `src/ai/agent.ts`), loaded with `import()` and a catch so
the app works without it:

```js
// in the app's entry script
import('./ai-agent-setup.js').then((m) => m.mountAgent()).catch((e) => console.warn('AI agent unavailable', e));
```

```js
// assets/js/ai-agent-setup.js
import { createAiAgent, DEFAULT_SYSTEM_PROMPT } from '../ai-agent/ai-agent.js';
import { store } from './store.js';                          // the object the tools call: `host`
import { buildItemsContent } from './ai-content.js';         // pure, unit-tested (§6)
import { VIEWS } from './views.js';

export function mountAgent() {
  const agent = createAiAgent({
    appId: 'inventory',                          // unique per app, never changed (namespaces users' data)
    title: 'Inventory assistant',
    toggle: '#ask-ai',
    push: 'main',
    capabilities: 'assets/ai/index.json',        // tools, toolsets, skills, agents, permissions, defaults, memories
    host: store,                                 // tools: run(args, { host })
    // workspace: location.hostname === '127.0.0.1',   // development: in-app tool authoring (in-app-authoring.md)
    app: { name: 'Inventory', purpose: 'Track stock levels and purchase orders.', capabilities: VIEWS.map((v) => `${v.title}: ${v.summary}`), limits: ['Cannot place orders with suppliers'] },
    page: {
      id: 'items', title: 'Items', purpose: 'Stock items with quantity, location and reorder level.',
      content: () => buildItemsContent(store.getState()),      // what is on screen (hashed)
      view: () => ({ selected: store.getState().selectedIds }), // volatile (not hashed)
    },
    systemPrompt: `${DEFAULT_SYSTEM_PROMPT}\n\nYou are the inventory assistant. …domain rules…`,
    welcome: 'I can see the items on your screen. Ask me about stock or reorders.',
    defaults: { provider: 'lmstudio' },
  });
  store.subscribe(() => agent.contextChanged());
  document.getElementById('ask-ai').hidden = false;
  return agent;
}
```

Client-side only, once. `agent.ready` resolves when the capability index, tool config, memory file and relay probe
are in. Recipes per framework: `references/frameworks.md`. Every option and method: `references/api.md`. The reference
integration: `examples/hello-world/ai-agent-setup.js`.

## 6. Pages

`agent.setPage({ id, title, purpose, content, view })` on navigation. **content** comes from a pure, unit-tested
builder over the app's state, not the DOM. Use `fromDom('main')` only when there is no state model. Details:
`spec/context-sync.md`.

## 7. Change signal

`agent.contextChanged()` wherever content can change: store subscription, fetch completion, edits, worker messages.
It is debounced with a max wait, so continuous updates need no throttle. `watch: 1500` polls as a last resort.
`agent.onContextStatus(fn)` mirrors the flag in the app.

## 8. Fit the host app

- Typing never reaches host shortcuts (`isolateKeys`); capture-phase handlers skip `.aia-scope`.
- `dialogs: 'dock'` for `showModal()`.
- Layout fixes go under `html.aia-drawer-open` (`--aia-push-width`).
- Theme with `--aia-*` on `.aia-scope`.
- The page's own element CSS (`label`, `button`, `input`, `p`, `body { text-align }`…) does not reach the agent's UI:
  never "fix" the drawer or the settings dialog from app CSS (`references/settings-layout.md`).
- **Layout guards**: on every run, read detect's `Layout guards` lines and fix a MISSING guard in that run.
  - An unchanged runtime copy is replaced (`references/upgrading.md`, U4).
  - An edited copy is patched in place with `node <skill>/scripts/guards.mjs <app-root> --apply`.
  - Tell the user in one line.

## 9. Tools

One tool module per toolset (or per tool) in `ai/tools/`, following `spec/framework.md` and `references/tools.md`:

```bash
node <skill>/scripts/scaffold.mjs <ai-dir> tool filter_orders --effect write --description "Show only orders with this status."
```

```js
export default {
  name: 'orders', title: 'Orders', description: 'The order list.',
  tools: [{
    name: 'filter_orders', effect: 'write', description: 'Show only orders with this status.',
    parameters: { status: { type: 'enum', values: ['open', 'shipped', 'cancelled'], required: true } },
    run: ({ status }, { host }) => { host.setFilter({ status }); return `Showing ${host.visibleCount()} ${status} orders.`; },
  }],
};
```

The handler calls the app through `host`, or drives the real control with `setControlValue`. Unit-test each module
with `host` mocked.

## 11. Memory, vision, attachments

All three work once the runtime is in. User notes are stored in the browser, or through `memorySave` to the app's
backend. Screenshots use the browser's screen capture of the tab. For a canvas or WebGL view, pass a `screenshot` hook
instead. `readFile` is only for the app's own file formats. Details: `references/memory-and-vision.md`.

## 12. App-specific actions

`codeActions` (buttons on fenced code blocks) and `replyActions` (on whole replies). Values are applied through the
real controls with `parseBlockValues` + `setControlValue`.

## 13. Transport and relay

Direct by default; for LM Studio, enable CORS in its server. A relay is needed when any of these hold:

- the app is deployed beyond localhost;
- keys must stay on the server;
- the provider blocks browsers.

Copy `assets/relay/relay.php` or `relay.mjs` unchanged (framework-owned). App settings go in its config file: local mode
by default, or public mode with a fixed preset, same-origin and rate limits. In the app:
`relayProbe: true, defaults: { relayUrl: 'api/relay.php' }`. Details: `references/providers.md`.

## 14. Verify

```bash
node <skill>/scripts/validate.mjs <app-root>
node <skill>/scripts/verify.mjs http://127.0.0.1:8787/ --change "<js that changes the screen>"   # --no-llm without a model
```

Then by hand at 1920×1080: run through the list in `references/checklist.md`.

## 15. Manifest

`"stack": { "id": "web", "host": "<static|php|node|tauri|aspnet>" }`, plus the 2.0 fields (`references/upgrading.md`,
"The manifest").

## Tauri (host: tauri)

The UI is a webview, so the web runtime draws the agent. Everything above applies, with these differences:

- **Survey**: the action surface is mostly the Rust side, the `#[tauri::command]` functions registered with
  `invoke_handler` (and plugin commands). Classify each one as read / write / destructive / external / system as usual.
  Note the front-end framework under `src/` (§5 recipes apply).
- **`host`**: an object in the integration module that wraps `invoke`, so tool modules never import Tauri:
  `const host = { listNotes: () => invoke('list_notes'), saveNote: (n) => invoke('save_note', { note: n }) }`. The
  business logic stays in the Rust commands; a tool never duplicates it.
- **Runtime and `ai/`** live in the front end's static folder (`public/` for Vite) and are bundled into the app. In
  development, the dev server serves them.
- **Transport**: the webview's origin is `tauri://localhost` or `http(s)://tauri.localhost`.
  - LM Studio and Ollama need CORS allowed for it (`OLLAMA_ORIGINS`, LM Studio's "Enable CORS").
  - Add the provider URLs to `connect-src` in the CSP in `tauri.conf.json`. Allow `data:` and `blob:` in `img-src` for
    screenshots and attachments.
  - There is no PHP or Node server in a shipped Tauri app. For a provider that blocks browsers, or a key that must not
    live in the webview, add one Rust command that forwards the request. Make it loopback-only for local providers, and
    keep the key on the Rust side in the OS keychain. This replaces the relay. Never ship a key inside the bundle.
- **Verify**: `verify.mjs` drives the dev server URL (`npm run tauri dev` serves it, usually
  `http://localhost:1420`); then check the packaged app by hand, maximised.

## ASP.NET Core / Blazor (host: aspnet)

The UI is HTML, so the web runtime draws the agent. Everything above applies, with these differences:

- **Survey**: pages are Razor Pages, MVC views or Blazor components; the root layout is `_Layout.cshtml`,
  `App.razor` or `MainLayout.razor`. The action surface is the app's JS (if any) and its HTTP endpoints, controllers
  or minimal APIs. Tools reach the endpoints through `host`, an object in the integration module wrapping `fetch` to
  the app's own API, with its antiforgery token.
- **Runtime and `ai/`**: `wwwroot/ai-agent/` and `wwwroot/ai/` (served by `UseStaticFiles` / `MapStaticAssets`).
  Load the integration with `<script type="module">` in the layout. In Blazor Server and WebAssembly, run it once
  after the first render (`IJSRuntime` from `OnAfterRenderAsync(firstRender)`). Call `agent.setPage(…)` and
  `agent.contextChanged()` through a small JS interop module on navigation and after state changes.
- **Relay**: there is no PHP. Use `relay.mjs` where Node may run beside the app. Otherwise go direct to a local model,
  or forward from a small authenticated endpoint in the app that keeps the key in configuration or user secrets.
- **Verify**: as above, against `dotnet run`'s URL.

## Quick API (JS runtime)

```
createAiAgent(options) -> agent        options: appId, title, app, page, systemPrompt, welcome, suggestions, toggle,
                                                push, hotkey, theme, width, watch, debounceMs, debounceMaxMs, isolateKeys,
                                                dialogs, devWarnings, relayProbe, contextWarnTokens, tools, toolsConfig,
                                                host, capabilities, agents, skills, toolsets, permissions, agent, workspace,
                                                memory, memoryFile, memorySave, screenshots, screenshot, screenshotMaxEdge,
                                                attachments, readFile, codeActions, replyActions, defaults, relayHeaders,
                                                saveChats, resume, mount
agent.setApp · setPage · setContent · setView · contextChanged · refreshContext · getContextStatus · onContextStatus
agent.tools.list · register · unregister · setEnabled · run · exportConfig · toolsets · mcp
agent.agents.list · current · use(name)        agent.skills.list · activate(name) · active
agent.capabilities.reload · problems · index    agent.workspace()
agent.memory.list · add · update · remove · clear · export · import     agent.screenshot() · agent.attach(files)
agent.open · close · toggle · isOpen · ask · stop · newChat · openSettings · systemPrompt · ready · relayInfo · destroy
agent.on('open'|'close'|'send'|'reply'|'error'|'context'|'settings'|'relay'|'tool'|'tool-state'|'memory'|'screenshot'|'attach'|'agent'|'skill'|'capabilities'|'workspace', fn)
fromDom · parseBlockValues · setControlValue · probeRelay · toMcpTool · fromJsonSchema · loadCapabilities · parseSkill · parseAgent
```

Full reference with types: `references/api.md` and `assets/ai-agent/ai-agent.d.ts`.
