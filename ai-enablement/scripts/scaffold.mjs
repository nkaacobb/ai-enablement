#!/usr/bin/env node
// Create a capability in an application's capability folder from the skill's templates, and register it — the
// mechanical half of "add a tool / a skill / an agent" (the coding agent then writes the real content, following
// references/capabilities.md). It never overwrites a file; it only adds to the index and the tool config.
//
//   node <skill>/scripts/scaffold.mjs <capability-folder> init
//   node <skill>/scripts/scaffold.mjs <capability-folder> tool <name> [--effect read|write|destructive|external|system]
//                                      [--title "…"] [--description "…"] [--on]
//   node <skill>/scripts/scaffold.mjs <capability-folder> toolset <name> --tools a,b [--title "…"] [--description "…"]
//   node <skill>/scripts/scaffold.mjs <capability-folder> skill <name> --description "…" [--title "…"]
//   node <skill>/scripts/scaffold.mjs <capability-folder> agent <name> --description "…" [--title "…"]
//                                      [--tools a,b] [--toolsets x,y] [--skills s,t]
//   any of them: [--stack web|rust-egui|dotnet-winforms]  (default: the "stack" in the nearest ai-enablement.json, else web)
//
// <capability-folder> is the folder that holds (or will hold) index.json, e.g. public/ai. `init` creates index.json,
// ai-tools.json and ai-memory.json there; the other kinds run `init` first when there is no index yet.
// --json prints what was done as JSON.

import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { STACKS, stackSupport } from './lib/stacks.mjs';

const SKILL = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..');
const TEMPLATES = path.join(SKILL, 'assets', 'templates', 'ai');
const TOOL_NAME = /^[A-Za-z][A-Za-z0-9_-]{0,63}$/;
const SKILL_NAME = /^(?=.{1,64}$)[a-z0-9]+(?:-[a-z0-9]+)*$/;
const EFFECTS = ['read', 'write', 'destructive', 'external', 'system'];
const KIND_LIST = { tool: 'tools', toolset: 'toolsets', skill: 'skills', agent: 'agents' };
const LANGUAGE = { rust: 'Rust', dotnet: 'C#' };

/** The app's stack for this capability folder: --stack, else the nearest manifest's "stack" above it, else web. */
export function stackFor(dir, explicit = '') {
  if (explicit) {
    if (!STACKS[explicit]) throw new Error(`--stack "${explicit}" is not a known stack (${Object.keys(STACKS).join(', ')}).`);
    return explicit;
  }
  let d = path.resolve(dir);
  for (let i = 0; i < 12; i++) {
    const f = path.join(d, 'ai-enablement.json');
    if (fs.existsSync(f)) {
      try { const s = JSON.parse(fs.readFileSync(f, 'utf8')).stack; const id = typeof s === 'string' ? s : s?.id; if (id && STACKS[id]) return id; } catch { /* unreadable: fall through */ }
      break;
    }
    const up = path.dirname(d);
    if (up === d) break;
    d = up;
  }
  return 'web';
}

