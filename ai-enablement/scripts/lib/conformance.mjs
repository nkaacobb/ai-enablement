// The conformance suites: language-neutral test vectors for the behaviour every runtime shares (spec/behaviour.md).
// Each suite names the operation it pins, its cases (JSON inputs), and how the reference implementation (the JS
// runtime) computes the output. scripts/conformance-export.mjs writes spec/conformance/<suite>.json from them;
// tests/conformance.test.mjs checks the JS runtime still produces exactly those outputs; the Rust and .NET runtimes
// run the same files with their own runners. Inputs and outputs are plain JSON: no functions, no clocks.

import { hashText, shortHash, stableStringify } from '../../assets/ai-agent/core/hash.js';
import { ContextManager, describe } from '../../assets/ai-agent/core/context.js';
import { planTurn, contextState, snapshotBlock, supersededStub, windowStart } from '../../assets/ai-agent/core/conversation.js';
import { buildSystemPrompt } from '../../assets/ai-agent/core/prompt.js';
import { normalizeTool, toMcpTool, readArguments, validateArgs, serializeResult } from '../../assets/ai-agent/core/tools.js';
import { normalizePolicy, mergePolicies, decidePermission, needsConfirmation } from '../../assets/ai-agent/core/permissions.js';
import { effectFromAnnotations, annotationsFor } from '../../assets/ai-agent/core/schema.js';
import { parseFrontmatter } from '../../assets/ai-agent/core/frontmatter.js';
import { parseSkill, skillFilePath, parseSlashCommand } from '../../assets/ai-agent/core/skills.js';
import { parseAgent, pickAgent } from '../../assets/ai-agent/core/agents.js';
import { parseMemoryFile, buildMemoryPrompt, memoryText } from '../../assets/ai-agent/core/memory.js';

const noRun = () => undefined;
/** A tool contract without its functions, as JSON. */
const contract = (t) => Object.fromEntries(Object.entries(t).filter(([, v]) => typeof v !== 'function'));
const long = (n, unit = 'abcdefghij') => unit.repeat(Math.ceil(n / unit.length)).slice(0, n);

