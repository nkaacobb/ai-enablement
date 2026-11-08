// The agent without its UI: which agent is active, what a request offers (tools, skills, memory, context), the system
// prompt, the tool loop with the user's decisions, and the saved chats. No DOM and no storage of its own: the drawer
// (ui/drawer.js) is a view over it, and the runtimes of other stacks port it (spec/behaviour.md §1–§7; the conformance
// suites execute-tool and tool-loop pin what it does).
//
// The view is a set of hooks (`ui`), all optional, so the session runs headless (tests, the conformance export):
//   paint(reply, now)                the reply's text / reasoning / status changed (now: render at once, not throttled)
//   notice(reply, text)              a notice line above the reply
//   row(reply, { title, call })      a tool row -> a handle the other row hooks receive
//   rowStatus(row, status, summary)  status: running | waiting | ok | error | declined | off | skipped
//   rowLabel(row, text)              replaces the status word ('Turned on', 'Saved', …)
//   rowDetail(row, info)             { name, title, status, detail: callDetail() }: what the row rolls down to show
//   rowShot(row, { id, thumb })      a screenshot the agent took
//   rowUndo(row, undo)               an Undo button (a memory the agent saved, changed or deleted)
//   decide(row, card, signal)        ask the user; resolves to a button id, or 'cancel' when the signal aborts
// A card is data: { kind, buttons: [{ id, label, primary? }], decline, tool?, reason?, confirm? } (see CARDS). The view
// writes the question; a headless session answers every card with its `decline` button.
//
// The reply is a plain object the view may extend: { raw, native, status }. raw is the streamed text (with any inline
// <think> reasoning), native the reasoning the provider sends separately; the answer is splitReasoning(raw).text.

import { streamChat, resolveTarget } from './client.js';
import { planTurn, contextState, buildRequestMessages, snapshotBlock, hasFiles } from './conversation.js';
import {
  REQUEST_TOOL, classifyTools, toolSpecs, requestToolSpec, buildToolPrompt, parseTextToolCalls, textTurnMessages,
  validateArgs, toolEnabled, toolAvailable, serializeResult, formatCall, normalizeTool, argumentsProblem, callDetail,
  wireCall,
} from './tools.js';
import { buildMemoryPrompt } from './memory.js';
import { buildSkillsPrompt, skillLoadedMessage, skillFilePath, parseSlashCommand, SKILL_FILE_MAX } from './skills.js';
import { agentToolNames, agentSkillNames, buildAgentPrompt, pickAgent, implicitAgent } from './agents.js';
import { mergePolicies, decidePermission, needsConfirmation } from './permissions.js';
import { AiError } from './transport.js';
import { buildSystemPrompt } from './prompt.js';
import { splitReasoning } from './reasoning.js';

export const CHAT_LIMIT = 50;
export const MESSAGE_LIMIT = 200;
/** Characters of each part of a tool row's detail kept in saved chats (the live row has all). */
export const DETAIL_SAVED = 2000;
/** The hard cap on tool rounds per question, whatever the settings and the agent say. */
export const MAX_ROUNDS = 64;

const uid = (prefix = 'm') => `${prefix}${Date.now().toString(36)}${Math.random().toString(36).slice(2, 7)}`;
/** Numbers in what the session writes (for the model and in notices): en-US grouping on every machine, as tools.js. */
const nf = (n) => Number(n || 0).toLocaleString('en-US');

/** What a confirmation says a tool of each effect does (the view shows it under "Run <tool>?"). */
export const EFFECT_NOTES = Object.freeze({
  write: 'This changes the application.',
  destructive: 'This removes or overwrites something.',
  external: 'This reaches outside the application (it sends, publishes or calls another service).',
  system: 'This changes the application itself.',
  read: 'The application asks before this tool runs.',
});

/** The cards the session asks the user, with their buttons. `decline` is the answer of a session without a view. */
export const CARDS = Object.freeze({
  // The model asked for a tool that is off (request_tool).
  request: { buttons: [{ id: 'on', label: 'Turn on', primary: true }, { id: 'off', label: 'Keep off' }], decline: 'off' },
  // The model called a tool that is off.
  'turn-on': { buttons: [{ id: 'on', label: 'Turn on and run', primary: true }, { id: 'off', label: 'Keep off' }], decline: 'off' },
  // Confirmation before a tool runs (the buttons depend on the tool: see confirmButtons).
  confirm: { buttons: [], decline: 'skip' },
  // The agent wants to take a screenshot and is not freed to.
  look: { buttons: [{ id: 'once', label: 'Allow once', primary: true }, { id: 'always', label: 'Always allow' }, { id: 'no', label: 'No' }], decline: 'no' },
  // The screen capture needs the user's gesture first (the browser asks to share the tab).
  'share-tab': { buttons: [{ id: 'share', label: 'Share this tab', primary: true }, { id: 'no', label: 'Not now' }], decline: 'no' },
});

/** Run / Allow for this chat (write tools only, unless the rule is ask) / Skip. */
export function confirmButtons(tool, decision) {
  const buttons = [{ id: 'run', label: 'Run', primary: true }];
  if (tool.effect === 'write' && decision !== 'ask') buttons.push({ id: 'chat', label: 'Allow for this chat' });
  buttons.push({ id: 'skip', label: 'Skip' });
  return buttons;
}

const card = (kind, extra = {}) => ({ kind, buttons: CARDS[kind].buttons.map((b) => ({ ...b })), decline: CARDS[kind].decline, ...extra });
const toolInfo = (t) => ({ name: t.name, title: t.title, effect: t.effect });

/** A tool row's detail as saved chats keep it: each part capped at DETAIL_SAVED characters. */
export const savedDetail = (d) => Object.fromEntries(Object.entries(d).map(([k, v]) => [k, v.length > DETAIL_SAVED
  ? `${v.slice(0, DETAIL_SAVED)}… [${nf(v.length - DETAIL_SAVED)} more characters not kept in saved chats]` : v]));

/** The question a message with attachments and no text asks. */
export function defaultQuestion(text, shots = [], files = []) {
  const t = String(text ?? '').trim();
  if (t || (!shots.length && !files.length)) return t;
  const images = shots.filter((s) => s.name).length;
  if (!files.length && !images) return 'Here is a screenshot of what I am looking at.';
  if (files.length && !shots.length) return files.length === 1 ? 'Please look at the attached file.' : 'Please look at the attached files.';
  if (images && !files.length && images === shots.length) return images === 1 ? 'Please look at the attached image.' : 'Please look at the attached images.';
  return 'Please look at what I attached.';
}

/**
 * What the integration's `host` offers, for the in-app agent writing a tool (development workspace): each function
 * with its parameter list as written in the source, and the other properties with their type (one level into plain
 * objects). Nothing is called.
 */
export function describeHost(host) {
  if (host === null || host === undefined) return 'There is no host: the integration passes none (createAiAgent({ host })). A tool can only use the page itself, e.g. document.querySelector, or call the agent.';
  const signature = (fn) => {
    const src = Function.prototype.toString.call(fn).replace(/\s+/g, ' ');
    const m = /^(?:async\s+)?(?:function\b[^(]*)?\s*(\([^)]*\)|[\w$]+)\s*(?:=>|\{)/.exec(src) || /^(?:async\s+)?[\w$]+\s*(\([^)]*\))/.exec(src);
    return m ? (m[1].startsWith('(') ? m[1] : `(${m[1]})`) : '(…)';
  };
  const describe = (value) => {
    if (typeof value === 'function') return `function${signature(value)}`;
    if (Array.isArray(value)) return `array (${value.length} items)`;
    if (typeof Element === 'function' && value instanceof Element) return `<${value.tagName.toLowerCase()}${value.id ? `#${value.id}` : ''}> element${'value' in value ? ' (value, checked: a form control — set it with setControlValue)' : ''}`;
    if (value && typeof value === 'object') return 'object';
    return typeof value;
  };
  const keys = (obj) => {
    const out = new Set();
    for (let o = obj; o && o !== Object.prototype && o !== Function.prototype; o = Object.getPrototypeOf(o)) for (const k of Object.getOwnPropertyNames(o)) if (k !== 'constructor') out.add(k);
    return [...out].slice(0, 80);
  };
  const lines = ['host offers (call these as host.<name>):'];
  for (const k of keys(host)) {
    let v;
    try { v = host[k]; } catch { continue; }
    lines.push(`- ${k}: ${describe(v)}`);
    if (v && typeof v === 'object' && !Array.isArray(v) && !(typeof Element === 'function' && v instanceof Element)) {
      for (const k2 of keys(v).slice(0, 30)) {
        let v2;
        try { v2 = v[k2]; } catch { continue; }
        lines.push(`  - ${k}.${k2}: ${describe(v2)}`);
      }
    }
  }
  if (lines.length === 1) lines.push('(nothing enumerable)');
  lines.push('Functions with no parameters that return data (such as a state() getter) can be called inside run(); check what they return by reading their source.');
  return lines.join('\n');
}

