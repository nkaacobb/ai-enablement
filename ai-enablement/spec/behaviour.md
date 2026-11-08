# The behaviour contract: what every runtime does

This is the product, independent of language and UI. The JS runtime (`assets/ai-agent/`) is the reference
implementation; a runtime for another stack (Rust, .NET) is "the same agent" when it does everything below and passes
the conformance vectors (`spec/conformance/`). Formats are in `framework.md`; the context-sync protocol in full is in
`context-sync.md`. Where this file names a JS module, that module is the authority when they disagree. Fix this file
when that happens.

Numbers are defaults. Settings ranges are given as `default (min–max)`.

## 1. Context: app, page, content, view

- **Layers**. These go into the system prompt on every request:
  - `app`: name, purpose, capabilities, limits.
  - `page`: `id`, `title`, `purpose` and other descriptive keys.

  These are hashed and sent only when they change: `content`, what the screen shows. These are sent with the newest
  question, never hashed: `view`, the volatile state such as cursor, selection and scroll. The view is clipped at
  2,000 characters.
- **Content text**: a string as written; `{ text }` alone → the text; anything else → stable JSON (sorted keys,
  2-space indent; Date → ISO, Map → object, Set → array, non-finite numbers → null, undefined / functions dropped,
  cycles → `"[Circular]"`).
- **Fingerprint**: `cyrb53(pageId + "\n" + pageTitle + "\n" + fullContent)`, computed on the full text **before**
  clipping. It is shown as 7 hex characters.
- **Clip**: `maxContextChars` 24,000 (1,000–400,000). The runtime keeps `max − 160` characters and adds
  `…[truncated: the first N of M characters are shown]`.
- **Sync decision per question** (`planTurn`): `off` → `empty` → `forced` → `unread` / `trimmed` → `changed` →
  `unchanged`. A snapshot is attached for forced, unread, trimmed and changed. Wire format, stubs and the
  unchanged line: `context-sync.md`.
- **Window**: `historyMessages` 20 (2–200); `windowStart(n) = max(0, n − max(2, history))`.
- **Flag states**: `synced | dirty | unread | none | off` (+ `pending` while a request runs). The flag is recomputed:
  - on the change signal, debounced 300 ms with a max wait of 1,000 ms;
  - on a page change, on open, after every reply and on a settings change;
  - optionally, by polling (`watch`).
- **A failed question with no answer is removed** from the transcript, so the sync state rolls back.
- **After a tool round** in which any non-read tool ran, the screen is re-read. If its hash changed, the new snapshot is
  appended to the last tool result as `[The screen after these actions]`.
- **Saved chats** keep hashes, not snapshot text, so a reopened chat is `unread`.
- **Agents** pick their layers (`context: [app, page, screen, view]`). Without `screen` there are no snapshots and the
  flag shows `off`.

## 2. The system prompt

Sections are joined with a blank line, in this order (`core/prompt.js`, `buildSystemPrompt`):

1. The base prompt. Precedence:
   1. what the user saved in settings;
   2. the app's `systemPrompt`;
   3. `DEFAULT_SYSTEM_PROMPT`.
2. `== AGENT: Title ==` with the active agent's description and instructions (none for the implicit agent).
3. `== APPLICATION ==`.
4. `== CURRENT PAGE ==`.
5. The screen protocol, or the "sharing switched off" sentence.
6. The files protocol, while attached files are in the window.
7. The vision protocol, or the image-files protocol, when the model sees images.
8. `== MEMORY ==`, `== SKILLS ==` and `== TOOLS ==`.
9. The formatting rules.

The protocol texts are part of the contract: copy them verbatim from `core/prompt.js`, `core/files.js`,
`core/memory.js`, `core/skills.js` and `core/tools.js` (the golden prompts in `spec/conformance/` check this).
Objects are rendered as `Humanized Key: value` lines and lists as `- ` bullets.

## 3. Tools

