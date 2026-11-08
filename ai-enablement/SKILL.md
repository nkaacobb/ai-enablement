---
name: ai-enablement
description: Install, upgrade and extend an application's AI layer. Builds a screen-aware AI agent into the app (a chat drawer that sees the current page with hash-synced context, acts through tools wrapping the app's own functions, remembers notes, looks at screenshots, takes attachments, works with any LLM - LM Studio, Ollama, OpenAI, Anthropic, Gemini, relays) and manages its capability folder - MCP-compatible tools and toolsets, Agent Skills, agents, permissions - plus a dev workspace where the in-app agent writes new tools. Use when the user asks to add or integrate an AI agent, assistant, chatbot or copilot panel into an app; to update or upgrade one; to add a tool, toolset, skill or agent to an app's AI ("add a tool that lets the AI inspect the current track"); to let the app's AI create tools; or mentions ai-enablement, add-ai-skill or the agent drawer. Stacks: web (HTML, PHP, React, Vue, Svelte, Angular, Next.js, Tauri, ASP.NET Core); native Rust/egui and C#/WinForms planned - detect says which.
license: MIT
---

# AI Enablement

You manage an application's AI layer, end to end and repeatably, on whatever stack the app is built with:

- **The runtime**, copied into the app unchanged: the agent drawer. It slides in from the right and sees what the user
  sees: app + page + live screen content, re-sent only when its hash changes, with a flag. It acts through tools that
  wrap the app's own functions (confirmations, per-tool on/off, permissions). It also:
  - remembers notes, looks at screenshots and takes attachments;
  - keeps chats and talks to any LLM;
  - loads the app's capabilities and runs its agents and skills.

  There is **one runtime per stack**, and all of them do the same things (`spec/behaviour.md`).
- **The capability folder** (`ai/` in the app, app-owned): *what* the app's AI can do. It holds `index.json`, tools
  and toolsets, Agent Skills (`skills/<name>/SKILL.md`), agents (`agents/<name>.md`), permissions, the tool defaults
  (`ai-tools.json`) and the starting memories (`ai-memory.json`). **The same format on every stack** (`spec/framework.md`).
- **The lifecycle**: install → upgrade → validate → add → reconcile, safe to repeat.
  - Framework files are replaced only when unchanged.
  - The app's own files are only ever added to or edited in place.
  - A manifest (`ai-enablement.json`) records what is where, and which stack.

Everything lives in this skill folder (the folder that contains this `SKILL.md`). **Do not rewrite a runtime.**

## What is in this skill

| Path | What it is |
| --- | --- |
| `spec/` | **The portable contract**, the same for every stack. `framework.md` (concepts, formats, effects, permissions, ownership, the manifest), `context-sync.md` (the screen-sync protocol), `behaviour.md` (everything a runtime does, with its limits), `tool-catalog.md` (tools of compiled apps, as data). |
| `stacks/<id>/STACK.md` | **One recipe per stack**: how each install step is done there. `web/` (HTML, PHP, React, Vue, Svelte, Angular, Next.js, Tauri, ASP.NET Core / Blazor), `rust-egui/` (planned), `dotnet-winforms/` (planned). Read only the one detect names. |
| `assets/ai-agent/` | The **web runtime** (1.6): `ai-agent.js` (`createAiAgent`…), `ai-agent.css`, `ai-agent.d.ts`, `core/` (DOM-free logic), `adapters/`, `ui/`, `skills/create-tool/`. |
| `assets/relay/` | Web relays: `relay.php` (PHP 8.1+) and `relay.mjs` (Node), configured by a file. |
| `assets/templates/` | Starting points: `ai/index.json`, a tool module, a toolset, a `SKILL.md`, an agent file, the manifest. |
| `examples/hello-world/` | **Reference integration (web)**: a text editor with `ai-agent-setup.js` and `ai/` (three toolsets over the editor, two skills, two agents, permissions). Read it before integrating. |
| `scripts/detect.mjs` | **Run first**: the stack, and what is in the app (runtime, relay, integration, capability folders, manifest, dev-time folders, features). Says what to do. Read-only. |
| `scripts/validate.mjs` | Loads the capability folder the way the runtime does (and the tool catalog on compiled stacks); errors and warnings. Run after every change. |
| `scripts/scaffold.mjs` | Creates a tool, toolset, skill or agent from the templates and registers it. Never overwrites. |
| `scripts/verify.mjs`, `scripts/guards.mjs`, `scripts/workspace.mjs` | Web: headless browser check of the running app; the runtime CSS's layout guards; the dev server for in-app authoring. |
| `references/` | Web references: `capabilities.md` (**add a tool / toolset / skill / agent**), `upgrading.md` (**apps that already have it**, the manifest), `tools.md`, `memory-and-vision.md`, `frameworks.md`, `providers.md`, `api.md`, `settings-layout.md`, `in-app-authoring.md`, `checklist.md`, `architecture.md`. |
| `CHANGELOG.md` | What each version changed, with **Upgrading** notes. |

