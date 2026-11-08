// The conformance vectors (spec/conformance/*.json) are the behaviour every runtime must reproduce. This test holds
// the reference implementation, the JS runtime, to them: a change in behaviour fails here until the vectors are
// rewritten on purpose (node scripts/conformance-export.mjs --write, with a CHANGELOG entry). The Rust and .NET
// runtimes run the same files with their own runners.

import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { SUITES, runSuite } from '../scripts/lib/conformance.mjs';
import { VECTORS, VECTOR_FORMAT, buildVectors } from '../scripts/conformance-export.mjs';

const files = fs.readdirSync(VECTORS).filter((f) => f.endsWith('.json')).sort();

test('conformance: every suite has its vector file, and no file is left without a suite', () => {
  assert.deepEqual(files, SUITES.map((s) => `${s.suite}.json`).sort());
});

for (const file of files) {
  test(`conformance: ${file}`, async () => {
    const v = JSON.parse(fs.readFileSync(path.join(VECTORS, file), 'utf8'));
    assert.equal(v.format, VECTOR_FORMAT);
    const suite = SUITES.find((s) => s.suite === v.suite);
    assert.ok(suite, `no suite named ${v.suite}`);
    assert.ok(v.cases.length > 0);
    const got = await runSuite(suite, v.cases);
    for (let i = 0; i < v.cases.length; i++) {
      assert.deepEqual(got[i].output, v.cases[i].output, `${v.suite} / ${v.cases[i].name}: the JS runtime no longer produces the frozen output (intended? node scripts/conformance-export.mjs --write)`);
    }
  });
}

test('conformance: the files are exactly what the export writes (descriptions and cases in sync)', async () => {
  const built = await buildVectors();
  for (const [name, text] of Object.entries(built)) {
    assert.equal(fs.readFileSync(path.join(VECTORS, name), 'utf8').replace(/\r\n/g, '\n'), text, `${name} is stale: node scripts/conformance-export.mjs --write`);
  }
});
