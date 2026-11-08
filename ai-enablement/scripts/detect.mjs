#!/usr/bin/env node
// What of AI Enablement is already in this app, and at which version? Run it first, every time the skill is used on
// an app — it is the lifecycle's router (install / upgrade / validate / add):
//
//   node <skill>/scripts/detect.mjs [app-root] [--json]
//
// It finds, without changing anything:
//   - the app's tech stack (web, including Tauri and ASP.NET Core; rust-egui; dotnet-winforms; others recognised) from
//     its project files, and whether the skill supports it, plans it, or has no recipe (scripts/lib/stacks.mjs): a
//     native app never gets the web runtime;
//   - copies of the runtime (ai-agent.js with `export const VERSION`), their version, and whether they are unchanged
//     since that release (fingerprints in scripts/release-hashes.json) — unchanged copies can be replaced wholesale;
//   - relays (relay.php / relay.mjs), their version, edits (1.0 constants, keys or checks added inside the file) and
//     config files (names only: never their contents, which may hold keys);
//   - the integration: files that call createAiAgent() (the appId, the options used), the manifest (ai-enablement.json,
//     or the 1.x record ai-agent.integration.json), tool config files (ai-tools.json), memory files (ai-memory.json);
//   - capability folders (an index.json with "format": "ai-enablement/1") and what they name — agents, skills, tool
//     modules, toolsets — with the paths that are missing (scripts/validate.mjs checks them in depth);
//   - development-time skill and agent folders (.claude/, .github/, .agents/, .codex/): reported apart, never taken
//     for the application's own;
//   - which features the installed runtime has and which of them the integration uses (tools, memory, vision,
//     attachments, capabilities, agents, skills, workspace), so an upgrade adds only what is missing;
//   - app code that looks like a workaround a newer runtime covers, and things to check for the new features (a
//     canvas/WebGL view that wants a screenshot hook, a Permissions-Policy or CSP header) — hints, not certainties;
//   - the layout guards of each runtime copy's ai-agent.css (runtime 1.6.1+; scripts/guards.mjs applies them):
//     the settings dialog's chrome pinned (pinnedChrome) and the host page's element CSS kept out (hostIsolation) —
//     missing guards are fixed in every run of the skill — and, for information, the host's own global element rules
//     (label, input, select, textarea, button, p, h1-h6, body text-align) in its CSS files and inline <style> blocks.
// Then it says what to do: install (nothing yet), upgrade (older version), or validate and add (current).
// Exit code: 0 always (it only reports).

import fs from 'node:fs';
import path from 'node:path';
import crypto from 'node:crypto';
import { fileURLToPath } from 'node:url';
import { layoutGuards, hostElementRules, inlineStyles, layoutWorkarounds } from './lib/css-guards.mjs';
import { detectStack } from './lib/stacks.mjs';

const SKILL = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const SKIP_DIRS = new Set(['node_modules', '.git', '.hg', '.svn', 'dist', 'build', 'out', '.next', '.nuxt', '.svelte-kit', '.output', 'coverage', '.verify', '.cache', '.turbo', '.vercel', 'bower_components', '.vs', '.idea']);
// PHP's Composer `vendor/` is skipped; a front-end `js/vendor/` (where a copied runtime often lives) is not.
const isComposerVendor = (dir, name) => name === 'vendor' && fs.existsSync(path.join(dir, 'autoload.php'));
// Build output of compiled stacks, skipped only beside the project file that makes it: Cargo's `target/` next to a
// Cargo.toml, .NET's `bin/` and `obj/` next to a .csproj/.fsproj/.vbproj (a web app's own `bin/` folder is kept).
const isBuildOutput = (parent, name) => {
  if (name === 'target') return fs.existsSync(path.join(parent, 'Cargo.toml'));
  if (name === 'bin' || name === 'obj') { try { return fs.readdirSync(parent).some((f) => /\.(cs|fs|vb)proj$/i.test(f)); } catch { return false; } }
  return false;
};
const CODE_EXT = new Set(['.js', '.mjs', '.cjs', '.ts', '.tsx', '.jsx', '.vue', '.svelte', '.php', '.html', '.htm', '.astro']);
const MAX_FILES = 20000;
const MAX_BYTES = 2 * 1024 * 1024;
const MAX_HOST_RULES = 60;
const STYLE_EXT = new Set(['.html', '.htm', '.php', '.vue', '.astro', '.svelte']);
const isTestFile = (rel) => /(^|\/)(tests?|__tests__|spec|e2e)\/|\.(test|spec)\.[a-z]+$/i.test(rel);

export const normalizedHash = (text) => crypto.createHash('sha256').update(String(text).replace(/^﻿/, '').replace(/\r\n?/g, '\n'), 'utf8').digest('hex');
const cmpVersion = (a, b) => String(a).localeCompare(String(b), 'en', { numeric: true });