const NO_UI = Object.freeze({
  paint() {}, notice() {}, row: () => ({}), rowStatus() {}, rowLabel() {}, rowDetail() {}, rowShot() {}, rowUndo() {},
  decide: async (row, c) => c.decline,
});

export class AgentSession {
  /**
   * @param {object} o
   * @param {import('./context.js').ContextManager} o.ctx
   * @param {object} o.store         settings store: get(), save(patch), keys.get(providerId)
   * @param {object} [o.caps]        capabilities: agents(), skills(), toolsets(), policy(), defaultAgent(), workspace(),
   *                                 fetchText(url), reload()
   * @param {object} [o.toolRegistry] { all(), get(name) }: the application's tools (app-wide and this page's)
   * @param {object} [o.memory]      memory store (core/memory.js) or null
   * @param {*} [o.host]             what the application's tools receive: run(args, { host })
   * @param {string} [o.title]       the implicit agent's title
   * @param {Function} [o.emit]      (event, detail) => void
   * @param {Function} [o.defaultPrompt] () => the application's system prompt
   * @param {object} [o.screen]      screenshots: { method() -> 'none' | …, needsGesture(), take(by) -> shot } or null
   * @param {boolean} [o.attachments] the user can attach images (the model then sees images even without screenshots)
   * @param {object} [o.storage]     where saved chats live ({ getItem, setItem }), or null for none
   * @param {string} [o.namespace]   storage namespace (the app id)
   * @param {boolean} [o.saveChats]
   * @param {*} [o.relayHeaders]
   * @param {Function} [o.chat]      the model call (core/client.js streamChat)
   * @param {object} [o.ui]          the view's hooks (see the top of this file)
   */
  constructor({
    ctx, store, caps = null, toolRegistry = null, memory = null, host = null, title = 'AI agent', emit = () => {},
    defaultPrompt = () => '', screen = null, attachments = true, storage = null, namespace = 'app.ai', saveChats = true,
    relayHeaders, chat = streamChat, ui = {},
  }) {
    this.ctx = ctx;
    this.store = store;
    this.caps = caps;
    this.toolRegistry = toolRegistry;
    this.memory = memory;
    this.host = host;
    this.title = title;
    this.emit = emit;
    this.defaultPrompt = defaultPrompt;
    this.screen = screen;
    this.attachments = attachments;
    this.storage = storage;
    this.chatsKey = `${namespace}.chats`;
    this.saveChats = saveChats;
    this.relayHeaders = relayHeaders;
    this.chat = chat;
    this.ui = { ...NO_UI, ...ui };
    this.api = null;                  // the public agent API, passed to tools as `agent` (set by createAiAgent)
    this.conv = { id: null, messages: [], skills: [] };   // skills: names of the skills active in this conversation
    this.agentName = '';              // the active agent ('' = the default one)
    this.forceReread = false;         // the user pressed Re-read: the next question carries the page
    this.allowedTools = new Set();    // write tools the user allowed "for this chat"
    this.textTools = new Set();       // provider+model pairs that refused native tool calls (auto mode)
    this.builtinTools = this.makeBuiltins();
  }

  keyFor = (id) => this.store.keys?.get(id) || '';

  /* ============================================================== agents */

  /** The agents the user can pick from (the application's implicit agent when it defines none). */
  agents() {
    return this.caps?.agents() || [implicitAgent(this.title)];
  }

  /** The active agent. */
  agent() {
    const list = this.agents();
    return list.find((a) => a.name === this.agentName) || pickAgent(list, this.caps?.defaultAgent() || '') || implicitAgent(this.title);
  }

  /** The permission rules in force: the application's, plus the active agent's. */
  policy() {
    return mergePolicies(this.caps?.policy(), this.agent().permissions);
  }

  /** The application's tools the active agent may use: its tools and toolsets (all when it names none), not denied. */
  agentTools() {
    const all = this.toolRegistry?.all() || [];
    const agent = this.agent();
    const { names } = agentToolNames(agent, { tools: all.map((t) => t.name), toolsets: this.caps?.toolsets() || new Map() });
    const policy = this.policy();
    return all.filter((t) => (!names || names.has(t.name)) && decidePermission(t, policy) !== 'deny');
  }

  agentHasTool(name) { return this.agentTools().some((t) => t.name === name); }

  /** The skills the active agent may use: its own list (all when it names none), plus built-in ones in use (workspace). */
  skillsForAgent() {
    const all = this.caps?.skills() || [];
    const app = all.filter((s) => s.source !== 'builtin');
    const { names } = agentSkillNames(this.agent(), app.map((s) => s.name));
    return all.filter((s) => s.source === 'builtin' || !names || names.has(s.name));
  }

  /** 'on' | 'read' | 'off': what the active agent may do with memory. */
  memoryMode() {
    return this.memory ? this.agent().memory || 'on' : 'off';
  }

  /** Is screen content shared with the active agent (the user's switch and the agent's context layers)? */
  shareOn(settings) {
    return !!settings.shareScreen && this.agent().context?.screen !== false;
  }

  /** The context texts the active agent receives: { appText, pageText, viewText, share }. */
  async contextTexts(settings) {
    const c = this.agent().context || {};
    const share = this.shareOn(settings);
    const [appText, pageText, viewText] = await Promise.all([
      c.app === false ? '' : this.ctx.appText(),
      c.page === false ? '' : this.ctx.pageText(),
      share && c.view !== false ? this.ctx.viewText() : '',
    ]);
    return { appText, pageText, viewText, share };
  }

  effectivePrompt(settings) {
    return settings.systemPrompt && settings.systemPrompt.trim() ? settings.systemPrompt : this.defaultPrompt();
  }

  /** What every request of a question shares: the editable prompt, the agent, the context, attached files. */
  async baseSystemFor(settings, texts = null) {
    const { appText, pageText, share } = texts || await this.contextTexts(settings);
    return {
      base: this.effectivePrompt(settings), agentText: buildAgentPrompt(this.agent()), appText, pageText, share,
      files: hasFiles(this.conv.messages, settings.historyMessages),
    };
  }

  /** The whole system prompt for the next request (tools, memory and skills as they stand now). */
  systemFor(settings, baseSystem) {
    const mode = this.toolMode(settings);
    return buildSystemPrompt({
      ...baseSystem,
      toolsText: this.toolPrompt(settings),
      memoryText: this.memoryPrompt(settings),
      skillsText: this.skillsPrompt(mode !== 'off'),
      vision: this.seesImages(settings),
      screenshots: this.visionOn(settings),
    });
  }

  /** The SKILLS section: the active agent's skills, and the instructions of those active in this conversation. */
  skillsPrompt(canLoad = true) {
    return buildSkillsPrompt({ skills: this.skillsForAgent(), active: this.conv.skills, canLoad });
  }

