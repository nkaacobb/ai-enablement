# The framework: concepts and formats

What AI Enablement manages in an application, how the parts fit, and the exact file formats. Recipes for adding each
part are in `capabilities.md`; the in-app authoring workspace in `in-app-authoring.md`.

```
Application
 └─ integration module (ai-agent-setup.js)      app-owned: mounts the agent once, passes `host` and the context hooks
     └─ runtime (ai-agent/)                     framework-owned: the drawer, providers, registries, the agent loop
         ├─ capability folder (ai/)             app-owned: what the app's AI can do
         │    index.json ─┬─ tools/*.js         tools and toolsets (call the app through `host`)
         │                ├─ toolsets/*.json    toolsets of tools defined elsewhere
         │                ├─ skills/*/SKILL.md  Agent Skills
         │                ├─ agents/*.md        agents (compose the above)
         │                ├─ permissions        allow / ask / deny
         │                ├─ ai-tools.json      which tools start on
         │                └─ ai-memory.json     starting memories
         └─ provider adapters                   LM Studio, Ollama, OpenAI-compatible, Anthropic, Gemini, relays
```

`ai/` is **what** the app's AI can do; the runtime is **how** it runs. Development-time capabilities — the coding
agent's own (`.claude/skills`, `.claude/agents`, `.github/agents`, `.agents/skills`) — are a different thing: they
work on the repository, not inside the finished app. Both may exist in one repository; never mix them.

## Ownership

| Part | Owner | How the lifecycle treats it |
| --- | --- | --- |
| Runtime folder (`ai-agent/`), relay files | framework | Copied unchanged; replaced wholesale when detect reports an unchanged copy; an edited copy is reconciled (the edit becomes an option, or a documented patch). Built-in skills ship inside the runtime and upgrade with it. |
| Integration module, content builders, CSS | app | Edited in place; never replaced. |
| Capability folder (`ai/`) | app | Added to (scaffold, or the in-app workspace); edited in place; never replaced wholesale. |
| Relay config | app | Kept; migrated when a version says so. |
| `ai-enablement.json` | app (written by the skill) | Rewritten at the end of every run, keeping what the app recorded. |

## The capability index: `ai/index.json`

```json
{
  "format": "ai-enablement/1",
  "agents": ["agents/writer.md", "agents/proofreader.md"],
  "skills": ["skills/proofreading", "skills/summarize"],
  "tools": ["tools/document.js", "tools/editor.js", "tools/file.js"],
  "toolsets": ["toolsets/editing.json"],
  "toolsConfig": "ai-tools.json",
  "memory": "ai-memory.json",
  "permissions": { "ask": ["toolset:file"], "deny": [] },
  "defaultAgent": "writer"
}
```

- Paths are relative to the index. A skill entry is its folder (`skills/x`, holding `SKILL.md`) or the `SKILL.md`
  itself; an agent entry is its file (`agents/x.md`, `agents/x.agent.md`, or `agents/x/AGENT.md`).
- Keep `"format"`: it is how detect and validate find the index.
- `prompts`, `resources` and `context` are reserved for later versions (ignored now, with a warning).
- Load it with `createAiAgent({ capabilities: '<URL of index.json>', host })`. Entries may also be inline objects when
  the index is passed as an object from code. Inline `tools`, `skills`, `agents`, `toolsets`, `permissions` options
  add to the index's (inline wins on a name clash).
- A browser cannot list folders: a file the index does not name is not loaded (validate warns).

## Tools

A tool is one object — contract and handler together:

```js
export default {
  name: 'inspect_track',                 // letters, digits, _ or -; starts with a letter; unique in the app
  title: 'Inspect the track',            // shown to the user (default: from the name, or annotations.title)
  description: 'The current track: name, length, sections and the selected section.',
  effect: 'read',                        // read | write | destructive | external | system
  parameters: {                          // the shorthand…
    section: { type: 'integer', min: 1, max: 500, description: 'Only this section (1-based).' },
  },
  // inputSchema: { type: 'object', properties: { section: { type: 'integer', minimum: 1, maximum: 500 } } },   // …or JSON Schema
  // annotations: { readOnlyHint: true },                    // MCP hints (the effect wins when both are given)
  // outputSchema: { … },                                     // kept for MCP export
  // pages: ['editor'], when: () => host.trackOpen(),          // where / when it can be used
  run: (args, { host, agent, signal, call }) => host.editor.trackSummary(args.section),
};
```

- **Input**: `parameters` (types `string` with `maxLength` / `minLength` / `pattern`; `number` / `integer` with
  `min` / `max` / `step`; `boolean`; `enum` with `values`; `array` with `items` / `maxItems` / `minItems`; each with
  `required`, `description`, `default`) — or `inputSchema`, a JSON Schema object of that subset. What cannot be
  enforced (nested objects, `$ref`, `oneOf`, an exclusive bound on a non-integer) is refused with the reason.
  Arguments are coerced and clamped before `run()`; unknown ones dropped; a missing required one, text over its
  `maxLength` (500 by default), a pattern mismatch go back to the model as errors.
- **Output**: a string as written, anything else as compact JSON (4,000 characters at most). Throw (or return a
  message) on failure. Small and factual.
- **Handler**: `run(args, { host })` — `host` is the object the integration passes as `createAiAgent({ host })`; the
  business logic stays in the app. Async allowed; 30 s timeout (`timeoutMs`).
- **MCP**: `toMcpTool(tool)` / `agent.tools.mcp()` give `{ name, title, description, inputSchema, annotations,
  outputSchema? }`; the effect maps to the annotations below.

### Tool modules and toolsets