- **Contract**:
  - `name`: letters, digits, `_` or `-`; starts with a letter; ≤ 64; unique; `request_tool` is reserved.
  - `description` is required.
  - `parameters` (shorthand) XOR `inputSchema` (JSON Schema subset).
  - Optional: `title`, `annotations`, `outputSchema`, `pages`, `when`, `timeoutMs` (30 s), `enabled` (false).
- **Effect**: an explicit valid effect; else derived from MCP annotations (readOnly → read, destructive →
  destructive, openWorld → external, an explicit `false` hint → write); else `write`.
- **On / off**: the user's choice, then the tool config file (`ai-tools.json`), then the tool's own `enabled`.
  Read tools usually start on; destructive, external and system tools start off.
- **Availability**: `pages` match exactly, or by prefix when the pattern ends in `*`. `when()` may veto; a `when()`
  that throws counts as false. Page tools default to their page.
- **Per request**, tools split into **callable / off / elsewhere**.
  - Callable tools go to the model natively, plus `request_tool` whose enum names the tools that are off.
  - Descriptions of non-read tools get a short suffix (for example ` (changes the application)`).
  - The TOOLS section also lists the off tools, and the elsewhere tools with their pages.
- **Text mode** (models without native tools):
  - The model writes fenced blocks tagged `tool`, `tool_call`, `tool-call` or `tool_code`, holding
    `{"name","arguments"}`.
  - Results go back as a user turn: `<tool_results>…</tool_results>`.
  - **Auto mode**: a native request that fails before any text, with an error naming tools or functions (not cancelled,
    auth, network or timeout), switches to text mode. This is remembered per transport, provider and model.
- **Arguments**:
  - **Parsing**:
    - An object is used as is. Otherwise JSON, then JSON with trailing commas removed, then loose `key: value` lines.
    - Unclosed JSON is "cut off". A call is never run on guessed arguments.
  - **Validation**:
    - Keys match case-insensitively; unknown keys are dropped; missing optional keys take their `default`; a missing
      required key is an error.
    - **Text over `maxLength` (500 by default) is an error, never silently cut.**
    - Numbers snap to `step`, then clamp to min / max; integers round.
    - Enums match exactly, then case-insensitively, then normalised.
    - Booleans accept `true/yes/on/1/enabled/enable/y` and their opposites.
    - Arrays: at most `maxItems` (100); `minItems` is checked.
  - **JSON Schema input**: `$ref`, `oneOf`, `anyOf`, `allOf`, `not`, `if`, `patternProperties`, object properties,
    tuples, arrays of arrays and multi-type unions (except `[T, null]`) are **refused with the reason**, never loosened.
- **Executing a call**, in this order (`executeTool`):
  1. `request_tool` → card *Turn on* / *Keep off*.
  2. Unknown or denied tool → the reason goes back to the model.
  3. The master switch is off → refused.
  4. The tool is off → card *Turn on and run* / *Keep off*.
  5. Not available on this page → refused.
  6. Argument error (the cut-off message if the reply was truncated) → back to the model.
  7. Validation errors → back to the model.
  8. Precheck.
  9. Confirmation (§4) → card *Run* / *Allow for this chat* (write tools only, unless the rule is `ask`) / *Skip*.
  10. Run with its timeout.
  11. Serialise the result.
- **Fixed messages** to the model:
  - `The user declined to run X. Do not call it again for this request.`
  - `Stopped by the user.`
  - `Error: <message>`
- **Results**: undefined / null / true → `Done.`; strings as written; anything else as compact stable JSON. Capped at
  4,000 characters with a note.
- **Loop**:
  - Calls run **sequentially**.
  - At most `min(maxToolSteps 8 (1–30), agent.maxToolSteps)` rounds, with a hard cap of 64.
  - Only the current question carries the provider-native tool exchange. Earlier assistant turns are prefixed
    `[Actions taken: call → done | failed | declined by the user | tool turned off | skipped (summary)]`.
  - If the step after a tool round has no text but has reasoning, the reasoning is shown as the answer.