  /** What Settings shows about agents and skills. */
  capabilityInfo() {
    const agent = this.agent();
    return {
      agents: this.agents().filter((a) => !a.implicit).map((a) => ({ name: a.name, title: a.title, description: a.description, model: a.model })),
      agent: agent.implicit ? '' : agent.name,
      skills: this.skillsForAgent().map((s) => ({ name: s.name, description: s.description, source: s.source, active: this.conv.skills.includes(s.name) })),
      workspace: this.caps?.workspace()?.info || null,
    };
  }

  /* ============================================================== skills */

  /** Make a skill active in this conversation: its instructions join the system prompt from the next request on. */
  activateSkill(name, { via = 'tool' } = {}) {
    const skill = this.skillsForAgent().find((s) => s.name === name);
    if (!skill) return null;
    if (!this.conv.skills.includes(name)) this.conv.skills = [...this.conv.skills, name];
    this.emit('skill', { name, via });
    return skill;
  }

  /** How a tool a skill names stands for the active agent: 'on' | 'off' | 'denied' | 'missing'. */
  toolStateFor(name) {
    const tool = this.toolRegistry?.get(name);
    if (!tool) return this.builtins(this.store.get()).some((b) => b.name === name) ? 'on' : 'missing';
    if (!this.agentHasTool(name)) return 'denied';
    return toolEnabled(tool, this.store.get()) ? 'on' : 'off';
  }

  /* =============================================================== tools */

  /** 'off' (no tools registered), 'native' tool calls, or 'text' tool blocks. */
  toolMode(settings) {
    if (!this.agentTools().length && !this.builtins(settings).length) return 'off';
    if (settings.toolMode === 'native' || settings.toolMode === 'text') return settings.toolMode;
    return this.textTools.has(this.modelKey(settings)) ? 'text' : 'native';
  }

  modelKey(settings) {
    return `${settings.transport}|${settings.provider}|${settings.profiles?.[settings.provider]?.model || ''}`;
  }

  /** The TOOLS section of the system prompt, as it would be sent now. */
  toolPrompt(settings = this.store.get()) {
    const mode = this.toolMode(settings);
    if (mode === 'off') return '';
    const set = this.toolSet(settings);
    return buildToolPrompt({ classes: set.classes, mode, enabled: set.active, appOff: set.appOff });
  }

  /**
   * The tools of one request: the application's that the active agent may use (when Settings > Tools has them
   * switched on) plus the built-in ones (memory, screenshots, skills, workspace), which follow their own settings.
   * `active` is false when nothing can be called or asked for.
   */
  toolSet(settings) {
    const app = this.agentTools();
    const builtins = this.builtins(settings).filter((b) => !app.some((t) => t.name === b.name));
    if (!builtins.length) return { classes: classifyTools(app, settings, this.ctx.page?.id), active: !!settings.toolsEnabled, appOff: false };
    const classes = settings.toolsEnabled ? classifyTools(app, settings, this.ctx.page?.id) : { callable: [], off: [], elsewhere: [] };
    for (const b of builtins) {
      if (toolEnabled(b, settings)) classes.callable.push(b);
      else if (b.offerWhenOff) classes.off.push(b);
    }
    return { classes, active: true, appOff: !settings.toolsEnabled && app.length > 0 };
  }

  /** A tool the active agent may use, by name (app tools it may see, then built-ins), or null. */
  findTool(name, settings = this.store.get()) {
    return this.agentTools().find((t) => t.name === name) || this.builtins(settings).find((b) => b.name === name) || null;
  }

  /** Why a tool by this name is not usable here ('' when there is no such tool at all). */
  hiddenReason(name) {
    const tool = this.toolRegistry?.get(name) || this.builtinTools.find((b) => b.name === name);
    if (!tool) return '';
    if (decidePermission(tool, this.policy()) === 'deny') return `${name} is not permitted in this application${this.agent().implicit ? '' : ` for the ${this.agent().title} agent`}. Do not call it; tell the user if it matters.`;
    if (!tool.builtin) return `${name} is not one of the ${this.agent().title} agent's tools. Use the tools you were given.`;
    return '';
  }

  /** Turn a tool on or off for this user (a built-in follows its own setting). */
  setToolEnabled(name, on) {
    const builtin = this.toolRegistry?.get(name) ? null : this.builtinTools.find((b) => b.name === name);
    if (builtin) { if (!builtin.setEnabled) return; builtin.setEnabled(!!on); } else this.store.save({ toolStates: { [name]: !!on } });
    this.emit('tool-state', { name, enabled: !!on });
  }

  /* ------------------------------------------------------- built-in tools */

  /** The built-in tools that exist with these settings and are not denied (whether the model may call them: toolEnabled()). */
  builtins(settings) {
    const policy = this.policy();
    return this.builtinTools.filter((b) => b.exists(settings) && decidePermission(b, policy) !== 'deny');
  }

  makeBuiltins() {
    const list = [];
    const mem = this.memory;
    const define = (def, extra) => list.push({ ...normalizeTool({ ...def, enabled: true }), ...extra });
    if (mem) {
      const writable = (s) => !!(s.memoryEnabled && s.memoryWrite) && this.memoryMode() === 'on';
      define({
        name: 'remember',
        title: 'Remember',
        description: 'Save a note to your long-term memory so that you still know it in future conversations. Use it when the user asks you to remember, note or keep something.',
        parameters: {
          text: { type: 'string', required: true, maxLength: mem.limits.chars, description: 'One short, self-contained sentence: what to remember and when it applies.' },
          id: { type: 'string', description: 'Only to correct an existing memory: its id (for example m3).' },
        },
        effect: 'read',
        run: ({ text, id }) => {
          if (id) {
            const prev = mem.get(id);
            if (!prev) return { status: 'error', content: `There is no memory with the id "${id}". Call remember without an id to add a new one.` };
            const m = mem.update(id, text);
            return { content: `Updated memory ${m.id}.`, summary: m.text, undo: () => mem.update(id, prev.text), label: 'Updated' };
          }
          const count = mem.list().length;
          const m = mem.add(text, { source: 'agent' });
          const added = mem.list().length > count;
          return { content: added ? `Saved as memory ${m.id}.` : `That is already saved (memory ${m.id}).`, summary: m.text, undo: added ? () => mem.remove(m.id) : null, label: added ? 'Saved' : 'Already saved' };
        },
      }, { builtin: 'memory', exists: writable, enabledIn: writable, offerWhenOff: false });
      define({
        name: 'forget',
        title: 'Forget',
        description: 'Delete one note from your long-term memory. Use it when the user asks you to forget something you saved.',
        parameters: { id: { type: 'string', required: true, description: 'The id of the memory to delete (for example m3).' } },
        effect: 'write',
        run: ({ id }) => {
          const m = mem.remove(id);
          if (!m) return { status: 'error', content: `There is no memory with the id "${id}".` };
          return { content: `Deleted memory ${id}.`, summary: m.text, undo: () => mem.replaceAll([...mem.list(), m]), label: 'Deleted' };
        },
      }, {
        builtin: 'memory', exists: writable, enabledIn: writable, offerWhenOff: false,
        confirm: ({ id }) => { const m = mem.get(id); return m ? { kind: 'memory-delete', id, text: m.text } : null; },
      });
    }
    if (this.screen) {
      define({
        name: 'take_screenshot',
        title: 'Take screenshot',
        description: 'Look at the user\'s screen: takes a screenshot of what they are looking at right now and shows it to you. Use it when the page snapshot text is not enough: layout, colours, charts, images, canvas or 3D views, visual problems.',
        parameters: {},
        effect: 'read',
        run: (args, { row, signal }) => this.agentScreenshot(row, signal, { allowed: true }),
      }, {
        builtin: 'vision',
        exists: (s) => !!s.vision && this.screen.method() !== 'none',
        enabledIn: (s) => !!s.screenshotAuto,
        offerWhenOff: true,
        setEnabled: (on) => this.store.save({ screenshotAuto: !!on }),
      });
    }
    this.defineSkillTools(define);
    this.defineWorkspaceTools(define);
    return list;
  }

