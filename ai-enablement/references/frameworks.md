# Framework recipes

The runtime is plain ES modules plus one stylesheet. Every recipe below does the same four things:

1. load `ai-agent.css` once;
2. create the agent **once, on the client**;
3. give it the current page (`setPage`) on every navigation, with `content`/`view` hooks that read the latest state;
4. call `contextChanged()` when that state changes.

In frameworks, prefer a button in your own component that calls `agent.toggle()` (with `launcher: false`) over a
`toggle` selector — the selector must already exist in the DOM when the agent is created. Show the sync flag with
`agent.onContextStatus()` (it provides `state` and `hash`).

## Loading and mounting safely (every stack)

- **Load the integration with `import()` and catch failures**, so the host app keeps working when the agent cannot
  load (a network error, a stale cached file after a deploy, an old browser):

  ```js
  // in the app's own entry script
  import('./ai-agent-setup.js')
    .then(({ mountAgent }) => mountAgent(app))
    .catch((e) => console.warn('AI agent unavailable; the app works without it.', e));
  ```

  Keep the AI controls (toggle, settings button, status chip) hidden until `mountAgent` shows them.
- **Mounting may be async.** With `relayProbe` or async `defaults`, `createAiAgent()` still returns at once and
  `agent.ready` resolves when the defaults are settled; questions wait for it. Await `agent.ready` only for code that
  reads `agent.settings.get()` or `agent.relayInfo()`.
- **Open-time setup.** `resume` can reopen the drawer during `createAiAgent()`. The `open` event is replayed (with
  `{ resumed: true }`) to listeners registered in the same tick; if you register later, also run the setup once when
  `agent.isOpen()` is already true. Make open-time setup idempotent.
- **Versioned assets.** Browsers may heuristically cache an old entry script after a deploy: the toggle then does
  nothing and the console shows no error. Version the URLs of the integration and runtime (`ai-agent-setup.js?v=1.1.0`,
  or a build hash), or send `Cache-Control: no-cache` for them. Hard reload (Ctrl+Shift+R) to confirm.

---

## Where the capability folder goes (every stack)

The runtime fetches `ai/index.json`, skills and agents, and imports tool modules, so the folder must be **served**:

| Stack | Capability folder | Tool modules |
| --- | --- | --- |
| Plain HTML / PHP / server-rendered | next to the other static assets: `assets/ai/`, `public/ai/` | in `ai/tools/`, loaded by the index |
| Vite (React, Vue, Svelte), CRA | `public/ai/` (served as-is) | in `public/ai/tools/` (they reach the app only through `host`), or imported in the integration module and passed as `tools` |
| Next.js | `public/ai/` | the same; the integration module is a client component |
| Angular | `src/assets/ai/` (copied to the build) | the same |

`capabilities: '/ai/index.json'` (an absolute path is safest in routed apps). `host` is whatever the stack's tools
need: the Redux/Pinia/Svelte store, an Angular service, the editor instance.

## Plain HTML / vanilla SPA

```html
<link rel="stylesheet" href="assets/ai-agent/ai-agent.css">
<header>… <button id="ask-ai" type="button">Ask AI</button></header>
<main id="main">…</main>
<script type="module" src="assets/js/ai-agent-setup.js"></script>
```

```js
// assets/js/ai-agent-setup.js
import { createAiAgent, DEFAULT_SYSTEM_PROMPT } from '../ai-agent/ai-agent.js';
import { router, state } from './app.js';

export const agent = createAiAgent({
  appId: 'my-app', title: 'Assistant', toggle: '#ask-ai', push: '#main',
  app: { name: 'My App', purpose: '…', capabilities: ['…'], limits: ['…'] },
  systemPrompt: `${DEFAULT_SYSTEM_PROMPT}\n\n…domain rules…`,
});

const PAGES = {
  list: { title: 'Customers', purpose: '…', content: () => state.visibleCustomers, view: () => ({ selected: state.selectedId }) },
  detail: { title: () => `Customer ${state.customer?.name ?? ''}`, purpose: '…', content: () => state.customer },
};
router.onChange((route) => agent.setPage({ id: route.name, ...PAGES[route.name] }));
state.onChange(() => agent.contextChanged());
```

