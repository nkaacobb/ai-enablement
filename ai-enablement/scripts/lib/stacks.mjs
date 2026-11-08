// Which tech stack is this app, and does the skill have a runtime and a recipe for it? Shared by detect.mjs,
// validate.mjs and scaffold.mjs. Read-only: it looks at manifests (Cargo.toml, *.csproj, package.json…) and file names.
//
// A stack is the app's UI technology, because that decides which runtime draws the agent:
//   web              the JS runtime (assets/ai-agent/) in a page: plain HTML/PHP, React, Vue… — and every app whose UI
//                    is a webview or HTML: Tauri (host "tauri"), ASP.NET Core / Blazor (host "aspnet")
//   rust-egui        a native Rust runtime drawn with egui (assets/rust/)
//   dotnet-winforms  a native .NET runtime drawn with Windows Forms (assets/dotnet/)
//   dotnet-wpf, dotnet-winui, dotnet-maui, rust-other   recognised, no recipe yet
// The recipe for a stack is stacks/<id>/STACK.md. A stack is "supported" when its recipe AND its runtime are in the
// skill, "planned" when only the recipe is, "unsupported" otherwise. The manifest's "stack" pins it for later runs.

import fs from 'node:fs';
import path from 'node:path';

/** What the skill knows per stack: its recipe, the runtime file that proves the runtime ships, and a short label. */
export const STACKS = {
  web: { family: 'web', label: 'web (HTML, PHP, React, Vue, Svelte, Angular, Next.js; Tauri; ASP.NET Core / Blazor)', profile: 'stacks/web/STACK.md', runtime: 'assets/ai-agent/ai-agent.js' },
  'rust-egui': { family: 'rust', label: 'Rust with egui / eframe', profile: 'stacks/rust-egui/STACK.md', runtime: 'assets/rust/ai-enablement-egui/Cargo.toml' },
  'dotnet-winforms': { family: 'dotnet', label: 'C# / .NET with Windows Forms', profile: 'stacks/dotnet-winforms/STACK.md', runtime: 'assets/dotnet/AiEnablement.WinForms/AiEnablement.WinForms.csproj' },
  'dotnet-wpf': { family: 'dotnet', label: 'C# / .NET with WPF', profile: null, runtime: null },
  'dotnet-winui': { family: 'dotnet', label: 'C# / .NET with WinUI 3', profile: null, runtime: null },
  'dotnet-maui': { family: 'dotnet', label: 'C# / .NET MAUI', profile: null, runtime: null },
  'rust-other': { family: 'rust', label: 'Rust with a native UI other than egui (Iced, Slint, GTK, FLTK…)', profile: null, runtime: null },
};

/** Native Rust UI crates. egui/eframe has a recipe; the others would need their own UI adapter over the Rust core. */
const RUST_UI = { eframe: 'rust-egui', egui: 'rust-egui', iced: 'rust-other', slint: 'rust-other', gtk: 'rust-other', gtk4: 'rust-other', fltk: 'rust-other', druid: 'rust-other', xilem: 'rust-other', floem: 'rust-other' };
const ORDER = ['rust-egui', 'dotnet-winforms', 'dotnet-wpf', 'dotnet-winui', 'dotnet-maui', 'rust-other'];

const readText = (f) => { try { return fs.statSync(f).size > 1024 * 1024 ? '' : fs.readFileSync(f, 'utf8'); } catch { return ''; } };
const rel = (root, f) => path.relative(root, f).split(path.sep).join('/');

/**
 * The runtime dependencies a Cargo.toml declares (dev- and build-dependencies are left out): name → version or ''.
 * Covers `name = "1.0"`, `name = { version = "=1.0", … }`, `name.workspace = true`, `[dependencies.name]` and
 * target-specific tables. A small line reader, not a TOML parser: enough to recognise a UI crate.
 */