  /** use_skill and read_skill_file: there while the active agent has skills. */
  defineSkillTools(define) {
    const hasSkills = () => this.skillsForAgent().length > 0;
    const always = { exists: hasSkills, enabledIn: hasSkills, offerWhenOff: false, offMessage: 'This agent has no skills.' };
    const named = (name) => this.skillsForAgent().find((s) => s.name === String(name || '').trim());
    const list = () => this.skillsForAgent().map((s) => s.name).join(', ') || 'none';
    define({
      name: 'use_skill',
      title: 'Use skill',
      description: 'Load a skill: the instructions for a kind of work, listed under SKILLS. Call it before doing work a skill describes; the skill then stays active for this conversation.',
      parameters: { name: { type: 'string', required: true, maxLength: 64, description: 'The skill\'s name, as listed under SKILLS.' } },
      effect: 'read',
      run: ({ name }) => {
        const skill = named(name);
        if (!skill) return { status: 'error', content: `There is no skill named "${name}". The skills you can use: ${list()}.` };
        const already = this.conv.skills.includes(skill.name);
        this.activateSkill(skill.name, { via: 'tool' });
        return { content: skillLoadedMessage(skill, (t) => this.toolStateFor(t)), summary: skill.description, label: already ? 'Already active' : 'Loaded' };
      },
    }, { builtin: 'skills', ...always });
    define({
      name: 'read_skill_file',
      title: 'Read skill file',
      description: 'Read a file that a skill\'s instructions point to (a path relative to the skill\'s folder, such as references/guide.md).',
      parameters: {
        skill: { type: 'string', required: true, maxLength: 64, description: 'The skill\'s name.' },
        path: { type: 'string', required: true, maxLength: 300, description: 'The file, relative to the skill\'s folder.' },
        offset: { type: 'integer', min: 0, description: 'Start at this character, to read a long file in parts (default 0).' },
      },
      effect: 'read',
      run: async ({ skill, path, offset = 0 }) => {
        const s = named(skill);
        if (!s) return { status: 'error', content: `There is no skill named "${skill}". The skills you can use: ${list()}.` };
        if (!s.base) return { status: 'error', content: `The skill "${s.name}" has no folder, so it has no files to read.` };
        let rel;
        try { rel = skillFilePath(path); } catch (e) { return { status: 'error', content: e.message }; }
        const url = new URL(rel, s.base).href;
        if (!url.startsWith(s.base)) return { status: 'error', content: `"${rel}" is outside the skill's folder.` };
        let text;
        try { text = await (this.caps?.fetchText || ((u) => fetch(u).then((r) => (r.ok ? r.text() : Promise.reject(new Error(`HTTP ${r.status}`))))))(url); } catch (e) {
          return { status: 'error', content: `${rel} could not be read (${e?.message || e}). Check the path against the skill's instructions.` };
        }
        const start = Math.min(Math.max(0, offset), text.length);
        const end = Math.min(text.length, start + SKILL_FILE_MAX);
        const more = end < text.length ? `\n\n[${nf(text.length - end)} more characters: call again with offset ${end}.]` : '';
        return { content: `${s.name}/${rel} (${nf(text.length)} characters${start || more ? `, showing ${nf(start)}–${nf(end)}` : ''}):\n\n${text.slice(start, end)}${more}`, summary: `${s.name}/${rel}` };
      },
    }, { builtin: 'skills', ...always });
  }

  /**
   * The development workspace (scripts/workspace.mjs): read the application's source, write in its capability
   * folder, reload the capabilities. There only while that server answers; every write is confirmed (effect system).
   */
  defineWorkspaceTools(define) {
    const ws = () => this.caps?.workspace() || null;
    const connected = () => !!ws();
    const always = { exists: connected, enabledIn: connected, offerWhenOff: false, offMessage: 'The workspace is not connected.' };
    const fail = (e) => ({ status: 'error', content: `Error: ${e?.message || e}` });
    define({
      name: 'list_source_files',
      title: 'List source files',
      description: 'List a folder of the application\'s source code (development workspace). Start with "" for the application folder.',
      parameters: { path: { type: 'string', maxLength: 300, description: 'A folder relative to the application folder ("" = the top).' } },
      effect: 'read',
      run: async ({ path = '' }) => {
        try {
          const r = await ws().client.list(path);
          const lines = r.entries.map((e) => (e.type === 'dir' ? `${e.name}/` : `${e.name}${Number.isFinite(e.size) ? ` (${nf(e.size)} bytes)` : ''}`));
          return { content: `${r.path || '.'}:\n${lines.join('\n') || '(empty)'}${r.truncated ? '\n[more entries not shown]' : ''}`, summary: `${r.path || '.'} · ${r.entries.length} entries` };
        } catch (e) { return fail(e); }
      },
    }, { builtin: 'workspace', ...always });
    define({
      name: 'read_source_file',
      title: 'Read source file',
      description: 'Read a file of the application\'s source code (development workspace), to see how a function works before wrapping it in a tool.',
      parameters: {
        path: { type: 'string', required: true, maxLength: 300, description: 'The file, relative to the application folder.' },
        offset: { type: 'integer', min: 0, description: 'Start at this character, to read a long file in parts (default 0).' },
      },
      effect: 'read',
      run: async ({ path, offset = 0 }) => {
        try {
          const r = await ws().client.read(path, offset);
          const more = r.truncated ? `\n\n[More: call again with offset ${r.end}.]` : '';
          return { content: `${r.path} (${nf(r.size)} characters${r.offset || r.truncated ? `, showing ${nf(r.offset)}–${nf(r.end)}` : ''}):\n\n${r.text}${more}`, summary: r.path };
        } catch (e) { return fail(e); }
      },
    }, { builtin: 'workspace', ...always });
    define({
      name: 'search_source',
      title: 'Search source',
      description: 'Find where a word or name appears in the application\'s source code (development workspace): file, line and the line\'s text.',
      parameters: {
        query: { type: 'string', required: true, maxLength: 200, description: 'The text to look for (plain text, not a pattern).' },
        path: { type: 'string', maxLength: 300, description: 'Only inside this folder (default: everywhere).' },
      },
      effect: 'read',
      run: async ({ query, path = '' }) => {
        try {
          const r = await ws().client.search(query, path);
          const lines = r.matches.map((m) => `${m.path}:${m.line}: ${m.text}`);
          return { content: lines.length ? `${lines.join('\n')}${r.truncated ? '\n[more matches not shown: narrow the search]' : ''}` : `"${query}" does not appear in the source.`, summary: `${r.matches.length} match${r.matches.length === 1 ? '' : 'es'}` };
        } catch (e) { return fail(e); }
      },
    }, { builtin: 'workspace', ...always });
    define({
      name: 'describe_host',
      title: 'Describe host',
      description: 'What `host` — the object tools receive as run(args, { host }) — offers: its functions with their parameters, and its other properties. Call it before writing a tool, and use only what it lists.',
      parameters: {},
      effect: 'read',
      run: () => ({ content: describeHost(this.host), summary: 'host described' }),
    }, { builtin: 'workspace', ...always });
    define({
      name: 'write_ai_file',
      title: 'Write capability file',
      description: 'Create a file in the application\'s capability folder (a new tool module, a skill, an agent), or replace one whole with replace: true (the capability index, the tool config — read it first and keep every entry). A new tool always goes in a new file. The user sees the file (or the changes) and confirms. Then call reload_capabilities.',
      parameters: {
        path: { type: 'string', required: true, maxLength: 300, description: 'The file, relative to the application folder; it must be inside the capability folder.' },
        content: { type: 'string', required: true, maxLength: 100000, description: 'The complete new content of the file.' },
        replace: { type: 'boolean', description: 'true to replace an existing file whole (read it first); omitted = create a new file (refused if it exists).' },
      },
      effect: 'system',
      run: async ({ path, content, replace = false }) => {
        try {
          const r = await ws().client.write(path, content, { replace });
          return { content: `${r.created ? 'Created' : 'Replaced'} ${r.path} (${nf(r.bytes)} bytes). Call reload_capabilities to load the change, then check it.`, summary: `${r.created ? 'Created' : 'Replaced'} ${r.path}` };
        } catch (e) { return fail(e); }
      },
    }, {
      builtin: 'workspace', ...always,
      // Creating a file that exists is refused before the user is asked (the server would refuse it too).
      precheck: async ({ path, content, replace = false }) => {
        // A tool module the runtime could not load is refused with what to write instead (cheaper than a reload).
        if (/(^|\/)tools\/[^/]+\.m?js$/.test(path)) {
          if (!/export\s+default\b/.test(content)) return 'a tool module must export the tool as its default: export default { name, title, description, effect, parameters, run: (args, { host }) => … } (the format in the create-tool skill).';
          if (/\brun\s*(?::\s*(?:async\s*)?(?:function\s*)?)?\(\s*[\w$]*\s*,\s*host\s*\)/.test(content)) return 'run\'s second argument is an object: write run: (args, { host }) => …, not run(args, host).';
        }
        if (replace) return '';
        try { await ws().client.read(path); } catch { return ''; }
        return `${path} already exists. A new tool goes in a new file (tools/<tool name>.js). To change this file, read it, then write it whole with replace: true.`;
      },
      // What the user confirms: a new file whole; a replacement against what it replaces (the view shows the diff),
      // so a dropped tool cannot slip by.
      confirm: async ({ path, content, replace = false }) => {
        let before = null;
        if (replace) { try { before = (await ws().client.read(path)).text; } catch { before = null; } }
        return { kind: 'file', path, content: String(content), replace: !!replace, before };
      },
    });
    define({
      name: 'reload_capabilities',
      title: 'Reload capabilities',
      description: 'Load the capability folder again (after writing to it), so new or changed tools, skills and agents are available. Returns the problems found, if any.',
      parameters: {},
      effect: 'read',
      run: async () => {
        try {
          const problems = [...await this.caps.reload()];
          const tools = this.toolRegistry?.all().length || 0;
          // Tool files the index does not name are not loaded: say which, so they get registered.
          try {
            const info = ws().info;
            const dir = `${info.aiDir === '.' ? '' : `${info.aiDir}/`}tools`;
            const listed = new Set((this.toolRegistry?.all() || []).map((t) => t.source).filter(Boolean));
            const files = (await ws().client.list(dir)).entries.filter((e) => e.type === 'file' && /\.m?js$/.test(e.name)).map((e) => `tools/${e.name}`);
            for (const f of files) if (!listed.has(f)) problems.push(`${dir}/${f.slice(6)} is not in the capability index, so it is not loaded: add "${f}" to "tools" in index.json (write it whole with replace: true).`);
          } catch { /* no tools folder: nothing to say */ }
          return {
            content: `Reloaded: ${tools} tool${tools === 1 ? '' : 's'}, ${this.skillsForAgent().length} skill(s), ${this.agents().filter((a) => !a.implicit).length} agent(s).${problems.length ? ` Problems:\n- ${problems.join('\n- ')}` : ' No problems.'} New tools start turned off unless the tool config turns them on; a tool that is off can be asked for with request_tool.`,
            summary: problems.length ? `Reloaded · ${problems.length} problem(s)` : 'Reloaded · no problems',
          };
        } catch (e) { return fail(e); }
      },
    }, { builtin: 'workspace', ...always });
  }

