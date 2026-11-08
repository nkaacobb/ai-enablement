# Conformance vectors

Each `<suite>.json` pins one piece of the shared behaviour (`spec/behaviour.md`) as input → output pairs, produced by
the reference implementation, the JS runtime (`assets/ai-agent/`).

```json
{
  "format": "ai-enablement/conformance/1",
  "suite": "permissions",
  "description": "what the suite pins, and the rules",
  "operation": "the operation, written against the JS API",
  "reference": "assets/ai-agent 1.6.1",
  "cases": [ { "name": "…", "input": { … }, "output": … } ]
}
```

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
- Inputs are plain JSON. Where the JS operation takes a function (a tool's handler, a clock), the description says what
  stands in for it.

## Changing behaviour on purpose

1. Change the JS runtime.
2. Run `node scripts/conformance-export.mjs --write`.
3. Review the diff of the vectors.
4. Add a CHANGELOG entry.
5. Update the other runtimes before their next release.

`tests/conformance.test.mjs` fails while the JS runtime and the files disagree. `scripts/conformance-export.mjs`
without `--write` lists the suites that differ.

## Not yet covered

The provider adapters' request bodies and stream parsing, the request-message builder, and the tool loop's decisions
(`executeTool`). They come with the Rust core (phase 2), which needs them first.
