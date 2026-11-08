#!/usr/bin/env node
// Validate an application's AI Enablement setup — the "current framework → validate" step of the lifecycle, and the
// check after adding a tool, a skill or an agent. Read-only.
//
//   node <skill>/scripts/validate.mjs [app-root | path/to/index.json] [--json]
//
// For each capability index (an index.json with "format": "ai-enablement/1"; development-time folders such as
// .claude/ are never taken for the app's) it loads everything the runtime would load — with the runtime's own
// modules — and reports:
//   errors    an entry that does not load; an invalid tool (name, description, input schema, effect); a tool name
//             defined twice; an agent, skill or toolset that refers to something that does not exist; a skill or
//             agent file that breaks its format; an unreadable tool config or memory file
//   warnings  a tool module that only imports in the browser (verify.mjs checks those in a real browser); files in
//             the capability folder the index does not name (the runtime will not load them); a file a skill links
//             to that is missing; tool config entries for tools that do not exist; the 1.x record instead of the
//             manifest; a manifest whose versions or paths no longer match; a runtime stylesheet without its layout
//             guards (scripts/guards.mjs)
// Exit code: 1 when there are errors, else 0.

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { detect, skillInfo, MANIFEST, LEGACY_RECORD } from './detect.mjs';
import { loadCapabilities, linkToolsets, checkReferences } from '../assets/ai-agent/core/capabilities.js';
import { normalizeTool } from '../assets/ai-agent/core/tools.js';
import { EFFECT_NAMES } from '../assets/ai-agent/core/schema.js';
import { parseMemoryFile } from '../assets/ai-agent/core/memory.js';

/** A stand-in `host` for tool-module factories: any property, any call, never fails (tools are not run here). */
const hostStandIn = () => {
  const proxy = new Proxy(function host() {}, { get: (t, k) => (k === Symbol.toPrimitive ? () => '' : proxy), apply: () => proxy });
  return proxy;
};

const fileText = async (url) => fs.readFileSync(fileURLToPath(url.split('?')[0]), 'utf8');

function listFiles(dir, depth = 3) {
  const out = [];
  const walk = (d, left) => {
    let entries;
    try { entries = fs.readdirSync(d, { withFileTypes: true }); } catch { return; }
    for (const e of entries) {
      if (e.name.startsWith('.') || e.name === 'node_modules') continue;
      const p = path.join(d, e.name);
      if (e.isDirectory() && left > 0) walk(p, left - 1);
      else if (e.isFile()) out.push(p);
    }
  };
  walk(dir, depth);
  return out;
}

export const CATALOG_FORMAT = 'ai-enablement/tools/1';
const noRun = () => undefined;

/**
 * The tool catalog an index names ("toolCatalog"), as inline index entries: each tool with a stand-in handler (the
 * real one is the app's code), each toolset as a toolset of names. null when the index names none.
 */
function readToolCatalog(index, dir, errors) {
  if (index.toolCatalog === undefined) return null;
  const name = String(index.toolCatalog);
  let cat;
  try { cat = JSON.parse(fs.readFileSync(path.resolve(dir, name), 'utf8')); } catch (e) {
    errors.push(`${name} (the tool catalog) could not be read (${e.code === 'ENOENT' ? 'missing: run the app\'s catalog test with AI_CATALOG_UPDATE=1' : e.message}).`);
    return [];
  }
  if (cat.format !== CATALOG_FORMAT) errors.push(`${name}: "format" must be "${CATALOG_FORMAT}" (spec/tool-catalog.md).`);
  const entries = [];
  for (const t of Array.isArray(cat.tools) ? cat.tools : []) {
    if (!t || typeof t !== 'object' || Array.isArray(t)) { errors.push(`${name}: a tool entry is not an object.`); continue; }
    entries.push({ ...t, run: noRun, source: name });
  }
  for (const s of Array.isArray(cat.toolsets) ? cat.toolsets : []) {
    if (s && typeof s === 'object' && Array.isArray(s.tools)) entries.push({ name: s.name, title: s.title, description: s.description, tools: s.tools.filter((x) => typeof x === 'string') });
    else errors.push(`${name}: a toolset entry needs a name and a list of tool names.`);
  }
  if (!Array.isArray(cat.tools)) errors.push(`${name}: "tools" must be a list.`);
  return entries;
}