  /** The MEMORY section of the system prompt, as it would be sent now. */
  memoryPrompt(settings = this.store.get()) {
    const mode = this.memoryMode();
    if (mode === 'off') return '';
    return buildMemoryPrompt({ items: this.memory.list(), enabled: !!settings.memoryEnabled, canWrite: mode === 'on' && !!(settings.memoryEnabled && settings.memoryWrite) });
  }

  /* ---------------------------------------------- screenshots (models that see) */

  /** Can screenshots be taken and shown to the model with these settings? */
  visionOn(settings = this.store.get()) {
    return !!this.screen && !!settings.vision && this.screen.method() !== 'none';
  }

  /** Can the model be shown images at all: screenshots, or image files the user attaches? */
  seesImages(settings = this.store.get()) {
    return !!settings.vision && (this.visionOn(settings) || !!this.attachments);
  }

  /**
   * The agent looks at the screen. When the user has not freed it to do so (`allowed` false) it asks first; with the
   * browser's screen capture, the first screenshot also needs a click, which the same card provides.
   */
  async agentScreenshot(row, signal, { allowed }) {
    if (!allowed) {
      const choice = await this.decide(row, card('look'), signal);
      if (choice === 'cancel') return { status: 'skipped', content: 'Stopped by the user.' };
      if (choice === 'no') return { status: 'declined', content: 'The user did not allow a screenshot. Do not ask again for this request: work from the page snapshot, or ask the user to describe what they see.' };
      if (choice === 'always') this.setToolEnabled('take_screenshot', true);
    } else if (this.screen.needsGesture()) {
      const choice = await this.decide(row, card('share-tab'), signal);
      if (choice === 'cancel') return { status: 'skipped', content: 'Stopped by the user.' };
      if (choice !== 'share') return { status: 'declined', content: 'The user did not share the screen. Do not ask again for this request: work from the page snapshot.' };
    }
    this.ui.rowStatus(row, 'running');
    const shot = await this.screen.take('agent');
    return {
      content: `Screenshot taken (${shot.width} × ${shot.height} px). It is attached: use what you see in it.`,
      summary: `${shot.width} × ${shot.height} px`,
      images: [{ mime: shot.mime, data: shot.data }],
      thumb: shot.thumb,
      shot: shot.id,
    };
  }

  /* ========================================================== the tool loop */

  /** Ask the user (the row waits meanwhile). */
  decide(row, c, signal) {
    this.ui.rowStatus(row, 'waiting');
    return this.ui.decide(row, c, signal);
  }