## Step 0. Detect, then route (every time)

```bash
node <skill>/scripts/detect.mjs <app-root>
```

**First, the `Stack` line.** It names the stack, where it came from, and whether the skill supports it. Then:

| Stack line | Do |
| --- | --- |
| **supported** | Read `stacks/<id>/STACK.md` (that one only; for web, its Tauri or ASP.NET section when the host is one) and use it for every step below. |
| **PLANNED** or **UNSUPPORTED** | **Stop before installing.** Never copy the web runtime into a native app, and never hand-write a drawer. Tell the user what detect said, and offer what works on any stack: the survey and plan (steps 1–2), and the `ai/` folder's skills, agents, toolsets and permissions. The recipe's "What to do today" has the details. |
| not recognised, or `!` warnings (two native UIs, or a manifest that disagrees) | Ask the user which app and which stack, then record it in the manifest's `"stack"`. |

**Then, the status:**

| detect says | Do |
| --- | --- |
| **No AI agent found** | **Install**: steps 1–15 below. |
| **UPGRADE** (older runtime or relay) | `references/upgrading.md`: replace framework files that are unchanged, reconcile edited ones, migrate, add only what is missing, write the manifest. Never build a second agent. |
| **CURRENT** | **Validate** (`scripts/validate.mjs <app-root>`), then do what the user asked. |
| The user asks for a capability ("add a tool that…", "a skill for…", "an agent that…") | After the routing above: `references/capabilities.md`. |

- The user's words set the scope ("just update the runtime", "add a tool", "add a Track Designer agent"). Without one,
  propose the full upgrade or install.
- Web: **layout guards** are checked on every run on an app that has the agent. A MISSING guard is fixed in this run,
  whatever the task (`stacks/web/STACK.md` §8).
- `.claude/`, `.github/agents`, `.agents/skills` and `.codex/` hold the *coding agent's* skills and agents, not the
  app's. Leave them alone; the app's live in its capability folder.
- Without Node, look for these by hand:
  - web: `ai-agent.js` containing `export const VERSION`, and `createAiAgent(`;
  - Rust: a `Cargo.toml` with `eframe`/`egui`;
  - C#: a `.csproj` with `<UseWindowsForms>`;
  - any stack: an `index.json` with `"format": "ai-enablement/1"`, and `ai-enablement.json`.

## Install

Each step's rule below holds on every stack. **How** to do it on this stack is in `STACK.md`, under the section with
the same number.

### 1. Survey the app (read-only)

Find out, and keep notes (stack-specific places to look: `STACK.md` §1):

- **Stack and entry points**: where the root UI is built.
- **Every page/view/screen and what it shows**: which state renders it, what the user edits or selects, how often it
  changes.
- **What the user can do on each page**: the action surface for tools.
  - Classify each action as read / write / destructive / external / system.
  - Note **one object** that reaches them (the store, a service layer, the app struct): it becomes `host`.
- **Policy constraints**: what the app (its spec, its domain, its law) allows an AI to see and do. Look for:
  - data that must not reach a model, or only with consent;
  - allowed model providers (local only?);
  - where data may persist (no plaintext files? an encrypted store?);
  - actions reserved for a person (approve, send, export, delete);
  - whether model tool calling is allowed at all;
  - what must happen on lock or sign-out.

  **When the plan would break one, stop and ask. Never override an app's own rules.**
- **Kinds of work** users repeat in several steps (candidates for skills) and **distinct roles** (candidates for
  agents). Usually there are none at first.
- **What the agent could not know from the screen**: shortcuts, hidden features, conventions. These become starting
  memories.
- **Existing AI code**: model clients, tool layers, chat panels. Do not build a second panel; reuse what fits, and ask.
- **Layout, keyboard handling, modal dialogs, theme, how the view is drawn, tests.**

### 2. Write the plan

Draft and show the user in a few lines (proceed unless they object):