/** Features an upgrade can add, the runtime version each arrived in, and the createAiAgent options that show use. */
export const FEATURES = [
  { id: 'tools', since: '1.2.0', what: 'the agent acts in the app through tools', options: ['tools', 'toolsConfig'], doc: 'references/tools.md' },
  { id: 'memory', since: '1.3.0', what: 'notes kept between conversations (Settings > Memory)', options: ['memoryFile', 'memorySave'], doc: 'references/memory-and-vision.md' },
  { id: 'vision', since: '1.3.0', what: 'screenshots for models that see images (Settings > Vision)', options: ['screenshot', 'screenshotMaxEdge'], doc: 'references/memory-and-vision.md' },
  { id: 'attachments', since: '1.5.0', what: 'the + button: images and files (PDF, Word, Excel, text…) attached to a question', options: ['attachments', 'readFile'], doc: 'references/memory-and-vision.md' },
  { id: 'capabilities', since: '1.6.0', what: 'a capability folder (ai/index.json): tools, toolsets, skills, agents, permissions', options: ['capabilities', 'host', 'permissions', 'toolsets'], doc: 'spec/framework.md' },
  { id: 'agents', since: '1.6.0', what: 'agents the user picks from (instructions + skills + tools + permissions)', options: ['agents', 'agent'], doc: 'spec/framework.md' },
  { id: 'skills', since: '1.6.0', what: 'Agent Skills the in-app agent loads when a request matches (SKILL.md)', options: ['skills'], doc: 'spec/framework.md' },
  { id: 'workspace', since: '1.6.0', what: 'development: the in-app agent adds tools itself (scripts/workspace.mjs)', options: ['workspace'], doc: 'references/in-app-authoring.md' },
];

/** The manifest an integration leaves (2.0+), and the record 1.x integrations left (read, then renamed on upgrade). */
export const MANIFEST = 'ai-enablement.json';
export const LEGACY_RECORD = 'ai-agent.integration.json';
export const INDEX_FORMAT = 'ai-enablement/1';
/** Folders that hold development-time capabilities (the coding agent's), not the application's. */
const DEV_TIME = ['.claude/skills', '.claude/agents', '.github/agents', '.github/skills', '.github/prompts', '.agents/skills', '.codex/skills', '.cursor/rules'];

export function skillInfo() {
  const js = fs.readFileSync(path.join(SKILL, 'assets', 'ai-agent', 'ai-agent.js'), 'utf8');
  let pkg = {};
  try { pkg = JSON.parse(fs.readFileSync(path.join(SKILL, 'package.json'), 'utf8')); } catch { /* optional */ }
  let hashes = { releases: {} };
  try { hashes = JSON.parse(fs.readFileSync(path.join(SKILL, 'scripts', 'release-hashes.json'), 'utf8')); } catch { /* optional */ }
  const relayVersions = {};
  try { relayVersions['relay.php'] = fs.readFileSync(path.join(SKILL, 'assets', 'relay', 'relay.php'), 'utf8').match(/AIA_RELAY_VERSION = '([^']+)'/)?.[1] || ''; } catch { /* optional */ }
  try { relayVersions['relay.mjs'] = fs.readFileSync(path.join(SKILL, 'assets', 'relay', 'relay.mjs'), 'utf8').match(/RELAY_VERSION = '([^']+)'/)?.[1] || ''; } catch { /* optional */ }
  return { runtimeVersion: js.match(/export const VERSION = '([^']+)'/)?.[1] || '', skillVersion: pkg.version || '', releases: hashes.releases || {}, relayVersions };
}

/** A capability index: what it names, and which of those paths are missing (a quick look; validate.mjs goes deeper). */
function capabilityInfo(file, rel) {
  let data;
  try { data = JSON.parse(read(file)); } catch { return { file: rel, error: 'not valid JSON' }; }
  const dir = path.dirname(file);
  const list = (k) => (Array.isArray(data[k]) ? data[k].filter((x) => typeof x === 'string') : []);
  const missing = [];
  const exists = (p) => fs.existsSync(path.resolve(dir, ...p.split('/')));
  for (const p of list('agents')) if (!exists(p)) missing.push(p);
  for (const p of list('skills')) if (!exists(/\.md$/i.test(p) ? p : `${p.replace(/\/+$/, '')}/SKILL.md`)) missing.push(p);
  for (const p of list('tools')) if (!exists(p)) missing.push(p);
  for (const p of list('toolsets')) if (!exists(p)) missing.push(p);
  for (const k of ['toolsConfig', 'memory']) if (typeof data[k] === 'string' && !exists(data[k])) missing.push(data[k]);
  return {
    file: rel,
    dir: path.dirname(rel) === '.' ? '.' : path.dirname(rel),
    agents: list('agents').length, skills: list('skills').length, tools: list('tools').length, toolsets: list('toolsets').length,
    toolsConfig: typeof data.toolsConfig === 'string' ? data.toolsConfig : '', memory: typeof data.memory === 'string' ? data.memory : '',
    permissions: !!data.permissions, defaultAgent: data.defaultAgent || '', missing,
  };
}