The Hello World example (`examples/hello-world/`) is a complete single-page integration.

---

## PHP / server-rendered multi-page apps

Each navigation is a full page load, so: one shared include, a per-page description rendered by PHP, and optional
page-specific content hooks. `resume` (on by default) keeps the conversation and the open drawer across page loads.

```php
<?php // partials/ai-agent.php — include at the end of the shared layout
$aiPage = $aiPage ?? ['id' => basename($_SERVER['SCRIPT_NAME'], '.php'), 'title' => $pageTitle ?? 'Page'];
?>
<link rel="stylesheet" href="/assets/ai-agent/ai-agent.css">
<script type="application/json" id="ai-page"><?= json_encode($aiPage, JSON_HEX_TAG | JSON_HEX_AMP | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE) ?></script>
<script type="module" src="/assets/js/ai-agent-setup.js"></script>
```

```php
<?php // orders.php
$aiPage = ['id' => 'orders', 'title' => 'Orders', 'purpose' => 'Lists open orders with status and totals. The user filters and opens orders here.'];
require __DIR__ . '/partials/layout.php';
```

```js
// assets/js/ai-agent-setup.js
import { createAiAgent, fromDom, DEFAULT_SYSTEM_PROMPT } from '../ai-agent/ai-agent.js';

const page = JSON.parse(document.getElementById('ai-page')?.textContent || '{}');

export const agent = createAiAgent({
  appId: 'my-php-app', title: 'Assistant', toggle: '#ask-ai', push: '.page',
  app: { name: '…', purpose: '…', capabilities: ['…'], limits: ['…'] },
  systemPrompt: `${DEFAULT_SYSTEM_PROMPT}\n\n…`,
  page: {
    ...page,
    // A page script may register a better hook: window.aiPageContent = () => ({ rows: … });
    content: () => (typeof window.aiPageContent === 'function' ? window.aiPageContent() : fromDom('main')()),
    view: () => (typeof window.aiPageView === 'function' ? window.aiPageView() : undefined),
  },
  relayProbe: true,                            // use the relay when it answers, direct requests otherwise
  defaults: { relayUrl: '/api/ai-relay.php' },
});
window.aiAgent = agent;   // lets page scripts call aiAgent.contextChanged() after AJAX updates
```

Data rendered by PHP can also be embedded for the hook: `<script type="application/json" id="ai-data"><?= json_encode($rows, JSON_HEX_TAG | JSON_HEX_AMP) ?></script>`
and `window.aiPageContent = () => JSON.parse(document.getElementById('ai-data').textContent)`.

For the relay, copy `assets/relay/relay.php` **unchanged** to e.g. `/api/ai-relay.php`. Its settings live in a
`relay.config.php` outside the web root (lookup order and every key: `providers.md`, "Relays"); the app's login/CSRF
check goes in that file's `authorize` function, and the page passes the token with
`relayHeaders: () => ({ 'X-CSRF-Token': csrfToken })`. No rewrites or `.htaccess` are needed, so the same setup works
on Apache (XAMPP) in development and Nginx + PHP-FPM in production.

---

## React (Vite / CRA)

Copy the runtime to `src/lib/ai-agent/` (TypeScript picks up `ai-agent.d.ts` automatically).

```ts
// src/ai/agent.ts
import { createAiAgent, DEFAULT_SYSTEM_PROMPT, type AiAgent } from '../lib/ai-agent/ai-agent.js';
import '../lib/ai-agent/ai-agent.css';

let agent: AiAgent | null = null;

export function getAgent(): AiAgent {
  agent ??= createAiAgent({
    appId: 'my-react-app',
    title: 'Assistant',
    launcher: false,                 // the header button calls getAgent().toggle()
    push: '#root',
    app: { name: '…', purpose: '…', capabilities: ['…'], limits: ['…'] },
    systemPrompt: `${DEFAULT_SYSTEM_PROMPT}\n\n…`,
  });
  return agent;
}
```

