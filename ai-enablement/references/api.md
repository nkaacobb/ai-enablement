# API reference

`import { createAiAgent, fromDom, DEFAULT_SYSTEM_PROMPT } from './ai-agent/ai-agent.js'` and load `ai-agent/ai-agent.css`.
Types: `assets/ai-agent/ai-agent.d.ts`. Runtime version: `VERSION` (1.6.0). Tools (the agent acting in the app):
`tools.md`. Capabilities — tools, toolsets, skills, agents, permissions, the capability index: `spec/framework.md`.
Memory, screenshots and attachments (the + button): `memory-and-vision.md`. In-app authoring: `in-app-authoring.md`.

## createAiAgent(options) → agent

Creates and mounts the toggle wiring (or a floating launcher), the drawer and the settings modal. Call once, in the
browser, after the DOM exists. It returns synchronously; `agent.ready` resolves once async `defaults`, the capability
index, the tool config, the memory file, the relay probe and the workspace check are done — questions wait for it
automatically.

| Option | Default | Meaning |
| --- | --- | --- |
| `appId` | `'app'` | **Required in practice.** Namespaces browser storage: `<appId>.ai.settings`, `.keys`, `.chats`, `.width`. |
| `title` | `'AI agent'` | Drawer title (e.g. "Security analyst", "Writing agent"). |
| `app` | – | App context hook: string, object or (async) function. Objects render as `Label: value` lines; arrays as bullets. Suggested keys: `name`, `purpose`, `audience`, `capabilities[]`, `limits[]`, `glossary{}`. Keep it lean (it is sent with every request). |
| `page` | – | Initial page: `{ id, title, purpose, content, view, ...more descriptive keys }`. See "Page". |
| `systemPrompt` | `DEFAULT_SYSTEM_PROMPT` | The app's default prompt (string or function). Users can override it in Settings > Agent; "Restore app default" brings this back. |
| `welcome` | generic | Markdown shown at the start of each chat. |
| `suggestions` | `[]` | Clickable starter questions under the welcome. |
| `placeholder` | `'Ask about what is on screen…'` | Composer placeholder. |
| `toggle` | `null` | Selector/element(s) that toggle the drawer. None → floating launcher (unless `launcher: false`). |
| `toggleBadge` | `true` | Adds a coloured context-state dot on toggles (`[data-aia-toggle][data-aia-context]`; see "Theming"). |
| `push` | `document.body` | Element(s) that get `padding-right` = drawer width while open. `false` = overlay. Below 900 px wide the drawer always overlays. Shells with fixed-width columns need the recipe in `frameworks.md` ("Layout"). |
| `hotkey` | `'mod+i'` | Toggle shortcut (`mod` = Ctrl/Cmd; also `shift+`, `alt+`). `false` disables. Works from inside the drawer too. |
| `theme` | `'auto'` | `'light'`/`'dark'` force a theme; auto follows the OS. |
| `width` | `440` | Default drawer width (users drag the left edge; double-click resets; remembered). |
| `watch` | `0` | Poll the content hook every N ms while the drawer is open. Use when you cannot call `contextChanged()`. |
| `debounceMs` | `300` | Trailing debounce for `contextChanged()`. |
| `debounceMaxMs` | `1000` | While `contextChanged()` keeps being called (animation, simulation, live data), refresh the flag at least this often anyway. `0` = plain debounce (1.0 behaviour: the flag waits until the changes stop). |
| `isolateKeys` | `true` | Keystrokes typed in the drawer and the settings modal do not reach the host page's bubbling key listeners, so host shortcuts (Space = play, letters, arrows) cannot swallow them. Ctrl/Cmd application shortcuts (Ctrl+S…) still reach the host; text-editing combos (Ctrl+A/C/V/X/Z/Y, Ctrl+arrows) stay in the field. |
| `dialogs` | `false` | `'dock'`: native modal `<dialog>`s (which make the drawer inert) are shown non-modally beside the open drawer and become modal again when it closes, without firing `close`/`toggle` at the host. `{ selector: 'dialog.x' }` manages only some. See `frameworks.md` ("Modal dialogs"). |
| `devWarnings` | `'auto'` | Console warnings for integration problems: the pushed layout overflows/hides things under the drawer (checked after opening and on resize), a modal dialog makes the drawer inert, the settings dialog does not match its layout spec (host CSS leaking in; checked when it opens and per tab — `settings-layout.md`). `'auto'` = on for `localhost`, `127.x`, `[::1]`, `*.localhost`, `*.test`, `*.local`, `file:`; `true`/`false` force. |
| `relayProbe` | `false` | `true` (probe `defaults.relayUrl`), a URL, or `{ url, timeoutMs = 2500 }`: GET the relay at startup; if it answers `available`, use it (with its preset provider/model), else send requests directly. See "Relay probe". |
| `contextWarnTokens` | `3000` | Settings > Context warns (local providers) when the first request is estimated above this. |
| `tools` | `[]` | Tools given inline: `[{ name, title?, description, parameters? \| inputSchema?, effect?: 'read'\|'write'\|'destructive'\|'external'\|'system', annotations?, pages?, when?, group?, enabled?, timeoutMs?, run(args, { host, agent, signal, call }) }]`, toolsets (`{ name, title, description, tools }`), or `(host) => that list`. Added to the capability index's (inline wins). Invalid definitions are skipped with a console error. |
| `toolsConfig` | `null` | The app's default tool selection and switches (`ai-tools.json`): a URL, an object, or a (possibly async) function. Applied like `defaults` (questions wait for it). Default: the index's `toolsConfig`. |
| `host` | `null` | The object tools call: `run(args, { host })` — the app's store, service layer or instance. Also what `describe_host` shows the in-app agent. |
| `capabilities` | `null` | The capability index: a URL (`'ai/index.json'`) or an index object (`spec/framework.md`). Loads tool modules, toolsets, skills, agents, permissions, the tool config and the memory file. `agent.capabilities.reload()` loads it again. Problems are collected (console, `agent.capabilities.problems()`), never thrown. |
| `agents` / `skills` / `toolsets` | `[]` | Given inline, added to the index's (inline wins on a name clash). |
| `permissions` | `null` | `{ allow, ask, deny }` rules (tool names, `name_*`, `toolset:<name>`, `effect:<effect>`), merged with the index's and the active agent's. deny > ask > allow > the user's settings; `system` tools always ask. |
| `agent` | `''` | The agent to start with (else the index's `defaultAgent`, the one marked `default`, the first). |
| `workspace` | `false` | Development only: `true` (`/ai-workspace` on this origin) or the URL of `scripts/workspace.mjs`. When it answers, the agent gets the workspace tools and the `create-tool` skill (`in-app-authoring.md`); otherwise nothing happens. |
| `memory` | `true` | Notes the agent keeps between conversations: the Settings > Memory tab, the MEMORY section of the system prompt, and the built-in `remember` / `forget` tools. `false` removes all of it (`agent.memory` is then `null`). |
| `memoryFile` | `null` | The app's starting memories (`ai-memory.json`): a URL, an object, or a (possibly async) function. What a user adds, edits or deletes is kept in their browser on top of it. Applied like `defaults` (questions wait for it). |
| `memorySave` | `null` | `(file) => void \| Promise`: called (debounced) with the whole memory file after every change made in this browser, for apps whose backend keeps the notes. |
| `screenshots` | `true` | Screenshots for models that see images: the Settings > Vision tab, the camera button, the built-in `take_screenshot` tool. `false` removes all of it. |
| `screenshot` | `null` | `({ reason }) => canvas \| image \| video \| ImageBitmap \| ImageData \| Blob \| data URL \| null` (or a promise): the app's own picture of what the user is looking at. Without it, or when it returns `null`, the browser's screen capture of this tab is used. |
| `screenshotMaxEdge` | `1280` | Screenshots (and attached image files) are scaled down so their longer edge is at most this many pixels (256–4096). |
| `attachments` | `true` | The **+** button left of the message field (*Attach an image* · *Upload a file*), plus drag and drop and paste. Images go to a model that sees them exactly like a screenshot; other files are read in the browser and sent as text (PDF, Word, Excel, PowerPoint, OpenDocument, RTF, text/code/CSV/JSON). `false` removes all of it. |
| `readFile` | `null` | `(file, { kind, name }) => text \| { text, label? } \| null` (or a promise): the app's own reader, tried before the built-in ones. `null` lets the runtime read the file; throwing refuses it with that message. For the app's own formats, or a reader it already ships (pdf.js…). |
| `codeActions` | `[]` | `[{ id, label, title?, when?(block), run(block, agent), doneLabel? }]` → buttons on fenced code blocks (`block = { language, code }`). Copy is built in. For values the app applies, see `parseBlockValues` and `setControlValue`. |
| `replyActions` | `[]` | `[{ id, label, title?, run(markdown, agent), doneLabel? }]` → buttons under each reply. *Copy* and *Copy tool log* (shown when the reply has tool rows; ids `copy`, `copy-tools`) are built in. |
| `defaults` | `{}` | App defaults for any setting (see "Settings"). User choices override them. May be an object, a promise, or a (possibly async) function. |
| `relayHeaders` | – | Object or function returning headers for relay calls (CSRF tokens). |
| `saveChats` | `true` | Keep conversations in localStorage ("Saved chats"). Snapshot *text* is never stored. |
| `resume` | `true` | Per browser tab (sessionStorage): after a reload or a multi-page navigation, reopen the active chat, and the drawer if it was open. The resumed chat re-reads the current page on its next question. |
| `mount` | `document.body` | Where the drawer and modal are appended. |

## Page

```js
agent.setPage({
  id: 'orders/123',                 // stable id; part of the fingerprint
  title: 'Order #123',              // string or hook
  purpose: 'Shows one order: lines, totals, shipping and payment status. The user reviews and edits it.',
  // any other descriptive keys are included in the page context, e.g.
  actionsAvailable: ['Edit quantities', 'Cancel order (with confirmation)'],
  content: () => buildOrderContent(store.getState()),              // FINGERPRINTED
  view: () => ({ selectedLine: current.selectedLineId, tab: current.tab }), // NOT fingerprinted
});
```

- `content` may return a string (sent as written) or JSON-able data (sent as sorted-key, 2-space JSON; key order never
  changes the hash). Async functions are fine. If it throws, the snapshot says the content could not be read.
- `view` is sent inside `<view_state>` with every question and is limited to 2,000 characters.
- `tools` (optional): tools that exist only on this page (same format as the `tools` option). They are sent as
  tools, not as page description.
- `setPage(null)` clears the page (flag shows *none*).
- Write `content` as a pure, tested function of the app's state (`spec/context-sync.md`, "Content builders").

## Agent methods

| Method | Does |
| --- | --- |
| `open()`, `close()`, `toggle()`, `isOpen()` | Drawer visibility. |
| `ask(text)` | Send a question as the user (opens the drawer). Like Send, it takes what waits in the composer along (screenshots, attached images and files; files still being read are waited for). Returns when the reply has finished. |
| `stop()` | Abort the streaming reply (partial text is kept and marked). |
| `newChat()` | Save the current chat and start a new one (flag → *unread*). |
| `openSettings('model' \| 'agent' \| 'tools' \| 'memory' \| 'vision' \| 'context')` | Open the settings modal on a tab. The Context tab shows exactly what the hooks produce, and the estimated size of the first request. |
| `setApp(app)` | Replace the app context. |
| `setPage(page)` | Replace the page (navigation). Recomputes the flag immediately. |
| `setContent(fn)`, `setView(fn)` | Replace only the current page's content/view hook. |
| `contextChanged()` | Debounced (`debounceMs`, at least every `debounceMaxMs` under continuous calls): re-hash the screen and update the flag. Call after every change to what is shown. Cheap: nothing is sent. |
| `refreshContext()` | Same, immediately; resolves to the status. |
| `getContextStatus()` | Current status (below). |
| `onContextStatus(fn)` | Subscribe; `fn` runs immediately and on every change. Returns unsubscribe. |
| `rereadPage()` | Force the next question to carry a fresh snapshot. |
| `systemPrompt()` | Resolves to the full system prompt as it would be sent now. |
| `ready` | Promise resolving to the agent once async `defaults`, the capability index, the tool config, the memory file, `relayProbe` and the workspace check are done (immediately without them). |
| `relayInfo()` | What the relay probe found (`{ url, available, mode, preset, providers, serverKeys, images, reason }`), or `null`. |
| `tools.list()` | Every tool known now (app-wide + this page): `{ name, title, description, effect, group, pages, toolsets, enabled, available, permission: 'allow'|'ask'|'deny'|'default', agent }` (`agent`: the active agent may use it). |
| `tools.register(defs)`, `tools.unregister(name)` | Add/replace (tools, lists or toolsets) or remove app-wide tools at runtime. |
| `tools.setEnabled(name, on)` | Turn a tool on/off for this user (saved like Settings > Tools). |
| `tools.run(name, args)` | Run a tool directly (validated arguments, no confirmation, ignores on/off) — for tests and scripted checks. Resolves to the text the model would receive. |
| `tools.exportConfig()` | The current selection as an `ai-tools.json` object. |
| `tools.toolsets()` | `[{ name, title, description, tools: [names] }]`. |
| `tools.mcp()` | Every tool as an MCP tool descriptor `{ name, title, description, inputSchema, annotations, outputSchema? }`. |
| `agents.list()`, `agents.current()`, `agents.use(name)` | The agents (`{ name, title, description, active, implicit, tools, toolsets, skills, model, memory, context }`; one implicit agent when none are defined), the active one's name, and switching (saves the conversation and starts a new one; `false` when there is no such agent or a reply is streaming). |
| `skills.list()`, `skills.activate(name)`, `skills.active()` | The active agent's skills (`{ name, description, source: 'app'|'builtin', active, allowedTools }`), activating one in this conversation (as `/name` does), and the active ones. |
| `capabilities.reload()`, `capabilities.problems()`, `capabilities.index()` | Load the capability index again — fresh copies of every file and module, the tool config and memory file — resolving to the problems found; the current problems; the index URL. |
| `workspace()` | The development workspace's info (`{ url, root, aiDir, index, writable, version }`) when it answered, else `null`. |
| `memory.list()` | The memories: `[{ id, text, created, updated?, source: 'user' \| 'agent' \| 'app' }]`. `agent.memory` is `null` with `memory: false`. |
| `memory.add(text)`, `memory.update(id, text)`, `memory.remove(id)`, `memory.clear()` | Change them (saved at once; the same note is not added twice; at most 100 notes of 500 characters). |
| `memory.export()`, `memory.import(file, { replace })` | The memories as an `ai-memory.json` object; add the memories of such a file (or make them the whole memory). |
| `screenshot()` | Take a screenshot and put it in the composer for the next question — what the camera button does. With the browser's screen capture, call it from a click. Resolves to `{ width, height, source: 'app' \| 'screen' }` or `null`. |
| `attach(files)` | Attach a `File`, `Blob`, `FileList` or array to the next question — what the + button does. Resolves to one `{ kind, name, size, width?, height?, chars?, totalChars?, truncated?, label? }` per file, or `null` for a file that could not be attached (the chat says why: an image for a text-only model, an old `.doc`, a scanned PDF…). |
| `on(event, fn)` | Events: `open` (`{}`, or `{ resumed: true }` — see below), `close`, `send` `{text, attached, reason, hash, images, files}`, `reply` `{text, provider, model, stopped, actions}` (an action: `{call, title, status, summary, name, detail: {args, sent, problem, result}}`, what its tool row shows rolled down), `error` `{error}`, `context` (status), `settings` (settings), `relay` (relay info), `tool` `{name, args, status: 'ok'\|'error'\|'declined'\|'off'\|'skipped', result}` (`result`: what the tool returned, or why it failed, e.g. *Cut off: …*), `tool-state` `{name, enabled}`, `memory` `{memories, change: {type, id?}}`, `screenshot` `{by: 'user'\|'agent', width, height, source}`, `attach` (what `attach()` resolves to, once a file is read), `agent` `{name, title}` (switched), `skill` `{name, via: 'tool'|'slash'|'api'}` (activated), `capabilities` `{problems, version}` (loaded or reloaded), `workspace` (its info, once it answered). Returns unsubscribe. |
| `settings.get()`, `settings.save(patch)`, `settings.reset()`, `settings.setKey(provider, key)` | Programmatic settings. |
| `destroy()` | Remove everything the agent added. |

**Open-time setup and `resume`.** With `resume` (default), a drawer that was open before a reload reopens *inside*
`createAiAgent()`, before the app could register listeners. The runtime replays that `open` once, as
`{ resumed: true }`, to listeners registered in the same tick. If you register later (after an `await`), check
`agent.isOpen()` once and run the open-time setup yourself:

```js
const agent = createAiAgent({ … });
const onOpen = () => startPreview();
agent.on('open', onOpen);
if (agent.isOpen()) onOpen();          // safe either way: make onOpen idempotent
```

### Context status

```ts
{ state: 'synced' | 'dirty' | 'unread' | 'none' | 'off',
  pending: boolean,        // the next question will attach a snapshot
  reason: string,          // 'unchanged' | 'changed' | 'unread' | 'trimmed' | 'forced' | 'empty' | 'off'
  hash: string | null,     // fingerprint of the screen now (show .slice(0, 7))
  syncedHash: string | null,// newest fingerprint the model has in this conversation
  pageId, title, chars, truncated, forced }
```

## Settings (Settings modal, `defaults`, `agent.settings`)

| Key | Default | |
| --- | --- | --- |
| `provider` | `'lmstudio'` | `lmstudio`, `ollama`, `custom`, `openai`, `anthropic`, `google`, `deepseek`, `openrouter` |
| `profiles` | `{}` | Per provider `{ baseUrl, model }`. Empty model = whatever LM Studio has loaded. |
| `transport` | `'direct'` | `'relay'` sends through the app's relay. |
| `relayUrl` | `''` | Absolute URL or same-origin path (`/ai-relay`, `api/relay.php`). |
| `fallbackProvider` | `''` | Used when the primary cannot be reached (before any text arrived). |
| `systemPrompt` | `''` | `''` = the app's default. |
| `temperature` | `0.4` | Not sent to providers that reject it (OpenAI reasoning models, Anthropic). |
| `maxOutputTokens` | `4096` | Local reasoning models need room to think. |
| `historyMessages` | `20` | Transcript entries sent per request. The snapshot is re-sent if it falls out of this window. |
| `maxContextChars` | `24000` | Screen content cap (≈ 6k tokens). |
| `reasoning` | `'show'` | `'hide'` hides the thinking panel; `'off'` asks the model not to think (where supported). |
| `shareScreen` | `true` | Off = the model gets app/page context but not the content. |
| `timeoutSec` | `120` | Idle timeout (resets while tokens or relay keepalives arrive). |
| `rememberKeys` | `false` | Keys in localStorage instead of sessionStorage. |
| `toolsEnabled` | `true` | Master switch for tools (Settings > Tools). |
| `toolMode` | `'auto'` | `'native'` tool calls, `'text'` tool blocks (any model), `'auto'` = native, text if the server refuses tools. |
| `confirmWrites` | `true` | Ask before tools with `effect: 'write'` ("Allow for this chat" skips it for that tool). |
| `confirmDestructive` | `true` | Ask before tools with `effect: 'destructive'`. |
| `maxToolSteps` | `8` | Tool rounds per question. |
| `toolStates` | `{}` | `{ toolName: true \| false }`, merged over `toolsConfig` and each tool's `enabled`. Only differences from the app defaults are stored. |
| `memoryEnabled` | `true` | The memories are part of every conversation (Settings > Memory). |
| `memoryWrite` | `true` | The agent may save, correct and delete memories when the user asks (`remember`, `forget`). Off: the model is told it cannot, and the tools are not offered. |
| `vision` | `true` | The model sees images: the camera button, screenshots and attached images are offered. Set `defaults: { vision: false }` for a text-only default model. A relay without image support switches it off. |
| `screenshotAuto` | `false` | The agent may take a screenshot on its own (`take_screenshot`). Off: only the camera button; the agent can ask, and the user allows it once or always. |
| `maxFileChars` | `40000` | Each attached file's text is cut to this many characters (1,000–400,000; Settings > Agent > *Max file content*). The model is told when a file was cut. Lower it for local models with a small context. |

Layering: built-in defaults < `defaults` (applied late when async / probed) < what the user saved.

## Relay probe

```js
const agent = createAiAgent({ …, relayProbe: true, defaults: { relayUrl: 'api/relay.php', provider: 'lmstudio' } });
```

At startup the runtime GETs `relayUrl` (2.5 s timeout). The relay answers `200 { relay: 'ai-agent-drawer', available,
mode, preset, … }` (contract in `providers.md`). `available` → `transport: 'relay'` plus the preset provider/model as
defaults; anything else (a static server returning the PHP source, a 404, a timeout, `available: false`) → direct
requests. Saved settings that no longer fit are adjusted when read, without touching storage: an unavailable relay is
not used, and a public relay's preset provider/model replace ones it does not offer. `agent.relayInfo()` and the
`relay` event report what was found.

## fromDom(target, { exclude, fields = true })

Returns a content hook reading an element's rendered text (`innerText`, so tables keep their tab-separated columns)
plus a "Form fields:" list of visible input/select/textarea values (never passwords or hidden fields). Skips the
agent's UI, `[data-aia-ignore]`, and anything matching `exclude`. Include an open dialog: `fromDom('main, dialog[open]')`
reads only the first match, so combine two hooks instead (`() => [fromDom('main')(), fromDom('dialog[open]')()].join('\n\n')`).

## parseBlockValues(block, { tags, schema, generic })

Reads values a model wrote in a fenced block, for code actions that apply them to the app:

```js
import { parseBlockValues, setControlValue } from './ai-agent/ai-agent.js';
const SCHEMA = {
  speed:   { type: 'number', min: 0, max: 10, step: 0.5, aliases: ['velocity'] },
  mode:    { type: 'enum', values: ['orbit', 'free'] },
  trails:  { type: 'boolean' },
};
const read = (b) => parseBlockValues(b, { tags: 'conditions', schema: SCHEMA });
codeActions: [{
  id: 'apply', label: 'Apply', when: (b) => !!read(b),
  run: (b) => { const { values } = read(b); if ('speed' in values) setControlValue('#speed', values.speed); /* … */ },
}]
```

- A block tagged with one of `tags` is accepted when at least one key is known (unknown keys are ignored).
- A generic block (`json`, `yaml`, `text`, untagged… — `generic` lists them) only when **every** key is known, so an
  unrelated JSON example never grows an Apply button, but a small model that answers in `json` still works.
- The body may be JSON or `key: value` / `key = value` lines (comments, trailing commas and quotes tolerated).
  Parsing is linear — no backtracking regular expressions run on model output.
- Keys match case- and punctuation-insensitively (`Wind speed` = `windSpeed`), plus `aliases`.
- Values are coerced (`number`, `integer`, `boolean` from yes/no/on/off/1/0, `enum`, `string`) and clamped to
  `min`/`max`/`step`. Returns `{ values, unknown, invalid, adjusted }`, or `null`.

## setControlValue(target, value)

Sets a form control the way a user would, so the host's own handlers and validation run: numbers are clamped to the
control's `min`/`max`/`step`; the native value setter is used (React and similar frameworks notice); bubbling `input`
and `change` events are dispatched; checkboxes and radios are clicked when they must flip; selects match an option's
value or text. Returns `true` when the control now holds the value.

## Other exports

`renderMarkdown(md, { codeActions })`, `hashText(str)`, `stableStringify(value)`, `probeRelay(url, { timeoutMs })`,
`parseMemoryFile(json)`, `exportMemoryFile(memories)`, `toMcpTool(tool)`, `fromJsonSchema(schema)`,
`loadCapabilities(source, { base, host, fetchText, importModule, bust })`, `parseSkill(text, { base, folder })`,
`parseAgent(text, { file })`, `parseFrontmatter(text)`, `DEFAULT_SYSTEM_PROMPT`, `PROVIDERS`, `PROVIDER_IDS`, `AiError`, `VERSION`.

## Built-in tools

Besides the app's tools (`tools.md`) the runtime has its own. They are not listed in Settings > Tools and do not
follow its master switch; each follows its own setting. An app tool with the same name replaces the built-in one; a
`deny` permission rule removes one.

| Tool | Offered when | Asks the user |
| --- | --- | --- |
| `remember(text, id?)` | Memory on, and "Let the agent save a memory…" on | No: the chip shows what was saved, with Undo |
| `forget(id)` | The same | Yes, like a tool that changes something ("Ask me before actions that change something") |
| `take_screenshot()` | "This model can see images" on and a picture can be taken. Callable when the user freed it; otherwise listed as turned off, so the model can ask for it | Not when freed (with the browser's screen capture: one click to share the tab, once per page load). Otherwise: *Allow once* / *Always allow* / *No* |
| `use_skill(name)`, `read_skill_file(skill, path, offset?)` | The active agent has skills | No |
| `describe_host()`, `list_source_files(path?)`, `read_source_file(path, offset?)`, `search_source(query, path?)`, `reload_capabilities()` | The development workspace answered (`workspace`) | No |
| `write_ai_file(path, content, replace?)` | The same | **Always** (effect `system`): the whole new file, or a diff for a replacement |

An agent with `memory: read` or `off` gets no `remember` / `forget`.

With these, a request carries tool definitions even in an app without tools of its own; a model server without tool
calling gets them as text blocks (Settings > Tools > "How tools are called", automatic by default).

## Theming and host hooks

Override variables on `.aia-scope` from the host stylesheet. The runtime's theme rules are wrapped in `:where()`
(zero specificity), so a host `.aia-scope { … }` rule wins in light **and** dark mode, whatever the load order:

```css
.aia-scope { --aia-accent: #7c3aed; --aia-accent-strong: #5b21b6; --aia-accent-soft: #ede9fe; --aia-radius: 6px; }
@media (prefers-color-scheme: dark) { .aia-scope { --aia-accent-soft: #2e1065; } }   /* dark-only values */
html.dark .aia-scope { --aia-bg: #0b0b0f; }                                           /* or the host's own dark class */
:root { --aia-z: 1000; }
```

Supported host hooks (stable across versions):

| Hook | What it is |
| --- | --- |
| `html.aia-drawer-open` | Class on `<html>` while the drawer is open. Scope layout fixes to it. |
| `--aia-drawer-width` | The drawer width, set on `:root` by the runtime (inline, so use the `width` option for the default). |
| `--aia-push-width` | The room the open drawer takes from the page: the drawer width while open, `0px` when closed and below 900 px (overlay). Use it for fixed host elements: `html.aia-drawer-open .toast { right: var(--aia-push-width) }`. |
| `--aia-badge-synced`, `-dirty`, `-unread` | Colours of the context dot on toggles (defaults green/amber/blue). |
| `--aia-badge-ring` | The ring around the dot (default `#fff`; set it to the toggle's background on dark hosts). |
| `--aia-badge-size`, `-top`, `-right` | Size and position of the dot (defaults 7px, 3px, 3px). Give a small text toggle some right padding (e.g. `padding-right: 18px`) so the dot never covers its label. |
| `.aia-docked-dialog` | Class on host dialogs docked beside the drawer (`dialogs: 'dock'`). |
| `[data-aia-ignore]` | Host elements `fromDom` skips. |

Set the badge variables on the toggle itself or any ancestor (`:root`). All runtime classes start with `aia-`.

The page's own element rules (`label`, `button`, `input`, `p`, `h2`, `body { text-align }`…) do not reach the drawer or
the settings dialog (runtime 1.6.1, `settings-layout.md`). Every runtime rule is `.aia-scope .aia-x`; to restyle one
from the app, use the same form in a stylesheet loaded after `ai-agent.css` (`.aia-scope .aia-btn-primary { … }`).