export function cargoDependencies(text) {
  const deps = {};
  let table = '';
  for (const raw of String(text).split(/\r?\n/)) {
    const line = raw.replace(/#.*$/, '').trim();
    if (!line) continue;
    const header = line.match(/^\[\[?\s*([^\]]+?)\s*\]\]?$/);
    if (header) {
      table = header[1];
      const named = table.match(/(?:^|\.)dependencies\.([A-Za-z0-9_-]+)$/);
      if (named && !/(dev|build)-dependencies/.test(table)) deps[named[1]] = deps[named[1]] || '';
      continue;
    }
    const inDeps = /(^|\.)dependencies$/.test(table) && !/(dev|build)-dependencies/.test(table);
    const inNamed = table.match(/(?:^|\.)dependencies\.([A-Za-z0-9_-]+)$/);
    if (inNamed && !/(dev|build)-dependencies/.test(table)) {
      const v = line.match(/^version\s*=\s*"([^"]*)"/);
      if (v) deps[inNamed[1]] = v[1].replace(/^[=^~]/, '');
      continue;
    }
    if (!inDeps) continue;
    const m = line.match(/^([A-Za-z0-9_-]+)\s*(?:\.\s*workspace\s*=|=)\s*(.*)$/);
    if (!m) continue;
    const value = m[2];
    const version = value.match(/^"([^"]*)"/)?.[1] ?? value.match(/version\s*=\s*"([^"]*)"/)?.[1] ?? '';
    deps[m[1]] = version.replace(/^[=^~]/, '') || deps[m[1]] || '';
  }
  return deps;
}

/** What a .csproj / .fsproj / .vbproj says about its UI: winforms, wpf, winui, maui, aspnet, or nothing. */
export function dotnetProjectKind(text) {
  const t = String(text);
  const flag = (name) => new RegExp(`<${name}>\\s*true\\s*</${name}>`, 'i').test(t);
  const framework = t.match(/<TargetFrameworks?>\s*([^<]+?)\s*<\/TargetFrameworks?>/i)?.[1] || '';
  if (flag('UseWindowsForms')) return { kind: 'dotnet-winforms', framework };
  if (flag('UseWPF')) return { kind: 'dotnet-wpf', framework };
  if (flag('UseMaui')) return { kind: 'dotnet-maui', framework };
  if (/Microsoft\.WindowsAppSDK/i.test(t)) return { kind: 'dotnet-winui', framework };
  if (/Sdk\s*=\s*"Microsoft\.NET\.Sdk\.(Web|BlazorWebAssembly|Razor)"/i.test(t)) return { kind: 'aspnet', framework };
  return { kind: null, framework };
}

/** The skill's own view of a stack: is its recipe there, does its runtime ship? */
export function stackSupport(id, skillRoot) {
  const s = STACKS[id];
  if (!s) return { status: 'unsupported', profile: null };
  const has = (p) => !!p && fs.existsSync(path.join(skillRoot, ...p.split('/')));
  const profile = has(s.profile) ? s.profile : null;
  if (profile && has(s.runtime)) return { status: 'supported', profile };
  if (profile) return { status: 'planned', profile };
  return { status: 'unsupported', profile: null };
}

/**
 * The app's stack, from the files detect.mjs walked (build output already skipped).
 * @param {string} root the app root
 * @param {string[]} files absolute paths
 * @param {{ manifestStack?: string|{id:string}, webRuntime?: boolean, skillRoot: string }} opts
 */
