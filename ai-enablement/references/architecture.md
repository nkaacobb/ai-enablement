# Architecture

The runtime (the *how*) under the application's capability folder (the *what*, `spec/framework.md`):

```
host app ──hooks──▶ ContextManager ──snapshot/hash──▶ conversation.js (planTurn / buildRequestMessages)
   │                    (core/context.js)                           │
   │                                                                ▼
   ├── toggle / push ◀── AgentDrawer (ui/drawer.js) ◀─view hooks── AgentSession (core/session.js)
   │                         │   ▲                                       │  tool loop, cards, built-ins, chats
   │                         │   └── markdown.js (render)                ▼
   │                         │                              client.js (streamChat, fallback)
   │                         │                                           │
   │                         ▼                          adapters/{openai-chat, anthropic, gemini, relay}.js
   └── settings button ─▶ Settings panel (ui/settings-panel.js)        │
                              │                                       ▼
                        core/settings.js (localStorage)        core/transport.js (fetch + SSE, errors)
                                                                      │
                                                  LM Studio · Ollama · OpenAI · Anthropic · Gemini · …
                                                  or the app's relay (relay.php / relay.mjs → same adapters)
```

## Files

| File | Responsibility | DOM? |
| --- | --- | --- |
| `ai-agent.js` | `createAiAgent()` wires everything and returns the public API; `fromDom()` helper. | yes |
| `ai-agent.css` | All styling, scoped under `.aia-scope`, themable with `--aia-*` variables, light/dark. Two layout guards: `aia-guard: host-isolation` (the page's element CSS does not reach the agent's UI) and `aia-guard: pinned-chrome` (the settings dialog's header, tabs and footer never shrink); every other rule is `.aia-scope .aia-x` and states what it depends on (`settings-layout.md`). | – |
| `core/context.js` | App/page/content/view hooks → text; snapshot + fingerprint; truncation. | no |
| `core/conversation.js` | The sync protocol: `planTurn`, `contextState`, `buildRequestMessages`, snapshot block format. | no |
| `core/hash.js` | `hashText` (cyrb53, 53-bit), `stableStringify` (sorted keys). | no |
| `core/prompt.js` | `DEFAULT_SYSTEM_PROMPT`, the screen protocol text, `buildSystemPrompt`. | no |
| `core/settings.js` | Schema, validation, layered defaults, per-app storage, key store (session vs device). | storage |
| `core/providers.js` | Provider catalog (presets, protocol, paths, quirks). | no |
| `core/client.js` | `resolveTarget`, `streamChat` (with fallback), `listModels`, `testConnection`. | no |
| `core/relay-probe.js` | `probeRelay` (the relay's GET), `relayDefaults`, `adjustForRelay` (keeps saved settings usable). | no |
| `core/blocks.js` | `parseBlockValues`: values from a fenced block, checked against an allowlisted schema, clamped. | no |
| `core/tools.js` | Tools: definitions (`parameters` or `inputSchema`, effects, annotations), JSON Schema out, MCP descriptors (`toMcpTool`), argument validation, on/off and availability, the TOOLS prompt section, text-mode blocks, `ai-tools.json` in/out. | no |
| `core/schema.js` | JSON Schema input → the runtime's fields (the enforceable subset; the rest refused with the reason); effects ↔ MCP annotations. | no |
| `core/permissions.js` | Allow / ask / deny policies (names, wildcards, `toolset:`, `effect:`), merging, the decision, whether to confirm. | no |
| `core/capabilities.js` | The capability index: loading tool modules, toolsets, skills and agents (fetch and import injected), linking toolsets, checking references. | no |
| `core/skills.js` | Agent Skills: `SKILL.md` parsing and rules, the SKILLS prompt section, active skills, safe skill-file paths, `/name`. | no |
| `core/agents.js` | Agent files: parsing, composition (tools, toolsets, skills), context layers, memory policy, the AGENT prompt section. | no |
| `core/frontmatter.js` | The YAML subset of skill and agent frontmatter (linear, reports what it cannot read). | no |
| `core/workspace.js` | The development workspace client (probe, list, read, search, write), path checks, the line diff shown before a replacement. | no |
| `core/memory.js` | Memory: the notes kept between conversations — the app's file as the base, this browser's changes on top, the MEMORY prompt section, `ai-memory.json` in/out. | storage |
| `core/files.js` | Attachments: what kind a file is, reading it (the app's `readFile` first), limits and refusals, the `<attached_file>` block and stubs the model reads. | no |
| `core/bytes.js` | Text decoding (UTF-8/16, Windows-1252), DEFLATE via `DecompressionStream`, a small ZIP reader. | no |
| `core/pdf.js`, `core/office.js` | Text of PDF files (page tree, fonts, ToUnicode, object streams, forms) and of Word / Excel / PowerPoint / OpenDocument / RTF. Loaded with `import()` on first use. | no |
| `core/transport.js` | `requestJson`, `requestStream` (idle timeout, abort), SSE parser, error codes + guidance, redaction. | no |
| `core/messages.js`, `core/reasoning.js` | Transcript normalisation and the neutral image format (`images: [{ mime, data }]`); `<think>` splitting. | no |
| `adapters/*.js` | One per wire protocol: build request, parse stream chunks / whole replies, list models. | no |
| `core/session.js` | The agent without its UI (`AgentSession`): the active agent (tools, skills, permissions, context layers, memory policy), the system prompt, the tool set of each request, the tool loop (`runModel`, `executeTool`: every check, the cards it asks as data, the run with its timeout), the built-in tools (`remember`, `forget`, `take_screenshot`, `use_skill`, `read_skill_file`, the workspace tools, `describeHost`), a question's start and end (`beginTurn`, `endTurn`), the context flag, saved chats. It shows things through view hooks, and runs headless (the `execute-tool` and `tool-loop` conformance suites drive it). This is what other stacks port. | no |
| `ui/drawer.js` | The drawer, a view over the session: toggles, open/close/push, streaming render, receipts, the flag, the tool rows and the cards (the question each card asks, including the diff before a file is replaced), the saved-chat library, resume, actions, errors, the agent picker, the camera button and thumbnails, the + menu, drag and drop, paste, file chips and the file viewer. | yes |
| `skills/create-tool/SKILL.md` | The built-in skill for in-app authoring (offered while the workspace answers). | – |
| `ui/capture.js` | Screenshots: the app's `screenshot` hook, or the browser's screen capture of this tab (drawer cropped off); scaled JPEG + thumbnail. Attached image files take the same road (`imageFromFile`). | yes |
| `ui/settings-panel.js` | The modal: Model / Agent (with the active agent and its skills) / Tools (with permission badges) / Memory / Vision / Context tabs, draft + save, load models, test connection. | yes |
| `ui/markdown.js` | Escape-first Markdown renderer with code-block actions. | no (string in/out) |
| `ui/dialogs.js` | `dialogs: 'dock'`: native modal dialogs shown non-modally beside the open drawer, switch events swallowed. | yes |
| `ui/layout-check.js` | Dev-time checks (`devWarnings`): the pushed layout fits beside the drawer; the settings dialog's computed styles match its layout spec (host CSS leaking in). Never change the page. | yes |
| `ui/resize.js`, `ui/dom.js`, `ui/icons.js` | Resize handle, DOM helpers (debounce with max wait, key isolation, `setControlValue`), inline SVG icons. | yes |

Outside the runtime:

| Path | What it is |
| --- | --- |
| `assets/relay/relay.php`, `relay.mjs`, `relay.config.example.php` | The relays (same contract, modes and config keys) and the documented config. |
| `spec/` | The portable contract every runtime implements: `framework.md`, `context-sync.md`, `behaviour.md`, `tool-catalog.md`, and `conformance/*.json` (frozen input → output vectors from this runtime). |
| `stacks/<id>/STACK.md` | One recipe per stack (`web`, `rust-egui`, `dotnet-winforms`): how each install step is done there. |
| `scripts/detect.mjs` | Run first on any app: the **stack** (`scripts/lib/stacks.mjs`: from Cargo.toml, .csproj, tauri.conf.json, package.json, PHP/HTML; supported / planned / unsupported), then what is there (runtime, relay, integration, capability folders, manifest or 1.x record, dev-time folders), which version, are the copies unchanged, which features are used, which workarounds are now redundant; the lifecycle route. |
| `scripts/conformance-export.mjs`, `scripts/lib/conformance.mjs`, `scripts/lib/conformance-runtime.mjs` | The conformance suites (what each pins, its cases, how this runtime computes the output: the pure functions; then requests, adapters and the session through a scripted harness) and the export that writes `spec/conformance/` (`--write`, only for an intended behaviour change). |
| `scripts/validate.mjs` | Loads a capability folder with the runtime's own modules (a compiled app's tools from its generated tool catalog): errors and warnings; checks the manifest and the stack. |
| `scripts/scaffold.mjs` | Creates a tool, toolset, skill or agent from `assets/templates/` and registers it; never overwrites. |
| `scripts/workspace.mjs` | The development workspace server (static site + relay + `/ai-workspace`): read the app's source, write in its capability folder. Never deployed. |
| `scripts/release-hashes.mjs`, `scripts/release-hashes.json` | Fingerprints of every released runtime and relay, so `detect.mjs` can tell unchanged copies from edited ones. |
| `scripts/verify.mjs` | Drives headless Edge/Chrome through an integration and reports pass/fail per check, including every Settings tab against its layout spec (`scripts/lib/settings-layout.mjs`, shared with the browser tests). |
| `scripts/guards.mjs`, `scripts/lib/css-guards.mjs` | The layout guards of an app's runtime stylesheet: a small CSS reader (rules, specificity, the winning declaration) that `detect.mjs` uses for `layoutGuards` and the host's element rules; `--apply` adds missing guard blocks to an edited copy in place. |
| `scripts/lib/cdp.mjs` | Zero-dependency DevTools-protocol driver (Node 22+ global WebSocket), shared by `verify.mjs` and the browser tests. |
| `tests/` | `conformance` (this runtime against `spec/conformance/`), `session` (the session headless: saved chats, agents, reset), `runtime` (pure modules), `framework` (frontmatter, schema, permissions, skills, agents, the capability index, the workspace client), `files` (attachment readers), `relay` (both relays + fake upstream), `browser` (real browser: keys, dialogs, tools, memory, screenshots, attachments), `framework-browser` (agents, skills, permissions, in-app authoring end to end), `workspace` (the dev server's rules), `lifecycle` (scaffold, validate), `example` (Hello World), `detect` (stacks too), `guards`. `fixtures/hostile-host/`: a page whose stylesheet styles bare elements (the settings-layout regression, also for `verify.mjs`). |

Everything under `core/` and `adapters/` is DOM-free, which is why the Node relay can import it and the unit tests can
run without a browser. What needs a browser (`ui/`) is covered by `tests/browser.test.mjs`.

## One request, end to end

1. User presses Enter. `AgentDrawer.send()` has the session check the configuration (`resolveTarget`) before touching
   the transcript.
2. `AgentSession.beginTurn()`: `ContextManager.snapshot()` reads the content hook and fingerprints it; `planTurn()`
   decides whether to attach it; the user message joins the transcript (a `/skill` activates that skill).
3. The drawer shows the user message with its receipt chip (Read the page / Page unchanged); the flag updates.
4. `buildSystemPrompt()` = editable prompt + the active agent's instructions + app context + page context (each as
   the agent's context layers allow) + screen protocol (+ the attached-files rules while the conversation has files,
   + the image rules for vision models) + memory (as the agent's memory policy allows) + skills (listed; the active
   ones in full) + tools (the agent's, minus denied ones) + formatting rules.
5. `buildRequestMessages()` = history window with the newest snapshot inlined, older ones stubbed, view state last;
   attached files as `<attached_file>` blocks on their question (a file attached again is sent once); images
   (screenshots and attached image files) of the two newest questions that have one go along as images.
6. `streamChat()` → adapter → `requestStream()`; fragments arrive as `text` / `reasoning` events. With tools, the
   reply may end in tool calls: the session checks each (on? here? arguments?), asks the user through the drawer's
   cards where the settings say so, runs the app's function, and sends the results — plus the screen, if it
   changed — back for another round, until the model answers (`AgentSession.runModel`, `executeTool`).
7. The drawer accumulates raw text, splits inline `<think>` blocks, and re-renders Markdown at most once per frame.
8. On finish (`AgentSession.endTurn`): the reply is stored, the chat persisted (without snapshot text), and the flag
   recomputed. On failure with no text and no action taken: the question is removed from the transcript (so the sync
   state is unchanged) and the drawer shows the error with hints and an "Open settings" button.

## Security posture

- Model output is escaped before Markdown decoration; links are http(s)/mailto only; code is shown, never executed.
- Screen content is fenced in `<page_snapshot>` with its closing tag neutralised, and the system prompt tells the model
  to treat it as data, not instructions.
- Actions run only when the user clicks, through the host app's own functions.
- Memory: the model saves a note only through the `remember` tool, which it is told to use only when the user asks
  in their own message; every save shows in the chat with Undo; deleting asks first. Notes are marked as data in the
  prompt, below the rules.
- Screenshots: taken when the user presses the camera button; the agent takes one itself only when the user has
  freed it, and otherwise asks (allow once / always / no). The browser's own share prompt stands in front of a screen
  capture. Saved chats keep thumbnails, never the images; relays check type, size and count.
- Keys: sessionStorage by default, never in the settings object, redacted from errors; relay for server-side keys.
- Keystrokes typed in the agent's UI do not reach host shortcut handlers (`isolateKeys`).
- Permissions: denied tools never reach the model and cannot be turned on from the chat; `ask` rules confirm every
  call; `system` tools always confirm. A skill's `allowed-tools` is information, never a pre-approval. Skill files are
  read only inside the skill's folder; skill scripts are never run.
- The development workspace (`scripts/workspace.mjs`) is not part of the runtime and never deployed: loopback only,
  a loopback Host header, the drawer's header, same origin (or listed origins); reads skip hidden files, dependencies
  and secrets; writes only inside the capability folder, an existing file only with `replace: true`; every write is
  shown (whole, or as a diff) and confirmed. Without that server the workspace tools do not exist.
- Relays: local mode (the default) answers only its own computer — a proxied request is never "local" — with an
  upstream allow-list (cloud: catalog address only; local: private hosts only). Public mode is explicit and uses only
  the configured preset and server key, enforces same-origin (no CORS headers), rate-limits per visitor (salted,
  daily-rotating hashes; no addresses or text stored) and site-wide, caps bodies/messages/tokens, and gives visitors
  generic errors (details only to the server itself and its log). Config files are `.php` (never served) and live
  outside the web root. Streams carry keepalive comments; the upstream call is aborted when the visitor leaves.

## Lineage

The drawer, resize handle, streaming chat with a reasoning panel, escape-first Markdown with code-block actions,
saved chats, the settings modal and the PHP proxy come from earlier in-house tools; the provider catalog without a
maintained model list, the protocol adapters behind one contract, the transport's error classification and the
connection test come from another. New in this skill: the app/page/content/view context layers, the hash-based sync
protocol with receipts and the flag, the editable system prompt layered with fixed protocol text, the Context
inspector, app-defined actions, per-tab resume, streaming for all protocols, the shared relay contract — and, from
the first real integration (1.1), the max-wait debounce, key isolation, dialog docking, the layout check, the relay
probe, production relays and the verification tooling; tools (1.2); memory and screenshots for vision models (1.3);
attachments — images and files from the + button, read in the browser without dependencies (1.5); and, as AI
Enablement (skill 2.0, runtime 1.6), the capability folder with MCP-compatible tools and toolsets, Agent Skills,
agents, permissions, and in-app tool authoring through the development workspace; then (runtime 1.6.1) the layout
guards that keep the settings dialog whole and the host page's element CSS out of the agent's UI; then (runtime 1.7) the
session taken out of the drawer, so the tool loop runs headless and other stacks port it against the conformance
vectors.
