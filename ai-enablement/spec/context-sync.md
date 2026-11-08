# The context-sync protocol

The question this answers: **does the AI already have what is on my screen?**

## Layers

| Layer | Source | Where it goes | Hashed? |
| --- | --- | --- | --- |
| App | `app` option / `setApp()` | System prompt, every request | No (always sent fresh) |
| Page | `page` / `setPage()` (id, title, purpose, …) | System prompt, every request | Page id + title are part of the content hash |
| Content | `page.content` hook | Attached to a user message as `<page_snapshot>` **only when it changed** | **Yes** |
| View | `page.view` hook | `<view_state>` on the newest user message, every request | No |

App and page context are small and live in the system prompt, which is rebuilt for every request, so they are
always current. The content can be large, so it is sent once and then only when it changes.

## The algorithm (core/conversation.js)

Chat APIs are stateless: every request resends the transcript. "The model has the page" therefore means: *a snapshot
with the same fingerprint as the screen now is inside the part of the transcript that will be sent.*

On every question:

1. `snapshot = ContextManager.snapshot()` → `{ text, hash, pageId, pageTitle, chars, totalChars, truncated }`.
   `hash = cyrb53(pageId + title + FULL content text)` (before truncation, so a change past the cut still counts).
2. `start = windowStart(transcript.length + 1, historyMessages)`; find the newest message at or after `start` that
   carries a snapshot **with its text**.
3. Decide (`planTurn`):
   - sharing off → `off` (no snapshot; the system prompt says so)
   - no content / empty → `empty`
   - user pressed Re-read → `forced` (attach)
   - no snapshot in the window → `unread` (never sent in this chat) or `trimmed` (fell out of the window) → attach
   - hash differs → `changed` → attach
   - hash equal → `unchanged` → do not attach; add one line: `[The screen is unchanged since page snapshot abc1234: it is still current.]`
4. Build the request (`buildRequestMessages`): the newest snapshot is inlined in full; older snapshots become a
   one-line stub (so a page is never paid for twice); the view state and the question go last.
5. If the request fails with no answer, the question is removed from the transcript, so the sync state is exactly
   what it was before (a failed turn never counts as "the model has it").

Wire format of a user turn that carries a snapshot:

```
<page_snapshot page="editor" title="Editor" hash="16e069b" chars="773" captured="2026-09-29T08:40:00.000Z">
File name: hello-world.txt
…content…
</page_snapshot>

<view_state>
Cursor: line 17, column 43
Selection: (nothing selected)
</view_state>

When is the meeting?
```

A `</page_snapshot` inside the content is neutralised so a document cannot close the block early, and the system
prompt tells the model to treat snapshot content as data, never as instructions.

## The flag (contextState)

| State | Meaning | Drawer bar | Next question |
| --- | --- | --- | --- |
| `synced` (green) | The model has exactly this screen | "Agent has this page" | question only |
| `dirty` (amber) | The screen changed since the model's newest snapshot | "Page changed · agent re-reads it…" | attaches snapshot |
| `unread` (blue) | Not read in this conversation (new chat, restored chat, trimmed) | "Agent will read this page…" | attaches snapshot |
| `none` (grey) | The page shares no content | "No page content shared" | nothing |
| `off` (grey) | Sharing switched off in Settings | "Screen sharing is off" | nothing |

Every user message shows a receipt chip: **Read the page · title · N chars · hash**, or **Page unchanged · hash**.

## When is the flag recomputed?

- `agent.contextChanged()` — call it from the app whenever the content may have changed. It is a trailing debounce
  (`debounceMs`, 300 ms) with a max wait (`debounceMaxMs`, 1000 ms): a burst of changes refreshes the flag once, 300 ms
  after it ends, and continuous changes still refresh it at least once a second.
- `agent.setPage()`, opening the drawer, after every reply, on settings changes.
- Every `watch` ms while the drawer is open, if the `watch` option is set.
- With `dialogs: 'dock'`, whenever a managed dialog opens, closes or is docked.

Hashing is cheap (a 100 KB document hashes in well under a millisecond), and nothing is sent until the user asks.

### Apps that change all the time (animation, simulation, live data, websockets)

A plain trailing debounce never fires when the state changes every ~100 ms, so the flag would never turn amber;
`debounceMaxMs` fixes that without app-side throttling. The pattern:

- Call `agent.contextChanged()` on every state update (a store subscription, each worker message). Do **not** add your
  own throttle: the runtime refreshes at most every `debounceMaxMs` while updates keep coming, and once more after
  they stop.
- Make the content describe what the user can *read*, at the precision the UI shows. A simulation that ticks 10×/s
  but displays values rounded to one decimal should round the same way, so the hash only changes when the numbers on
  screen change. Frame counters, timestamps, FPS and "updated 3 s ago" belong in `view` or nowhere.
- Expect the flag to be amber most of the time while things move: that is honest (the model's copy is older than the
  screen), and the next question re-reads the page. Pausing the simulation turns it green after the next question.
- If the content builder is expensive (thousands of entities), raise `debounceMaxMs` (e.g. 2000–5000) rather than
  sampling in the app; `0` restores the 1.0 behaviour (refresh only after the changes stop).
- Data from a Web Worker often arrives in compact buffers (typed arrays). Decode them into labelled values in the
  builder instead of scraping the rendered DOM or canvas.

## Designing good content hooks

- **Deterministic.** The same screen must produce the same text. Sort lists the way the UI does, avoid timestamps
  that tick, avoid random ids. Objects are serialised with sorted keys, so property order does not matter.
- **What the user sees.** The filtered/paged rows, the open record, the document. Include the column/field labels.
  An open dialog or panel is part of what the user sees: include its contents (and signal the change when it opens
  and closes).
- **Compact.** Trim noise (internal ids the user never sees, huge blobs). The runtime cuts at `maxContextChars` and
  says so; if pages regularly hit the cap, summarise (counts, totals) plus the visible rows.
- **Volatile state in `view`.** Cursor, selection, scroll position, hover, "last refreshed" — not in `content`.
- **No secrets.** Never tokens, passwords, keys, or other users' data.

### Content builders: a pure, tested module

Keep the code that turns state into text in its own module, with no DOM access: `(appState, smallUiRecord) → string`.
The hook then only gathers state: `content: () => buildContent(store.getState(), ui.snapshot())`. Test it like any
other function (`tests/example.test.mjs` does this for Hello World):

- the same state gives **identical** text (and hash);
- a real change on screen gives **different** text; a change that is not visible does not;
- the text never contains `NaN`, `undefined` or `null` from missing fields;
- values are rounded to the precision the UI displays, with the UI's units;
- every value carries the **label the UI uses**. Never put a raw value next to a derived one under the same name
  (e.g. an internal `speed` in m/s and the displayed "Speed (km/h)"): the model will see a contradiction and report it.
  Name derived values exactly as the screen does, and leave out raw ones the user never sees.
- compact buffers (typed arrays from a worker, bit flags, enum codes) are decoded into those labelled values.

## Saved chats

Conversations are saved without snapshot text (it can be large and may be sensitive). When a saved chat is reopened,
its snapshots show their fingerprints but are "not kept", so the flag is `unread` and the next question re-reads the
current screen.