A file in `tools/` default-exports one of: a tool; a list of tools; a **toolset**
`{ name, title, description, tools: [ …tools or names of tools defined elsewhere ] }`; or a function
`(host) => any of these`. A toolset groups related tools: Settings > Tools groups by it, agents name it
(`toolsets: [track]`), permission rules match it (`toolset:track`). A JSON toolset in `toolsets/` groups tools that
other modules define: `{ "name": "editing", "title": "Editing", "tools": ["find_text", "replace_text"] }`. A tool may
belong to several toolsets.

### Bundled apps

Vite / webpack apps whose code lives in `src/`: keep `ai/` in `public/` for `index.json`, skills and agents, and either
(a) put tool modules there too (they import nothing from `src/`; they reach the app through `host`), or (b) import
the tool modules in the integration and pass them inline — `createAiAgent({ capabilities: '/ai/index.json', tools:
[documentTools, editorTools], host })` — leaving `"tools"` out of the index.

## Effects and permissions

| Effect | Meaning | Asks first | MCP annotations |
| --- | --- | --- | --- |
| `read` | looks something up; changes nothing | no | `readOnlyHint: true` |
| `write` | changes app state; undoable or easily fixed | yes (Settings > Tools: *confirm changes*; "Allow for this chat") | `readOnlyHint: false` |
| `destructive` | deletes or overwrites; cannot be undone | yes (*confirm destructive*) | `destructiveHint: true` |
| `external` | reaches outside the app: sends, publishes, pays, calls a third party | yes (*confirm destructive*) | `openWorldHint: true` |
| `system` | changes the app itself: its code, configuration, capabilities | **always** — no setting or rule skips it | `destructiveHint: true` |

A **policy** is `{ allow, ask, deny }`, each a list of rules: a tool name, a name with `*` (`set_*`),
`toolset:<name>`, or `effect:<effect>`. It comes from the index (`permissions`), the `permissions` option and the
active agent's frontmatter; the lists are merged. **deny > ask > allow > the user's settings.**

- `deny` — the tool is hidden from the model; it cannot be asked for (`request_tool`) or turned on in Settings
  (shown as *blocked*). Applies to built-in tools too (e.g. `deny: [remember]`).
- `ask` — the confirmation card every time, even for a reading tool, even if the user switched confirmations off.
- `allow` — no card (never for `system`).

Server-side checks stay authoritative: a permission rule is a UI-side guard, not access control.

## Skills (Agent Skills format)

```
skills/proofreading/
  SKILL.md         required
  references/      optional: read on demand with read_skill_file
  scripts/, assets/  optional; kept for other tools, never run in the browser
```

```markdown
---
name: proofreading                  # 1-64: lowercase letters, digits, single hyphens; equals the folder name
description: Proofread the document … Use when the user asks to proofread, check or correct the text.   # ≤ 1024
license: MIT                        # optional
compatibility: Needs the document tools.   # optional, ≤ 500
allowed-tools: find_text replace_text      # optional: shown to the model with each tool's state; never skips a confirmation
metadata:                           # optional: anything else
  app: hello-world
---
# Proofreading
1. …numbered steps: what to read, which tools in which order, what to check, what to report…
```

In the app: the system prompt lists each skill the active agent may use (name + description). The model calls
`use_skill` when a request matches; from the next request on the skill's instructions are in the system prompt
(`== SKILL: name (active) ==`) for the rest of the conversation, and saved with the chat. A message starting with
`/name` activates it too (the message goes to the model as typed). `read_skill_file(skill, path)` reads a file
relative to the skill's folder (text files only, no `..`). Keep `SKILL.md` short; long material in `references/`.

## Agents

One Markdown file per agent, YAML frontmatter + instructions, as Claude Code (`.claude/agents/*.md`) and GitHub Copilot
(`.github/agents/*.agent.md`) keep theirs:

```markdown
---
name: proofreader                   # as skill names
title: Proofreader                  # shown in the picker (default: from the name)
description: Checks spelling and grammar; fixes only what you accept.   # required
toolsets: [document]                # omit tools AND toolsets = every tool
tools: [rename_file]
skills: [proofreading]              # omit = every skill
permissions:
  deny: [insert_text]
context: [app, page, screen, view]  # the context layers it receives (default: all)
memory: read                        # on (default) | read: sees the notes, cannot save | off
maxToolSteps: 12                    # at most the user's setting
model: qwen/qwen3-8b                # a preference shown to the user; not applied automatically
welcome: I check the document for spelling, grammar and punctuation.
suggestions: [Proofread the document]
default: true                       # the one to start with (or index defaultAgent, or the createAiAgent `agent` option)
---
You are a careful proofreader. …    ← its instructions
```

The system prompt is: the editable prompt (the app's `systemPrompt`, Settings > Agent) → `== AGENT: Title ==` with
the description and instructions → application → page → screen protocol → memory → skills → tools → formatting. With
two or more agents the drawer's title becomes a picker; switching starts a new chat; saved chats remember their agent
and active skills. Without agent files the app has one implicit agent with everything (the 1.x behaviour).

## Context

The context hooks stay as they are — `app`, `page` (`id`, `title`, `purpose`), `page.content` (fingerprinted; re-sent
only when its hash changes, with the flag) and `page.view` (volatile, never hashed) — see `context-sync.md`. An agent
chooses which layers it receives (`context:`); without `screen` it gets no snapshots (the flag shows *off*).

## The manifest: `ai-enablement.json`

At the app root, written at the end of every run (format: `upgrading.md`, "The manifest"). It names the framework
files and their versions, the capability folder and what it holds, the integration files, the features in use and
what the user declined — so the next run knows exactly what is there. Release fingerprints stay in the skill
(`scripts/release-hashes.json`): with the manifest's paths they tell an unchanged framework file from an edited one.