  /**
   * Run one tool call from the model, in this order: request_tool; unknown or denied; the master switch; the tool is
   * off; not on this page; unreadable arguments; invalid arguments; the precheck; the confirmation; the run with its
   * timeout. Returns { id, name, content, changed, images? } — content is what the model reads.
   */
  async executeTool(call, { reply, signal, actions }) {
    const ui = this.ui;
    const result = (content, changed = false) => ({ id: call.id, name: call.name, content, changed });

    if (call.name === REQUEST_TOOL) {
      const want = this.findTool(String(call.arguments?.name || ''));
      if (!want) return result(`There is no tool named "${call.arguments?.name}".`);
      if (toolEnabled(want, this.store.get())) return result(`${want.name} is already turned on: call it.`);
      if (want.builtin === 'vision') {
        // Asking to look at the screen: the user can allow it once (the screenshot comes back straight away).
        const row = ui.row(reply, { title: want.title, call: formatCall(want.name, {}) });
        return this.finishBuiltin(want, () => this.agentScreenshot(row, signal, { allowed: false }), { row, actions, result, args: {} });
      }
      const title = `Turn on ${want.title}`;
      const row = ui.row(reply, { title, call: formatCall(want.name, {}) });
      const reason = call.arguments?.reason ? String(call.arguments.reason).slice(0, 300) : '';
      const choice = await this.decide(row, card('request', { tool: toolInfo(want), reason }), signal);
      // The row rolls down like any other: what the model asked for and what it was told.
      const answer = (status, content) => {
        const detail = callDetail({ call, args: call.arguments || {}, result: content });
        ui.rowDetail(row, { name: REQUEST_TOOL, title, status, detail });
        actions.push({ call: `turn on ${want.name}`, title, status, name: REQUEST_TOOL, detail: savedDetail(detail) });
        return result(content);
      };
      if (choice === 'on') {
        this.setToolEnabled(want.name, true);
        ui.rowStatus(row, 'ok', 'Turned on');
        ui.rowLabel(row, 'Turned on');
        return answer('ok', `The user turned on ${want.name}. You can call it now.`);
      }
      ui.rowStatus(row, 'off');
      return answer('declined', `The user kept ${want.name} turned off. Do not call it; explain how they can do it themselves, or that they can turn it on in Settings > Tools.`);
    }

    const settings = this.store.get();
    const tool = this.findTool(call.name, settings);
    if (!tool) return result(this.hiddenReason(call.name) || `There is no tool named "${call.name}". Use only the tools you were given.`);
    const decision = decidePermission(tool, this.policy());
    const v = validateArgs(tool, call.arguments);
    const row = ui.row(reply, { title: tool.title, call: formatCall(tool.name, v.args) });
    let entry = null;
    let problem = '';
    const record = (status, summary = '') => {
      ui.rowStatus(row, status, summary);
      entry = { call: formatCall(tool.name, v.args), title: tool.title, status, summary: String(summary).slice(0, 200) };
      actions.push(entry);
      this.emit('tool', { name: tool.name, args: v.args, status, result: summary });
    };
    // Every way out after the row exists: the row rolls down to show the arguments and what the model was told.
    const reply_ = (content, changed = false) => {
      const detail = callDetail({ call, args: v.args, problem, result: content });
      ui.rowDetail(row, { name: tool.name, title: tool.title, status: entry?.status || 'error', detail });
      if (entry) Object.assign(entry, { name: tool.name, detail: savedDetail(detail) });
      return result(content, changed);
    };

    if (!settings.toolsEnabled && !tool.builtin) { record('off'); return reply_('Tools are switched off by the user (Settings > Tools).'); }
    let consented = false;
    if (tool.builtin === 'vision' && !toolEnabled(tool, settings)) {
      return this.finishBuiltin(tool, () => this.agentScreenshot(row, signal, { allowed: false }), { row, actions, result, args: v.args });
    }
    if (tool.builtin && !toolEnabled(tool, settings)) { record('off'); return reply_(tool.offMessage || 'Saving memories is switched off by the user (Settings > Memory).'); }
    if (!toolEnabled(tool, settings)) {
      const choice = await this.decide(row, card('turn-on', { tool: toolInfo(tool) }), signal);
      if (choice !== 'on') { record('off'); return reply_(`${tool.name} is turned off and the user kept it off. Do not call it again.`); }
      this.setToolEnabled(tool.name, true);
      consented = true;
    }
    if (!toolAvailable(tool, this.ctx.page?.id)) {
      record('skipped', 'Not available on this screen');
      return reply_(`${tool.name} is not available on this screen${tool.pages.length ? ` (it works on: ${tool.pages.join(', ')})` : ''}.`);
    }
    if (call.argsError) {
      // The arguments could not be read (often: the reply hit Max reply tokens mid-call). Say so; do not guess them.
      const p = argumentsProblem(call, { cutOff: call.cutOff, maxTokens: settings.maxOutputTokens });
      problem = p.summary;
      record('error', p.summary);
      return reply_(p.content);
    }
    if (!v.ok) {
      problem = `Invalid arguments: ${v.errors.join('; ')}.`;
      record('error', v.errors.join('; '));
      return reply_(`Invalid arguments: ${v.errors.join('; ')}. Check the tool's parameters and try again.`);
    }

    // A built-in may refuse a call before the user is asked (e.g. creating a file that exists): no card for a call
    // that cannot work.
    if (typeof tool.precheck === 'function') {
      let why = '';
      try { why = (await tool.precheck(v.args)) || ''; } catch { why = ''; }
      if (why) { problem = why; record('error', why); return reply_(`Not run: ${why}`); }
    }
    // Permissions (core/permissions.js): deny was handled by findTool; ask / allow / the user's settings decide here.
    const ask = needsConfirmation({ tool, decision, settings, allowedForChat: this.allowedTools.has(tool.name) });
    if (ask && (!consented || tool.effect === 'system')) {
      let confirm = null;
      try { confirm = (await tool.confirm?.(v.args)) || null; } catch { confirm = null; }
      const c = { ...card('confirm', { tool: toolInfo(tool), decision: decision || '', confirm }), buttons: confirmButtons(tool, decision) };
      const choice = await this.decide(row, c, signal);
      if (choice === 'cancel') { record('skipped'); return reply_('Stopped by the user.'); }
      if (choice === 'skip') { record('declined'); return reply_(`The user declined to run ${tool.name}. Do not call it again for this request.`); }
      if (choice === 'chat') this.allowedTools.add(tool.name);
    }

    ui.rowStatus(row, 'running');
    reply.status = `Running ${tool.title}…`;
    ui.paint(reply);
    if (tool.builtin) return this.finishBuiltin(tool, () => tool.run(v.args, { row, signal }), { row, actions, result, args: v.args });
    try {
      let timer;
      const value = await Promise.race([
        Promise.resolve().then(() => tool.run(v.args, { host: this.host, agent: this.api, signal, call: { id: call.id, name: tool.name } })),
        new Promise((_, reject) => { timer = setTimeout(() => reject(new Error(`no result after ${Math.round(tool.timeoutMs / 1000)} s`)), tool.timeoutMs); }),
      ]).finally(() => clearTimeout(timer));
      const content = serializeResult(value);
      record('ok', content);
      return reply_(content, tool.effect !== 'read');
    } catch (e) {
      const msg = String(e?.message || e);
      record('error', msg);
      return reply_(`Error: ${msg}`, tool.effect !== 'read');
    }
  }

  /**
   * Run a built-in tool (memory, screenshot, skills, workspace) and turn what it returns — { content, status?,
   * summary?, images?, thumb?, shot?, undo?, label? } — into the row, the action record and the result for the model.
   */
  async finishBuiltin(tool, run, { row, actions, result, args }) {
    const ui = this.ui;
    let out;
    try { out = await run(); } catch (e) { out = { status: 'error', content: `Error: ${e?.message || e}`, summary: String(e?.message || e) }; }
    const status = out.status || 'ok';
    const summary = String(out.summary ?? out.content ?? '').slice(0, 200);
    ui.rowStatus(row, status, summary);
    if (out.label && status === 'ok') ui.rowLabel(row, out.label);
    if (out.thumb) ui.rowShot(row, { id: out.shot, thumb: out.thumb });
    if (typeof out.undo === 'function' && status === 'ok') ui.rowUndo(row, out.undo);
    const detail = callDetail({ call: { arguments: args }, args, result: String(out.content ?? 'Done.') });
    ui.rowDetail(row, { name: tool.name, title: tool.title, status, detail });
    actions.push({ call: formatCall(tool.name, args), title: tool.title, status, summary, name: tool.name, detail: savedDetail(detail), ...(out.thumb ? { thumb: out.thumb, shot: out.shot } : {}) });
    this.emit('tool', { name: tool.name, args, status, result: summary });
    const r = result(String(out.content ?? 'Done.'));
    if (Array.isArray(out.images) && out.images.length) r.images = out.images;
    return r;
  }

