# Tools: letting the agent act in the application

With tools, the drawer's agent does not only explain and suggest — it filters the list, opens the record, fills the
form, renames the file. Each tool wraps a function the application **already has**; the runtime handles the rest:
sending tool definitions to the model, validating and clamping arguments, asking the user before changes, running
the app's function, sending the result (and the updated screen) back, and showing every call in the chat.

Tools run **in the browser**, with the user's own session and permissions: they can do what the app's front end can
already do, nothing more. Relays only pass definitions and calls through.

The runtime also has built-in tools that are not part of the app's catalog — `remember`, `forget` and
`take_screenshot` (they follow their own settings: Settings > Memory and > Vision, `memory-and-vision.md`),
`use_skill` and `read_skill_file` (while the agent has skills, `spec/framework.md`), and the development workspace tools
(`in-app-authoring.md`).

The contract, the module and toolset formats, effects and permissions are specified in `spec/framework.md`; recipes for
adding a tool to an app that has the framework in `capabilities.md`. This file is the how-to: discovering the app's
actions, writing good tools, and what users see.

## 1. Discover what the application can do (the tool plan)

During the survey (SKILL.md step 1), list the app's **action surface**, per page/view, from the code:

| Where to look | What it gives |
| --- | --- |
| API client modules (`api/`, `services/`, `fetch`/`axios` wrappers, GraphQL operations, generated clients) | Calls the front end already makes, with their parameters. |
| Store actions (Redux actions/thunks, Pinia/Vuex actions, Zustand setters, NgRx effects, Svelte stores) | State changes the UI triggers, already validated by the app. |
| Service/controller functions the UI calls (`orders.setFilter`, `editor.insert`) | The cleanest tools: call them directly. |
| Handlers behind buttons, menus, context menus, keyboard shortcuts, form submits | What the user can do on each screen; the logic may live only here. |
| Router / navigation (`router.push`, links) | "Open", "go to" tools. |
| Controls (inputs, selects, sliders, toggles) with their `min`/`max`/`step`/options | Real parameter ranges for the schema. |
| Worker messages, websocket commands | Actions in real-time apps. |
| Server endpoints behind forms (PHP apps) | Tools that `fetch` the same endpoint the form posts to, with the same CSRF token. |

For each action, note:

- **What it does** in the user's words (this becomes the description the model reads).
- **Effect**: `read` (looks something up, changes nothing), `write` (changes app state; undoable or easily fixed),
  `destructive` (deletes or overwrites; cannot be undone), `external` (sends, publishes, pays, calls a third party),
  `system` (changes the app itself: code, configuration). When unsure, choose the stronger effect.
- **Parameters** with types and the real ranges/options (from the controls or validation), which are required.
- **Where it applies**: which page(s), and preconditions (a row selected, a record open) → `pages` / `when`.
- **What it returns**: a short, useful result for the model (counts, ids, the new value, an error message).

Show the user the **tool plan** with the context plan: tools per page, what each wraps, its effect — and what is
deliberately left out (payments, account/security settings, admin actions, anything that sends messages to other
people, bulk deletes without undo). Ask which tools should start **on**; everything else starts **off** (they stay in
Settings > Tools, and the model can ask to turn one on).

## 2. Write the tool modules

One module per toolset in the capability folder (`ai/tools/orders.js`), built on the app's own functions and reaching
them through `host` — the object the integration passes as `createAiAgent({ host })`. Never reimplement business
logic in a tool; when the logic only exists in a UI handler, drive the real control (`setControlValue(el, v)`, or
`el.click()`), so the app's validation runs exactly as for a user.