export function detectStack(root, files, { manifestStack, webRuntime = false, skillRoot }) {
  const evidence = [];
  const native = new Map();           // stack id → { ui, evidence[] }
  const add = (id, ui, why) => {
    const e = native.get(id) || { ui: '', evidence: [] };
    if (ui && !e.ui) e.ui = ui;
    e.evidence.push(why);
    native.set(id, e);
  };
  let tauri = '';
  let aspnet = '';
  const web = { html: 0, php: 0, packageJson: '', composer: '' };
  const workspaceVersions = {};
  const cargoFiles = [];
  const inDotDir = (f) => rel(root, f).split('/').slice(0, -1).some((p) => p.startsWith('.'));

  for (const f of files) {
    if (inDotDir(f)) continue;
    const base = path.basename(f);
    const ext = path.extname(base).toLowerCase();
    if (base === 'Cargo.toml') cargoFiles.push(f);
    else if (base === 'tauri.conf.json' || base === 'Tauri.toml') tauri = tauri || rel(root, f);
    else if (ext === '.csproj' || ext === '.fsproj' || ext === '.vbproj') {
      const { kind, framework } = dotnetProjectKind(readText(f));
      if (kind === 'aspnet') aspnet = aspnet || rel(root, f);
      else if (kind) add(kind, framework ? `.NET ${framework}` : '.NET', `${rel(root, f)} (${kind.replace('dotnet-', '')})`);
    } else if (base === 'package.json') web.packageJson = web.packageJson || rel(root, f);
    else if (base === 'composer.json') web.composer = web.composer || rel(root, f);
    else if (ext === '.php') web.php++;
    else if (ext === '.html' || ext === '.htm') web.html++;
  }
  // Workspace-level versions first, so `eframe = { workspace = true }` in a member shows the pinned version.
  const parsed = cargoFiles.map((f) => {
    const text = readText(f);
    const deps = cargoDependencies(text);
    if (/^\s*\[workspace\.dependencies\]/m.test(text)) Object.assign(workspaceVersions, deps);
    return { f, deps };
  });
  for (const { f, deps } of parsed) {
    if ('tauri' in deps) tauri = tauri || `${rel(root, f)} (tauri)`;
    // A workspace root that only pins versions for its members is not itself a UI crate.
    const text = readText(f);
    if (/^\s*\[workspace\.dependencies\]/m.test(text) && !/^\s*\[(package|dependencies)\]/m.test(text)) continue;
    const byStack = new Map();
    for (const [crate, id] of Object.entries(RUST_UI)) if (crate in deps) byStack.set(id, [...(byStack.get(id) || []), crate]);
    for (const [id, crates] of byStack) {
      const crate = crates[0];
      const version = deps[crate] || workspaceVersions[crate] || '';
      add(id, `${crate === 'eframe' || crate === 'egui' ? 'egui/eframe' : crate}${version ? ` ${version}` : ''}`, `${rel(root, f)} (${crates.join(', ')})`);
    }
  }

  // The web stack's host: what serves the page or wraps it.
  const webHost = tauri ? 'tauri' : aspnet ? 'aspnet' : web.php || web.composer ? 'php' : web.packageJson ? 'node' : web.html ? 'static' : null;
  const webEvidence = [tauri, aspnet, web.packageJson, web.composer, web.php ? `${web.php} .php file(s)` : '', web.html ? `${web.html} .html file(s)` : ''].filter(Boolean);

  const candidates = ORDER.filter((id) => native.has(id));
  let id = null;
  let host = null;
  let ui = '';
  let pinned = false;
  const warnings = [];
  const pin = typeof manifestStack === 'string' ? manifestStack : manifestStack?.id;
  if (pin) {
    id = pin;
    pinned = true;
    if (id === 'web') { host = (typeof manifestStack === 'object' && manifestStack.host) || webHost; ui = 'HTML'; } else ui = native.get(id)?.ui || (typeof manifestStack === 'object' && manifestStack.ui) || '';
    const seen = webRuntime ? 'web' : (tauri || aspnet) ? 'web' : candidates[0] || (webHost ? 'web' : null);
    if (seen && seen !== id) warnings.push(`the manifest pins "${id}" but the files look like "${seen}": check which one is right before changing anything`);
    evidence.push(`ai-enablement.json: "stack": "${id}"`);
  } else if (webRuntime) {
    id = 'web'; host = webHost; ui = 'HTML';
    evidence.push('ai-agent.js installed', ...webEvidence);
  } else if (tauri || aspnet) {
    // The UI is a webview or HTML, so the web runtime draws the agent even when native crates are also present.
    id = 'web'; host = tauri ? 'tauri' : 'aspnet'; ui = tauri ? 'Tauri webview' : 'ASP.NET Core / Blazor';
    evidence.push(...webEvidence);
  } else if (candidates.length) {
    id = candidates[0];
    ui = native.get(id).ui;
    evidence.push(...native.get(id).evidence);
  } else if (webHost) {
    id = 'web'; host = webHost; ui = 'HTML';
    evidence.push(...webEvidence);
  }
  if (!pinned && candidates.length > 1) warnings.push(`more than one native UI in this folder (${candidates.join(', ')}): ask the user which application gets the agent, and pin it in the manifest`);

  const support = id ? stackSupport(id, skillRoot) : { status: 'unsupported', profile: null };
  return {
    id,
    family: id ? STACKS[id]?.family || null : null,
    host,
    ui,
    label: id ? STACKS[id]?.label || id : null,
    status: support.status,
    profile: support.profile,
    pinned,
    evidence: evidence.slice(0, 8),
    candidates: candidates.map((c) => ({ id: c, ui: native.get(c).ui, evidence: native.get(c).evidence.slice(0, 4) })),
    warnings,
  };
}