```ts
// src/ai/useAiPage.ts
import { useEffect, useRef } from 'react';
import { getAgent } from './agent';

/** Register the current page with the agent. `content`/`view` are read lazily, so they are always current. */
export function useAiPage(page: { id: string; title: string; purpose?: string }, content: unknown, view?: unknown) {
  const latest = useRef({ content, view });
  latest.current = { content, view };

  useEffect(() => {
    const agent = getAgent();
    agent.setPage({ ...page, content: () => latest.current.content, view: () => latest.current.view });
    return () => agent.setPage(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [page.id, page.title]);

  useEffect(() => { getAgent().contextChanged(); }, [content]);
}
```

```tsx
// in a page component
useAiPage({ id: 'orders', title: 'Orders', purpose: 'Open orders with status and totals.' },
  { filters, rows: visibleRows }, { selectedId });

// in the header
const [flag, setFlag] = useState('unread');
useEffect(() => getAgent().onContextStatus((s) => setFlag(s.state)), []);
<button onClick={() => getAgent().toggle()} data-flag={flag}>Ask AI</button>
```

Memoise `content` (e.g. `useMemo`) so the effect only fires when the data really changes; `contextChanged()` is
debounced and hash-based, so an extra call is harmless.

## Next.js (App Router)

Same files as React, plus: the agent must only be created in the browser.

```tsx
// app/ai-agent-boot.tsx
'use client';
import { useEffect } from 'react';
export default function AiAgentBoot() {
  useEffect(() => { import('@/ai/agent').then(({ getAgent }) => getAgent()); }, []);
  return null;
}
```

Render `<AiAgentBoot />` in `app/layout.tsx`, import `@/lib/ai-agent/ai-agent.css` there (global CSS is allowed in the
root layout), and mark any component that calls `useAiPage` with `'use client'`. Use a route handler
(`app/api/ai-relay/route.ts`) ported from `assets/relay/relay.mjs` if keys must stay on the server.

---

## Vue 3

```js
// src/ai/agent.js
import { onMounted, onBeforeUnmount, watch, unref } from 'vue';
import { createAiAgent, DEFAULT_SYSTEM_PROMPT } from '../lib/ai-agent/ai-agent.js';
import '../lib/ai-agent/ai-agent.css';

let agent;
export const getAgent = () => (agent ??= createAiAgent({ appId: 'my-vue-app', title: 'Assistant', launcher: false, push: '#app', app: { … }, systemPrompt: `${DEFAULT_SYSTEM_PROMPT}\n\n…` }));

export function useAiPage(page, content, view) {
  onMounted(() => getAgent().setPage({ ...page, content: () => unref(content), view: () => unref(view) }));
  onBeforeUnmount(() => getAgent().setPage(null));
  watch(content, () => getAgent().contextChanged(), { deep: true });
}
```

`content` can be a `ref`/`computed`; reactive proxies serialise normally.

## Svelte / SvelteKit

```js
// src/lib/ai.js
import { browser } from '$app/environment';
import { createAiAgent } from '$lib/ai-agent/ai-agent.js';
let agent;
export const getAgent = () => (browser ? (agent ??= createAiAgent({ appId: 'my-svelte-app', launcher: false, app: { … } })) : null);
```

```svelte
<script>
  import { onMount } from 'svelte';
  import { getAgent } from '$lib/ai.js';
  import { rows, filters } from './stores.js';
  onMount(() => {
    const agent = getAgent();
    agent.setPage({ id: 'orders', title: 'Orders', purpose: '…', content: () => ({ filters: $filters, rows: $rows }) });
    const off = rows.subscribe(() => agent.contextChanged());
    return () => { off(); agent.setPage(null); };
  });
</script>
```