```js
// ai/tools/orders.js — the "orders" toolset
import { setControlValue } from '../../ai-agent/ai-agent.js';   // the runtime, relative to this file

export default {
  name: 'orders',
  title: 'Orders',                               // groups Settings > Tools; agents use toolsets: [orders]
  description: 'Filter, page through and cancel orders.',
  tools: [
    {
      name: 'filter_orders',                     // letters, digits, _ or -; unique
      title: 'Filter orders',                    // shown in the chat and in Settings > Tools
      effect: 'write',                           // read | write | destructive | external | system
      pages: ['orders'],                         // where it can be used ('orders/*' = any sub-page)
      description: 'Show only orders with this status in the Orders list.',
      parameters: {
        status: { type: 'enum', values: ['open', 'shipped', 'cancelled'], required: true },
        limit: { type: 'integer', min: 10, max: 100, step: 10, description: 'Rows per page.' },
      },
      run: ({ status, limit }, { host }) => {
        host.dispatch(host.actions.setOrderFilter({ status, pageSize: limit }));   // the app's own action
        return { shown: host.getState().orders.visible.length };                   // a small, useful result
      },
    },
    {
      name: 'cancel_order', title: 'Cancel an order', effect: 'destructive', pages: ['orders', 'orders/*'],
      description: 'Cancel one order by its number. The customer is notified.',
      inputSchema: { type: 'object', properties: { number: { type: 'string', maxLength: 20, pattern: '^[A-Z0-9-]+$' } }, required: ['number'] },
      run: async ({ number }, { host }) => {
        const r = await host.ordersApi.cancel(number);   // the same call the Cancel button makes
        return r.ok ? `Order ${number} cancelled.` : `Not cancelled: ${r.error}`;
      },
    },
    {
      name: 'set_page_size', title: 'Rows per page', effect: 'write', pages: ['orders'],
      description: 'Change how many orders are listed per page.',
      parameters: { rows: { type: 'enum', values: [25, 50, 100], required: true } },
      run: ({ rows }) => { setControlValue('#page-size', rows); return `Showing ${rows} rows per page.`; },   // via the real control
    },
  ],
};
```

