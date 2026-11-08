## AI Enablement — an application's AI layer, as a reusable skill

<p align="center">
  <img src="skill-overview.png" alt="AI Enablement skill overview" width="1000">
</p>

This repository packages an application AI framework so that Claude Code, Codex and GitHub Copilot can install it in
an app, upgrade it safely, and extend it on request:

> **Application X has an AI agent that slides out from the right, sees what is on the current screen (plus what the
> app and the page are for), acts in the app through tools that wrap its own functions (you confirm changes and
> choose which tools are on), follows skills written for the app's kinds of work, comes as one or several agents you
> pick from, remembers what you ask it to, looks at screenshots, takes images and files you attach, answers in rich
> Markdown, and works with any LLM — LM Studio by default. While you develop the app, that agent can even add new
> tools to it.**

| | |
| --- | --- |
| **The skill** — [`ai-enablement/`](ai-enablement/) | Instructions ([`SKILL.md`](ai-enablement/SKILL.md), the portable contract in [`spec/`](ai-enablement/spec/), one recipe per stack in [`stacks/`](ai-enablement/stacks/), [`references/`](ai-enablement/references/)), the runtime and relays copied into apps, templates, lifecycle scripts (detect, validate, scaffold, verify, workspace), conformance vectors and tests. |
| **The Hello World app** — [`ai-enablement/examples/hello-world/`](ai-enablement/examples/hello-world/) | A small text editor with the agent built in, on the framework: three toolsets, two skills, two agents. The reference integration. |
| **The installer** — [`install-skill.ps1`](install-skill.ps1) / [`install-skill.sh`](install-skill.sh) | Copies the skill to the user-wide skill folders so it is available in every project. |
| **Design notes** — [`docs/ai-enablement-design.md`](docs/ai-enablement-design.md) | How `add-ai-skill` became `ai-enablement`: what was kept, what was decided, what comes next. |

## Which apps

The skill works out the app's **stack** first (`scripts/detect.mjs`), because the stack decides which runtime draws
the agent:

| Stack | Apps | Status |
| --- | --- | --- |
| `web` | HTML, PHP, React, Vue, Svelte, Angular, Next.js; **Tauri** (a webview over Rust); **ASP.NET Core / Blazor** (HTML from C#) | supported: the JS runtime |
| `rust-egui` | native Rust desktop apps with egui / eframe | planned: the Rust core is in (3.1, `assets/rust/`); the egui drawer is next |
| `dotnet-winforms` | native C# desktop apps with Windows Forms | planned: a .NET runtime (core + WinForms panel) |

What the AI can do lives in the app's `ai/` folder, in the same format on every stack: skills, agents, toolsets,
permissions, tool defaults and memories. Every runtime behaves the same way. [`spec/behaviour.md`](ai-enablement/spec/behaviour.md)
says how, and [`spec/conformance/`](ai-enablement/spec/conformance/) pins it as test vectors that each runtime must
reproduce. On a planned stack the skill stops before installing, and offers the survey, the plan and the `ai/` folder.

## What the framework is

```
Application
 └─ integration (ai-agent-setup.js)          the app's: mounts the agent, says what is on screen, passes `host`
     └─ runtime (ai-agent/)                  the framework's: drawer, context sync, agent loop, providers, registries
         ├─ capability folder (ai/)          the app's: what its AI can do
         │    index.json · tools/*.js · toolsets/*.json · skills/*/SKILL.md · agents/*.md
         │    permissions · ai-tools.json (which tools start on) · ai-memory.json (starting notes)
         └─ LM Studio · Ollama · OpenAI-compatible · Anthropic · Gemini · the app's relay
```

| Concept | What it is | Standard it follows |
| --- | --- | --- |
| **Tool** | One capability: name, description, input contract, effect, handler calling the app through `host` | JSON Schema inputs and MCP annotations; `agent.tools.mcp()` exports MCP descriptors |
| **Toolset** | A named group of tools (a module, or a JSON list) | — |
| **Skill** | Written procedure for a kind of work; the agent loads it when a request matches, or on `/name` | [Agent Skills](https://agentskills.io) (`SKILL.md`) |
| **Agent** | A worker composed of instructions + tools/toolsets + skills + permissions + context + memory policy | Markdown + YAML frontmatter, as Claude Code and Copilot agents |
| **Permissions** | allow / ask / deny rules over tools, toolsets and effects (read, write, destructive, external, system) | — |
| **Context** | App, page, live screen content (re-sent only when its hash changes, with a flag), view state | — |

Application agents and skills live in the app's `ai/` folder. The coding agent's own (`.claude/skills`,
`.github/agents`…) are a different thing and are left alone.

## Install the skill for Claude Code, Codex and Copilot

```powershell
.\install-skill.ps1            # or: ./install-skill.sh on macOS/Linux/Git Bash
```

This copies `ai-enablement/` to:

| Folder | Read by |
| --- | --- |
| `~/.claude/skills/ai-enablement` | Claude Code |
| `~/.agents/skills/ai-enablement` | Codex, GitHub Copilot |

**This repository is the source of truth.** Edit the skill here, then re-run the installer to update the installed
copies. Other installed skills are never touched; copies under the skill's earlier names (`add-ai-skill`,
`ai-agent-drawer`) are reported and kept — `-RemoveLegacy` (`--remove-legacy`) removes them. `-Uninstall` removes
ai-enablement; `-Targets claude` installs for one tool only.

## Use it in an app

Open the app and ask your agent:

- **Claude Code** — "Use the ai-enablement skill to build an AI agent into this app", or `/ai-enablement`
- **Codex** — `$ai-enablement build an AI agent into this app`
- **GitHub Copilot** (agent mode) — `/ai-enablement build an AI agent into this app`

Every run starts with `scripts/detect.mjs` and goes where the app is:

| The app has | The skill |
| --- | --- |
| no agent | **installs**: surveys the app, proposes the context, tool, skill and agent plans, copies the runtime, creates the capability folder, mounts one agent, wires every page, builds the tools over the app's own functions, verifies, writes `ai-enablement.json` |
| an older agent (including add-ai-skill 1.x) | **upgrades**: replaces unchanged framework files, reconciles edited ones, migrates, adds only what is missing, keeps everything app-owned |
| the current agent | **validates** (`scripts/validate.mjs`) and makes the change you ask for |

And it takes requests like these, composing what the app already has:

- "Add a tool that lets the AI inspect the current track."
- "Add a skill for proofreading release notes."
- "Add a Track Designer agent using the track-design skill and the track-editor tools."
- "The proofreader must never replace the whole document." (a `deny` rule)

```powershell
node ai-enablement/scripts/detect.mjs D:\path\to\app       # what an app has, without changing anything
node ai-enablement/scripts/validate.mjs D:\path\to\app     # check its capability folder
```

## The app's AI adds its own tools

While you develop an app, its own agent can give itself a tool: you ask in the app ("I need a tool that tells me
where the cursor is"), it reads the app's source, writes a tool module into the capability folder, registers it,
reloads and uses it. Every file is shown to you — whole, or as a diff when it replaces one — and waits for your
confirmation; you commit the result like any other change.

```powershell
node ai-enablement/scripts/workspace.mjs ai-enablement/examples/hello-world --static ai-enablement
# open http://127.0.0.1:8790/examples/hello-world/ and type: /create-tool add a tool that tells me where the cursor is
```

The workspace server is development-only (loopback, the app's own pages, reads that skip secrets, writes only in the
capability folder) and never ships with the app. Details: [`references/in-app-authoring.md`](ai-enablement/references/in-app-authoring.md).

## Run the Hello World app

1. Start **LM Studio**: Developer tab → *Start server* (the template expects `http://127.0.0.1:9000`; change it in
   Settings if yours uses 1234), turn on **Enable CORS**, and load a model (with at least 8k context).
2. From this repository's root:

   ```powershell
   node ai-enablement/assets/relay/relay.mjs --static .
   ```

3. Open <http://127.0.0.1:8787/>. (Any static server works too; this one also provides the optional relay at
   `/ai-relay`. Use `scripts/workspace.mjs` as above to try in-app authoring.)

Then try it: click **Ask AI** (or `Ctrl+I`) and ask "Proofread this" — your message shows **Read the page · Editor ·
N chars · hash** and the status bar flag turns green; a model that follows its skills loads *proofreading* (a
**Use skill** row) and lists the fixes. Edit the text: the flag turns amber; ask again and the page is re-read; once more without editing:
**Page unchanged**. Ask "fix the spelling mistakes": it calls `replace_text` and asks you to confirm first. Pick
**Proofreader** at the top of the panel: a new chat with an agent that only has the document tools and may not insert
text. Type `/summarize` to start the summarize skill yourself. **Settings → Tools** has a checkbox per tool, grouped by
toolset, with permission badges; **Settings → Agent** lists the agent and its skills; the flag opens **Settings →
Context**, exactly what the AI receives.

Memory, vision, attachments: ask "which shortcut saves the document?" (it knows from `ai/ai-memory.json`), then
"remember that I prefer British spelling". With a model that sees images, the camera button attaches a screenshot.
The **+** button attaches images and files (PDF, Word, Excel, text…).

## How the screen context works (short version)

| Layer | Comes from | Sent |
| --- | --- | --- |
| **App context**: what the app is, what it can and cannot do | `app` option | in the system prompt, every time |
| **Page context**: which page, and what it is for | `setPage({ id, title, purpose })` | in the system prompt, every time |
| **Screen content**: the document, the table rows, the record | `page.content` hook | **only when its hash changed** |
| **View state**: cursor, selection, filters | `page.view` hook | with every question, never hashed |

Before each question, the runtime hashes the screen content and compares it with the newest snapshot the model
already has in the conversation. If they match, it sends only the question. If not, it attaches a fresh snapshot.
That result drives the flag: green = in sync, amber = changed, blue = not read yet. Details:
[`spec/context-sync.md`](ai-enablement/spec/context-sync.md).

## What's inside the skill

```
ai-enablement/
  SKILL.md                 what the coding agent follows: detect (the stack first), then install / upgrade / validate / add
  spec/                    the portable contract: framework · context-sync · behaviour · tool-catalog · conformance/ (vectors)
  stacks/                  one recipe per stack: web/STACK.md (incl. Tauri, ASP.NET Core) · rust-egui/ · dotnet-winforms/
  CHANGELOG.md             what changed per version, with upgrade notes for apps
  agents/openai.yaml       Codex display metadata
  assets/ai-agent/         the runtime (copied into apps unchanged): ai-agent.js · ai-agent.css · ai-agent.d.ts
    core/                  session (the agent without its UI: the tool loop, built-ins, saved chats), context, sync,
                           hashing, prompt, settings, providers, client, transport, relay probe, tools, memory,
                           attachments (files · bytes · pdf · office), and the framework layer: capabilities ·
                           agents · skills · permissions · schema (JSON Schema, MCP) · frontmatter · workspace
    adapters/              openai-chat (LM Studio/Ollama/OpenAI/DeepSeek/OpenRouter/custom), anthropic, gemini, relay
    ui/                    drawer (a view over the session: rows, cards, agent picker), settings modal, markdown,
                           resize, dialogs, layout check, screenshot capture
    skills/create-tool/    the built-in skill for in-app authoring
  assets/rust/             the Rust runtime: ai-enablement-core (the agent without a UI, passing every conformance
                           vector); the egui drawer comes next
  assets/relay/            relay.php (PHP 8.1+) · relay.mjs (Node) · relay.config.example.php — copied unchanged
  assets/templates/        index.json, a tool module, a toolset, a SKILL.md, an agent file, the manifest
  examples/hello-world/    the demo: app.js (the editor) · ai-agent-setup.js (the integration) · content.js (pure, tested)
                           · ai/ (index, toolsets document/editor/file, skills, agents, tool defaults, memories)
                           · ai-enablement.json (the manifest)
  references/              capabilities · in-app-authoring · upgrading · tools · api · memory-and-vision · frameworks ·
                           providers · settings-layout · checklist · architecture
  scripts/detect.mjs       run first on any app: the stack, what is there, which version, what to do
  scripts/conformance-export.mjs   writes spec/conformance/ from the JS runtime (on purpose only)
  scripts/validate.mjs     the capability folder, loaded as the runtime loads it: errors and warnings
  scripts/scaffold.mjs     a tool, toolset, skill or agent from the templates, registered (never overwrites)
  scripts/verify.mjs       checks a running integration in headless Edge/Chrome (scripts/lib/cdp.mjs is its driver)
  scripts/workspace.mjs    development server for in-app authoring (static site + relay + workspace endpoint)
  tests/                   node --test: conformance · runtime · framework · attached files · relays (Node + PHP) ·
                           real browser · framework in the browser · workspace · lifecycle · example · detect (+ stacks)
  package.json             npm test · npm run demo · npm run workspace · npm run verify · npm run validate
```

## Changing the skill

```powershell
cd ai-enablement
node --test                                    # all suites; no model needed
node assets/relay/relay.mjs --static .         # Hello World at http://127.0.0.1:8787/examples/hello-world/
node scripts/verify.mjs http://127.0.0.1:8787/examples/hello-world/ --no-llm
node scripts/validate.mjs examples/hello-world
```

- The relay tests also run the PHP relay when `php` (with curl) is on the PATH, or `PHP_BIN` points at it
  (e.g. `$env:PHP_BIN = 'D:\xampp\php\php.exe'`).
- The browser tests need Node 22+ and Edge/Chrome/Chromium (`AIA_BROWSER` to choose one; `AIA_SKIP_BROWSER=1` to skip).
- With LM Studio running, `verify.mjs` without `--no-llm` also runs the read → change → re-read → "Page unchanged" loop.

When an integration needed a workaround, fold the lesson back in here so the next one does not:

1. Reproduce it in a test first; check browser/server behaviour for real before encoding it.
2. Keep changes additive and generic (no app names, domains or server paths); anything that could surprise an
   existing app is opt-in or has a safe default. No dependencies, no build step. Follow the standards the framework
   follows (Agent Skills, JSON Schema/MCP, the agent-file convention) rather than inventing formats.
3. Update `ai-agent.d.ts`, `references/api.md` and other affected references, `SKILL.md` if the workflow changes,
   Hello World if it demonstrates the feature, and `CHANGELOG.md` (with **Upgrading** notes, and any new
   workaround the upgrade should remove in `references/upgrading.md`); bump the version (`VERSION` in `ai-agent.js`
   and the relay versions when their code changed, `package.json` always).
4. When the runtime or a relay changed: `node scripts/release-hashes.mjs` (the tests fail until you do).
5. Run the tests, try Hello World, commit, and re-run the installer.

Apps that already have a copy of `ai-agent/` take an update by copying the folder over theirs (see the upgrade notes
in `CHANGELOG.md`); integrations only use the options and methods, so the copy can be replaced wholesale. The app's
capability folder is never replaced.

## License

[MIT](LICENSE) © 2026 Nate Cobb
