# Adding capabilities: tools, toolsets, skills, agents

Recipes for the requests AI Enablement gets once an app has the framework — "add a tool that lets the AI inspect the
current track", "add a Track Designer agent using the track-design skill and the track-editor tools". Each one follows
the app's conventions, composes what exists instead of duplicating it, and ends with validate.

Before any of them: `node <skill>/scripts/detect.mjs <app-root>` (status CURRENT, a capability folder listed). No
capability folder yet → adopt one first (`upgrading.md`, "Adopting the framework"). Read `index.json`, one existing
tool module, and the integration module (what `host` is) so the new piece looks like the others.

The formats are in `spec/framework.md`. `scaffold.mjs` writes the starting file and registers it; you write the content.

## Create a tool

> "Add a tool that lets the AI inspect the current track."

1. **Find what the app already does.** Search the code for the feature: the button or menu that shows it, the store
   selector or service method, the API call. A tool wraps that function — it never re-implements it. If it only lives
   in a UI handler, drive the real control (`setControlValue`, `.click()`). If nothing in the app does it, say so: a
   tool cannot add a feature the app does not have.
2. **Reach it through `host`.** The integration passes one object as `createAiAgent({ host })`. If the function is not
   reachable from it, add the smallest accessor to the integration module (not to app logic), e.g.
   `host: { ...store, trackEditor }`.
3. **Decide the contract**: a verb-first `name` (`inspect_track`, `set_track_speed`), a one-sentence description in the
   user's words, the **effect** (read / write / destructive / external / system — when unsure, the stronger),
   parameters with the real ranges and options (from the controls or validation), `required`, a `maxLength` on long
   text, `pages` / `when` if it only works in some places, a small factual result.
4. **Scaffold and write it**:

   ```bash
   node <skill>/scripts/scaffold.mjs <ai-dir> tool inspect_track --effect read --description "The current track: name, length, sections."
   ```

   That creates `tools/inspect_track.js` and adds it to `index.json` and (off) to `ai-tools.json`. Fill in `run`:

   ```js
   run: ({ section }, { host }) => {
     const track = host.trackEditor.current();
     if (!track) return 'No track is open.';
     const s = section ? track.sections[section - 1] : null;
     return s ? { section, kind: s.kind, length: s.length } : { name: track.name, length: track.length, sections: track.sections.length };
   },
   ```

   If it belongs with existing tools, put it in their toolset module instead (and remove the scaffolded file and its
   index entry), or list it in a JSON toolset.
5. **On or off**: reading tools usually start on; write tools on if the user agrees; destructive / external / system
   start off. Set it in `ai-tools.json` (`"inspect_track": { "enabled": true }`).
6. **Permissions** if the app needs them: `"ask": ["set_track_speed"]` in the index, or a `deny` in an agent.
7. **Test**: a unit test with `host` mocked (the right function, the right arguments, clamping, the result), then
   `node <skill>/scripts/validate.mjs <app-root>`, then in the app ask for it (`verify.mjs` lists and runs reading tools).
8. **Mention it** where the user expects it: the app context's capabilities line if it adds something users should
   know the agent can do; a skill whose steps should use it.

## Create a toolset

Group tools that belong together (one screen, one object, one kind of work):

- **In a module** (tools defined there): `export default { name: 'track', title: 'Track', description: '…', tools: [ … ] }`.
- **As JSON** (tools defined elsewhere): `node <skill>/scripts/scaffold.mjs <ai-dir> toolset track --tools inspect_track,set_track_speed`.

Then agents can take `toolsets: [track]`, and rules can say `toolset:track`.

## Create a skill

> "Add a skill for designing tracks."

A skill is worth writing when the work takes several steps the model would otherwise improvise (which tools, in what
order, what rules apply, what to check). Not for a single tool call.

```bash
node <skill>/scripts/scaffold.mjs <ai-dir> skill track-design --description "Design or change a race track: layout rules, curve radii, safety run-off. Use when the user asks to design, lay out or fix a track."
```

- The **description** says what it does **and when to use it** — the model decides from it alone.
- The body: numbered steps naming the tools by their exact names; the rules (limits, conventions, what never to do);
  what to check before answering; what to report. Under ~300 lines; long material (style guides, tables, examples) in
  `references/*.md`, linked from the step that needs it.
- `allowed-tools: inspect_track set_track_speed` lists the tools it uses (the model is told whether each is on); it
  never skips a confirmation.
- Validate (it checks the name/folder match, the description, the links to `references/`).

## Create an agent

> "Add a Track Designer agent using the track-design skill and track-editor tools."

Only when users do a distinct kind of work that needs its own instructions, a narrower set of tools, or stricter
permissions. An agent **composes**: it names existing tools, toolsets and skills — create those first if they do not
exist, never inside the agent.

```bash
node <skill>/scripts/scaffold.mjs <ai-dir> agent track-designer --description "Designs and adjusts tracks with you." --toolsets track-editor --skills track-design
```

Then write its instructions (who it is, how it works, what it never does), and set what differs from the defaults:
`permissions` (`deny` what it must not do), `memory` (`read` for an agent that should not save notes), `context` (drop
`screen` for an agent that works without the page), `maxToolSteps`, `welcome`, `suggestions`, `default: true` for the
one to start with. The first agent file makes the drawer show a picker once there are two; the app's `title` stays the
fallback.

## Add or change context

The context hooks live in the integration module, not the capability folder (`spec/context-sync.md`): add what a page
shows to its `content` builder (and its unit test), volatile state to `view`, a new page with `setPage`. An agent that
should not see the screen gets `context: [app, page]`.

## Add memory

Starting notes go in `ai-memory.json` (`memory-and-vision.md`): one short sentence each, facts no screen shows, never
secrets. Per-user notes stay in the browser unless the app passes `memorySave`.

## Add a prompt

Suggestion chips: the agent's `suggestions` (or the app's). Reusable instructions that start a task: a skill
(`/name` in the composer). Rules for every agent: the integration's `systemPrompt`.

## After any change

1. `node <skill>/scripts/validate.mjs <app-root>` — no errors; read the warnings.
2. The app's own tests; `node <skill>/scripts/verify.mjs <url>` (the *capabilities* line lists agents, skills,
   toolsets and anything that did not load).
3. Update `ai-enablement.json` (the capabilities it lists).
4. Tell the user what was added, what is on by default, and how to use it (`/skill-name`, the agent picker).