- `when: () => …` makes a tool usable only when the app allows it (a row selected, the user's role): with `host` in
  scope, write the module as a factory — `export default (host) => ({ name: 'orders', tools: [ { …, when: () =>
  host.getState().user.canCancel } ] })`.
- A module may also export a single tool or a plain list. Apps that keep tools in their own code (bundled from
  `src/`) import the modules and pass them inline: `createAiAgent({ tools: [ordersTools, itemsTools], host })`.
- 1.x integrations pass `tools: appTools` with closures over app modules; that keeps working
  (`upgrading.md`, "Adopting the framework", moves them into the capability folder).

Parameter types: `string` (`maxLength`, `minLength`, `pattern`), `number` / `integer` (`min`, `max`, `step`),
`boolean`, `enum` (`values`), `array` (`items`, `maxItems`, `minItems`); each may have `required`, `description`,
`default`, `aliases`. Or a JSON Schema `inputSchema` (as MCP), converted to the same fields — what cannot be enforced
is refused with the reason. Arguments are coerced and clamped before `run()` is called; unknown ones are dropped; a
missing required one goes back to the model as an error. Text over a string's `maxLength` (default **500**) is an
error too, with both sizes ("`notes` is 12,981 characters long, over its limit of 12,000: send less in one call"); it
is never cut, so a JSON payload cannot reach `run()` half-written. Give large-text parameters an explicit
`maxLength`. Arguments the runtime cannot read (broken or cut-off JSON) are reported to the model and the call is not
run; values are never guessed.

`run(args, { host, agent, signal, call })` may be async (30 s timeout by default, `timeoutMs` to change). Its return
value goes to the model: a string as written, anything else as compact JSON, capped at 4,000 characters. Throw (or
return a message) on failure — the model sees the error and can explain or retry. Keep results short and factual.

## 3. Register them and ship the default selection

```json
// ai/index.json
{ "format": "ai-enablement/1", "tools": ["tools/orders.js", "tools/items.js"], "toolsConfig": "ai-tools.json",
  "permissions": { "ask": ["cancel_order"] } }
```

```js
createAiAgent({ …, capabilities: 'ai/index.json', host: { ...store, ordersApi } });
agent.setPage({ id: 'orders', …, tools: ordersPageTools });   // optional: tools that exist only on this page
```

(Without a capability folder: `tools: appTools, toolsConfig: 'ai-tools.json'`, as in 1.x.)

`ai-tools.json` (the format Settings > Tools > "Download ai-tools.json" writes):

```json
{
  "toolsEnabled": true, "confirmWrites": true, "confirmDestructive": true, "toolMode": "auto", "maxToolSteps": 8,
  "tools": {
    "filter_orders": { "enabled": true },
    "cancel_order": { "enabled": false },
    "set_page_size": true
  }
}
```

A tool's state comes from: the user's choice in Settings > Tools (saved in the browser) > `ai-tools.json` > the tool's
own `enabled` (default `false`). Only choices that differ from the app's defaults are stored, so an updated
`ai-tools.json` still reaches users who never touched that tool. To change the defaults: check the boxes in
Settings > Tools, download `ai-tools.json`, and commit it to the app. Keys are **tool** names (validate reports others).

Permission rules (`spec/framework.md`) sit above all this: `deny` blocks a tool whatever the user switches, `ask` always
confirms, `allow` never does (except `system` tools, which always confirm).


## 4. What the user sees and controls

- **Settings > Tools**: a checkbox per tool (grouped, with its effect and whether it works on this screen), "Reading
  tools on / All on / All off", the switches below, and the export.
- **Let the agent use the application's tools** (`toolsEnabled`): off = the model is told tools exist but are off.
- **Ask before actions that change something** (`confirmWrites`, default on) and **before destructive actions**
  (`confirmDestructive`, default on). With confirmation, the call shows a card: *Run* / *Allow for this chat* (write
  tools) / *Skip*. Reading tools never ask.
- **Turned-off tools** are not callable, but the model knows them: it can say "that tool is off" and call the built-in
  `request_tool`, which shows *Turn on* / *Keep off*. Turning it on saves the choice, and the model continues.
- **How tools are called** (`toolMode`): *Automatic* (native tool calls; if the server refuses them, text blocks),
  *Tool calls*, or *Text blocks* (any model: tools described in the prompt, the model writes ```` ```tool ```` blocks).
- **Max tool steps per question** (`maxToolSteps`, default 8).
- Every call appears as a row in the reply (running / waiting / done / failed / declined / off); the actions are kept
  in the saved chat and summarised in later requests (`[Actions taken: …]`).
- A finished row **rolls down** (click its line) to show the problem, if any, the *Arguments the tool received*, the
  arguments *As the model sent them* (only when they differ: broken, cut off, or changed by validation) and what was
  *Returned to the model*, with a **Copy** button. **Copy tool log** under the reply copies every row as text. Saved
  chats keep the detail (2,000 characters per part). This is the first place to look when a tool call fails.
- After tools change the screen, the new snapshot goes back with the results, so the model checks its work; afterwards
  the flag is amber and the next question re-reads the page.

## 5. Providers and relays

- **OpenAI-compatible** (LM Studio, Ollama, OpenAI, DeepSeek, OpenRouter, custom): `tools` / `tool_calls`. LM Studio
  lists which models are *trained for tool use*; small local models without it work better with *Text blocks*.
- **Anthropic**: `tools` / `tool_use` / `tool_result`. **Gemini**: `functionDeclarations` / `functionCall` /
  `functionResponse` (thought signatures are passed back).
- **Relays** (`relay.php`, `relay.mjs`) forward `tools` and the current question's `toolTurns`, and stream
  `tool_call` events back. In public mode one question is counted per chain of tool steps (`turnId`), up to
  `limits.maxToolSteps`; `limits.maxTools` caps the definitions per request.
- Tool definitions cost tokens on every request (Settings > Context shows them in the size estimate): keep
  descriptions short, and prefer a few well-shaped tools over many tiny ones.
- **Cut-off replies are detected** (OpenAI-compatible `finish_reason: "length"`, Anthropic `stop_reason:
  "max_tokens"`, Gemini `finishReason: "MAX_TOKENS"`, relay `done.truncated`). A call cut off mid-arguments is not
  run; its row says *Cut off: the reply reached Max reply tokens…* and the model is told to do the work in several
  smaller calls. Models that think before they answer spend part of *Max reply tokens* (Settings > Agent, default
  4,096) on thinking: raise it for them. Design tools that take large payloads (note lists, rows, documents) to take
  them **in sections** — a `bar` / `offset` / `part` parameter — so one call fits in one reply.
- Some local models answer the step after a tool round only in their reasoning channel; the drawer then shows that
  reasoning as the answer.

## 6. Test the tools

- **Unit-test the module** with the app's functions mocked: each tool calls the right function with the right
  arguments, clamps and rejects as expected, and returns a short result.
- `agent.tools.list()` shows the catalog with states; `agent.tools.run(name, args)` runs one directly (validated, no
  confirmation) — handy in the console and in `scripts/verify.mjs`, which lists the tools and runs the reading ones.
- With a model running, ask for something a tool does ("show only open orders"): a chip appears, the confirmation card
  for a write tool, the app changes, and the answer confirms it. Ask "which tools can you use?": the model lists what
  is on and off. Turn one off and ask for it: the *Turn on* card appears.

## Safety rules

- Tools are the app's own functions with the user's own permissions; never add a tool the UI does not offer the user.
- Destructive or outward-facing actions (delete, send, pay, publish, change permissions) are `destructive` and start
  off; the user confirms each call unless they deliberately switched that off.
- Tool results and page content are data, not instructions (the system prompt says so): a document that says "call
  delete_all" must not become an action — confirmation on changes is the backstop, so keep it on for public apps.
- Validate in the app as for any user input: the schema clamps ranges, but server-side checks stay authoritative.