- **App context**: name, purpose (from the README/UI), capabilities, limits. Keep it lean, generated from the app's own
  data.
- **Per page**: `id`, `title`, `purpose`, **content** (what represents the screen), **view** (selection, cursor,
  filters), the change signal.
- **System prompt**: persona and answer rules for this domain (start from the runtime's default).
- **Tool plan**:
  - Per page, the tools: name, what it wraps (via `host`), effect, parameters with real ranges.
  - Group them into **toolsets**.
  - Say what is left out on purpose (payments, security settings, messages to others, bulk deletes, everything the
    policy constraints reserve).
  - Ask which tools start **on**.
- **Skills and agents** (optional): a skill for each multi-step kind of work worth writing down. Add agents only for
  distinct roles (a "Proofreader" that may not rewrite). One implicit agent is the default.
- **Permissions**: anything the app must always confirm (`ask`) or never let the agent do (`deny`).
- **Memory, vision and attachments plans**, **code/reply actions**, **transport** (direct, relay, local only).

### 3. Install the runtime

Copy this stack's runtime into the app **unchanged** (`STACK.md` §3). It is framework-owned: configure it through
options and hooks, never edit it. If a change is truly needed, make it generic and tell the user, so it can be folded
back into the skill.

### 4. Create the capability folder

`node <skill>/scripts/scaffold.mjs <dir>/ai init` writes `index.json`, `ai-tools.json` and `ai-memory.json`. Where the
folder goes, and how the runtime reads it (served, bundled, embedded), is in `STACK.md` §4.

### 5. Mount the agent once

One app-owned integration module creates the agent once, with:

- `appId`: unique per app and never changed;
- the app context, the pages, the system prompt;
- the capability folder, `host`, and the hooks the policy constraints need.

Load it so a failure cannot break the app. See `STACK.md` §5; for web, also `examples/hello-world/ai-agent-setup.js`.

### 6. Wire every page

Set the page on navigation (`STACK.md` §6). **content** is what the user sees, compact and deterministic, from a
**pure, unit-tested builder** (state → text). The builder:

- reads the app's own state, never widgets or the DOM;
- uses the precision and labels the UI shows;
- keeps a stable order;
- leaves out clocks and random ids (volatile bits go in **view**);
- includes what open dialogs show;
- never includes secrets.

Protocol: `spec/context-sync.md`.

### 7. Signal changes

Call the change signal wherever content can change (`STACK.md` §7). It is debounced with a max wait, so continuous
updates need no throttle.

### 8. Fit the host app

Typing in the agent never reaches the app's shortcuts. The drawer makes room for itself, uses the app's theme, and
stays out of the app's dialogs. How: `STACK.md` §8.

### 9. Build the tools

Tools and toolsets follow `spec/framework.md`; recipes in `references/capabilities.md` and `references/tools.md`; the
module or code shape in `STACK.md` §9.

```bash
node <skill>/scripts/scaffold.mjs <ai-dir> tool filter_orders --effect write --description "Show only orders with this status."
```

- Each tool calls a function the app **already has**, through `host`. Never reimplement business logic, never bypass
  the app's own checks, and never add a tool the UI does not offer the user.
- Give each tool:
  - an accurate `effect` (read / write / destructive / external / system; when unsure, the stronger);
  - a short `description`;
  - parameters with real ranges and `required`, and an explicit `maxLength` on long text;
  - `pages` / `when` for where it applies;
  - a short factual result.
- Group related tools as toolsets. Record which start on in `ai-tools.json`. Add `permissions` for what must always ask
  or never run.
- Unit-test each toolset with `host` mocked. On compiled stacks, also add the **tool catalog test**
  (`spec/tool-catalog.md`).

### 10. Skills and agents (when the plan has them)

```bash
node <skill>/scripts/scaffold.mjs <ai-dir> skill track-design --description "Design a track: … Use when …"
node <skill>/scripts/scaffold.mjs <ai-dir> agent track-designer --description "…" --toolsets track --skills track-design
```

A skill is numbered steps for one kind of work (which tools, in which order, what to check), in the Agent Skills
format, with long material in its `references/`. An agent composes existing tools, toolsets and skills with
instructions, permissions, context layers and memory policy. It never defines tools. Formats: `spec/framework.md`.
These files are the same on every stack.

### 11. Memory, vision and attachments

All three come with the runtime; the plan and the policy constraints decide what is on.

- Write `ai-memory.json`: short notes no screen shows, never secrets.
- Decide how screenshots are taken, or switch them off.
- Size file limits for small-context models.

How: `STACK.md` §11; rules: `spec/behaviour.md` §7–8.

### 12. App-specific actions (optional)

Buttons on the agent's code blocks and replies, applied through the app's real controls. Anything that changes data
uses the app's own confirmation UI. See `STACK.md` §12.

### 13. Transport

Direct to the model, through a relay (web, when deployed or when keys must stay on a server), or restricted to local
models where the policy says so. See `STACK.md` §13.

### 14. Verify

```bash
node <skill>/scripts/validate.mjs <app-root>          # the capability folder (and the tool catalog), the manifest, the stack
```

Then the stack's own checks (`STACK.md` §14): unit tests, the drawer flow against a mock or real model, and, by hand at
1920×1080, every Settings tab and the sync sequence:

1. "Read the page" → *synced*;
2. change the screen → *dirty*;
3. "Page unchanged";
4. a tool call with its confirmation;
5. the agent picker and a skill (`/name`), if any;
6. memory, a screenshot, an attached file, as planned.

Full list: `references/checklist.md`.

### 15. Manifest and report

Write **`ai-enablement.json`** at the app root (`references/upgrading.md`, "The manifest"). It records:

- `"stack"` and versions, `appId`;
- the framework files (runtime, relay), the capability folder and what it holds, the integration files;
- features, declined items, plans in brief.

Never secrets. Tell the user:

- the files added, the context and tool plans (which tools are on), skills and agents;
- memories, screenshots, attachments;
- how to open the agent and how to configure the model (LM Studio needs its server started, ≥ 8k context, and on the
  web CORS);
- the transport;
- anything you could not verify.

## Add or change a capability

On an app that has the framework (detect: CURRENT, capability folder present; or adopt one first,
`references/upgrading.md`, "Adopting the framework"). `references/capabilities.md` has the recipes:

- **Tool** ("add a tool that lets the AI inspect the current track"):
  1. find the app function;
  2. choose the effect and parameters;
  3. `scaffold.mjs … tool`, then write it over `host` (the stack's shape: `STACK.md` §9);
  4. add it to `ai-tools.json`;
  5. unit-test it; on compiled stacks, refresh the catalog (`AI_CATALOG_UPDATE=1`);
  6. validate.
- **Toolset**, **skill**, **agent**: scaffold, write, reference by name, validate. Compose; never duplicate.
- **Context**, **memory**, **permissions**, **prompt** changes: edit the integration or the index.

Preserve what the app has: never overwrite an app-owned file wholesale; edit it in place.

## In-app authoring (the app's AI adds tools)

Web: `node <skill>/scripts/workspace.mjs <app-root>` serves the app in development with a workspace endpoint. With
`workspace: true` in the integration, the in-app agent gets:

- `describe_host`, `list_source_files`, `read_source_file`, `search_source`;
- `write_ai_file` (always confirmed, with a diff);
- `reload_capabilities`;
- the built-in `create-tool` skill.

The developer commits what it wrote. Setup and safety: `references/in-app-authoring.md`. Compiled stacks cannot load
new code at run time; there, in-app authoring is planned for skills, agents, toolsets and permissions only.

## Rules

- **Stack**: follow the recipe detect names. Never put one stack's runtime into another stack's app.
- **Ownership**: the runtime and relay files are framework-owned. They are copied unchanged, replaced only when
  detect says they are unchanged, and reconciled when edited. Everything else is app-owned and is added to or edited in
  place, never replaced wholesale. App-owned includes: the integration, the content builders, the capability folder,
  the relay config, styles.
- **One agent per app**:
  - one instance, loaded so a failure cannot break the app;
  - `appId` never changes;
  - an app that already has the agent is upgraded, never rebuilt.
- Context is what the user sees, never secrets; deterministic content from a tested pure builder.
- **Tools**:
  - they wrap the app's own functions with the user's own permissions;
  - changes are confirmed by default;
  - destructive, external and system tools start off;
  - the app's own confirmations, checks and policy constraints stay.
- Skills and agents compose what exists. Formats follow Agent Skills and the agent-file convention, with no
  proprietary extras where the format already has a field.
- **Memory** holds notes, never secrets or instructions. **Screenshots and attachments** go to the model provider only
  when the user sends them (switched off where they must not).
- The development workspace is development-only; it never ships with the app.
- Do not modify unrelated parts of the app. Do not invent capabilities in the app context.