/** A missing-tool reference: on a compiled stack without a tool catalog it cannot be checked, so it is a warning. */
const TOOL_REFERENCE = /: tool "[^"]+" does not exist\.$|allowed-tools names "[^"]+", which is not a tool/;

/**
 * Validate one capability index (absolute path).
 * @param {{ toolsFromCode?: boolean }} [opts] toolsFromCode: the app's tools are code (Rust, C#): without a tool catalog,
 *   references to tools are reported as unchecked warnings instead of errors.
 */
export async function validateIndex(indexFile, { toolsFromCode = false } = {}) {
  const errors = [];
  const warnings = [];
  const notes = [];
  const dir = path.dirname(indexFile);
  const relDir = (p) => path.relative(dir, p).split(path.sep).join('/');
  let index;
  try { index = JSON.parse(fs.readFileSync(indexFile, 'utf8')); } catch (e) { return { index: indexFile, errors: [`index.json is not valid JSON (${e.message}).`], warnings, notes, counts: {} }; }

  // A compiled app's tools are code; the generated tool catalog (spec/tool-catalog.md) stands in for them here.
  const catalog = readToolCatalog(index, dir, errors);
  const source = catalog
    ? (() => { const { toolCatalog, ...rest } = index; return { ...rest, tools: [...(Array.isArray(rest.tools) ? rest.tools : []), ...catalog] }; })()
    : pathToFileURL(indexFile).href;
  const loaded = await loadCapabilities(source, {
    base: pathToFileURL(indexFile).href,
    host: hostStandIn(),
    fetchText: fileText,
    importModule: (u) => import(u),
  });
  for (const p of loaded.problems) {
    if (/could not be imported/.test(p)) warnings.push(`${p} It may only import in the browser or through the app's bundler: scripts/verify.mjs loads it in a real browser.`);
    else if (/reserved for a later version|is not an index key/.test(p)) warnings.push(p);
    else errors.push(p);
  }

  // Tools: valid definitions, one name each.
  const tools = [];
  const seen = new Map();
  for (const def of loaded.tools) {
    try {
      const t = normalizeTool(def);
      if (seen.has(t.name)) errors.push(`tool "${t.name}" is defined twice (${seen.get(t.name) || 'inline'} and ${def.source || 'inline'}).`);
      seen.set(t.name, def.source || '');
      tools.push(t);
    } catch (e) {
      errors.push(`${def.source || 'tool'}: ${e.message}`);
    }
    if (!def.effect && !def.annotations) warnings.push(`tool "${def.name}" (${def.source || 'inline'}) states no effect: it is treated as "write" (asks before running). Say read / write / destructive / external / system.`);
    else if (def.effect && !EFFECT_NAMES.includes(def.effect)) errors.push(`tool "${def.name}" (${def.source || 'inline'}): effect "${def.effect}" is not one of ${EFFECT_NAMES.join(', ')} (the runtime treats it as "write").`);
  }
  const linked = linkToolsets(tools, loaded.toolsets);
  const references = [...linked.problems, ...checkReferences({ agents: loaded.agents, tools, toolsets: linked.toolsets, skills: loaded.skills, defaultAgent: loaded.defaultAgent })];
  for (const p of references) {
    if (toolsFromCode && !catalog && TOOL_REFERENCE.test(p)) warnings.push(`${p.replace(/\.$/, '')} (not checked: the tools are code and there is no tool catalog yet, spec/tool-catalog.md).`);
    else errors.push(p);
  }

  // The tool config and the memory file it names.
  if (typeof index.toolsConfig === 'string') {
    const f = path.resolve(dir, index.toolsConfig);
    try {
      const cfg = JSON.parse(fs.readFileSync(f, 'utf8'));
      const names = new Set(tools.map((t) => t.name));
      for (const n of Object.keys(cfg.tools || {})) if (!names.has(n)) warnings.push(`${index.toolsConfig}: "${n}" is not a tool of this application (left over from a removed or renamed tool?).`);
      const off = tools.filter((t) => !(t.name in (cfg.tools || {})) && !t.enabled).map((t) => t.name);
      if (off.length) notes.push(`not in ${index.toolsConfig}, so they start off: ${off.join(', ')}.`);
    } catch (e) {
      errors.push(`${index.toolsConfig} could not be read (${e.code === 'ENOENT' ? 'missing' : e.message}).`);
    }
  }
  if (typeof index.memory === 'string') {
    const f = path.resolve(dir, index.memory);
    try {
      const json = JSON.parse(fs.readFileSync(f, 'utf8'));
      const notesList = parseMemoryFile(json);
      if (Array.isArray(json.memories) && notesList.length < json.memories.length) warnings.push(`${index.memory}: ${json.memories.length - notesList.length} entr${json.memories.length - notesList.length === 1 ? 'y is' : 'ies are'} not usable notes.`);
      if (notesList.some((m) => /password|api[ _-]?key|secret|token/i.test(m.text))) warnings.push(`${index.memory}: a note mentions a password, key or token. Memories go to the model provider: never put secrets in them.`);
    } catch (e) {
      errors.push(`${index.memory} could not be read (${e.code === 'ENOENT' ? 'missing' : e.message}).`);
    }
  }

  // Files the index does not name: the runtime will not load them.
  const named = new Set();
  for (const p of [...(index.tools || []), ...(index.toolsets || []), ...(index.agents || [])]) if (typeof p === 'string') named.add(path.resolve(dir, p));
  for (const p of index.skills || []) if (typeof p === 'string') named.add(path.resolve(dir, /\.md$/i.test(p) ? p : `${p.replace(/\/+$/, '')}/SKILL.md`));
  for (const f of listFiles(dir)) {
    const r = relDir(f);
    const isCapability = /^tools\/[^/]+\.(js|mjs)$/.test(r) || /^toolsets\/[^/]+\.json$/.test(r) || /^agents\/[^/]+\.md$/.test(r) || /^agents\/[^/]+\/AGENT\.md$/.test(r) || /^skills\/[^/]+\/SKILL\.md$/.test(r);
    if (isCapability && !named.has(f)) warnings.push(`${r} is in the capability folder but not in index.json: the runtime does not load it.`);
  }

  // Files a skill links to.
  for (const s of loaded.skills) {
    const folder = fileURLToPath(s.base);
    for (const m of s.body.replace(/<!--[\s\S]*?-->/g, '').matchAll(/\]\(([^)#\s]+)(?:#[^)]*)?\)/g)) {
      const target = m[1];
      if (/^[a-z][a-z0-9+.-]*:/i.test(target) || target.startsWith('/')) continue;
      if (!fs.existsSync(path.resolve(folder, target))) warnings.push(`skill "${s.name}" links to ${target}, which does not exist in its folder.`);
    }
  }

  return {
    index: indexFile,
    errors: [...new Set(errors)],
    warnings: [...new Set(warnings)],
    notes,
    counts: { agents: loaded.agents.length, skills: loaded.skills.length, tools: tools.length, toolsets: linked.toolsets.size },
    agents: loaded.agents.map((a) => a.name),
    skills: loaded.skills.map((s) => s.name),
    tools: tools.map((t) => `${t.name} (${t.effect})`),
  };
}

/** Validate an app: every capability index in it, and the manifest. */
export async function validate(target) {
  const abs = path.resolve(target || '.');
  const isIndex = fs.existsSync(abs) && fs.statSync(abs).isFile();
  const root = isIndex ? path.dirname(abs) : abs;
  const d = isIndex ? null : detect(root);
  const indexes = isIndex ? [abs] : d.capabilities.map((c) => path.join(root, ...c.file.split('/')));
  const results = [];
  const toolsFromCode = !!d && (d.stack?.family === 'rust' || d.stack?.family === 'dotnet');
  for (const f of indexes) results.push(await validateIndex(f, { toolsFromCode }));

  const app = { errors: [], warnings: [], notes: [] };
  if (d) {
    if (!indexes.length) app.notes.push('No capability index (index.json with "format": "ai-enablement/1") in this app yet: scaffold.mjs <folder> init creates one.');
    for (const w of d.stack?.warnings || []) app.warnings.push(`Stack: ${w}.`);
    // Compiled stacks: tools are code, so references to them can only be checked against the generated catalog.
    if (d.stack?.family === 'rust' || d.stack?.family === 'dotnet') {
      for (const f of indexes) {
        let idx = {};
        try { idx = JSON.parse(fs.readFileSync(f, 'utf8')); } catch { continue; }
        if (idx.toolCatalog === undefined) app.warnings.push(`${path.relative(root, f).split(path.sep).join('/')}: on ${d.stack.id} the tools are code; name the generated tool catalog ("toolCatalog": "tools.catalog.json", spec/tool-catalog.md) so references to tools can be checked.`);
      }
    }
    const manifestFile = d.manifest ? path.join(root, ...d.manifest.split('/')) : null;
    if (!d.manifest && d.legacyRecord) app.warnings.push(`${LEGACY_RECORD} is the 1.x record: write ${MANIFEST} in its place (references/upgrading.md, "The manifest").`);
    else if (!d.manifest && d.status !== 'none') app.warnings.push(`No ${MANIFEST}: write it at the end of this run (references/upgrading.md, "The manifest").`);
    if (manifestFile) {
      let m = null;
      try { m = JSON.parse(fs.readFileSync(manifestFile, 'utf8')); } catch (e) { app.errors.push(`${d.manifest} is not valid JSON (${e.message}).`); }
      if (m) {
        const installed = d.runtimes.map((r) => r.version).sort((a, b) => a.localeCompare(b, 'en', { numeric: true }))[0];
        if (m.skill && m.skill !== 'ai-enablement') app.warnings.push(`${d.manifest}: "skill" is "${m.skill}"; it should be "ai-enablement".`);
        if (!m.stack && d.stack?.id) app.notes.push(`${d.manifest} does not record the stack yet: add "stack": { "id": "${d.stack.id}"${d.stack.host ? `, "host": "${d.stack.host}"` : ''} } (references/upgrading.md, "The manifest").`);
        else if (m.stack && !(typeof m.stack === 'string' ? m.stack : m.stack.id)) app.warnings.push(`${d.manifest}: "stack" needs an "id" (web, rust-egui, dotnet-winforms…).`);
        if (installed && m.runtimeVersion && m.runtimeVersion !== installed) app.warnings.push(`${d.manifest} says runtime ${m.runtimeVersion}, but the app has ${installed}: update the manifest.`);
        const idx = m.capabilities?.index;
        if (idx && !fs.existsSync(path.join(root, ...String(idx).split('/')))) app.warnings.push(`${d.manifest} names the capability index ${idx}, which does not exist.`);
        if (m.framework?.runtime && !fs.existsSync(path.join(root, ...String(m.framework.runtime).split('/')))) app.warnings.push(`${d.manifest} names the runtime folder ${m.framework.runtime}, which does not exist.`);
        if (/sk-[A-Za-z0-9]{12,}|AIza[0-9A-Za-z_-]{20,}|"(api[_-]?key|token|password)"\s*:\s*"[^"]+"/i.test(fs.readFileSync(manifestFile, 'utf8'))) app.errors.push(`${d.manifest} looks like it holds a key or password: the manifest is committed; remove it.`);
      }
    }
    for (const rt of d.runtimes) if (rt.known && (rt.modified.length || rt.missing.length)) app.warnings.push(`The runtime copy ${rt.dir} differs from release ${rt.version} (${[...rt.modified, ...rt.missing].join(', ')}): framework files are copied unchanged; reconcile the edit (references/upgrading.md, U1).`);
    for (const g of d.layoutGuards || []) {
      if (g.missing.length) app.warnings.push(`${g.css} lacks the layout guard${g.missing.length > 1 ? 's' : ''} ${g.missing.join(' and ')} (the settings dialog's tabs clip, host CSS leaks in): ${g.fix === 'replace' ? 'replace the runtime (references/upgrading.md, U4)' : 'node <skill>/scripts/guards.mjs <app-root> --apply'} in this run (references/upgrading.md, "Layout guards").`);
    }
  }
  const errors = results.reduce((n, r) => n + r.errors.length, 0) + app.errors.length;
  return { root, skill: skillInfo().skillVersion, indexes: results, app, ok: errors === 0 };
}

function report(v) {
  const out = [];
  const say = (s = '') => out.push(s);
  say(`ai-enablement ${v.skill} · validate: ${v.root}`);
  for (const r of v.indexes) {
    say('');
    const rel = path.relative(v.root, r.index).split(path.sep).join('/') || 'index.json';
    say(`${r.errors.length ? '✗' : '✓'} ${rel} — ${r.counts.agents ?? 0} agent(s), ${r.counts.skills ?? 0} skill(s), ${r.counts.tools ?? 0} tool(s), ${r.counts.toolsets ?? 0} toolset(s)`);
    if (r.agents?.length) say(`  agents   ${r.agents.join(', ')}`);
    if (r.skills?.length) say(`  skills   ${r.skills.join(', ')}`);
    if (r.tools?.length) say(`  tools    ${r.tools.join(', ')}`);
    for (const e of r.errors) say(`  ERROR    ${e}`);
    for (const w of r.warnings) say(`  warning  ${w}`);
    for (const n of r.notes) say(`  note     ${n}`);
  }
  if (v.app.errors.length || v.app.warnings.length || v.app.notes.length) say('');
  for (const e of v.app.errors) say(`ERROR    ${e}`);
  for (const w of v.app.warnings) say(`warning  ${w}`);
  for (const n of v.app.notes) say(`note     ${n}`);
  say('');
  say(v.ok ? '=> OK' : '=> ERRORS: fix them, then run validate again.');
  return out.join('\n');
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  const argv = process.argv.slice(2);
  const target = argv.find((a) => !a.startsWith('--')) || '.';
  validate(target).then((v) => {
    console.log(argv.includes('--json') ? JSON.stringify(v, null, 2) : report(v));
    process.exit(v.ok ? 0 : 1);
  }).catch((e) => { console.error(e?.stack || e); process.exit(1); });
}