function walk(root) {
  const files = [];
  const stack = [root];
  while (stack.length && files.length < MAX_FILES) {
    const dir = stack.pop();
    let entries;
    try { entries = fs.readdirSync(dir, { withFileTypes: true }); } catch { continue; }
    for (const e of entries) {
      const p = path.join(dir, e.name);
      if (e.isDirectory()) { if (!SKIP_DIRS.has(e.name) && !e.isSymbolicLink() && !isComposerVendor(p, e.name) && !isBuildOutput(dir, e.name)) stack.push(p); } else if (e.isFile()) files.push(p);
    }
  }
  return files;
}

const read = (f) => { try { const st = fs.statSync(f); return st.size > MAX_BYTES ? '' : fs.readFileSync(f, 'utf8'); } catch { return ''; } };

function listRel(dir) {
  const out = [];
  for (const f of walk(dir)) out.push(path.relative(dir, f).split(path.sep).join('/'));
  return out.sort();
}

/** Compare a runtime copy with the release it says it is. */
function runtimeIntegrity(dir, version, releases) {
  const expected = releases[version]?.runtime;
  if (!expected) return { known: false, modified: [], missing: [], extra: [] };
  const files = listRel(dir);
  const modified = [];
  const missing = [];
  for (const [f, h] of Object.entries(expected)) {
    const p = path.join(dir, ...f.split('/'));
    if (!fs.existsSync(p)) missing.push(f);
    else if (normalizedHash(read(p)) !== h) modified.push(f);
  }
  const extra = files.filter((f) => !expected[f]);
  return { known: true, modified, missing, extra };
}