- **MCP export** (`toMcpTool`): `{ name, title, description, inputSchema, outputSchema?, annotations }`. The
  annotations come from the effect:

  | Annotation | Value |
  | --- | --- |
  | `readOnlyHint` | the effect is read |
  | `destructiveHint` | the effect is destructive or system |
  | `openWorldHint` | the effect is external |

## 4. Effects, permissions, confirmations

- **Effects**: `read`, `write`, `destructive`, `external` (leaves the app: sends, publishes, pays) and `system` (changes
  the app itself: code, configuration, capabilities).
- **Policy**: `{ allow, ask, deny }`. Each rule is a tool name, a `*` glob, `toolset:<glob>` or `effect:<glob>`. The
  lists merge from the index, from options and from the **active agent**. **deny > ask > allow > the user's settings.**
  - **`deny`** hides the tool from the model and from `request_tool`. Settings cannot turn it on. It removes built-ins
    too (`deny: [remember]`).
- **Confirmation** (`needsConfirmation`): the first rule that matches decides.

  | Situation | Confirm? |
  | --- | --- |
  | `system` | Always. No rule or setting skips it. |
  | `ask` | Yes |
  | `allow` | No |
  | `write` | While `confirmWrites`, unless *Allow for this chat* |
  | `destructive` or `external` | While `confirmDestructive` |
  | `read` | No |
- **Not access control**: permission rules are a UI-side guard. The app's own checks stay authoritative.
- **`allowed-tools` in a skill** is information shown to the model, **never** a pre-approval.

## 5. Built-in tools

These are application tools under the same rules (a `deny` removes them; an app tool with the same name replaces
them):

| Tool | Effect | Exists when |
| --- | --- | --- |
| `remember(text, id?)` | read, with an Undo | memory on, saving allowed, agent `memory: on` |
| `forget(id)` | write (asks) | same |
| `take_screenshot` | read | vision on and capture possible. Callable only when the user has freed the agent to look; otherwise a card *Allow once / Always / No*. |
| `use_skill(name)` | read | the active agent has skills |
| `read_skill_file(skill, path)` | read | same |
| `describe_host`, `list_source_files`, `read_source_file`, `search_source`, `write_ai_file` (system), `reload_capabilities` | as listed | the development workspace answers (`references/in-app-authoring.md`) |

## 6. Agents and skills

- **Agents**:
  - With no agent files there is one implicit agent with every tool and skill.
  - The start agent is the `agent` option, else `defaultAgent`, else the agent marked `default: true`, else the first.
  - Omitted `tools` / `toolsets` / `skills` lists mean "all".
  - `memory: read | off` removes `remember` / `forget`.
  - With two or more agents there is a picker. Switching saves the chat and starts a new one; chats remember their
    agent and their active skills.
- **Skills**, progressive disclosure:
  - The model sees each skill's name and description.
  - `use_skill` makes the skill active for the rest of the conversation, with its body in the prompt (clipped at
    16,000 characters).
  - A message starting `/name` activates it too.
  - `read_skill_file` takes relative paths only (no `..`, no hidden segments), text extensions only, in 20,000-character
    chunks with an offset.
  - A skill's `scripts/` are never run.

## 7. Memory

- Notes, not instructions: at most 100 notes of at most 500 characters each, flattened to one line. Ids are `m<n>`.
  Duplicates are detected case-insensitively.
- The app's file (`ai-memory.json`) is the base. The user's changes are stored on top: only the items that differ, plus
  a list of deleted ids. The newer timestamp wins. The app's save hook is debounced 400 ms.
- The prompt marks the notes as data below the rules. The screen wins over a note, and the model saves a note only
  when the user asks in their own message.
- **Storage is the host's choice**:
  - Browser: localStorage.
  - Native: a file in the user's app-data folder, or the app's own store (an encrypted vault, a database) through a
    hook.

## 8. Images and files

