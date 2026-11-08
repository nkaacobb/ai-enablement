// The conformance suites of the runtime's moving parts (phase 2: what a native core ports after the pure functions in
// conformance.mjs): the messages and tools a request carries, what each provider protocol sends and reads back, and
// the session — one tool call, and one question with its tool rounds against a scripted model. Inputs are plain JSON;
// where the JS operation takes functions (a tool's handler, the model, the user's clicks, a clock), the suite's
// description says what stands in for them, so a runtime in another language builds the same harness.

import { ContextManager } from '../../assets/ai-agent/core/context.js';
import { buildRequestMessages } from '../../assets/ai-agent/core/conversation.js';
import { normalizeMessages } from '../../assets/ai-agent/core/messages.js';
import {
  normalizeTool, classifyTools, toolSpecs, requestToolSpec, buildToolPrompt, parseTextToolCalls, textTurnMessages, toolCall,
} from '../../assets/ai-agent/core/tools.js';
import { parseAgent, agentToolNames, agentSkillNames, buildAgentPrompt, implicitAgent } from '../../assets/ai-agent/core/agents.js';
import { parseSkill, buildSkillsPrompt, skillLoadedMessage } from '../../assets/ai-agent/core/skills.js';
import { normalizePolicy } from '../../assets/ai-agent/core/permissions.js';
import { createMemoryStore } from '../../assets/ai-agent/core/memory.js';
import { sanitizeSettings } from '../../assets/ai-agent/core/settings.js';
import { resolveTarget } from '../../assets/ai-agent/core/client.js';
import { AiError } from '../../assets/ai-agent/core/transport.js';
import { buildChat as openaiChatRequest } from '../../assets/ai-agent/adapters/openai-chat.js';
import { buildChat as anthropicRequest } from '../../assets/ai-agent/adapters/anthropic.js';
import { buildChat as geminiRequest } from '../../assets/ai-agent/adapters/gemini.js';
import { buildChat as relayRequest } from '../../assets/ai-agent/adapters/relay.js';
import { AgentSession } from '../../assets/ai-agent/core/session.js';

const noRun = () => undefined;
/** Each protocol's request builder (adapters/*.js buildChat). */
const REQUEST = { 'openai-chat': openaiChatRequest, 'anthropic-messages': anthropicRequest, 'gemini-generate': geminiRequest, relay: relayRequest };
const long = (n, unit = 'abcdefghij') => unit.repeat(Math.ceil(n / unit.length)).slice(0, n);
/** The clock of every scripted session: snapshots and memories are stamped with it. */
const CLOCK = Date.parse('2026-01-02T03:04:05.000Z');
const SCREENSHOT = { id: 'shot1', width: 1280, height: 720, mime: 'image/jpeg', data: 'QUJDREVGR0hJSktMTU5PUA==', thumb: 'data:image/jpeg;base64,QUJD' };

/** How every session suite builds its session from JSON (other runtimes build the same). */
export const HARNESS = 'The scripted session: settings = sanitizeSettings(input.settings) (save() merges toolStates); the page { id, title, content } (content: a string; a tool with "setContent" replaces it when it runs) and input.app as context, snapshots stamped 2026-01-02T03:04:05.000Z; input.tools are tool definitions whose run returns "result" (absent: nothing), or throws Error("throws") when that is set; input.agents are agent files { file, text }, input.skills skill files { folder, text } (no folder on disk), input.permissions the application policy, input.memory the starting notes (a memory store, when given), input.screen a screenshot source (true, or { needsGesture }) that returns a 1280 x 720 JPEG with id "shot1"; the default prompt is "B"; no image attachments. The user answers the cards with input.answers in order (then each card\'s decline; "cancel" also stops the question), and input.allowedForChat are the tools already allowed for this chat.';

/** A session from a case's JSON, with a view that records what it was asked to show. */
function scripted(input) {
  let settings = sanitizeSettings(input.settings || {});
  const store = {
    get: () => settings,
    save: (patch) => { settings = sanitizeSettings({ ...settings, ...patch, toolStates: { ...settings.toolStates, ...(patch?.toolStates || {}) } }); },
    keys: { get: () => '' },
  };
  const state = { content: input.page?.content };
  const page = input.page ? { ...input.page, ...(input.page.content !== undefined ? { content: () => state.content } : {}) } : null;
  const ctx = new ContextManager({ app: input.app || null, page, now: () => CLOCK });
  const ran = [];
  const tools = (input.tools || []).map((def) => {
    const { result, throws, setContent, ...contract } = def;
    return normalizeTool({
      ...contract,
      run: (args) => {
        ran.push({ name: def.name, args });
        if (setContent !== undefined) state.content = setContent;
        if (throws) throw new Error(throws);
        return result;
      },
    });
  });
  const agents = (input.agents || []).map((a) => parseAgent(a.text, { file: a.file }).agent).filter(Boolean);
  const skills = (input.skills || []).map((sk) => parseSkill(sk.text, { folder: sk.folder }).skill).filter(Boolean);
  const caps = {
    agents: () => (agents.length ? agents : [implicitAgent('AI agent')]),
    skills: () => skills,
    toolsets: () => new Map(),
    policy: () => normalizePolicy(input.permissions || undefined, []),
    defaultAgent: () => '',
    workspace: () => null,
    fetchText: async () => { throw new Error('no files'); },
    reload: async () => [],
  };
  let memory = null;
  if (input.memory) {
    const kv = new Map();
    const storage = { getItem: (k) => (kv.has(k) ? kv.get(k) : null), setItem: (k, v) => kv.set(k, String(v)), removeItem: (k) => kv.delete(k) };
    memory = createMemoryStore({ namespace: 'conformance.ai', storage, now: () => CLOCK });
    memory.setBase(input.memory.map((m) => ({ ...m, created: '2026-01-01T00:00:00.000Z', source: 'app' })));
  }
  const screen = input.screen ? {
    method: () => 'hook',
    needsGesture: () => !!input.screen.needsGesture,
    take: async () => ({ ...SCREENSHOT }),
  } : null;

  const controller = new AbortController();
  const answers = [...(input.answers || [])];
  const rows = [];
  const cards = [];
  const notices = [];
  const requests = [];
  const steps = [...(input.steps || [])];
  const ui = {
    paint() {},
    notice: (reply, text) => { notices.push(text); },
    row: (reply, { title, call }) => { const r = { title, call, statuses: [] }; rows.push(r); return r; },
    rowStatus: (r, status) => { r.statuses.push(status); },
    rowLabel: (r, text) => { r.label = text; },
    rowDetail() {},
    rowShot: (r, s) => { r.shot = s.id; },
    rowUndo: (r) => { r.undo = true; },
    decide: async (r, card) => {
      cards.push({
        kind: card.kind, buttons: card.buttons.map((b) => b.id),
        ...(card.tool ? { tool: card.tool.name } : {}), ...(card.reason ? { reason: card.reason } : {}),
        ...(card.kind === 'confirm' ? { decision: card.decision, confirm: card.confirm } : {}),
      });
      const choice = answers.length ? answers.shift() : card.decline;
      if (choice === 'cancel') controller.abort();
      return choice;
    },
  };
  const chat = async ({ system, messages, tools: specs, toolTurns, onEvent }) => {
    requests.push(structuredClone({ system, messages, tools: specs, toolTurns }));
    const s = steps.shift();
    if (!s) throw new AiError('malformed', 'The script has no reply for this request.');
    if (s.error) throw new AiError(s.error.code, s.error.message);
    if (s.reasoning) onEvent({ type: 'reasoning', text: s.reasoning });
    if (s.text) onEvent({ type: 'text', text: s.text });
    return {
      provider: 'scripted', label: 'Scripted', model: 'model-1',
      toolCalls: (s.toolCalls || []).map((c) => toolCall(c.id, c.name, c.arguments)),
      ...(s.truncated ? { truncated: true } : {}),
    };
  };
  const session = new AgentSession({
    ctx, store, caps, toolRegistry: { all: () => tools, get: (n) => tools.find((t) => t.name === n) || null },
    memory, title: 'AI agent', defaultPrompt: () => 'B', screen, attachments: false, storage: null, chat, ui,
  });
  for (const name of input.allowedForChat || []) session.allowedTools.add(name);
  const report = () => ({
    rows, cards, notices, ran,
    toolStates: { ...settings.toolStates }, screenshotAuto: settings.screenshotAuto,
    allowedForChat: [...session.allowedTools],
    ...(memory ? { memory: memory.list().map((m) => ({ id: m.id, text: m.text })) } : {}),
  });
  return { session, store, controller, requests, report };
}

