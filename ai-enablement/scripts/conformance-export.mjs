#!/usr/bin/env node
// Write the conformance vectors (spec/conformance/*.json) from the reference implementation, the JS runtime.
//
//   node <skill>/scripts/conformance-export.mjs            check: report which suites differ from the frozen files
//   node <skill>/scripts/conformance-export.mjs --write    (re)write them
//
// The files are the contract every runtime (JS, Rust, .NET) must reproduce. Rewrite them only on purpose: when a
// behaviour change is intended, in the same commit as the change and its CHANGELOG entry. tests/conformance.test.mjs
// fails while the JS runtime and the files disagree.

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { SUITES, runSuite } from './lib/conformance.mjs';

const SKILL = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
export const VECTORS = path.join(SKILL, 'spec', 'conformance');
export const VECTOR_FORMAT = 'ai-enablement/conformance/1';

export async function buildVectors() {
  const runtime = fs.readFileSync(path.join(SKILL, 'assets', 'ai-agent', 'ai-agent.js'), 'utf8').match(/export const VERSION = '([^']+)'/)?.[1] || '';
  const files = {};
  for (const s of SUITES) {
    files[`${s.suite}.json`] = `${JSON.stringify({ format: VECTOR_FORMAT, suite: s.suite, description: s.description, operation: s.operation, reference: `assets/ai-agent ${runtime}`, cases: await runSuite(s) }, null, 2)}\n`;
  }
  return files;
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  const write = process.argv.includes('--write');
  const files = await buildVectors();
  let differ = 0;
  fs.mkdirSync(VECTORS, { recursive: true });
  for (const [name, text] of Object.entries(files)) {
    const file = path.join(VECTORS, name);
    const old = fs.existsSync(file) ? fs.readFileSync(file, 'utf8').replace(/\r\n/g, '\n') : null;
    if (old === text) continue;
    differ++;
    if (write) { fs.writeFileSync(file, text); console.log(`${old === null ? 'created' : 'updated'}  spec/conformance/${name}`); } else console.log(`differs  spec/conformance/${name}`);
  }
  if (!differ) console.log(`${Object.keys(files).length} suites, all up to date.`);
  else if (!write) { console.log('Run with --write to update them (only for an intended behaviour change).'); process.exit(1); }
}