Import `ai-agent.css` in the root `+layout.svelte`.

## Angular

```ts
// ai-agent.service.ts
import { Injectable, Inject, PLATFORM_ID } from '@angular/core';
import { isPlatformBrowser } from '@angular/common';
import { createAiAgent, type AiAgent, type PageContext } from '../lib/ai-agent/ai-agent.js';

@Injectable({ providedIn: 'root' })
export class AiAgentService {
  private agent: AiAgent | null = null;
  constructor(@Inject(PLATFORM_ID) private platform: object) {}
  get(): AiAgent | null {
    if (!isPlatformBrowser(this.platform)) return null;
    return (this.agent ??= createAiAgent({ appId: 'my-ng-app', launcher: false, app: { … } }));
  }
  setPage(page: PageContext) { this.get()?.setPage(page); }
  changed() { this.get()?.contextChanged(); }
}
```

Add `"src/lib/ai-agent/ai-agent.css"` to `styles` in `angular.json`, call `setPage` in each routed component's
`ngOnInit` (content hook reading the component's fields), and `changed()` after data loads.

---

## Host-app situations

These come up in real apps; each has a runtime option or a recipe. `scripts/verify.mjs` checks most of them.

### Keyboard shortcuts (Space = play, letters, arrows)

Host handlers on `window`/`document` that exempt only INPUT/SELECT/BUTTON treat the drawer's TEXTAREA as "not typing"
and swallow keystrokes. With `isolateKeys` (default on) key events that start inside the agent's UI never reach the
host's bubbling listeners; Ctrl/Cmd application shortcuts still do. What is left to check:

- Search the host for global key handlers (`addEventListener('keydown'`, `onkeydown`, `useHotkeys`, `Mousetrap`,
  `hotkeys(`) and for **capture-phase** ones (`, true)` / `{ capture: true }`): those run before any element and
  cannot be isolated — make them skip `e.target.closest('.aia-scope')` (a good idea in every handler).
- Type a space and a few letters in the composer: they must appear, and nothing in the app may react.

### Modal dialogs (`showModal`, popovers, focus traps)

`dialog.showModal()` makes everything outside the dialog inert, the drawer included: the hotkey still opens the
drawer, but behind the modal, where it cannot be used. Options:

1. **`dialogs: 'dock'`** (recommended for native dialogs): while the drawer is open, managed dialogs open non-modally
   and are centred in the space left of the drawer (class `.aia-docked-dialog`); opening the drawer docks the dialogs
   that are open modally, closing it makes them modal again. The switch (`close()` + `show()`) fires no `close`,
   `beforetoggle` or `toggle` events at the host, so close handlers (unloading a video player, resetting a form) do
   not run. `{ selector: 'dialog.panel' }` limits it to some dialogs.

   ```js
   createAiAgent({ …, dialogs: 'dock' });   // or { selector: 'dialog:not(.confirm)' }
   ```

   Keep real confirmations (delete? discard?) out of the selector: those should stay modal.
2. **Your own switch**, if you need different behaviour — the recipe the option implements:
   (a) per dialog instance, override `showModal` to call `show()` while the drawer is open;
   (b) on `agent.on('open')`, for each `dialog:modal`: `close()` then `show()`; on `agent.on('close')`, `close()` then
   `HTMLDialogElement.prototype.showModal.call(d)`;
   (c) before each switch, add a **capturing, once-only** `close` listener on `window` that calls
   `stopImmediatePropagation()` when `d.open` is true at that moment (the event arrives late — after other tasks —
   while the dialog is already open again; a timer-based cleanup would miss it) and swallow the synchronous
   `beforetoggle` and the merged `toggle` (old state = new state) events the same way;
   (d) CSS for docked dialogs: `position: fixed; inset: 0 var(--aia-push-width) 0 0; margin: auto;`;
   (e) where `:modal` is unsupported, remember which dialogs you opened modally.
3. **Popovers** (`popover` attribute) do not make the page inert; auto popovers light-dismiss when the drawer is
   clicked.
4. **JS focus traps** (modal libraries, `focus-trap`): pause the trap while the drawer is open —
   `agent.on('open', () => trap.pause()); agent.on('close', () => trap.unpause());` — and exclude `.aia-scope` from
   any `inert`/`aria-hidden` the library sets on the rest of the page.

An open dialog changes what the user sees: include its contents in the page content and signal the change when it
opens and closes (the `dialogs` option signals docking changes itself).

### Layout: `push` with fixed-width shells

`push` adds `padding-right` equal to the drawer width. Shells with fixed-width grid columns, or a header whose
min-content width is large (many toolbar buttons, `white-space: nowrap`), do not shrink into what is left: the shell
overflows, side panels are clipped and header buttons — sometimes the toggle — slide under the drawer. The fix is
host CSS scoped to the supported hook `html.aia-drawer-open`:

```css
html.aia-drawer-open .app-shell  { grid-template-columns: minmax(0, 1fr); }            /* the pushed container */
html.aia-drawer-open .app-header { grid-template-rows: minmax(56px, auto); }           /* the header may grow… */
html.aia-drawer-open .toolbar    { flex-wrap: wrap; }                                  /* …its actions wrap */
html.aia-drawer-open .layout     { grid-template-columns: minmax(0, 280px) minmax(0, 1fr) minmax(0, 320px); }
html.aia-drawer-open .fixed-bar  { right: var(--aia-push-width); }                     /* fixed elements are not pushed */
```

With `devWarnings` (on for localhost) the runtime checks the pushed layout after opening and on resize and names what
reaches under the drawer. Check 1280, 1366 and 1600 px wide (`scripts/verify.mjs` does, with screenshots).

### Change signals: a catch-all

Besides store subscriptions and data-load callbacks, one capture-phase listener catches most user-driven changes
cheaply (`contextChanged()` is debounced and hash-based, so extra calls cost nothing):

```js
const signal = (e) => { if (!(e.target instanceof Element && e.target.closest('.aia-scope'))) agent.contextChanged(); };
for (const type of ['click', 'change', 'input']) document.addEventListener(type, signal, true);
document.addEventListener('close', signal, true);   // <dialog> closed (close does not bubble; capture sees it)
```

### Real-time apps (simulation loop, Web Worker, websockets)

Call `contextChanged()` on every update; `debounceMaxMs` (default 1000) keeps the flag moving without app-side
throttling. Build the content from the app's state (decode the worker's typed arrays), rounded to what the UI shows.
See `spec/context-sync.md`, "Apps that change all the time".

### Code actions that small local models can drive

Small models ignore requested fence tags (asked for a `conditions` block, a 9B model answered with `json`), add
comments, quote numbers or write `key: value` lines. So:

- `when` predicates **validate the content** rather than trusting the tag: `when: (b) => !!parseBlockValues(b, { tags:
  'conditions', schema })` accepts the custom tag, or `json` when every key is in the schema.
- Parse leniently (JSON or `key: value` lines) — `parseBlockValues` does, in linear time; never run
  backtracking-prone regular expressions over model output.
- **Clamp** to the real control ranges (put them in the schema: `min`, `max`, `step`, `values`).
- **Apply through the app's real controls** with `setControlValue(el, value)` (native setter + `input`/`change`
  events), so the host's own handlers and validation run exactly as for a user. Hello World's "Apply editor settings"
  (`examples/hello-world/ai-agent-setup.js`) is a complete example.
- Tell the model the tag and the keys in the system prompt, with one example block.

---

## Electron / Tauri / desktop webviews

Same as plain HTML. `file://` pages cannot load ES modules in some shells: serve the UI from the app's local
server/protocol handler. Local LLMs (LM Studio, Ollama) work directly; cloud providers may need the relay.