  /**
   * One question to the model, with tool rounds: stream a reply; if it asks for tools, run them (asking the user
   * where the settings say so), send the results back, and repeat until it answers or the step limit is reached.
   * Returns the last step's result ({ provider, label, model, toolCalls, truncated?, … }).
   */
  async runModel({ settings, baseSystem, messages, reply, signal, actions, snapshotHash }) {
    const ui = this.ui;
    const turnId = uid('t');
    const toolTurns = [];
    let lastHash = snapshotHash;
    let mode = this.toolMode(settings);
    let result = null;
    const onEvent = (e) => {
      if (e.type === 'text') reply.raw += e.text;
      else if (e.type === 'reasoning') reply.native += e.text;
      else if (e.type === 'notice') ui.notice(reply, e.text);
      else if (e.type === 'status') reply.status = e.text;
      ui.paint(reply);
    };

    for (let step = 0; step < MAX_ROUNDS; step++) {
      const s = this.store.get();
      const set = mode === 'off' ? { classes: { callable: [], off: [], elsewhere: [] }, active: false, appOff: false } : this.toolSet(s);
      const { classes } = set;
      const toolsText = mode === 'off' ? '' : buildToolPrompt({ classes, mode, enabled: set.active, appOff: set.appOff });
      const native = mode === 'native' && set.active;
      const specs = native ? [...toolSpecs(classes.callable), ...(classes.off.length ? [requestToolSpec(classes.off)] : [])] : [];
      const stepStart = reply.raw.length;
      const nativeStart = reply.native.length;
      try {
        result = await this.chat({
          settings,
          keyFor: this.keyFor,
          system: buildSystemPrompt({ ...baseSystem, toolsText, memoryText: this.memoryPrompt(s), skillsText: this.skillsPrompt(mode !== 'off'), vision: this.seesImages(s), screenshots: this.visionOn(s) }),
          messages: mode === 'text' ? [...messages, ...textTurnMessages(toolTurns)] : messages,
          signal,
          relayHeaders: this.relayHeaders,
          tools: specs,
          toolTurns: native ? toolTurns : [],
          turnId,
          onEvent,
        });
      } catch (e) {
        // auto mode: a model/server that refuses tool definitions gets the text protocol instead.
        if (native && s.toolMode === 'auto' && specs.length && reply.raw.length === stepStart && e instanceof AiError && e.code !== 'cancelled'
          && e.code !== 'auth' && e.code !== 'network' && e.code !== 'timeout' && /tool|function/i.test(e.message)) {
          this.textTools.add(this.modelKey(settings));
          mode = 'text';
          ui.notice(reply, 'This model does not accept tool calls, so tools are described to it as text blocks.');
          step -= 1;
          continue;
        }
        if (e && typeof e === 'object' && (messages.some((m) => m.images) || toolTurns.some((t) => t.results.some((r) => r.images)))) e.hadImages = true;
        if (e && typeof e === 'object' && messages.some((m) => m.role === 'user' && m.content.includes('<attached_file '))) e.hadFiles = true;
        throw e;
      }

      const stepRaw = reply.raw.slice(stepStart);
      let stepText = splitReasoning(stepRaw).text.trim();
      let calls = result.toolCalls || [];
      if (mode === 'text' && set.active) {
        const parsed = parseTextToolCalls(stepText, `t${step}_`, { cutOff: !!result.truncated });
        calls = parsed.calls;
        if (calls.length) {
          reply.raw = reply.raw.slice(0, stepStart) + parsed.text;
          stepText = parsed.text;
          ui.paint(reply, true);
        }
      }
      // A reply that hit Max reply tokens while writing a call: that call's arguments are cut off (executeTool says so).
      const lastCall = calls[calls.length - 1];
      if (result.truncated && lastCall?.argsError) lastCall.cutOff = true;
      if (!calls.length || mode === 'off') {
        if (result.truncated) {
          ui.notice(reply, `The reply reached Max reply tokens (${nf(settings.maxOutputTokens)}) and was cut off. Raise it in Settings > Agent, or ask for less at once.`);
        }
        // After tool rounds, some local models (seen with LM Studio + a Qwen 3.5 9B) put their whole closing answer
        // in the reasoning channel and leave the text empty. Then that last step's reasoning is what they said.
        if (toolTurns.length && !stepText) {
          const said = (reply.native.slice(nativeStart) || splitReasoning(stepRaw).reasoning).trim();
          if (said) {
            reply.native = reply.native.slice(0, nativeStart);
            reply.raw = reply.raw.slice(0, stepStart) + said;
            ui.paint(reply, true);
          }
        }
        break;
      }
      const agentSteps = this.agent().maxToolSteps;
      const maxSteps = agentSteps ? Math.min(s.maxToolSteps, agentSteps) : s.maxToolSteps;
      if (step >= maxSteps) {
        ui.notice(reply, maxSteps < s.maxToolSteps
          ? `Stopped after ${maxSteps} tool steps (the ${this.agent().title} agent's limit).`
          : `Stopped after ${maxSteps} tool steps (Settings > Tools > Max tool steps).`);
        break;
      }

      const results = [];
      let changed = false;
      for (const call of calls) {
        if (signal?.aborted) break;
        const r = await this.executeTool(call, { reply, signal, actions });
        results.push(r);
        changed = changed || r.changed;
      }
      if (signal?.aborted) throw new AiError('cancelled', 'Cancelled.');
      // Send the screen back with the results when the actions changed it, so the model sees their effect.
      if (changed && this.shareOn(settings) && this.ctx.hasContent && results.length) {
        try {
          const snap = await this.ctx.snapshot();
          if (!snap.empty && snap.hash !== lastHash) {
            results[results.length - 1].content += `

[The screen after these actions]
${snapshotBlock(snap)}`;
            lastHash = snap.hash;
          }
        } catch { /* the content hook failed: the results still go back */ }
      }
      toolTurns.push({ text: stepText, calls: calls.map(wireCall), results: results.map(({ id, name, content, images }) => (images ? { id, name, content, images } : { id, name, content })) });
      if (reply.raw && !reply.raw.endsWith('\n\n')) reply.raw += '\n\n';
      reply.status = 'Continuing…';
      ui.paint(reply);
    }
    return result;
  }

  /* ============================================================ a question */

  /** Throws the configuration problem (missing key or model) before anything is added to the conversation. */
  checkTarget(settings) {
    resolveTarget(settings, this.keyFor);
  }

  /**
   * A question starts: the snapshot plan, the user's message (added to the transcript; "/skill …" activates that
   * skill), and what its first request sends. `imageFor(shot)` gives an image's data while it is in memory.
   * Returns { userMsg, plan, snapshot, vision, baseSystem, messages }.
   */
  async beginTurn(text, { shots = [], files = [], settings = this.store.get(), imageFor = null } = {}) {
    const vision = this.seesImages(settings);
    const share = this.shareOn(settings);
    const snapshot = share && this.ctx.hasContent ? await this.ctx.snapshot() : null;
    const plan = planTurn({ messages: this.conv.messages, snapshot, historyMessages: settings.historyMessages, share, force: this.forceReread });
    const userMsg = { id: uid('u'), role: 'user', content: text, at: Date.now(), sync: plan.attach ? 'attached' : plan.reason };
    // "/proofreading …" activates that skill for this conversation (the message goes to the model as typed).
    const slash = parseSlashCommand(text, this.skillsForAgent().map((sk) => sk.name));
    if (slash && this.activateSkill(slash.name, { via: 'slash' })) userMsg.skill = slash.name;
    if (plan.attach) {
      userMsg.snapshot = {
        hash: snapshot.hash, text: snapshot.text, pageId: snapshot.pageId, pageTitle: snapshot.pageTitle,
        chars: snapshot.chars, totalChars: snapshot.totalChars, truncated: snapshot.truncated, at: snapshot.at,
      };
    } else if (plan.reason === 'unchanged') {
      userMsg.snapshotRef = snapshot.hash;
    }
    if (vision && shots.length) userMsg.shots = shots.map(({ id, thumb, width, height, name }) => ({ id, thumb, width, height, ...(name ? { name, kind: 'image' } : {}) }));
    if (files.length) userMsg.files = files.map(({ reading, ...f }) => f);
    this.forceReread = false;
    this.conv.messages.push(userMsg);

    const texts = await this.contextTexts(settings);
    const baseSystem = await this.baseSystemFor(settings, texts);
    const messages = buildRequestMessages({
      messages: this.conv.messages,
      historyMessages: settings.historyMessages,
      viewText: texts.viewText,
      unchangedHash: plan.reason === 'unchanged' ? snapshot.hash : null,
      imageFor: vision ? imageFor : null,
    });
    this.emit('send', { text, attached: plan.attach, reason: plan.reason, hash: snapshot?.hash || null, images: userMsg.shots?.length || 0, files: (userMsg.files || []).map((f) => f.name) });
    return { userMsg, plan, snapshot, vision, baseSystem, messages };
  }