function relayInfo(file, text, releases) {
  const kind = path.basename(file);
  const version = (kind === 'relay.php' ? text.match(/AIA_RELAY_VERSION = '([^']+)'/) : text.match(/RELAY_VERSION = '([^']+)'/))?.[1] || '1.0.0';
  const h = normalizedHash(text);
  const matches = Object.entries(releases).filter(([, e]) => e.relay?.[kind] === h).map(([v]) => v);
  const edits = [];
  if (kind === 'relay.php') {
    if (/^const AIA_ALLOW_REMOTE = true/m.test(text)) edits.push('AIA_ALLOW_REMOTE = true (1.0 constant) -> config "allowRemote" (plus an "authorize" check)');
    if (/^const AIA_ALLOW_ANY_UPSTREAM = true/m.test(text)) edits.push('AIA_ALLOW_ANY_UPSTREAM = true (1.0 constant) -> config "allowAnyUpstream"');
    const keysBlock = text.match(/const AIA_KEYS = \[([\s\S]*?)\];/)?.[1] || '';
    if (keysBlock.split('\n').some((l) => /=>/.test(l) && !/^\s*\/\//.test(l))) edits.push('server keys written in the file (AIA_KEYS) -> config "keys" or environment variables (values not shown)');
    if (/^const AIA_TIMEOUT = (?!300;)/m.test(text)) edits.push('AIA_TIMEOUT changed -> config "timeout"');
    const afterAuth = text.split(/enforce it here, e\.g\.:[^\n]*\n[^\n]*\n/)[1] || '';
    if (afterAuth && /^\s*(require|include|if|\$|[a-z_]+\()/m.test(afterAuth.split('$method')[0] || '')) edits.push('code added at the "enforce it here" spot (sign-in/CSRF) -> config "authorize"');
  }
  return { file, kind, version, unchanged: matches.length > 0, matches, edits };
}

/** Keys at the top level of the object literal passed to createAiAgent({ … }) (strings and comments skipped). */
export function createAgentKeys(text) {
  const start = text.search(/createAiAgent\s*\(\s*\{/);
  if (start < 0) return [];
  let i = text.indexOf('{', start);
  let depth = 0;
  let token = '';
  const keys = [];
  for (; i < text.length; i++) {
    const c = text[i];
    const n = text[i + 1];
    if (c === '/' && n === '/') { i = text.indexOf('\n', i); if (i < 0) break; continue; }
    if (c === '/' && n === '*') { i = text.indexOf('*/', i + 2) + 1; if (i <= 0) break; continue; }
    if (c === '"' || c === "'" || c === '`') {
      for (i++; i < text.length && text[i] !== c; i++) if (text[i] === '\\') i++;
      token = '';
      continue;
    }
    if (c === '{' || c === '[' || c === '(') { depth++; token = ''; continue; }
    if (c === '}' || c === ']' || c === ')') { depth--; token = ''; if (depth === 0) break; continue; }
    if (depth === 1 && c === ':' && /^[A-Za-z_$][\w$]*$/.test(token.trim())) { keys.push(token.trim()); token = ''; continue; }
    if (c === ',' || c === '\n') { token = ''; continue; }
    token += c;
  }
  return keys;
}

function integrationInfo(file, text) {
  const appId = text.match(/appId\s*:\s*['"`]([^'"`]+)['"`]/)?.[1] || '';
  return { file, appId, options: createAgentKeys(text).filter((k) => k !== 'appId') };
}

function workaroundHints(file, text) {
  const hints = [];
  if (/\.showModal\s*=|prototype\.showModal|showModal\.call\(/.test(text) && !/aia-docked-dialog/.test(text)) hints.push('overrides dialog.showModal: the runtime\'s `dialogs: \'dock\'` option (1.1) may replace this code');
  if (/contextChanged/.test(text) && /(throttle|lastSignal|lastChange|setInterval\([^)]*contextChanged)/i.test(text)) hints.push('throttles contextChanged(): the runtime keeps the flag moving itself since 1.1 (debounceMaxMs)');
  if (/(fetch|probe)[\s\S]{0,300}available[\s\S]{0,300}createAiAgent|relayUrl[\s\S]{0,200}available/.test(text)) hints.push('probes the relay before createAiAgent: the `relayProbe` option (1.1) does this');
  if (/keydown[\s\S]{0,400}(aia-|ai-agent)/.test(text) && !/aia-scope/.test(text)) hints.push('filters key events for the drawer: typing is isolated by the runtime since 1.1 (isolateKeys)');
  if (/\[data-aia-theme|:not\(\[data-aia-theme/.test(text)) hints.push('theme overrides with high-specificity selectors: since 1.1 a plain `.aia-scope { --aia-… }` rule wins in light and dark');
  if (/createAiAgent|agent\.ask\(/.test(text) && /FileReader|readAsText|readAsDataURL|type=["'`]file["'`]|pdfjs|getDocument\(|mammoth|\bXLSX\.read/.test(text)) {
    hints.push('reads files for the agent itself (file input, FileReader, pdf.js, mammoth, SheetJS): since 1.5 the + button attaches images and files to a question; keep a reader only for formats the runtime does not read, as the `readFile` hook');
  }
  return hints.map((h) => ({ file, hint: h }));
}

/** Things in the app that matter for memory and vision (1.3): where a screenshot hook or a policy change may be needed. */
function featureHints(file, text) {
  const hints = [];
  if (/getContext\(\s*['"`](webgl2?|experimental-webgl|webgpu|bitmaprenderer)['"`]|WebGLRenderer|new\s+(BABYLON\.)?Engine\(|PIXI\.Application|regl\(|\bnew\s+Deck\(/.test(text)) {
    hints.push('draws with WebGL / a GPU canvas: the browser screen capture shows it as part of the whole page; if this canvas IS the view (or a share prompt is unwanted), add a `screenshot` hook that renders a frame and returns the canvas');
  } else if (/getContext\(\s*['"`]2d['"`]/.test(text)) {
    hints.push('draws on a 2D canvas: if this canvas is the main view, a `screenshot` hook returning it gives the agent the picture without a share prompt');
  }
  if (/Permissions-Policy|Feature-Policy/i.test(text) && /display-capture/i.test(text)) hints.push('sets display-capture in a Permissions-Policy: the browser\'s screen capture needs display-capture=(self)');
  else if (/Permissions-Policy/i.test(text)) hints.push('sets a Permissions-Policy: check it does not switch off display-capture (the browser\'s screen capture)');
  if (/Content-Security-Policy/i.test(text) && /img-src/i.test(text) && !/img-src[^;"']*data:/i.test(text)) hints.push('Content-Security-Policy img-src without data: — screenshot thumbnails in the chat are data: images');
  return hints.map((h) => ({ file, hint: h }));
}

export function detect(appRoot, skill = skillInfo()) {
  const root = path.resolve(appRoot);
  const files = walk(root);
  const rel = (f) => path.relative(root, f).split(path.sep).join('/') || '.';
  const runtimes = [];
  const relays = [];
  const relayConfigs = [];
  const integrations = [];
  const records = [];
  const toolConfigs = [];
  const memoryFiles = [];
  const hints = [];
  const checks = [];
  const runtimeDirs = [];
  const capabilities = [];
  const devTime = DEV_TIME.map((d) => {
    const dir = path.join(root, ...d.split('/'));
    let n = 0;
    try { n = fs.readdirSync(dir, { withFileTypes: true }).filter((e) => !e.name.startsWith('.')).length; } catch { return null; }
    return { dir: d, entries: n };
  }).filter(Boolean);
  // Anything inside a dot-folder (.claude, .github, .agents…) belongs to the development tools, not to the app.
  const inDotDir = (f) => rel(f).split('/').slice(0, -1).some((p) => p.startsWith('.'));

  for (const f of files) {
    const base = path.basename(f);
    if (base === 'ai-agent.js') {
      const t = read(f);
      const version = t.match(/export const VERSION = '([^']+)'/)?.[1];
      if (version && /export function createAiAgent/.test(t)) {
        const dir = path.dirname(f);
        runtimeDirs.push(dir);
        runtimes.push({ dir: rel(dir), version, ...runtimeIntegrity(dir, version, skill.releases) });
      }
    }
  }
  const inRuntime = (f) => runtimeDirs.some((d) => f.startsWith(d + path.sep));
  const hostRules = [];

  for (const f of files) {
    if (inRuntime(f)) continue;
    const base = path.basename(f);
    if (base === 'relay.php' || base === 'relay.mjs') {
      const t = read(f);
      if (t.includes('ai-agent-drawer')) relays.push({ ...relayInfo(f, t, skill.releases), file: rel(f) });
      continue;
    }
    if (/^relay\.config\.(php|json|mjs)$/.test(base)) { relayConfigs.push(rel(f)); continue; }
    if (base === MANIFEST || base === LEGACY_RECORD) {
      let data = null;
      try { data = JSON.parse(read(f)); } catch { data = { error: 'not valid JSON' }; }
      records.push({ file: rel(f), data, legacy: base === LEGACY_RECORD });
      continue;
    }
    // A capability index carries "format": "ai-enablement/1" (templates and scaffold.mjs write it; tools find it by it).
    if (base === 'index.json' && !inDotDir(f)) {
      if (read(f).includes(INDEX_FORMAT)) capabilities.push(capabilityInfo(f, rel(f)));
      continue;
    }
    if (base === 'ai-tools.json') { toolConfigs.push(rel(f)); continue; }
    if (base === 'ai-memory.json') { memoryFiles.push(rel(f)); continue; }
    if (base === '.htaccess' || /\.conf$/i.test(base)) { const t = read(f); if (t) checks.push(...featureHints(rel(f), t)); continue; }
    if (!CODE_EXT.has(path.extname(f).toLowerCase())) continue;
    const t = read(f);
    if (!t) continue;
    if (STYLE_EXT.has(path.extname(f).toLowerCase()) && /<style/i.test(t) && !isTestFile(rel(f))) {
      for (const st of inlineStyles(t, f)) hostRules.push(...hostElementRules(st.css, `${rel(f)} <style>`, st.lineOffset));
    }
    // A call in code, not one mentioned in a comment (tool modules and docs often name createAiAgent).
    const code = t.split('\n').filter((l) => !/^\s*(\/\/|\/?\*)/.test(l)).join('\n');
    if (/createAiAgent\s*\(/.test(code) && !/export function createAiAgent/.test(t)) integrations.push({ ...integrationInfo(f, code), file: rel(f) });
    if (/createAiAgent|contextChanged|aia-|ai-agent/.test(t)) hints.push(...workaroundHints(rel(f), t));
    if (checks.length < 12 && !isTestFile(rel(f))) checks.push(...featureHints(rel(f), t));
  }
  for (const f of files) {
    if (path.extname(f).toLowerCase() === '.css' && !inRuntime(f)) {
      const t = read(f);
      if (/\.aia-/.test(t)) {
        hints.push(...workaroundHints(rel(f), t));
        hints.push(...layoutWorkarounds(t, rel(f)).map((w) => ({ file: `${w.file}:${w.line}`, hint: w.hint })));
      }
      if (!isTestFile(rel(f))) hostRules.push(...hostElementRules(t, rel(f)));
    }
  }

  // The layout guards of each runtime copy's stylesheet. A copy unchanged since its release is replaced to get them
  // (references/upgrading.md, U4); an edited one gets them in place (scripts/guards.mjs --apply).
  const guards = runtimes.map((rt) => {
    const cssFile = path.join(root, ...rt.dir.split('/'), 'ai-agent.css');
    if (!fs.existsSync(cssFile)) return null;
    const g = layoutGuards(read(cssFile));
    const unchanged = rt.known && !rt.modified.length && !rt.missing.length;
    return { dir: rt.dir, css: rel(cssFile), version: rt.version, pinnedChrome: g.pinnedChrome, hostIsolation: g.hostIsolation, missing: g.missing, markers: g.markers, details: g.details, fix: g.missing.length ? (unchanged ? 'replace' : 'patch') : null };
  }).filter(Boolean);

  const installed = runtimes.map((r) => r.version).sort(cmpVersion);
  const oldest = installed[0] || null;
  let status;
  if (!runtimes.length && !integrations.length) status = 'none';
  else if (!runtimes.length) status = 'integration-without-runtime';
  else if (cmpVersion(oldest, skill.runtimeVersion) < 0) status = 'upgrade';
  // A relay is compared with the skill's own copy of that relay (relays keep their own version numbers).
  else if (relays.some((r) => cmpVersion(r.version, skill.relayVersions?.[r.kind] || skill.runtimeVersion) < 0)) status = 'upgrade';
  else status = 'current';

  // Per feature: is it in the installed runtime, and does the integration use it? (An upgrade adds what is missing.)
  const used = new Set(integrations.flatMap((i) => i.options));
  const features = {};
  for (const ft of FEATURES) {
    features[ft.id] = {
      since: ft.since,
      inRuntime: oldest ? cmpVersion(oldest, ft.since) >= 0 : null,      // null: no runtime copy found
      options: ft.options.filter((o) => used.has(o)),
    };
  }
  features.tools.config = toolConfigs;
  features.memory.file = memoryFiles;
  features.capabilities.index = capabilities.map((c) => c.file);
  features.agents.defined = capabilities.reduce((n, c) => n + (c.agents || 0), 0);
  features.skills.defined = capabilities.reduce((n, c) => n + (c.skills || 0), 0);
  const hasScreenshotHook = features.vision.options.includes('screenshot');
  const manifest = records.find((x) => !x.legacy) || null;
  const legacyRecord = records.find((x) => x.legacy) || null;
  // Framework adoption: an agent is there (status upgrade/current) but no capability folder holds what it can do yet.
  const adopted = capabilities.length > 0 || used.has('capabilities');
  // The tech stack decides which runtime and which recipe (stacks/<id>/STACK.md) the rest of the run uses.
  const stack = detectStack(root, files, { manifestStack: manifest?.data?.stack, webRuntime: runtimes.length > 0 || integrations.length > 0, skillRoot: SKILL });

  return {
    app: root,
    skill: { skillVersion: skill.skillVersion, runtimeVersion: skill.runtimeVersion, relayVersions: skill.relayVersions || {} },
    stack,
    status,
    runtimes: runtimes.map((r) => ({ ...r, extra: r.extra.slice(0, 20) })),
    relays,
    relayConfigs,
    integrations,
    records,
    manifest: manifest ? manifest.file : null,
    legacyRecord: legacyRecord ? legacyRecord.file : null,
    capabilities,
    devTime,
    framework: { adopted, manifest: !!manifest, legacyRecord: !!legacyRecord },
    toolConfigs,
    memoryFiles,
    hints,
    // Only what is still open: no canvas hint once the integration has a screenshot hook.
    checks: checks.filter((c) => !(hasScreenshotHook && /`screenshot` hook/.test(c.hint))).slice(0, 8),
    features: { ...features, record: records.length > 0 },
    layoutGuards: guards,
    hostRules: hostRules.slice(0, MAX_HOST_RULES),
  };
}

/** One Features line's "used by the integration?" part. */
function featureUse(ft, f) {
  const usedBy = [...f.options, ...(ft.id === 'tools' ? f.config : ft.id === 'memory' ? f.file : ft.id === 'capabilities' ? f.index : [])];
  if (ft.id === 'agents' && f.defined) usedBy.push(`${f.defined} in the capability folder`);
  if (ft.id === 'skills' && f.defined) usedBy.push(`${f.defined} in the capability folder`);
  if (usedBy.length) return `used: ${usedBy.join(', ')}`;
  const later = {
    tools: 'not used — the app can gain tools',
    memory: f.inRuntime ? 'on by default; no app memory file yet (ai-memory.json)' : 'to add: comes with the runtime; seed ai-memory.json',
    vision: f.inRuntime ? 'on by default with the browser\'s screen capture; no `screenshot` hook' : 'to add: comes with the runtime; decide on a `screenshot` hook',
    attachments: f.inRuntime ? 'on by default (the + button); no app `readFile` hook' : 'to add: comes with the runtime (the + button); a `readFile` hook only for the app\'s own file formats',
    capabilities: 'not adopted — offer the capability folder (references/upgrading.md, "Adopting the framework")',
    agents: 'none — the app has its one implicit agent; offer agents where users do distinct kinds of work',
    skills: 'none — offer skills for multi-step work the app\'s users repeat',
    workspace: 'off — development only (references/in-app-authoring.md)',
  };
  return later[ft.id] || 'not used';
}

/** The Stack lines: which stack, from what, and whether the skill can build the agent there. */
function stackLines(s) {
  if (!s.id) return ['Stack     not recognised (no package.json, HTML or PHP files, Cargo.toml with a UI crate, or .csproj with a UI) — ask the user what the app is built with'];
  const what = `${s.id}${s.host ? ` (host: ${s.host})` : ''}${s.ui && s.id !== 'web' ? ` — ${s.ui}` : s.host === 'tauri' || s.host === 'aspnet' ? ` — ${s.ui}` : ''}`;
  const support = s.status === 'supported' ? `supported · recipe ${s.profile}`
    : s.status === 'planned' ? `PLANNED: recipe ${s.profile}, runtime not in this skill yet`
      : 'UNSUPPORTED: no recipe in this skill';
  const lines = [`Stack     ${what}  ${s.pinned ? '(pinned by the manifest)' : `(from ${s.evidence.slice(0, 3).join(', ') || '?'})`} — ${support}`];
  for (const w of s.warnings) lines.push(`          ! ${w}`);
  return lines;
}

/** What to do when nothing is installed yet, per stack. */
function installRoute(s) {
  if (!s.id) return ['No AI agent found, and the app\'s stack is not recognised: ask the user what it is built with before installing anything.'];
  if (s.status === 'supported') {
    const extra = s.host === 'tauri' ? ' and its "Tauri" section' : s.host === 'aspnet' ? ' and its "ASP.NET Core / Blazor" section' : '';
    return [`No AI agent found in this app: INSTALL — follow SKILL.md "Install" with the ${s.id} recipe (${s.profile}${extra}).`];
  }
  const label = s.label || s.id;
  const lines = s.status === 'planned'
    ? [`No AI agent found. STOP before installing: this is ${label}, and the skill's runtime for it is not built yet`, `   (${s.profile} says what is planned and what can be done now).`]
    : [`No AI agent found. STOP before installing: the skill has no recipe for ${label}.`];
  lines.push('   Do NOT copy the web runtime (assets/ai-agent/) into this app: it needs a browser page and cannot draw in this UI.');
  lines.push('   Tell the user. What works on any stack today: the survey and plan (SKILL.md steps 1-2) and the ai/ folder\'s');
  lines.push('   skills, agents, toolsets and permissions (spec/framework.md), which every runtime reads unchanged.');
  return lines;
}

export function report(r) {
  const out = [];
  const say = (s = '') => out.push(s);
  say(`ai-enablement ${r.skill.skillVersion} (runtime ${r.skill.runtimeVersion}) · app: ${r.app}`);
  say('');
  for (const l of stackLines(r.stack)) say(l);
  for (const d of r.devTime) say(`Dev-time  ${d.dir}/  (${d.entries} entr${d.entries === 1 ? 'y' : 'ies'}: the coding agent's, not the application's — leave them alone)`);
  if (r.status === 'none') {
    for (const c of r.capabilities) say(`Capabilities ${c.file}${c.error ? `  — ${c.error}` : `  — ${c.agents} agent(s), ${c.skills} skill(s), ${c.tools} tool module(s), ${c.toolsets} toolset file(s)`}`);
    for (const rec of r.records) say(`${rec.legacy ? 'Record  ' : 'Manifest'}  ${rec.file}`);
    say('');
    for (const l of installRoute(r.stack)) say(l);
    return out.join('\n');
  }
  for (const rt of r.runtimes) {
    const state = !rt.known ? 'unknown release (no fingerprints): compare with the app\'s git history before replacing'
      : rt.modified.length || rt.missing.length ? `EDITED since ${rt.version}: ${[...rt.modified.map((f) => `${f} (changed)`), ...rt.missing.map((f) => `${f} (missing)`)].join(', ')}`
        : `unchanged ${rt.version} copy: framework-owned, safe to replace wholesale`;
    say(`Runtime   ${rt.dir}  v${rt.version}  — ${state}${rt.extra.length ? `; extra files: ${rt.extra.join(', ')}` : ''}`);
  }
  if (r.runtimes.length > 1) say('          More than one runtime copy: find out which one the app loads; remove the others.');
  for (const rl of r.relays) {
    const current = r.skill.relayVersions?.[rl.kind];
    say(`Relay     ${rl.file}  v${rl.version}${current && cmpVersion(rl.version, current) < 0 ? ` (current: ${current})` : ''}  — ${rl.unchanged ? 'unchanged copy' : 'EDITED (or an unknown version)'}`);
    for (const e of rl.edits) say(`          · ${e}`);
  }
  for (const c of r.relayConfigs) say(`Config    ${c}  (relay config: app-owned; keep it; contents not read)`);
  for (const i of r.integrations) say(`Agent     ${i.file}  createAiAgent(appId: ${i.appId ? `'${i.appId}'` : '?'})  options: ${i.options.join(', ') || '—'}`);
  if (r.integrations.length > 1) say('          More than one createAiAgent() call: make sure only one runs (one agent per app).');
  for (const c of r.capabilities) {
    if (c.error) { say(`Capabilities ${c.file}  — ${c.error}`); continue; }
    say(`Capabilities ${c.file}  — ${c.agents} agent(s), ${c.skills} skill(s), ${c.tools} tool module(s), ${c.toolsets} toolset file(s)${c.toolsConfig ? `, tools config ${c.toolsConfig}` : ''}${c.memory ? `, memory ${c.memory}` : ''}${c.permissions ? ', permissions' : ''}`);
    if (c.missing.length) say(`          MISSING: ${c.missing.join(', ')}`);
  }
  for (const rec of r.records) {
    say(`${rec.legacy ? 'Record  ' : 'Manifest'}  ${rec.file}  (skill ${rec.data?.skill || '?'} ${rec.data?.skillVersion || '?'}, updated ${rec.data?.updated || '?'})${rec.legacy ? ' — the 1.x record: rename it to ai-enablement.json and add the 2.0 fields (references/upgrading.md)' : ''}`);
  }
  if (!r.records.length) say('Manifest  none yet — write ai-enablement.json at the end (references/upgrading.md, "The manifest").');
  say('');
  say('Features  (in the installed runtime? · used by the integration?)');
  for (const ft of FEATURES) {
    const f = r.features[ft.id];
    const has = f.inRuntime === null ? `runtime copy not found (needs ${ft.since}+)` : f.inRuntime ? 'in the runtime' : `NOT in the runtime (arrives with ${ft.since})`;
    say(`  ${ft.id.padEnd(12)} ${has} · ${featureUse(ft, f)}  (${ft.doc})`);
  }
  if (r.layoutGuards.length) {
    say('');
    say('Layout guards  (ai-agent.css, runtime 1.6.1+: the settings dialog keeps its tabs and ignores host element CSS)');
    for (const g of r.layoutGuards) {
      const fix = g.fix === 'replace' ? `FIX IN THIS RUN: replace the runtime (unchanged ${g.version} copy; references/upgrading.md, U4)`
        : g.fix === 'patch' ? `FIX IN THIS RUN: node <skill>/scripts/guards.mjs <app-root> --apply (edited copy: adds ${g.missing.join(' + ')} in place)` : 'in place';
      say(`  ${g.css}  pinned chrome: ${g.pinnedChrome ? 'yes' : 'MISSING'} · host isolation: ${g.hostIsolation ? 'yes' : 'MISSING'} — ${fix}`);
    }
  }
  if (r.hostRules.length && r.stack.family !== 'rust' && r.stack.family !== 'dotnet') {
    say('');
    say('Host element rules the agent\'s UI must withstand (information: what verify.mjs\'s settings check is up against):');
    for (const h of r.hostRules.slice(0, 12)) say(`  · ${h.file}:${h.line}  ${h.selector} { ${h.props.join(', ')} }${h.media ? `  (${h.media})` : ''}`);
    if (r.hostRules.length > 12) say(`  … ${r.hostRules.length - 12} more (--json)`);
  }
  if (r.hints.length) {
    say('');
    say('Check (possible workarounds a newer runtime covers):');
    for (const h of r.hints) say(`  · ${h.file}: ${h.hint}`);
  }
  if (r.checks.length) {
    say('');
    say('Check for memory and vision:');
    for (const h of r.checks) say(`  · ${h.file}: ${h.hint}`);
  }
  say('');
  if (r.status === 'upgrade') {
    const from = r.runtimes.map((x) => x.version).sort(cmpVersion)[0];
    say(`=> UPGRADE: an agent is already built in (runtime ${from}). Do NOT build a second one. Follow references/upgrading.md,`);
    say(`   reading CHANGELOG.md from ${from} to ${r.skill.runtimeVersion}.${r.framework.adopted ? '' : ' Then offer the capability folder (adoption is optional).'}`);
  } else if (r.status === 'current') {
    say(`=> CURRENT: the runtime is up to date (${r.skill.runtimeVersion}). VALIDATE (node <skill>/scripts/validate.mjs ${r.capabilities.length ? '<app-root>' : 'once there is a capability folder'}), then make the`);
    say('   change the user asked for (add a tool, a skill, an agent: references/capabilities.md) or offer what the app does not use yet');
    say('   (references/upgrading.md, U6). Do NOT build a second agent.');
  } else {
    say('=> An integration calls createAiAgent() but no runtime copy was found (bundled from a package, or outside this folder).');
    say('   Find where the runtime comes from before changing anything. Do NOT build a second agent.');
  }
  if (r.framework.legacyRecord && !r.framework.manifest) say('   The record is the 1.x ai-agent.integration.json: write ai-enablement.json in its place.');
  const unguarded = r.layoutGuards.filter((g) => g.missing.length);
  if (unguarded.length) {
    say(`   Layout guards missing in ${unguarded.map((g) => g.css).join(', ')}: fix them in this run, whatever the task (references/upgrading.md,`);
    say('   "Layout guards"), and tell the user in one line.');
  }
  return out.join('\n');
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  const argv = process.argv.slice(2);
  const dir = argv.find((a) => !a.startsWith('--')) || '.';
  const result = detect(dir);
  console.log(argv.includes('--json') ? JSON.stringify(result, null, 2) : report(result));
}