const title = (name) => String(name).replace(/[_-]+/g, ' ').replace(/^\w/, (c) => c.toUpperCase());
const fill = (text, values) => text.replace(/\{\{(\w+)\}\}/g, (m, k) => (k in values ? values[k] : m));
const list = (v) => String(v ?? '').split(',').map((x) => x.trim()).filter(Boolean);
/** A YAML scalar: plain when that reads the same, else double-quoted (JSON escaping is valid YAML). */
const yamlString = (v) => (/^[A-Za-z0-9(][^:#\n"']*$/.test(v) && !/\s$/.test(v) ? v : JSON.stringify(v));

/** JSON with 2-space indentation, short arrays and small objects kept on one line (as people write index files). */
export function formatJson(value, indent = '') {
  const inner = `${indent}  `;
  if (Array.isArray(value)) {
    if (!value.length) return '[]';
    const flat = `[${value.map((v) => JSON.stringify(v)).join(', ')}]`;
    if (value.every((v) => v === null || typeof v !== 'object') && flat.length + indent.length <= 110) return flat;
    return `[\n${value.map((v) => `${inner}${formatJson(v, inner)}`).join(',\n')}\n${indent}]`;
  }
  if (value && typeof value === 'object') {
    const entries = Object.entries(value);
    if (!entries.length) return '{}';
    const flat = `{ ${entries.map(([k, v]) => `${JSON.stringify(k)}: ${formatJson(v, inner)}`).join(', ')} }`;
    if (indent && !flat.includes('\n') && flat.length + indent.length <= 110) return flat;
    return `{\n${entries.map(([k, v]) => `${inner}${JSON.stringify(k)}: ${formatJson(v, inner)}`).join(',\n')}\n${indent}}`;
  }
  return JSON.stringify(value);
}

const readJson = (file) => JSON.parse(fs.readFileSync(file, 'utf8'));
const writeJson = (file, value) => fs.writeFileSync(file, `${formatJson(value)}\n`);

/**
 * Do one scaffolding step. Returns { created: [paths], updated: [paths], next: [what to do now] } (paths relative to
 * the capability folder). Throws on invalid names and on files that already exist.
 */
export function scaffold(dir, kind, name = '', opts = {}) {
  const root = path.resolve(dir);
  const done = { created: [], updated: [], next: [] };
  const indexFile = path.join(root, 'index.json');
  const create = (rel, text) => {
    const file = path.join(root, ...rel.split('/'));
    if (fs.existsSync(file)) throw new Error(`${rel} already exists; scaffold never overwrites. Edit it, or pick another name.`);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text);
    done.created.push(rel);
  };

  const stack = stackFor(root, opts.stack);
  const family = STACKS[stack].family;
  const compiled = family !== 'web';
  if (kind === 'init' || !fs.existsSync(indexFile)) {
    fs.mkdirSync(root, { recursive: true });
    if (!fs.existsSync(indexFile)) {
      // Compiled stacks: tools are code, listed by the generated tool catalog, never by module paths.
      const template = readJson(path.join(TEMPLATES, 'index.json'));
      if (compiled) {
        delete template.tools;
        template.$comment = `The application's capability index: what its AI can do (AI Enablement, stack ${stack}). Paths are relative to this file. The tools are ${LANGUAGE[family]} code: once the app's catalog test has written tools.catalog.json, name it here as "toolCatalog" (spec/tool-catalog.md). Check it with: node <skill>/scripts/validate.mjs <app-root>`;
      }
      create('index.json', compiled ? `${formatJson(template)}\n` : fs.readFileSync(path.join(TEMPLATES, 'index.json'), 'utf8'));
    }
    if (!fs.existsSync(path.join(root, 'ai-tools.json'))) {
      create('ai-tools.json', `${formatJson({ $comment: 'Which tools start on (named as "toolsConfig" in index.json). Users change it in Settings > Tools; "Download ai-tools.json" there writes this format.', toolsEnabled: true, confirmWrites: true, confirmDestructive: true, toolMode: 'auto', maxToolSteps: 8, tools: {} })}\n`);
    }
    if (!fs.existsSync(path.join(root, 'ai-memory.json'))) {
      create('ai-memory.json', `${formatJson({ $comment: 'What the agent knows from the start (named as "memory" in index.json): facts no screen shows. Never secrets.', version: 1, memories: [] })}\n`);
    }
    if (kind === 'init') {
      if (compiled) done.next.push(`Load it as stacks/${stack}/STACK.md §4 says (embedded or next to the executable).`, 'Add capabilities: scaffold.mjs <folder> skill|agent|toolset <name>; tools are written in code (§9).');
      else done.next.push('Load it: createAiAgent({ capabilities: \'<URL of index.json>\', host: <the object your tools call> }).', 'Add capabilities: scaffold.mjs <folder> tool|skill|agent <name>.');
      return done;
    }
  }

  const index = readJson(indexFile);
  const register = (key, rel) => {
    if (!Array.isArray(index[key])) index[key] = [];
    if (!index[key].includes(rel)) { index[key].push(rel); return true; }
    return false;
  };
  const description = String(opts.description || '').replace(/\s+/g, ' ').trim();

  if (kind === 'tool' && compiled) {
    const { status, profile } = stackSupport(stack, SKILL);
    throw new Error(`On ${stack} a tool is ${LANGUAGE[family]} code in the app, not a file in the capability folder (${profile || `stacks/${stack}/STACK.md`} §9).${status === 'supported' ? '' : ` The ${stack} runtime is not in this skill yet, so there is nothing to write tools against: scaffold skills, agents and toolsets now, tools once the runtime ships.`}`);
  }
  if (kind === 'tool') {
    if (!TOOL_NAME.test(name)) throw new Error(`"${name}" is not a tool name: letters, digits, _ or -, starting with a letter.`);
    const effect = opts.effect || 'write';
    if (!EFFECTS.includes(effect)) throw new Error(`--effect must be one of ${EFFECTS.join(', ')}.`);
    const rel = `tools/${name}.js`;
    const values = { name, title: opts.title || title(name), description: (description || `TODO: what ${name} does, in the user's words.`).replace(/'/g, "\\'"), effect };
    create(rel, fill(fs.readFileSync(path.join(TEMPLATES, 'tools', 'tool.js'), 'utf8'), values));
    if (register('tools', rel)) done.updated.push('index.json');
    const cfgRel = typeof index.toolsConfig === 'string' ? index.toolsConfig : '';
    const cfgFile = cfgRel ? path.resolve(root, cfgRel) : '';
    if (cfgFile && fs.existsSync(cfgFile)) {
      const cfg = readJson(cfgFile);
      cfg.tools = cfg.tools && typeof cfg.tools === 'object' ? cfg.tools : {};
      if (!(name in cfg.tools)) {
        cfg.tools[name] = { enabled: !!opts.on, effect, description: description || values.title };
        writeJson(cfgFile, cfg);
        done.updated.push(cfgRel);
      }
    }
    done.next.push(`Write ${rel}: call the application's own function through host; set the real parameters and effect.`, `Unit-test it with host mocked; then node <skill>/scripts/validate.mjs.`);
  } else if (kind === 'toolset') {
    if (!TOOL_NAME.test(name)) throw new Error(`"${name}" is not a toolset name: letters, digits, _ or -, starting with a letter.`);
    const tools = list(opts.tools);
    if (!tools.length) throw new Error('A toolset needs --tools a,b (tools defined in tool modules).');
    const rel = `toolsets/${name}.json`;
    create(rel, fill(fs.readFileSync(path.join(TEMPLATES, 'toolsets', 'toolset.json'), 'utf8'), { name, title: JSON.stringify(opts.title || title(name)).slice(1, -1), description: JSON.stringify(description).slice(1, -1), tools: JSON.stringify(tools) }));
    if (register('toolsets', rel)) done.updated.push('index.json');
    done.next.push(`Agents use it with "toolsets: [${name}]"; permission rules with "toolset:${name}".`);
  } else if (kind === 'skill') {
    if (!SKILL_NAME.test(name)) throw new Error(`"${name}" is not a skill name: lowercase letters, digits and single hyphens (Agent Skills format).`);
    if (!description) throw new Error('A skill needs --description "what it does and when to use it" (the model reads it to decide).');
    const rel = `skills/${name}/SKILL.md`;
    create(rel, fill(fs.readFileSync(path.join(TEMPLATES, 'skills', 'skill', 'SKILL.md'), 'utf8'), { name, title: opts.title || title(name), description: yamlString(description) }));
    if (register('skills', `skills/${name}`)) done.updated.push('index.json');
    done.next.push(`Write the steps in ${rel}; put long reference material in skills/${name}/references/.`);
  } else if (kind === 'agent') {
    if (!SKILL_NAME.test(name)) throw new Error(`"${name}" is not an agent name: lowercase letters, digits and single hyphens.`);
    if (!description) throw new Error('An agent needs --description "what it is for" (shown in the picker).');
    const lines = [];
    for (const key of ['toolsets', 'tools', 'skills']) if (list(opts[key]).length) lines.push(`${key}: [${list(opts[key]).join(', ')}]`);
    const rel = `agents/${name}.md`;
    create(rel, fill(fs.readFileSync(path.join(TEMPLATES, 'agents', 'agent.md'), 'utf8'), { name, title: opts.title || title(name), description: yamlString(description), lists: lines.map((l) => `${l}\n`).join('') }));
    if (register('agents', rel)) done.updated.push('index.json');
    done.next.push(`Write the agent's instructions in ${rel}. It composes existing tools, toolsets and skills; omit tools and toolsets to give it all of them.`);
  } else {
    throw new Error(`Unknown kind "${kind}": init, tool, toolset, skill or agent.`);
  }
  writeJson(indexFile, index);
  done.updated = [...new Set(done.updated)];
  return done;
}

const isMain = process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isMain) {
  const argv = process.argv.slice(2);
  const valued = new Set(['effect', 'title', 'description', 'tools', 'toolsets', 'skills', 'stack']);
  const opts = {};
  const pos = [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a.startsWith('--')) { const k = a.slice(2); if (valued.has(k)) { opts[k] = argv[i + 1]; i++; } else opts[k] = true; } else pos.push(a);
  }
  const [dir, kind, name] = pos;
  if (!dir || !kind || opts.help) {
    console.log(fs.readFileSync(new URL(import.meta.url), 'utf8').split('\n').slice(1, 17).map((l) => l.replace(/^\/\/ ?/, '')).join('\n'));
    process.exit(dir && kind ? 0 : 1);
  }
  try {
    const r = scaffold(dir, kind, name, opts);
    if (opts.json) console.log(JSON.stringify(r, null, 2));
    else {
      for (const f of r.created) console.log(`created  ${path.join(dir, f)}`);
      for (const f of r.updated) console.log(`updated  ${path.join(dir, f)}`);
      for (const n of r.next) console.log(`next     ${n}`);
    }
  } catch (e) {
    console.error(e.message);
    process.exit(1);
  }
}