export const SUITES = [
  {
    suite: 'hash',
    description: 'hashText: cyrb53 over the UTF-16 code units of the text, 53 bits, as 14 lowercase hex digits; shortHash: the first 7. The screen fingerprint and every hash shown to the user.',
    operation: 'hashText(input.text) -> { hash, short }',
    cases: [
      { name: 'empty', input: { text: '' } },
      { name: 'ascii', input: { text: 'hello world' } },
      { name: 'page fingerprint', input: { text: 'editor\nEditor\nThe quick brown fox.' } },
      { name: 'unicode (BMP and astral)', input: { text: 'Grüße — 日本語 🙂' } },
      { name: 'one character differs', input: { text: 'hello worle' } },
      { name: 'long text', input: { text: long(5000) } },
    ],
    run: ({ text }) => ({ hash: hashText(text), short: shortHash(hashText(text)) }),
  },
  {
    suite: 'stable-stringify',
    description: 'stableStringify: JSON with object keys sorted (code-unit order) at every level, arrays kept in order; indent 0 means compact. Numbers are written as ECMAScript Number::toString writes them (1e+21, not 1e21): a runtime in another language needs a JS-compatible formatter (Rust: ryu-js). Content builders, tool results and fingerprints rely on it being deterministic.',
    operation: 'stableStringify(input.value, input.indent) -> string',
    cases: [
      { name: 'sorted keys, indented', input: { value: { b: 1, a: { d: [3, 1], c: 'x' } }, indent: 2 } },
      { name: 'compact', input: { value: { z: true, a: null, m: [{ y: 1, x: 2 }] }, indent: 0 } },
      { name: 'uppercase sorts before lowercase', input: { value: { b: 1, B: 2, a: 3, A: 4 }, indent: 0 } },
      { name: 'string escapes', input: { value: { q: 'say "hi"\n\ttab \\ backslash' }, indent: 0 } },
      { name: 'empty containers', input: { value: { a: {}, b: [] }, indent: 2 } },
      { name: 'numbers', input: { value: [0, -1.5, 1e21, 123456789012], indent: 0 } },
    ],
    run: ({ value, indent }) => stableStringify(value, indent),
  },
  {
    suite: 'describe',
    description: 'describe: how app and page context objects become prompt text: "Humanized Key: value" lines, lists as "- " bullets under their key, nested objects as JSON, empty values left out.',
    operation: 'describe(input.value) -> string',
    cases: [
      { name: 'app context', input: { value: { name: 'Inventory', purpose: 'Track stock.', capabilities: ['Items: stock items', 'Orders: purchase orders'], limits: ['Cannot place orders'] } } },
      { name: 'page context with camelCase keys', input: { value: { id: 'orders', title: 'Orders', purpose: 'Open orders.', openDialog: 'Edit order 12' } } },
      { name: 'nested object and empty values', input: { value: { name: 'X', filters: { status: 'open', page: 2 }, note: '', tags: [] } } },
      { name: 'a plain string', input: { value: 'Just text.' } },
    ],
    run: ({ value }) => describe(value),
  },
  {
    suite: 'context-snapshot',
    description: 'ContextManager.snapshot: the content hook serialised (a string as written; { text } alone as its text; anything else as stableStringify with indent 2), clipped at maxChars (keep max-160 chars and add the truncation note), fingerprinted over "pageId\\npageTitle\\nFULL text" before clipping. Output without the clock ("at").',
    operation: 'new ContextManager({ page: input.page, maxChars: input.maxChars }).snapshot() -> { pageId, pageTitle, text, chars, totalChars, truncated, hash, empty }',
    cases: [
      { name: 'string content', input: { page: { id: 'editor', title: 'Editor', content: 'Hello.' }, maxChars: 24000 } },
      { name: '{ text } alone is its text', input: { page: { id: 'editor', title: 'Editor', content: { text: 'Hello.' } }, maxChars: 24000 } },
      { name: 'object content is stable JSON', input: { page: { id: 'orders', title: 'Orders', content: { rows: [{ id: 2, status: 'open' }, { status: 'shipped', id: 1 }], total: 2 } }, maxChars: 24000 } },
      { name: 'empty content', input: { page: { id: 'blank', title: 'Blank', content: '   ' }, maxChars: 24000 } },
      { name: 'clipped, hash over the full text', input: { page: { id: 'log', title: 'Log', content: long(1500) }, maxChars: 1000 } },
      { name: 'same text, other page: other hash', input: { page: { id: 'other', title: 'Editor', content: 'Hello.' }, maxChars: 24000 } },
    ],
    run: async ({ page, maxChars }) => {
      const s = await new ContextManager({ page, maxChars }).snapshot();
      return { pageId: s.pageId, pageTitle: s.pageTitle, text: s.text, chars: s.chars, totalChars: s.totalChars, truncated: s.truncated, hash: s.hash, empty: s.empty };
    },
  },
  {
    suite: 'plan-turn',
    description: 'planTurn and contextState: does the next question carry a snapshot, and what does the flag show? messages is the transcript before the question; a message carries a snapshot { hash, text? } (text absent = a stub). The window is windowStart(n) = max(0, n - max(2, historyMessages)).',
    operation: 'planTurn(input) -> { attach, reason, syncedHash }; contextState(input) -> { state, pending }; windowStart(messages.length + 1, historyMessages)',
    cases: (() => {
      const snap = (hash, text = 'page') => ({ hash, text });
      const now = { hash: 'aaaaaaaaaaaaaa', text: 'page', empty: false };
      return [
        { name: 'sharing off', input: { messages: [], snapshot: now, share: false } },
        { name: 'empty screen', input: { messages: [], snapshot: { hash: 'e', text: '', empty: true } } },
        { name: 'first question: unread', input: { messages: [], snapshot: now } },
        { name: 'unchanged', input: { messages: [{ role: 'user', snapshot: snap('aaaaaaaaaaaaaa') }, { role: 'assistant' }], snapshot: now } },
        { name: 'changed', input: { messages: [{ role: 'user', snapshot: snap('bbbbbbbbbbbbbb') }, { role: 'assistant' }], snapshot: now } },
        { name: 'forced re-read', input: { messages: [{ role: 'user', snapshot: snap('aaaaaaaaaaaaaa') }, { role: 'assistant' }], snapshot: now, force: true } },
        { name: 'a stub (no text) does not count', input: { messages: [{ role: 'user', snapshot: { hash: 'aaaaaaaaaaaaaa' } }, { role: 'assistant' }], snapshot: now } },
        { name: 'trimmed out of the window', input: { messages: [{ role: 'user', snapshot: snap('aaaaaaaaaaaaaa') }, { role: 'assistant' }, { role: 'user' }, { role: 'assistant' }], snapshot: now, historyMessages: 2 } },
        { name: 'newest snapshot wins', input: { messages: [{ role: 'user', snapshot: snap('bbbbbbbbbbbbbb') }, { role: 'assistant' }, { role: 'user', snapshot: snap('aaaaaaaaaaaaaa') }, { role: 'assistant' }], snapshot: now } },
      ];
    })(),
    run: (input) => ({ plan: planTurn(input), state: contextState(input), windowStart: windowStart((input.messages || []).length + 1, input.historyMessages) }),
  },
  {
    suite: 'snapshot-wire',
    description: 'The wire format of a snapshot inside a user turn, and the one-line stub that replaces an older snapshot.',
    operation: 'snapshotBlock(input.snapshot) -> string; supersededStub(input.snapshot) -> string',
    cases: [
      { name: 'plain', input: { snapshot: { pageId: 'editor', pageTitle: 'Editor', text: 'Hello.', chars: 6, totalChars: 6, truncated: false, hash: '0123456789abcd', at: '2026-01-02T03:04:05.000Z' } } },
      { name: 'truncated, quotes in the title, closing tag inside', input: { snapshot: { pageId: 'log', pageTitle: 'The "log"', text: 'a </page_snapshot> b', chars: 20, totalChars: 900, truncated: true, hash: 'fedcba98765432', at: '2026-01-02T03:04:05.000Z' } } },
    ],
    run: ({ snapshot }) => ({ block: snapshotBlock(snapshot), stub: supersededStub(snapshot) }),
  },
  {
    suite: 'system-prompt',
    description: 'buildSystemPrompt: the section order and the fixed protocol texts, verbatim. A runtime embeds the same texts.',
    operation: 'buildSystemPrompt(input) -> string',
    cases: [
      { name: 'defaults only', input: {} },
      { name: 'app, page, sharing on', input: { base: 'You help with stock.', appText: 'Name: Inventory', pageText: 'Id: items\nTitle: Items' } },
      { name: 'sharing off', input: { base: 'B', appText: 'Name: X', share: false } },
      { name: 'every section', input: { base: 'B', agentText: '== AGENT: Proofreader ==\nCheck spelling.', appText: 'Name: X', pageText: 'Id: p', files: true, vision: true, memoryText: '== MEMORY ==\n- m1: note', skillsText: '== SKILLS ==\n- s: d', toolsText: '== TOOLS ==\n- t' } },
      { name: 'vision without screenshots', input: { base: 'B', vision: true, screenshots: false } },
    ],
    run: (input) => buildSystemPrompt(input),
  },
  {
    suite: 'tool-contract',
    description: 'normalizeTool and toMcpTool: a tool definition (handler left out) becomes the runtime\'s contract and its MCP descriptor, or is refused with the reason. Effects come from "effect", else MCP annotations, else "write".',
    operation: 'normalizeTool({ ...input.tool, run }) -> { tool (functions left out), mcp: toMcpTool(tool) } | { error }',
    cases: [
      { name: 'shorthand parameters', input: { tool: { name: 'filter_orders', description: 'Show only orders with this status.', effect: 'write', parameters: { status: { type: 'enum', values: ['open', 'shipped'], required: true }, limit: { type: 'integer', min: 1, max: 50, default: 10 } } } } },
      { name: 'JSON Schema input', input: { tool: { name: 'set_size', description: 'Font size.', effect: 'write', inputSchema: { type: 'object', properties: { size: { type: 'integer', minimum: 11, maximum: 24, description: 'px' }, wrap: { type: 'boolean' } }, required: ['size'] } } } },
      { name: 'effect from annotations', input: { tool: { name: 'peek', description: 'Look.', annotations: { readOnlyHint: true } } } },
      { name: 'no effect: write', input: { tool: { name: 'poke', description: 'Change.' } } },
      { name: 'external', input: { tool: { name: 'send_mail', description: 'Send.', effect: 'external', parameters: { to: { type: 'string', maxLength: 200, required: true } } } } },
      { name: 'system', input: { tool: { name: 'write_file', description: 'Write.', effect: 'system' } } },
      { name: 'oneOf refused', input: { tool: { name: 'bad', description: 'x', inputSchema: { type: 'object', properties: { a: { oneOf: [{ type: 'string' }] } } } } } },
      { name: 'invalid name refused', input: { tool: { name: '9lives', description: 'x' } } },
      { name: 'reserved name refused', input: { tool: { name: 'request_tool', description: 'x' } } },
      { name: 'missing description refused', input: { tool: { name: 'quiet' } } },
    ],
    run: ({ tool }) => {
      try {
        const t = normalizeTool({ ...tool, run: noRun });
        return { tool: contract(t), mcp: toMcpTool(t) };
      } catch (e) {
        return { error: String(e.message) };
      }
    },
  },
  {
    suite: 'tool-arguments',
    description: 'readArguments (what the model sent, as text or object) and validateArgs (checked and coerced against the tool\'s parameters). The tool loop refuses a call whose read.error is set before it validates anything, so a cut-off call never runs on guessed arguments (validateArgs on its own would see an empty object). Text over maxLength is an error, never cut; arrays keep their first maxItems items.',
    operation: 'readArguments(input.raw) -> { args, error }; validateArgs(normalizeTool(input.tool), input.raw) -> { ok, args, errors }',
    cases: (() => {
      const tool = { name: 't', description: 'd', parameters: {
        n: { type: 'integer', min: 1, max: 10 },
        x: { type: 'number', min: 0, max: 1, step: 0.25 },
        s: { type: 'string', maxLength: 5 },
        mode: { type: 'enum', values: ['Fast', 'slow-mode'] },
        on: { type: 'boolean' },
        tags: { type: 'array', items: { type: 'string' }, maxItems: 2 },
        need: { type: 'string', required: false, default: 'dflt' },
      } };
      const req = { name: 'r', description: 'd', parameters: { id: { type: 'string', required: true } } };
      return [
        { name: 'object as is', input: { tool, raw: { n: 3 } } },
        { name: 'JSON text', input: { tool, raw: '{"n": 3, "on": "yes"}' } },
        { name: 'trailing comma', input: { tool, raw: '{"n": 3,}' } },
        { name: 'loose key: value lines', input: { tool, raw: 'n: 4\nmode: fast' } },
        { name: 'unclosed JSON is cut off', input: { tool, raw: '{"n": 3, "s": "ab' } },
        { name: 'clamp and snap', input: { tool, raw: { n: 99, x: 0.6 } } },
        { name: 'integer rounds', input: { tool, raw: { n: 2.6 } } },
        { name: 'case-insensitive key', input: { tool, raw: { N: 5 } } },
        { name: 'enum matched loosely', input: { tool, raw: { mode: 'SLOW MODE' } } },
        { name: 'enum not matched', input: { tool, raw: { mode: 'medium' } } },
        { name: 'too long is an error', input: { tool, raw: { s: 'abcdefgh' } } },
        { name: 'booleans in words', input: { tool, raw: { on: 'off' } } },
        { name: 'array capped', input: { tool, raw: { tags: ['a', 'b', 'c'] } } },
        { name: 'unknown keys dropped', input: { tool, raw: { zzz: 1, n: 1 } } },
        { name: 'missing required', input: { tool: req, raw: {} } },
        { name: 'empty string counts as missing', input: { tool: req, raw: { id: '' } } },
      ];
    })(),
    run: ({ tool, raw }) => ({ read: readArguments(raw), validated: validateArgs(normalizeTool({ ...tool, run: noRun }), raw) }),
  },
  {
    suite: 'tool-results',
    description: 'serializeResult: what a tool\'s return value becomes for the model. undefined/null/true -> "Done."; strings as written; anything else as compact stableStringify; capped with a note.',
    operation: 'serializeResult(input.value, input.max) -> string',
    cases: [
      { name: 'null', input: { value: null } },
      { name: 'true', input: { value: true } },
      { name: 'false', input: { value: false } },
      { name: 'string', input: { value: 'Showing 3 open orders.' } },
      { name: 'object', input: { value: { b: [1, 2], a: 'x' } } },
      { name: 'number', input: { value: 42 } },
      { name: 'capped', input: { value: long(120), max: 50 } },
    ],
    run: ({ value, max }) => serializeResult(value, max),
  },
  {
    suite: 'permissions',
    description: 'Permission rules (tool name, * glob, toolset:<glob>, effect:<glob>) merged from index, options and the active agent; deny > ask > allow > the user\'s settings; system always confirms.',
    operation: 'decision = decidePermission(tool, mergePolicies(...input.policies.map(normalizePolicy))); confirm = needsConfirmation({ tool, decision, settings: input.settings, allowedForChat: input.allowedForChat })',
    cases: (() => {
      const tool = (name, effect, toolset = '') => ({ name, effect, ...(toolset ? { toolset } : {}) });
      return [
        { name: 'read: no rule, no card', input: { tool: tool('find', 'read'), policies: [] } },
        { name: 'write: confirm by default', input: { tool: tool('set_x', 'write'), policies: [] } },
        { name: 'write: confirmations off', input: { tool: tool('set_x', 'write'), policies: [], settings: { confirmWrites: false } } },
        { name: 'write: allowed for this chat', input: { tool: tool('set_x', 'write'), policies: [], allowedForChat: true } },
        { name: 'destructive follows confirmDestructive', input: { tool: tool('del', 'destructive'), policies: [], settings: { confirmDestructive: false } } },
        { name: 'external confirms', input: { tool: tool('send', 'external'), policies: [] } },
        { name: 'system always confirms, even allowed', input: { tool: tool('write_file', 'system'), policies: [{ allow: ['write_file'] }] } },
        { name: 'ask beats settings for a read tool', input: { tool: tool('peek', 'read', 'file'), policies: [{ ask: ['toolset:file'] }] } },
        { name: 'allow skips the card', input: { tool: tool('set_x', 'write'), policies: [{ allow: ['set_*'] }] } },
        { name: 'deny beats allow (agent adds deny)', input: { tool: tool('insert_text', 'write', 'document'), policies: [{ allow: ['toolset:document'] }, { deny: ['insert_text'] }] } },
        { name: 'ask beats allow', input: { tool: tool('set_x', 'write'), policies: [{ allow: ['set_x'], ask: ['effect:write'] }] } },
        { name: 'effect glob', input: { tool: tool('drop', 'destructive'), policies: [{ deny: ['effect:destr*'] }] } },
        { name: 'string lists', input: { tool: tool('a_b', 'write'), policies: [{ deny: 'x, a_*' }] } },
      ];
    })(),
    run: ({ tool, policies = [], settings = {}, allowedForChat = false }) => {
      const policy = mergePolicies(...policies.map((p) => normalizePolicy(p)));
      const decision = decidePermission(tool, policy);
      return { decision: decision || '', confirm: decision === 'deny' ? null : needsConfirmation({ tool, decision, settings, allowedForChat }) };
    },
  },
  {
    suite: 'effects-annotations',
    description: 'Effects <-> MCP annotations, both ways.',
    operation: 'annotationsFor(input.effect) and effectFromAnnotations(input.annotations)',
    cases: [
      ...['read', 'write', 'destructive', 'external', 'system'].map((effect) => ({ name: `annotations for ${effect}`, input: { effect } })),
      { name: 'readOnly -> read', input: { annotations: { readOnlyHint: true } } },
      { name: 'destructive -> destructive', input: { annotations: { destructiveHint: true } } },
      { name: 'openWorld -> external', input: { annotations: { openWorldHint: true } } },
      { name: 'readOnly false -> write', input: { annotations: { readOnlyHint: false } } },
      { name: 'nothing -> none', input: { annotations: {} } },
    ],
    run: ({ effect, annotations }) => (effect ? { annotations: annotationsFor(effect) } : { effect: effectFromAnnotations(annotations) || '' }),
  },
  {
    suite: 'frontmatter',
    description: 'The YAML subset of skill and agent frontmatter: scalars, flow and block lists, nested mappings, quoted strings, comments; what it cannot read is reported, never guessed.',
    operation: 'parseFrontmatter(input.text) -> { data, body, errors }',
    cases: [
      { name: 'scalars and lists', input: { text: '---\nname: proofreader\nmaxToolSteps: 12\ndefault: true\ntools: [find_text, replace_text]\nskills:\n  - proofreading\n---\nYou proofread.\n' } },
      { name: 'quoted strings and a comment', input: { text: '---\ndescription: "Fix: spelling and grammar" # why\ntitle: \'It\'\'s me\'\n---\nBody' } },
      { name: 'nested mapping', input: { text: '---\npermissions:\n  deny: [insert_text]\n  ask:\n    - toolset:file\nmetadata:\n  app: hello\n---\n' } },
      { name: 'folded description', input: { text: '---\ndescription: >\n  Line one\n  line two.\n---\nB' } },
      { name: 'no frontmatter', input: { text: '# Just Markdown' } },
      { name: 'unreadable line is reported', input: { text: '---\nname: x\n: oops\n---\nB' } },
    ],
    run: ({ text }) => parseFrontmatter(text),
  },
  {
    suite: 'skills',
    description: 'Agent Skills: SKILL.md parsed and checked (name = folder, lowercase-hyphen, description required, body required, allowed-tools read as information); safe skill file paths; /name commands.',
    operation: 'parseSkill(input.text, { folder: input.folder }) -> { skill (base left out), problems }; skillFilePath(path); parseSlashCommand(text, names)',
    cases: [
      { name: 'valid skill', input: { folder: 'proofreading', text: '---\nname: proofreading\ndescription: Proofread the document. Use when asked to check the text.\nallowed-tools: find_text replace_text\nlicense: MIT\n---\n# Proofreading\n1. Read.\n2. Fix.' } },
      { name: 'name differs from folder', input: { folder: 'proof', text: '---\nname: proofreading\ndescription: d\n---\nBody' } },
      { name: 'bad name and missing description', input: { folder: 'Bad_Name', text: '---\nname: Bad_Name\n---\nBody' } },
      { name: 'empty body', input: { folder: 'x', text: '---\nname: x\ndescription: d\n---\n' } },
      { name: 'paths', input: { paths: ['references/checklist.md', '../secret.md', '.hidden/x.md', 'a/./b.md', '/abs.md', 'notes.exe'] } },
      { name: 'slash commands', input: { commands: ['/proofreading fix this', '/unknown hi', 'no slash', '/Proofreading caps'], names: ['proofreading', 'summarize'] } },
    ],
    run: ({ text, folder, paths, commands, names }) => {
      if (paths) return paths.map((p) => { try { return { path: p, ok: skillFilePath(p) }; } catch (e) { return { path: p, error: String(e.message) }; } });
      if (commands) return commands.map((c) => ({ text: c, command: parseSlashCommand(c, names) ?? null }));
      const r = parseSkill(text, { folder });
      if (r.skill) delete r.skill.base;
      return r;
    },
  },
  {
    suite: 'agents',
    description: 'Agent files (Markdown + YAML frontmatter): fields, defaults (omitted lists = all), context layers, memory modes, limits; which agent starts.',
    operation: 'parseAgent(input.text, { file: input.file }) -> { agent, problems }; pickAgent(agents, wanted) -> name',
    cases: [
      { name: 'full agent', input: { file: 'agents/proofreader.md', text: '---\nname: proofreader\ntitle: Proofreader\ndescription: Checks spelling.\ntoolsets: [document]\ntools: [rename_file]\nskills: [proofreading]\npermissions:\n  deny: [insert_text]\ncontext: [app, page, screen]\nmemory: read\nmaxToolSteps: 12\nmodel: qwen/qwen3-8b\nwelcome: Hi.\nsuggestions: [Proofread the document]\ndefault: true\n---\nYou are a careful proofreader.' } },
      { name: 'minimal: name from the file', input: { file: 'agents/writer.md', text: '---\ndescription: Writes.\n---\nYou write.' } },
      { name: 'bad values reported', input: { file: 'agents/x.md', text: '---\nname: x\ndescription: d\nmemory: sometimes\nmaxToolSteps: 99\ncontext: [app, mind]\ncolour: blue\n---\nI.' } },
      { name: 'missing description', input: { file: 'agents/y.md', text: '---\nname: y\n---\nI.' } },
      { name: 'pick: wanted, else default, else first', input: { pick: [{ agents: ['a', 'b*', 'c'], wanted: 'c' }, { agents: ['a', 'b*', 'c'], wanted: '' }, { agents: ['a', 'c'], wanted: 'zzz' }] } },
    ],
    run: ({ text, file, pick }) => {
      if (pick) return pick.map(({ agents, wanted }) => pickAgent(agents.map((n) => ({ name: n.replace('*', ''), default: n.endsWith('*') })), wanted)?.name ?? null);
      return parseAgent(text, { file });
    },
  },
  {
    suite: 'memory',
    description: 'Memory notes: the app\'s ai-memory.json read into notes (valid ids kept, others numbered m<n>; one line; 500 chars; 100 notes; source defaults to app; created defaults to now), and the MEMORY prompt section. The case-insensitive duplicate check belongs to adding a note (remember), not to reading the file.',
    operation: 'parseMemoryFile(input.json, { now }) -> notes; buildMemoryPrompt({ items, enabled, canWrite }) -> string; memoryText(input.text) -> string',
    cases: [
      { name: 'file with ids and sources', input: { json: { version: 1, memories: [{ id: 'm1', text: 'Ctrl+S saves.', created: '2026-01-01T00:00:00.000Z', source: 'app' }, { id: 'bad id!', text: 'Two\nlines here.', created: '2026-01-01T00:00:00.000Z' }, { text: 'ctrl+s SAVES.' }] } } },
      { name: 'plain list of strings', input: { json: ['First note.', '', '  Second   note.  '] } },
      { name: 'one long note is cut', input: { text: long(600) } },
      { name: 'prompt: read and write', input: { prompt: { items: [{ id: 'm1', text: 'Ctrl+S saves.' }, { id: 'm2', text: 'Units are mm.' }], enabled: true, canWrite: true } } },
      { name: 'prompt: read only', input: { prompt: { items: [{ id: 'm1', text: 'Ctrl+S saves.' }], enabled: true, canWrite: false } } },
      { name: 'prompt: off', input: { prompt: { items: [{ id: 'm1', text: 'x' }], enabled: false } } },
    ],
    run: ({ json, text, prompt }) => {
      if (prompt) return buildMemoryPrompt(prompt);
      if (text !== undefined) return memoryText(text);
      return parseMemoryFile(json, { now: () => Date.parse('2026-06-01T00:00:00.000Z') });
    },
  },
];

/** Run one suite over its cases (or over cases read back from a vector file). */
export async function runSuite(suite, cases = suite.cases) {
  const out = [];
  for (const c of cases) {
    const value = await suite.run(structuredClone(c.input));
    out.push({ name: c.name, input: c.input, output: JSON.parse(JSON.stringify(value === undefined ? null : value)) });
  }
  return out;
}