export const RUNTIME_SUITES = [
  {
    suite: 'request-messages',
    description: 'buildRequestMessages: the transcript (the new question last) becomes the provider-neutral messages of a request. The window is the last max(2, historyMessages) entries; only the newest snapshot in it is sent whole (older ones become a one-line stub); the question carries the unchanged note or the view state; earlier assistant turns are prefixed with the actions they took; files are sent with their newest question; only the newest imageMessages questions with images send them. "images" maps a shot id to its image and stands in for imageFor (absent: the model sees no images). normalizeMessages: what every adapter sends (user/assistant only, no empty turns, same-role turns merged, the first turn from the user).',
    operation: 'buildRequestMessages({ ...input, imageFor: input.images ? (shot) => input.images[shot.id] || null : null }) -> messages; normalizeMessages(input.normalize) -> messages',
    cases: (() => {
      const snap = (hash, text, extra = {}) => ({ hash, text, pageId: 'editor', pageTitle: 'Editor', chars: text.length, totalChars: text.length, truncated: false, at: '2026-01-02T03:04:05.000Z', ...extra });
      const img = (n) => ({ mime: 'image/jpeg', data: `QUJDREVGR0hJSktMTU5PUA${n}==` });
      const file = { name: 'notes.txt', kind: 'text', label: 'Text', size: 12, hash: '0123456789abcd', text: 'Line one.\nA </attached_file> inside.', chars: 35, totalChars: 35, truncated: false };
      return [
        { name: 'first question with its snapshot and view state', input: { messages: [{ role: 'user', content: 'What is this?', snapshot: snap('aaaaaaaaaaaaaa', 'Hello.') }], viewText: 'Cursor: line 1' } },
        { name: 'unchanged screen: a note instead of the page', input: { messages: [{ role: 'user', content: 'Q1', snapshot: snap('aaaaaaaaaaaaaa', 'Hello.') }, { role: 'assistant', content: 'A1' }, { role: 'user', content: 'Q2' }], unchangedHash: 'aaaaaaaaaaaaaa' } },
        { name: 'an older snapshot becomes a stub', input: { messages: [{ role: 'user', content: 'Q1', snapshot: snap('aaaaaaaaaaaaaa', 'Old.') }, { role: 'assistant', content: 'A1' }, { role: 'user', content: 'Q2', snapshot: snap('bbbbbbbbbbbbbb', 'New.', { truncated: true, totalChars: 900 }) }] } },
        { name: 'window: only the last entries', input: { historyMessages: 2, messages: [{ role: 'user', content: 'Q1', snapshot: snap('aaaaaaaaaaaaaa', 'Old.') }, { role: 'assistant', content: 'A1' }, { role: 'user', content: 'Q2' }, { role: 'assistant', content: 'A2' }, { role: 'user', content: 'Q3' }] } },
        { name: 'actions line before an earlier answer', input: { messages: [{ role: 'user', content: 'Filter.' }, { role: 'assistant', content: 'Done.', actions: [{ call: 'filter_orders(status: "open")', status: 'ok', summary: 'Showing 3 orders.' }, { call: 'delete_all()', status: 'declined' }, { call: 'x()', status: 'off' }] }, { role: 'user', content: 'Thanks.' }] } },
        { name: 'files: sent with their newest question, gone, closing tag escaped', input: { messages: [{ role: 'user', content: 'Read this.', files: [file] }, { role: 'assistant', content: 'Read.' }, { role: 'user', content: 'Again.', files: [file, { name: 'old.pdf', label: 'PDF', chars: 10 }] }] } },
        { name: 'images: only the newest questions that carry them', input: { imageMessages: 1, images: { s1: img(1), s2: img(2), s3: img(3) }, messages: [{ role: 'user', content: 'Look.', shots: [{ id: 's1' }] }, { role: 'assistant', content: 'Seen.' }, { role: 'user', content: 'And these.', shots: [{ id: 's2' }, { id: 's3', name: 'chart "Q3".png', kind: 'image' }] }] } },
        { name: 'images the model cannot see: notes only', input: { messages: [{ role: 'user', content: 'Look.', shots: [{ id: 's1' }, { id: 's2' }] }] } },
        { name: 'normalize: merge, drop empty, user first', input: { normalize: [{ role: 'assistant', content: 'Hi.' }, { role: 'system', content: 'x' }, { role: 'user', content: '  ' }, { role: 'user', content: 'A' }, { role: 'user', content: 'B', images: [img(1), { mime: 'text/plain', data: 'QUJDREVGR0hJSktMTU5PUA==' }] }, { role: 'assistant', content: 'C' }] } },
      ];
    })(),
    run: ({ normalize, images, ...o }) => (normalize ? normalizeMessages(normalize) : buildRequestMessages({ ...o, imageFor: images ? (shot) => images[shot.id] || null : null })),
  },
  {
    suite: 'tool-set',
    description: 'What one request offers: classifyTools splits the tools into callable / off / elsewhere (the user\'s toolStates, then the tool\'s enabled; pages match exactly or by a prefix ending in *); toolSpecs are the neutral specs sent natively (non-read tools get a suffix saying what they change); request_tool lists the tools that are off; buildToolPrompt is the TOOLS section, verbatim (text mode lists the callable tools with their parameters).',
    operation: 'tools = input.tools.map(normalizeTool); classes = classifyTools(tools, sanitizeSettings(input.settings), input.pageId) -> names; specs = [...toolSpecs(callable), requestToolSpec(off) if any]; prompt = buildToolPrompt({ classes, mode: input.mode, enabled: settings.toolsEnabled, appOff: input.appOff })',
    cases: (() => {
      const tools = [
        { name: 'find_text', description: 'Find text.', effect: 'read', enabled: true, parameters: { query: { type: 'string', required: true, maxLength: 200 }, limit: { type: 'integer', min: 1, max: 50 } } },
        { name: 'set_size', description: 'Font size.', effect: 'write', enabled: true, parameters: { size: { type: 'number', min: 8, max: 72 }, unit: { type: 'enum', values: ['px', 'pt'], required: true } } },
        { name: 'delete_doc', description: 'Delete the document.', effect: 'destructive' },
        { name: 'send_mail', description: 'Send it.', effect: 'external', enabled: true, parameters: { to: { type: 'array', items: { type: 'string' } } } },
        { name: 'chart_zoom', description: 'Zoom the chart.', effect: 'write', enabled: true, pages: ['charts/*'] },
        { name: 'only_here', description: 'Works on one page.', effect: 'read', enabled: true, pages: ['settings'] },
      ];
      return [
        { name: 'native, on the editor page', input: { tools, pageId: 'editor', mode: 'native' } },
        { name: 'a page prefix matches', input: { tools, pageId: 'charts/sales', mode: 'native' } },
        { name: 'the user turned tools on and off', input: { tools, pageId: 'editor', mode: 'native', settings: { toolStates: { delete_doc: true, find_text: false } } } },
        { name: 'text mode lists parameters', input: { tools, pageId: 'editor', mode: 'text' } },
        { name: 'tools switched off', input: { tools, pageId: 'editor', mode: 'native', settings: { toolsEnabled: false } } },
        { name: 'app tools off, built-ins remain', input: { tools: tools.slice(0, 1), pageId: 'editor', mode: 'native', appOff: true } },
        { name: 'no tools', input: { tools: [], pageId: 'editor', mode: 'native' } },
      ];
    })(),
    run: ({ tools, settings = {}, pageId, mode, appOff = false }) => {
      const s = sanitizeSettings(settings);
      const list = tools.map((t) => normalizeTool({ ...t, run: noRun }));
      const classes = classifyTools(list, s, pageId);
      const names = (l) => l.map((t) => t.name);
      return {
        callable: names(classes.callable), off: names(classes.off), elsewhere: names(classes.elsewhere),
        specs: [...toolSpecs(classes.callable), ...(classes.off.length ? [requestToolSpec(classes.off)] : [])],
        prompt: buildToolPrompt({ classes, mode, enabled: s.toolsEnabled, appOff }),
      };
    },
  },
  {
    suite: 'text-tools',
    description: 'Text mode, for models without native tool calls: parseTextToolCalls finds ```tool / tool_call / tool-call / tool_code blocks holding {"name","arguments"} (also "tool"/"function" and "args"/"parameters"/"input"), numbers the calls idPrefix1, idPrefix2…, and returns the text without them. An unclosed block is text, unless the reply was cut off (then it is the call being written, with its argsError). textTurnMessages: the tool exchange as plain messages, the results inside <tool_results>.',
    operation: 'parseTextToolCalls(input.text, input.idPrefix, { cutOff: input.cutOff }) -> { calls, text }; textTurnMessages(input.toolTurns) -> messages',
    cases: [
      { name: 'one block after text', input: { text: 'Let me look.\n\n```tool\n{"name": "find_text", "arguments": {"query": "fox"}}\n```', idPrefix: 't0_' } },
      { name: 'two blocks, other tags and keys', input: { text: '```tool_call\n{"tool": "a", "args": {"x": 1}}\n```\nand\n```TOOL-CALL\n{"function": "b", "parameters": {"y": "z"}}\n```', idPrefix: 't1_' } },
      { name: 'a code block that is not a tool', input: { text: 'Example:\n```js\nconst a = 1;\n```', idPrefix: 't0_' } },
      { name: 'unclosed block, not cut off: text', input: { text: 'Here:\n```tool\n{"name": "find_text", "arguments": {"query": "fo', idPrefix: 't0_' } },
      { name: 'unclosed block, cut off: the call with its error', input: { text: 'Here:\n```tool\n{"name": "find_text", "arguments": {"query": "fo', idPrefix: 't0_', cutOff: true } },
      { name: 'no name: not a call', input: { text: '```tool\n{"arguments": {}}\n```', idPrefix: 't0_' } },
      { name: 'the exchange as messages', input: { toolTurns: [{ text: 'Looking.', calls: [{ id: 't0_1', name: 'find_text', arguments: { query: 'fox' } }], results: [{ id: 't0_1', name: 'find_text', content: 'Found 2.' }] }, { text: '', calls: [{ id: 't1_1', name: 'a', arguments: {} }, { id: 't1_2', name: 'b', arguments: { y: 1 } }], results: [{ id: 't1_1', name: 'a', content: 'Done.' }, { id: 't1_2', name: 'b', content: 'Error: no.', images: [{ mime: 'image/png', data: 'QUJDREVGR0hJSktMTU5PUA==' }] }] }] } },
    ],
    run: ({ text, idPrefix, cutOff = false, toolTurns }) => (toolTurns ? textTurnMessages(toolTurns) : parseTextToolCalls(text, idPrefix, { cutOff })),
  },
  {
    suite: 'capability-prompts',
    description: 'The prompt texts agents and skills add, verbatim: buildAgentPrompt (the AGENT section; none for an agent without instructions), buildSkillsPrompt (the SKILLS section: every skill the agent may use, then the instructions of the active ones, clipped at 16,000 characters), skillLoadedMessage (what use_skill tells the model, with the state of the tools the skill names), and which tools and skills an agent may use (agentToolNames / agentSkillNames: null = all; names in insertion order, its tools then its toolsets\' tools).',
    operation: 'buildAgentPrompt(parseAgent(input.agent).agent); buildSkillsPrompt({ skills: input.skills.map(parseSkill), active, canLoad }); skillLoadedMessage(parseSkill(input.loaded), (t) => input.states[t] || "missing"); agentToolNames(agent, { tools, toolsets }) / agentSkillNames(agent, skillNames)',
    cases: (() => {
      const skill = (name, description, body, allowed = '') => ({ folder: name, text: `---\nname: ${name}\ndescription: ${description}\n${allowed ? `allowed-tools: ${allowed}\n` : ''}---\n${body}` });
      const proof = skill('proofreading', 'Proofread the document. Use when asked to check spelling.', '# Proofreading\n1. Read.\n2. Fix.', 'find_text replace_text rename_file gone_tool');
      const sum = skill('summarize', 'Summarise.', 'Write three bullets.');
      const agent = { file: 'agents/proofreader.md', text: '---\nname: proofreader\ntitle: Proofreader\ndescription: Checks spelling.\ntools: [rename_file, missing_tool]\ntoolsets: [document, nosuch]\nskills: [proofreading, nope]\n---\nYou are a careful proofreader.' };
      return [
        { name: 'agent section', input: { agent } },
        { name: 'agent without instructions: none', input: { agent: { file: 'agents/q.md', text: '---\nname: q\ndescription: Quiet.\n---\n' } } },
        { name: 'skills: none active, can load', input: { skills: [proof, sum], active: [], canLoad: true } },
        { name: 'skills: one active', input: { skills: [proof, sum], active: ['proofreading'], canLoad: true } },
        { name: 'skills: active, tools unusable this request', input: { skills: [proof], active: ['proofreading'], canLoad: false } },
        { name: 'skills: a long body is clipped', input: { skills: [skill('long', 'Long.', long(17000))], active: ['long'], canLoad: true } },
        { name: 'skill loaded, with its tools', input: { loaded: proof, states: { find_text: 'on', replace_text: 'off', rename_file: 'denied' } } },
        { name: 'skill loaded, no tools named', input: { loaded: sum, states: {} } },
        { name: 'which tools and skills an agent may use', input: { agent, tools: ['rename_file', 'find_text', 'replace_text', 'insert_text'], toolsets: { document: ['find_text', 'insert_text', 'unknown'] }, skillNames: ['proofreading', 'summarize'] } },
        { name: 'an agent that names none: all', input: { agent: { file: 'agents/w.md', text: '---\nname: w\ndescription: Writes.\n---\nWrite.' }, tools: ['a'], toolsets: {}, skillNames: ['s'] } },
      ];
    })(),
    run: ({ agent, skills, active, canLoad, loaded, states, tools, toolsets, skillNames }) => {
      const parsed = agent ? parseAgent(agent.text, { file: agent.file }).agent : null;
      if (tools) {
        const t = agentToolNames(parsed, { tools, toolsets: new Map(Object.entries(toolsets).map(([k, v]) => [k, { name: k, tools: v }])) });
        const s = agentSkillNames(parsed, skillNames);
        return { tools: t.names ? [...t.names] : null, toolProblems: t.problems, skills: s.names ? [...s.names] : null, skillProblems: s.problems };
      }
      if (agent) return buildAgentPrompt(parsed);
      const parse = (s) => parseSkill(s.text, { folder: s.folder }).skill;
      if (loaded) return skillLoadedMessage(parse(loaded), (t) => states[t] || 'missing');
      return buildSkillsPrompt({ skills: skills.map(parse), active, canLoad });
    },
  },
  {
    suite: 'adapter-requests',
    description: 'What each protocol sends: the provider preset (core/providers.js) and the settings resolve into a target (resolveTarget, with its key and model checks), and the adapter builds { url, headers, body } from the neutral request: system, messages with images, tools, toolTurns with images and Gemini thought signatures. The key is "sk-test-key-123456" unless the case gives one.',
    operation: 'settings = sanitizeSettings({ provider, transport, relayUrl, reasoning, maxOutputTokens: maxTokens, temperature, profiles: { [provider]: { baseUrl?, model } } }); t = resolveTarget(settings, () => key, provider); buildChat of adapters/<t.adapter.protocol>.js({ cfg: t.cfg, key: t.key, system, messages, maxTokens: settings.maxOutputTokens, temperature: settings.temperature, tools, toolTurns, turnId }) -> { url, headers, body } | { error: { code, message } }',
    cases: (() => {
      const KEY = 'sk-test-key-123456';
      const image = { mime: 'image/png', data: 'iVBORw0KGgoAAAANSUhEUg==' };
      const messages = [{ role: 'user', content: 'Q1' }, { role: 'assistant', content: 'A1' }, { role: 'user', content: 'Look at this.', images: [image] }];
      const tools = [{ name: 'find_text', description: 'Find text.', parameters: { type: 'object', properties: { query: { type: 'string', maxLength: 200 } }, required: ['query'] } }];
      const toolTurns = [{ text: 'Searching.', calls: [{ id: 'call_1', name: 'find_text', arguments: { query: 'fox' }, signature: 'sig-abc' }], results: [{ id: 'call_1', name: 'find_text', content: 'Found 2.', images: [image] }] }];
      const base = { system: 'You help.', messages, maxTokens: 1000, temperature: 0.3 };
      return [
        { name: 'LM Studio: no model, reasoning off', input: { ...base, provider: 'lmstudio', key: '', reasoning: 'off' } },
        { name: 'LM Studio: model, tools and the exchange', input: { ...base, provider: 'lmstudio', key: '', model: 'qwen/qwen3-8b', tools, toolTurns } },
        { name: 'OpenAI: max_completion_tokens, no temperature, a base URL ending in /v1', input: { ...base, provider: 'openai', key: KEY, model: 'gpt-5-mini', baseUrl: 'https://api.openai.com/v1/' } },
        { name: 'DeepSeek: its own path and reasoning switch', input: { ...base, provider: 'deepseek', key: KEY, model: 'deepseek-chat', reasoning: 'off' } },
        { name: 'Anthropic: tools, the exchange, images, reasoning off', input: { ...base, provider: 'anthropic', key: KEY, model: 'claude-sonnet-5', tools, toolTurns, reasoning: 'off' } },
        { name: 'Gemini: tools, the exchange with its signature', input: { ...base, provider: 'google', key: KEY, model: 'models/gemini-2.5-flash', tools, toolTurns } },
        { name: 'relay: the neutral request as it is', input: { ...base, provider: 'openai', transport: 'relay', relayUrl: 'https://app.example/ai-relay', key: '', model: 'gpt-5-mini', tools, toolTurns, turnId: 't123' } },
        { name: 'a cloud provider without a key', input: { ...base, provider: 'anthropic', key: '', model: 'claude-sonnet-5' } },
        { name: 'a model required but missing', input: { ...base, provider: 'openai', key: KEY } },
      ];
    })(),
    run: ({ provider: id, transport = 'direct', relayUrl = '', reasoning = 'show', key = 'sk-test-key-123456', model = '', baseUrl, system, messages, maxTokens, temperature, tools, toolTurns, turnId }) => {
      const settings = sanitizeSettings({ provider: id, transport, relayUrl, reasoning, maxOutputTokens: maxTokens, temperature, profiles: { [id]: { ...(baseUrl ? { baseUrl } : {}), model } } });
      try {
        const t = resolveTarget(settings, () => key, id);
        const r = REQUEST[t.adapter.protocol]({ cfg: t.cfg, key: t.key, system, messages, maxTokens: settings.maxOutputTokens, temperature: settings.temperature, tools, toolTurns, turnId });
        return { url: r.url, headers: r.headers, body: r.body };
      } catch (e) {
        if (!(e instanceof AiError)) throw e;
        return { error: { code: e.code, message: e.message } };
      }
    },
  },
  {
    suite: 'adapter-streams',
    description: 'What each protocol reads back. An HTTP response (status, content type, the body in chunks) goes through the transport: SSE parsed as EventSource does (event and data fields, a blank line dispatches, ":" comments, CRLF, records split across chunks anywhere); a whole JSON reply when it is not a stream; a stream without its content type still read; HTTP errors classified with their guidance and the key redacted. Then the adapter: text, reasoning, tool calls assembled from their pieces, truncation, in-stream errors. The request is fixed: system "S", one user message "Hi", max tokens 1000, temperature 0.3, the key "sk-test-key-123456". Output: the events in order and the result, or the events and the error.',
    operation: 'adapter.stream({ cfg: resolveTarget(...).cfg, key, system: "S", messages: [{ role: "user", content: "Hi" }], maxTokens: 1000, temperature: 0.3, onEvent, fetch: () => a response of input.response.status (200), content-type input.response.contentType, body input.response.chunks }) -> { events, result: { usage, toolCalls, truncated? } } | { events, error: { code, message, hints? } }',
    cases: (() => {
      const sse = (...records) => records.map((r) => (typeof r === 'string' ? r : `data: ${JSON.stringify(r)}\n\n`)).join('');
      const SSE = 'text/event-stream';
      const oa = (delta, finish = null) => ({ choices: [{ delta, finish_reason: finish }] });
      return [
        { name: 'openai: text and reasoning, comments, usage, [DONE]', input: { provider: 'lmstudio', response: { contentType: SSE, chunks: [`: keepalive\n\n${sse(oa({ reasoning_content: 'Think.' }), oa({ content: 'Hel' }))}`, sse(oa({ content: 'lo.' }, 'stop'), { usage: { prompt_tokens: 9, completion_tokens: 3 }, choices: [] }), 'data: [DONE]\n\n'] } } },
        { name: 'openai: a record split across chunks, CRLF', input: { provider: 'lmstudio', response: { contentType: SSE, chunks: ['data: {"choices":[{"delta":{"con', 'tent":"A"}}]}\r', '\n\r\ndata: {"choices":[{"delta":{"content":"B"}}]}\r\n\r\n'] } } },
        { name: 'openai: tool calls in pieces', input: { provider: 'lmstudio', response: { contentType: SSE, chunks: [sse(oa({ tool_calls: [{ index: 0, id: 'call_a', function: { name: 'find_text', arguments: '{"que' } }] }), oa({ tool_calls: [{ index: 0, function: { arguments: 'ry": "fox"}' } }, { index: 1, id: 'call_b', function: { name: 'set_size', arguments: '{"size": 12}' } }] }), oa({}, 'tool_calls'))] } } },
        { name: 'openai: cut off while writing a call', input: { provider: 'lmstudio', response: { contentType: SSE, chunks: [sse(oa({ tool_calls: [{ index: 0, id: 'call_a', function: { name: 'write', arguments: '{"text": "abc' } }] }), oa({}, 'length'))] } } },
        { name: 'openai: a whole JSON reply', input: { provider: 'lmstudio', response: { contentType: 'application/json', chunks: [JSON.stringify({ choices: [{ message: { content: 'Whole.', reasoning_content: 'R.', tool_calls: [{ id: 'c1', function: { name: 'find_text', arguments: '{"query":"x"}' } }] }, finish_reason: 'length' }], usage: { prompt_tokens: 1, completion_tokens: 2 } })] } } },
        { name: 'openai: an error inside the stream', input: { provider: 'lmstudio', response: { contentType: SSE, chunks: [sse(oa({ content: 'Par' }), { error: { message: 'Model unloaded' } })] } } },
        { name: 'openai: a stream without its content type', input: { provider: 'lmstudio', response: { contentType: 'text/plain', chunks: [sse(oa({ content: 'Still read.' }))] } } },
        { name: 'anthropic: thinking, text, a tool call, max_tokens', input: { provider: 'anthropic', model: 'claude-sonnet-5', response: { contentType: SSE, chunks: [sse({ type: 'message_start', message: { usage: { input_tokens: 12 } } }, { type: 'content_block_delta', index: 0, delta: { type: 'thinking_delta', thinking: 'Hmm.' } }, { type: 'content_block_delta', index: 1, delta: { type: 'text_delta', text: 'Let me check.' } }, { type: 'content_block_start', index: 2, content_block: { type: 'tool_use', id: 'toolu_1', name: 'find_text' } }, { type: 'content_block_delta', index: 2, delta: { type: 'input_json_delta', partial_json: '{"query":' } }, { type: 'content_block_delta', index: 2, delta: { type: 'input_json_delta', partial_json: ' "fox"}' } }, { type: 'message_delta', delta: { stop_reason: 'max_tokens' }, usage: { output_tokens: 40 } })] } } },
        { name: 'anthropic: overloaded', input: { provider: 'anthropic', model: 'claude-sonnet-5', response: { contentType: SSE, chunks: [sse({ type: 'error', error: { type: 'overloaded_error', message: 'Overloaded' } })] } } },
        { name: 'gemini: thought, text, a call with its signature, MAX_TOKENS', input: { provider: 'google', model: 'gemini-2.5-flash', response: { contentType: SSE, chunks: [sse({ candidates: [{ content: { parts: [{ text: 'Plan.', thought: true }, { text: 'Calling.' }] } }] }, { candidates: [{ content: { parts: [{ functionCall: { name: 'find_text', args: { query: 'fox' } }, thoughtSignature: 'sig-1' }] }, finishReason: 'MAX_TOKENS' }], usageMetadata: { promptTokenCount: 5, candidatesTokenCount: 7 } })] } } },
        { name: 'gemini: blocked prompt', input: { provider: 'google', model: 'gemini-2.5-flash', response: { contentType: SSE, chunks: [sse({ promptFeedback: { blockReason: 'SAFETY' } })] } } },
        { name: 'relay: events, a tool call with its raw text, done', input: { provider: 'lmstudio', transport: 'relay', relayUrl: 'https://app.example/ai-relay', response: { contentType: SSE, chunks: [': open\n\nevent: status\ndata: {"message":"Loading"}\n\nevent: reasoning\ndata: {"text":"R."}\n\nevent: delta\ndata: {"text":"Hi."}\n\nevent: tool_call\ndata: {"id":"r1","name":"find_text","arguments":{"query":"x"},"raw":"{\\"query\\": \\"x\\"}"}\n\nevent: done\ndata: {"usage":{"input":1,"output":2},"truncated":true}\n\n'] } } },
        { name: 'relay: an error event with hints', input: { provider: 'lmstudio', transport: 'relay', relayUrl: 'https://app.example/ai-relay', response: { contentType: SSE, chunks: ['event: error\ndata: {"code":"rate-limit","message":"Too many questions.","hints":["Wait a minute."]}\n\n'] } } },
        { name: 'HTTP 401 with the key in the message: redacted', input: { provider: 'openai', model: 'gpt-5-mini', response: { status: 401, contentType: 'application/json', chunks: ['{"error":{"message":"Incorrect API key provided: sk-test-key-123456.","code":"invalid_api_key"}}'] } } },
        { name: 'HTTP 404 naming a model: missing-model', input: { provider: 'lmstudio', response: { status: 404, contentType: 'application/json', chunks: ['{"error":{"message":"model \\"x\\" not found"}}'] } } },
        { name: 'HTTP 404 for a path: bad-endpoint', input: { provider: 'lmstudio', response: { status: 404, contentType: 'text/plain', chunks: ['Cannot POST /v1/chat/completions'] } } },
        { name: 'HTTP 429: rate-limit', input: { provider: 'lmstudio', response: { status: 429, contentType: 'application/json', chunks: ['{"error":{"message":"slow down"}}'] } } },
        { name: 'HTTP 500: network', input: { provider: 'lmstudio', response: { status: 500, contentType: 'text/plain', chunks: ['boom'] } } },
        { name: 'HTTP 200 with only an error: unexpected endpoint', input: { provider: 'lmstudio', response: { contentType: 'application/json', chunks: ['{"error":"Unexpected endpoint or method. (POST /v1/chat/completion)"}'] } } },
        { name: 'neither a stream nor JSON', input: { provider: 'lmstudio', response: { contentType: 'text/html', chunks: ['<html>Hello</html>'] } } },
      ];
    })(),
    run: async ({ provider: id, transport = 'direct', relayUrl = '', model = '', response }) => {
      const key = 'sk-test-key-123456';
      const settings = sanitizeSettings({ provider: id, transport, relayUrl, maxOutputTokens: 1000, temperature: 0.3, profiles: { [id]: { model } } });
      const t = resolveTarget(settings, () => key, id);
      const enc = new TextEncoder();
      const fetch = async () => new Response(new ReadableStream({ start(c) { for (const ch of response.chunks) c.enqueue(enc.encode(ch)); c.close(); } }), { status: response.status || 200, headers: { 'content-type': response.contentType } });
      const events = [];
      try {
        const result = await t.adapter.stream({ cfg: t.cfg, key, system: 'S', messages: [{ role: 'user', content: 'Hi' }], maxTokens: 1000, temperature: 0.3, onEvent: (e) => events.push(e), fetch });
        return { events, result };
      } catch (e) {
        if (!(e instanceof AiError)) throw e;
        return { events, error: { code: e.code, message: e.message, ...(e.hints?.length ? { hints: e.hints } : {}) } };
      }
    },
  },
  {
    suite: 'execute-tool',
    description: `One tool call from the model through the session (core/session.js executeTool), in this order: request_tool; unknown or denied; the master switch; the tool is off; not on this page; unreadable arguments; invalid arguments; the confirmation; the run. ${HARNESS} Output: the result for the model, the rows the view was asked to show (title, call, the statuses in order, a label, a screenshot, an Undo), the cards (kind, button ids, tool, reason; for a confirmation the permission decision and a built-in's own confirm data), the notices, the tools that actually ran with their arguments, the tool states, screenshotAuto, the tools allowed for this chat and the memory afterwards, and the action records.`,
    operation: 'session = scripted(input); session.executeTool(input.call, { reply, signal, actions }) -> { result, rows, cards, notices, ran, toolStates, screenshotAuto, allowedForChat, memory?, actions }',
    cases: (() => {
      const tools = [
        { name: 'find_text', title: 'Find text', description: 'Find text.', effect: 'read', enabled: true, parameters: { query: { type: 'string', required: true, maxLength: 20 } }, result: { matches: 2, first: 'line 3' } },
        { name: 'set_size', title: 'Set size', description: 'Font size.', effect: 'write', enabled: true, parameters: { size: { type: 'integer', min: 8, max: 72, required: true } }, result: 'Size set.' },
        { name: 'delete_doc', title: 'Delete document', description: 'Delete.', effect: 'destructive', enabled: true, result: null },
        { name: 'turned_off', title: 'Turned off', description: 'Off by default.', effect: 'write', result: 'Ran.' },
        { name: 'broken', title: 'Broken', description: 'Throws.', effect: 'write', enabled: true, throws: 'the disk is full' },
        { name: 'chart_zoom', title: 'Zoom chart', description: 'Zoom.', effect: 'write', enabled: true, pages: ['charts/*'] },
        { name: 'write_code', title: 'Write code', description: 'Changes the app.', effect: 'system', enabled: true, result: 'Written.' },
      ];
      const page = { id: 'editor', title: 'Editor', content: 'The quick brown fox.' };
      const base = { tools, page };
      const call = (name, args, extra = {}) => ({ id: 'call_1', name, arguments: args, ...extra });
      const proof = { folder: 'proofreading', text: '---\nname: proofreading\ndescription: Proofread.\nallowed-tools: find_text set_size turned_off\n---\nRead, then fix.' };
      return [
        { name: 'a read tool runs without a card', input: { ...base, call: call('find_text', { query: 'fox' }) } },
        { name: 'a write tool: Run', input: { ...base, answers: ['run'], call: call('set_size', { size: '14' }) } },
        { name: 'a write tool: Allow for this chat (the size is clamped)', input: { ...base, answers: ['chat'], call: call('set_size', { size: 99 }) } },
        { name: 'a write tool already allowed for this chat', input: { ...base, allowedForChat: ['set_size'], call: call('set_size', { size: 9 }) } },
        { name: 'a write tool: Skip', input: { ...base, answers: ['skip'], call: call('set_size', { size: 9 }) } },
        { name: 'confirmations off', input: { ...base, settings: { confirmWrites: false }, call: call('set_size', { size: 9 }) } },
        { name: 'destructive confirms; Stop while asked', input: { ...base, answers: ['cancel'], call: call('delete_doc', {}) } },
        { name: 'a tool that is off: Turn on and run (no second card)', input: { ...base, answers: ['on'], call: call('turned_off', {}) } },
        { name: 'a tool that is off: Keep off', input: { ...base, answers: ['off'], call: call('turned_off', {}) } },
        { name: 'request_tool: Turn on', input: { ...base, answers: ['on'], call: call('request_tool', { name: 'turned_off', reason: 'To do what you asked.' }) } },
        { name: 'request_tool: Keep off', input: { ...base, answers: ['off'], call: call('request_tool', { name: 'turned_off' }) } },
        { name: 'request_tool for a tool already on', input: { ...base, call: call('request_tool', { name: 'find_text' }) } },
        { name: 'request_tool for no such tool', input: { ...base, call: call('request_tool', { name: 'nope' }) } },
        { name: 'unknown tool', input: { ...base, call: call('nope', {}) } },
        { name: 'denied by the application', input: { ...base, permissions: { deny: ['set_*'] }, call: call('set_size', { size: 9 }) } },
        { name: 'not one of the agent\'s tools', input: { ...base, agents: [{ file: 'agents/reader.md', text: '---\nname: reader\ntitle: Reader\ndescription: Reads.\ntools: [find_text]\n---\nRead.' }], call: call('set_size', { size: 9 }) } },
        { name: 'tools switched off', input: { ...base, settings: { toolsEnabled: false }, call: call('find_text', { query: 'x' }) } },
        { name: 'not on this page', input: { ...base, call: call('chart_zoom', {}) } },
        { name: 'unreadable arguments', input: { ...base, call: call('find_text', {}, { raw: 'query = fox', argsError: 'not valid JSON' }) } },
        { name: 'arguments cut off at the reply limit', input: { ...base, settings: { maxOutputTokens: 512 }, call: call('find_text', {}, { raw: '{"query": "fo', argsError: 'the JSON stops before it is closed, after 13 characters', cutOff: true }) } },
        { name: 'invalid arguments', input: { ...base, call: call('find_text', { query: 'a text that is far too long' }) } },
        { name: 'ask: even a read tool confirms, without "for this chat"', input: { ...base, permissions: { ask: ['find_text'] }, answers: ['run'], call: call('find_text', { query: 'fox' }) } },
        { name: 'allow: a write tool runs without a card', input: { ...base, permissions: { allow: ['effect:write'] }, call: call('set_size', { size: 9 }) } },
        { name: 'system always confirms, even allowed', input: { ...base, permissions: { allow: ['write_code'] }, answers: ['run'], call: call('write_code', {}) } },
        { name: 'the tool throws', input: { ...base, answers: ['run'], call: call('broken', {}) } },
        { name: 'memory: remember (read, no card, an Undo)', input: { ...base, memory: [{ id: 'm1', text: 'Units are mm.' }], call: call('remember', { text: 'Ctrl+S saves.' }) } },
        { name: 'memory: remember what is already saved', input: { ...base, memory: [{ id: 'm1', text: 'Units are mm.' }], call: call('remember', { text: 'units are MM.' }) } },
        { name: 'memory: forget asks, with the note', input: { ...base, memory: [{ id: 'm1', text: 'Units are mm.' }], answers: ['run'], call: call('forget', { id: 'm1' }) } },
        { name: 'memory: a denied built-in is unknown', input: { ...base, memory: [], permissions: { deny: ['remember'] }, call: call('remember', { text: 'x' }) } },
        { name: 'screenshot: not freed to look: Allow once', input: { ...base, screen: true, answers: ['once'], call: call('take_screenshot', {}) } },
        { name: 'screenshot: not freed to look: No', input: { ...base, screen: true, answers: ['no'], call: call('take_screenshot', {}) } },
        { name: 'screenshot: Always allow frees it', input: { ...base, screen: true, answers: ['always'], call: call('request_tool', { name: 'take_screenshot' }) } },
        { name: 'screenshot: freed, with the gesture card', input: { ...base, screen: { needsGesture: true }, settings: { screenshotAuto: true }, answers: ['share'], call: call('take_screenshot', {}) } },
        { name: 'skills: use_skill', input: { ...base, skills: [proof], call: call('use_skill', { name: 'proofreading' }) } },
        { name: 'skills: no such skill', input: { ...base, skills: [proof], call: call('use_skill', { name: 'nope' }) } },
      ];
    })(),
    run: async (input) => {
      const h = scripted(input);
      const actions = [];
      const reply = { raw: '', native: '', status: '' };
      const result = await h.session.executeTool(input.call, { reply, signal: h.controller.signal, actions });
      return { result, ...h.report(), actions };
    },
  },
  {
    suite: 'tool-loop',
    description: `One question through the session (beginTurn, runModel, endTurn) against a scripted model: steps[i] is the reply to request i ({ text?, reasoning?, toolCalls?: [{ id, name, arguments: an object, or the model's text }], truncated?, error?: { code, message } }), from the provider "Scripted", model "model-1"; with no step left the model fails. ${HARNESS} input.history is the transcript before the question (a snapshot hash "__current__" stands for the screen's current hash). Output: every request exactly as the session sent it to the model (system, messages, tools, toolTurns: the neutral request the adapters translate), the rows, cards and notices, the reply's text and reasoning, the answer, whether the question was kept, the error, and the transcript (role, content, sync, skill, the actions' call / status / summary, meta).`,
    operation: 'session = scripted(input); turn = await session.beginTurn(input.question, { settings }); result = await session.runModel({ settings, baseSystem: turn.baseSystem, messages: turn.messages, reply, signal, actions, snapshotHash: turn.snapshot?.hash }); end = session.endTurn({ turn, reply, actions, result, error })',
    cases: (() => {
      const tools = [
        { name: 'find_text', title: 'Find text', description: 'Find text.', effect: 'read', enabled: true, parameters: { query: { type: 'string', required: true } }, result: 'Found 2 matches.' },
        { name: 'replace_text', title: 'Replace text', description: 'Replace text.', effect: 'write', enabled: true, parameters: { find: { type: 'string', required: true }, with: { type: 'string', required: true } }, result: 'Replaced 2.', setContent: 'The quick brown cat.' },
        { name: 'export_pdf', title: 'Export PDF', description: 'Export the document.', effect: 'external', result: 'Exported.' },
      ];
      const page = { id: 'editor', title: 'Editor', content: 'The quick brown fox.' };
      const base = { tools, page, app: { name: 'Editor App' } };
      const fc = (id, name, args) => ({ id, name, arguments: args });
      return [
        { name: 'a plain answer', input: { ...base, question: 'What does it say?', steps: [{ text: 'It is about a fox.' }] } },
        { name: 'a read tool, then the answer', input: { ...base, question: 'Find fox.', steps: [{ text: 'Searching.', toolCalls: [fc('c1', 'find_text', { query: 'fox' })] }, { text: 'Two matches.' }] } },
        { name: 'a write tool changes the screen: it comes back with the result', input: { ...base, answers: ['run'], question: 'Replace fox with cat.', steps: [{ toolCalls: [fc('c1', 'replace_text', { find: 'fox', with: 'cat' })] }, { text: 'Done: it now says cat.' }] } },
        { name: 'two calls in one round run in order; one declined', input: { ...base, answers: ['skip'], question: 'Find and replace.', steps: [{ toolCalls: [fc('c1', 'find_text', { query: 'fox' }), fc('c2', 'replace_text', { find: 'fox', with: 'cat' })] }, { text: 'Found it; you declined the change.' }] } },
        { name: 'request_tool turns a tool on; the next request offers it', input: { ...base, answers: ['on', 'run'], question: 'Export it.', steps: [{ toolCalls: [fc('c1', 'request_tool', { name: 'export_pdf', reason: 'To export.' })] }, { toolCalls: [fc('c2', 'export_pdf', {})] }, { text: 'Exported.' }] } },
        { name: 'the step limit (the agent\'s)', input: { ...base, agents: [{ file: 'agents/finder.md', text: '---\nname: finder\ntitle: Finder\ndescription: Finds.\nmaxToolSteps: 1\n---\nFind things.' }], question: 'Find twice.', steps: [{ toolCalls: [fc('c1', 'find_text', { query: 'a' })] }, { toolCalls: [fc('c2', 'find_text', { query: 'b' })] }] } },
        { name: 'auto mode: a model that refuses tools gets text blocks', input: { ...base, question: 'Find fox.', steps: [{ error: { code: 'refused', message: 'This model does not support tools' } }, { text: 'Let me search.\n```tool\n{"name": "find_text", "arguments": {"query": "fox"}}\n```' }, { text: 'Two matches.' }] } },
        { name: 'a call cut off at the reply limit is not run', input: { ...base, question: 'Replace.', steps: [{ toolCalls: [fc('c1', 'replace_text', '{"find": "fox", "with": "a very long')], truncated: true }, { text: 'I will do it in smaller steps.' }] } },
        { name: 'an answer only in the reasoning after tools', input: { ...base, question: 'Find fox.', steps: [{ toolCalls: [fc('c1', 'find_text', { query: 'fox' })] }, { reasoning: 'There are two matches.' }] } },
        { name: 'the reply hits the limit: a notice', input: { ...base, question: 'Write a lot.', steps: [{ text: 'Part one…', truncated: true }] } },
        { name: 'a failed question is taken out', input: { ...base, question: 'Hello?', steps: [{ error: { code: 'network', message: 'Could not reach the server.' } }] } },
        { name: 'a failure after actions keeps the question', input: { ...base, question: 'Find, then fail.', steps: [{ toolCalls: [fc('c1', 'find_text', { query: 'fox' })] }, { error: { code: 'timeout', message: 'No response for 120 s.' } }] } },
        { name: 'Stop at a card: cancelled, the question kept with its action', input: { ...base, answers: ['cancel'], question: 'Replace.', steps: [{ text: 'Replacing.', toolCalls: [fc('c1', 'replace_text', { find: 'fox', with: 'cat' })] }] } },
        { name: 'an unchanged screen; earlier actions in the history', input: { ...base, history: [{ role: 'user', content: 'Find fox.', sync: 'attached', snapshot: { hash: '__current__', text: 'The quick brown fox.', pageId: 'editor', pageTitle: 'Editor', chars: 20, totalChars: 20, truncated: false, at: '2026-01-02T03:04:05.000Z' } }, { role: 'assistant', content: 'Two matches.', actions: [{ call: 'find_text(query: "fox")', status: 'ok', summary: 'Found 2 matches.' }] }], question: 'And now?', steps: [{ text: 'Still two.' }] } },
        { name: 'a skill by /name; sharing off', input: { ...base, settings: { shareScreen: false }, skills: [{ folder: 'proofreading', text: '---\nname: proofreading\ndescription: Proofread.\n---\nRead, then fix.' }], question: '/proofreading check it', steps: [{ text: 'Checked.' }] } },
        { name: 'memory in the prompt; remember', input: { ...base, memory: [{ id: 'm1', text: 'Units are mm.' }], question: 'Remember that Ctrl+S saves.', steps: [{ toolCalls: [fc('c1', 'remember', { text: 'Ctrl+S saves.' })] }, { text: 'Noted.' }] } },
      ];
    })(),
    run: async (input) => {
      const h = scripted(input);
      const s = h.session;
      if (input.history) {
        const current = await s.ctx.snapshot();
        s.conv.messages = structuredClone(input.history).map((m) => (m.snapshot?.hash === '__current__' ? { ...m, snapshot: { ...m.snapshot, hash: current.hash } } : m));
      }
      const settings = h.store.get();
      const turn = await s.beginTurn(input.question, { settings });
      const reply = { raw: '', native: '', status: '' };
      const actions = [];
      let result = null;
      let error = null;
      try {
        result = await s.runModel({ settings, baseSystem: turn.baseSystem, messages: turn.messages, reply, signal: h.controller.signal, actions, snapshotHash: turn.snapshot?.hash || null });
      } catch (e) {
        error = e;
      }
      const end = s.endTurn({ turn, reply, actions, result, error });
      return {
        requests: h.requests,
        ...h.report(),
        reply: { raw: reply.raw, native: reply.native },
        answer: end.answer,
        kept: end.kept,
        error: error ? { code: error.code, message: error.message } : null,
        transcript: s.conv.messages.map((m) => ({
          role: m.role, content: m.content, ...(m.sync ? { sync: m.sync } : {}), ...(m.skill ? { skill: m.skill } : {}),
          ...(m.actions ? { actions: m.actions.map((a) => ({ call: a.call, status: a.status, summary: a.summary ?? '' })) } : {}),
          ...(m.meta ? { meta: m.meta } : {}),
        })),
      };
    },
  },
];
