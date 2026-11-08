# Conformance vectors

Each `<suite>.json` pins one piece of the shared behaviour (`spec/behaviour.md`) as input → output pairs, produced by
the reference implementation, the JS runtime (`assets/ai-agent/`).

```json
{
  "format": "ai-enablement/conformance/1",
  "suite": "permissions",
  "description": "what the suite pins, and the rules",
  "operation": "the operation, written against the JS API",
  "reference": "assets/ai-agent 1.7.0",
  "cases": [ { "name": "…", "input": { … }, "output": … } ]
}
```

## The suites

| Group | Suites |
| --- | --- |
| Context | `hash`, `stable-stringify`, `describe`, `context-snapshot`, `plan-turn`, `snapshot-wire` |
| Prompt | `system-prompt`, `capability-prompts` |
| Tools | `tool-contract`, `tool-arguments`, `tool-results`, `tool-set`, `text-tools`, `effects-annotations` |
| Permissions | `permissions` |
| Formats | `frontmatter`, `skills`, `agents`, `memory` |
| Requests | `request-messages` |
| Providers | `adapter-requests` (what each protocol sends), `adapter-streams` (SSE, whole JSON replies, HTTP errors, what each protocol reads back) |
| The session | `execute-tool` (one call through every check and card), `tool-loop` (one question with its tool rounds against a scripted model) |

`execute-tool` and `tool-loop` drive the session (`core/session.js`) through a scripted harness. Its tools return fixed
values, the model replies from a script, the user's clicks come from a list, and the clock is fixed. Their
descriptions say exactly how to build it. The `tool-loop` output records every request as the session sent it: the
system prompt, the messages, the tools and the tool exchange. That neutral request is the contract between a runtime's
session and its adapters, and `adapter-requests` pins what each adapter makes of it.

## For a runtime in another language

- Write one runner per suite. It reads the file, performs `operation` on each case's `input` with the runtime's own
  code, and compares the result with `output`, deep-equal on JSON values.
  - Object key order doesn't matter.
  - Array order does.
  - Strings are compared exactly, including every protocol text in `system-prompt.json`.
- Read the files from this folder (for a vendored copy, the skill's own path). Never copy them into the runtime's
  sources, where they would go stale.
- A failing case is a behaviour difference. Fix the runtime; the vectors are the contract. If the vector itself looks
  wrong, the reference implementation is wrong too: fix both on purpose (below).
- Inputs are plain JSON. Where the JS operation takes a function (a tool's handler, a clock, the model, the user), the
  description says what stands in for it.

## Changing behaviour on purpose

1. Change the JS runtime.
2. Run `node scripts/conformance-export.mjs --write`.
3. Review the diff of the vectors.
4. Add a CHANGELOG entry.
5. Update the other runtimes before their next release.

`tests/conformance.test.mjs` fails while the JS runtime and the files disagree. `scripts/conformance-export.mjs`
without `--write` lists the suites that differ. The suites are defined in `scripts/lib/conformance.mjs` (pure
functions) and `scripts/lib/conformance-runtime.mjs` (requests, adapters, the session).

## Not covered

- Transport timing: the idle timeout, caller cancellation mid-stream, and the request and response size caps.
- Network failures before any HTTP response. Their guidance is browser-specific (CORS, mixed content).
- Listing models and Test connection.
- Reading attached files (PDF, Office), images and screenshots.
- Loading the capability index.
- Settings sanitising beyond the values the session suites use.
- Markdown rendering and the views. The views are each stack's own.