- **Images**: at most 3 per question. Only the 2 newest questions that carry images send them. Images are JPEG with the
  longer edge at most 1,280 px. Saved chats keep thumbnails only.
- **Files**:
  - At most 5 per question, 25 MB each; the text is clipped at `maxFileChars` 40,000 (1,000–400,000).
  - Sent as `<attached_file name type … chars [truncated] hash>` with the closing tag escaped.
  - The same content attached again is sent only with its newest question.
  - Old binary formats are refused with what to save them as.
  - The app's `readFile` hook goes first.
- **Screenshot source is the host's choice**: the browser's screen capture or a canvas hook on the web; the app's own
  window or framebuffer natively. A host may switch screenshots or attachments off entirely.

## 9. Providers and transport

- **Protocols**: OpenAI-compatible chat (LM Studio, Ollama, OpenAI, DeepSeek, OpenRouter, custom), Anthropic Messages,
  Gemini generateContent, and the relay. One neutral contract sits in front of them:
  - tools `[{ name, description, parameters }]`;
  - tool turns `[{ text, calls: [{ id, name, arguments, signature? }], results: [{ id, name, content, images? }] }]`;
  - images `{ mime, data }`.

  Wire details per protocol: `adapters/*.js`. The replay of tool exchanges and Gemini's `thoughtSignature` echo are
  part of the contract.
- **Streaming**: SSE following the EventSource rules. The idle timeout is re-armed on every chunk. A whole JSON reply
  is accepted too.
- **Truncation**: `finish_reason: length` / `max_tokens` / `MAX_TOKENS` marks the reply as truncated. A tool call cut off
  this way is diagnosed, never run.
- **Caps**: request 1 MB, images 12 MB, response 8 MB.
- **Errors**: `auth, missing-model, bad-endpoint, network, timeout, rate-limit, refused, malformed, cancelled, budget`,
  each with guidance. Keys are redacted from every message.
- **Fallback provider**: only when no text has arrived yet, and only for `network`, `bad-endpoint` or `timeout`. A
  notice is shown.
- **Test connection**: list models, check the selected model, then run a tiny generation with reasoning off.
- **Settings** (defaults):

  | Setting | Default |
  | --- | --- |
  | `provider` | `lmstudio`, at `127.0.0.1:9000` |
  | `temperature` | 0.4 |
  | `maxOutputTokens` | 4096 (64–64,000) |
  | `timeoutSec` | 120 (10–900) |
  | `reasoning` | `show` |
  | `shareScreen` | on |
  | `confirmWrites`, `confirmDestructive` | on |
  | `maxToolSteps` | 8 |
  | `vision` | on |
  | `screenshotAuto` | off |

  The order of precedence is: built-in defaults, then the app's `defaults`, then what the user saved.
- **Keys are never stored with the settings**:
  - Browser: sessionStorage (or localStorage when the user ticks "remember").
  - Native: the OS keychain (Windows Credential Manager / DPAPI).
  - A host may **restrict providers** (for example loopback only). The runtime then refuses others before any request.

## 10. Chats and the UI

- **Chats**: at most 50 saved chats of 200 messages each, namespaced by `appId`, saved without snapshot text. A host
  that must not persist chats switches saving off. The previous chat is resumed per tab or window.
- **Markdown**: escaped first; links only http(s) and mailto; code is shown, never run. Code-block and reply actions are
  app-defined buttons that apply values through the app's real controls.
- **Required screens**:
  - The drawer: toggle, a hotkey (Ctrl+I by default), the flag, receipts (*Read the page* / *Page unchanged*), streaming
    with reasoning, tool rows with their confirmation cards, and the agent picker.
  - Settings tabs **Model / Agent / Tools / Memory / Vision / Context**. The Context tab shows exactly what the model
    receives.
- **Keyboard**: typing in the agent never reaches the app's shortcuts.
- **Lock / sign-out**: a host with sessions can reset the agent. A reset drops the transcript, pending tool calls and
  in-flight requests.