  /**
   * A question ended. With an answer or actions taken, the reply joins the transcript (the app changed, so the history
   * must say so); with neither, the question is taken out again, so the transcript and the sync state are as before.
   * Returns { kept, answer, cancelled, meta?, message? }.
   */
  endTurn({ turn, reply, actions = [], result = null, error = null, loaded = '' }) {
    const answer = splitReasoning(reply.raw).text.trim();
    const cancelled = error?.code === 'cancelled';
    if (answer || actions.length) {
      const who = result ? `${result.label}${result.model ? ` · ${result.model}` : loaded ? ` · ${loaded}` : ''}` : '';
      const meta = cancelled ? 'Stopped' : who;
      const message = { id: uid('a'), role: 'assistant', content: cancelled && answer ? `${answer}\n\n_(stopped)_` : answer, at: Date.now(), meta };
      if (actions.length) message.actions = actions;
      this.conv.messages.push(message);
      this.emit('reply', { text: answer, provider: result?.provider, model: result?.model, stopped: cancelled, actions });
      return { kept: true, answer, cancelled, meta, message };
    }
    this.conv.messages = this.conv.messages.filter((m) => m !== turn.userMsg);
    if (error && !cancelled) this.emit('error', { error });
    return { kept: false, answer, cancelled };
  }

  /* ========================================================= context flag */

  /** The sync flag for the screen as it is now. No side effects. */
  async computeStatus(snapshot) {
    const settings = this.store.get();
    this.ctx.setMaxChars(settings.maxContextChars);
    const share = this.shareOn(settings);
    let snap = snapshot;
    if (!snap && share && this.ctx.hasContent) {
      try { snap = await this.ctx.snapshot(); } catch { snap = null; }
    }
    const st = contextState({ messages: this.conv.messages, snapshot: this.ctx.hasContent ? snap : null, historyMessages: settings.historyMessages, share, force: this.forceReread });
    return {
      state: st.state,
      pending: st.pending,
      reason: st.reason,
      hash: st.currentHash,
      syncedHash: st.syncedHash,
      pageId: snap?.pageId || this.ctx.page?.id || null,
      title: snap?.pageTitle || '',
      chars: snap?.totalChars || 0,
      truncated: !!snap?.truncated,
      forced: this.forceReread,
    };
  }

  /* ========================================================== saved chats */

  readChats() {
    if (!this.storage) return [];
    try {
      const v = JSON.parse(this.storage.getItem(this.chatsKey) || '[]');
      return Array.isArray(v) ? v.filter((c) => c && c.id) : [];
    } catch {
      return [];
    }
  }

  writeChats(list) {
    if (!this.storage) return false;
    const attempt = (l) => { try { this.storage.setItem(this.chatsKey, JSON.stringify(l)); return true; } catch { return false; } };
    if (attempt(list)) return true;
    // Over the storage quota: the text of files in the other chats goes first, then the oldest chats.
    const lean = list.map((c) => (c.id === this.conv.id ? c : {
      ...c, messages: (c.messages || []).map((m) => (m.files ? { ...m, files: m.files.map(({ text, ...f }) => f) } : m)),
    }));
    for (let n = lean.length; n > 0; n = Math.floor(n * 0.7)) {
      if (attempt(lean.slice(0, n))) return true;
    }
    return false;
  }

  deriveTitle() {
    const first = this.conv.messages.find((m) => m.role === 'user');
    const t = String(first?.content || '').replace(/\s+/g, ' ').trim();
    return !t ? 'New chat' : t.length > 48 ? `${t.slice(0, 47).trim()}…` : t;
  }

  /**
   * Save the transcript. Snapshot TEXT is not kept (it can be large, and the screen can be read again); fingerprints
   * and titles are. Attached files keep their text (it cannot be read again), unless storage runs out (writeChats).
   * Returns the chat's id, or null when nothing was saved.
   */
  persist() {
    if (!this.conv.messages.length || this.saveChats === false || !this.storage) return null;
    if (!this.conv.id) this.conv.id = uid('c');
    const messages = this.conv.messages.slice(-MESSAGE_LIMIT).map((m) => {
      const out = { role: m.role, content: m.content, at: m.at };
      if (m.sync) out.sync = m.sync;
      if (m.meta) out.meta = m.meta;
      if (m.actions) out.actions = m.actions;
      if (m.shots) out.shots = m.shots;          // thumbnails only: the full images are never stored
      if (m.files) out.files = m.files;
      if (m.skill) out.skill = m.skill;
      if (m.snapshotRef) out.snapshotRef = m.snapshotRef;
      if (m.snapshot) {
        const { text, ...rest } = m.snapshot;
        out.snapshot = rest;
      }
      return out;
    });
    const list = this.readChats();
    const now = Date.now();
    const existing = list.find((c) => c.id === this.conv.id);
    // Which agent the chat was with, and the skills it activated: loading it brings both back.
    const extra = { agent: this.agentName || '', skills: [...this.conv.skills] };
    if (existing) Object.assign(existing, { title: this.deriveTitle(), updatedAt: now, messages, ...extra });
    else list.push({ id: this.conv.id, title: this.deriveTitle(), createdAt: now, updatedAt: now, messages, page: this.ctx.page?.id || '', ...extra });
    list.sort((a, b) => (b.updatedAt || 0) - (a.updatedAt || 0));
    this.writeChats(list.slice(0, CHAT_LIMIT));
    return this.conv.id;
  }

  /** Make a saved chat the conversation (with its agent, when that still exists). Returns { record, agentChanged } or null. */
  loadChat(id) {
    const record = this.readChats().find((c) => c.id === id);
    if (!record) return null;
    this.allowedTools.clear();
    this.conv = {
      id: record.id,
      messages: (record.messages || []).map((m) => ({ ...m, id: uid(m.role === 'user' ? 'u' : 'a'), role: m.role === 'user' ? 'user' : 'assistant', content: String(m.content || '') })),
      skills: Array.isArray(record.skills) ? record.skills.map(String) : [],
    };
    let agentChanged = false;
    if (typeof record.agent === 'string' && record.agent !== this.agentName && (record.agent === '' || this.agents().some((a) => a.name === record.agent))) {
      this.agentName = record.agent;
      agentChanged = true;
    }
    return { record, agentChanged };
  }

  /** Delete a saved chat (the conversation itself goes on, unsaved, when it is the one deleted). */
  deleteChat(id) {
    this.writeChats(this.readChats().filter((c) => c.id !== id));
    if (id === this.conv.id) this.conv.id = null;
  }

  /** Start an empty conversation (saving the current one first unless `save` is false). */
  newConversation({ save = true } = {}) {
    this.allowedTools.clear();
    if (save) this.persist();
    this.conv = { id: null, messages: [], skills: [] };
    this.forceReread = false;
  }

  /**
   * The active agent changes to `name` and a new conversation starts (save the current one first: persist(), so it
   * keeps the agent it was with). Returns { agent, changed }, or null when there is no such agent.
   */
  switchAgent(name) {
    const target = this.agents().find((a) => a.name === name);
    if (!target) return null;
    if (target.name === this.agent().name) return { agent: target, changed: false };
    this.agentName = target.implicit ? '' : target.name;
    this.newConversation({ save: false });
    return { agent: target, changed: true };
  }

  /** Lock / sign-out: drop the transcript, the decisions taken in it and the remembered fallbacks. Nothing is saved. */
  reset() {
    this.conv = { id: null, messages: [], skills: [] };
    this.forceReread = false;
    this.allowedTools.clear();
    this.textTools.clear();
  }
}
