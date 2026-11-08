#!/usr/bin/env node
// The fake upstream (fake-upstream.mjs) for a runtime in another language — the Rust core's end-to-end tool-loop
// test. Starts it with a scripted plan, prints its URL as the first line of stdout, and when stdin closes prints every
// request it received (one JSON line) and exits.
//
//   node tests/fixtures/fake-upstream-cli.mjs '<plan JSON>'
//
// plan: { replies: [ { text: [pieces] } | { toolCalls: [{ id, name, arguments | rawArguments }], finish? } ],
//         models?: [ids], delayMs? }
// Reply n answers the n-th chat request; past the end of the list, the last one answers.

import { startFakeUpstream } from './fake-upstream.mjs';

const plan = JSON.parse(process.argv[2] || '{}');
const replies = Array.isArray(plan.replies) ? plan.replies : [];
const upstream = await startFakeUpstream({
  delayMs: plan.delayMs || 0,
  models: plan.models,
  respond: (body, n) => replies[Math.min(n, replies.length) - 1] || { text: ['Hello'] },
});
process.stdout.write(`${upstream.url}\n`);
process.stdin.resume();
process.stdin.on('end', async () => {
  process.stdout.write(`${JSON.stringify(upstream.requests)}\n`);
  await upstream.close();
  process.exit(0);
});
