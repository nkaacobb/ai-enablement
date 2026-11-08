// AI Enablement runtime (the agent drawer) — public entry point.
//
//   import { createAiAgent } from './ai-agent/ai-agent.js';   // plus <link rel="stylesheet" href="./ai-agent/ai-agent.css">
//
//   const agent = createAiAgent({
//     appId: 'my-app',
//     title: 'Assistant',
//     app:  { name: 'My App', purpose: 'What the app is for', capabilities: [...], limits: [...] },
//     page: { id: 'orders', title: 'Orders', purpose: 'What this page is for',
//             content: () => visibleRows,            // what is on screen (fingerprinted)
//             view:    () => ({ selection, filters }) },  // volatile UI state (not fingerprinted)
//     toggle: '#ask-ai',
//   });
//   agent.contextChanged();          // call whenever the screen content may have changed
//   agent.setPage({ ... });          // call on navigation
//
// The application's capabilities — tools, toolsets, skills, agents, permissions — can be given inline (`tools`,
// `skills`, `agents`, …) or loaded from its capability folder: createAiAgent({ capabilities: 'ai/index.json',
// host: app }) (see core/capabilities.js). `host` is what tools call: run(args, { host }).
//
// Everything is vanilla ES modules with no dependencies and no build step. See ai-agent.d.ts for the full API and
// the skill's references/ folder for the design.

import { ContextManager } from './core/context.js';
import { createSettingsStore, safeStorage } from './core/settings.js';
import { DEFAULT_SYSTEM_PROMPT } from './core/prompt.js';
import { createSettingsPanel } from './ui/settings-panel.js';
import { AgentDrawer } from './ui/drawer.js';
import { debounce } from './ui/dom.js';
import { isDevHost } from './ui/layout-check.js';
import { probeRelay, relayDefaults, adjustForRelay, mergeSettings } from './core/relay-probe.js';
import { normalizeTool, toolEnabled, toolAvailable, toolsConfigPatch, exportToolsConfig, validateArgs, serializeResult, toolSpecs, toMcpTool } from './core/tools.js';
import { createMemoryStore } from './core/memory.js';
import { loadCapabilities, readToolModule, linkToolsets, checkReferences, defaultFetchText } from './core/capabilities.js';
import { normalizeSkill, parseSkill } from './core/skills.js';
import { normalizeAgent, implicitAgent } from './core/agents.js';
import { normalizePolicy, mergePolicies, decidePermission } from './core/permissions.js';
import { probeWorkspace, workspaceClient, DEFAULT_WORKSPACE_URL } from './core/workspace.js';

export { PROVIDERS, PROVIDER_IDS } from './core/providers.js';
export { DEFAULT_SYSTEM_PROMPT } from './core/prompt.js';
export { renderMarkdown } from './ui/markdown.js';
export { hashText, stableStringify } from './core/hash.js';
export { AiError } from './core/transport.js';
export { parseBlockValues } from './core/blocks.js';
export { probeRelay } from './core/relay-probe.js';
export { setControlValue } from './ui/dom.js';
export { parseMemoryFile, exportMemoryFile } from './core/memory.js';
export { toMcpTool } from './core/tools.js';
export { fromJsonSchema } from './core/schema.js';
export { loadCapabilities } from './core/capabilities.js';
export { parseSkill } from './core/skills.js';
export { parseAgent } from './core/agents.js';
export { parseFrontmatter } from './core/frontmatter.js';

export const VERSION = '1.7.0';

/** Skills that ship with the runtime (in its skills/ folder) and when they are offered. */
const BUILTIN_SKILLS = [{ folder: 'create-tool', when: 'workspace' }];

const isPlainObject = (v) => !!v && typeof v === 'object' && !Array.isArray(v) && typeof v.then !== 'function';

const DEFAULT_WELCOME = 'I can see what is on your screen. Ask me about it — to explain, summarise, check or improve what you are looking at.';

/**
 * Create and mount the agent: a toggle (or floating launcher), the drawer, and the settings modal.
 * @param {import('./ai-agent').AiAgentOptions} options
 * @returns {import('./ai-agent').AiAgent}
 */
export function createAiAgent(options = {}) {
  if (typeof document === 'undefined') throw new Error('createAiAgent() needs a browser document. Call it on the client only.');

  const o = {
    appId: 'app',
    title: 'AI agent',
    placeholder: 'Ask about what is on screen…',
    welcome: DEFAULT_WELCOME,
    suggestions: [],
    toggle: null,
    launcher: true,
    toggleBadge: true,
    push: document.body,
    hotkey: 'mod+i',
    theme: 'auto',
    width: 440,
    watch: 0,
    debounceMs: 300,
    debounceMaxMs: 1000,
    isolateKeys: true,
    dialogs: false,
    devWarnings: 'auto',
    relayProbe: false,
    contextWarnTokens: 3000,
    tools: [],
    toolsConfig: null,
    memory: true,               // notes kept between conversations (Settings > Memory); false = none
    memoryFile: null,           // the app's memory file, e.g. 'ai-memory.json' (URL, object, or function)
    memorySave: null,           // (file) => void | Promise: keep the memories somewhere besides this browser
    screenshots: true,          // screenshots for models that see images (Settings > Vision); false = none
    screenshot: null,           // () => canvas | image | Blob | data URL: the app's own way to capture its view
    screenshotMaxEdge: 1280,    // screenshots are scaled down to this many pixels on their longer edge
    attachments: true,          // the + button (and drag and drop, paste): attach images and files; false = none
    readFile: null,             // (file, { kind, name }) => text | { text } | null: the app's own reader, tried first
    host: null,                 // the object the application's tools call: run(args, { host })
    capabilities: null,         // the capability index: 'ai/index.json' (a URL), or an index object
    agents: [],                 // agents given inline (added to the index's)
    skills: [],                 // skills given inline (added to the index's)
    toolsets: [],               // toolsets given inline (added to the index's)
    permissions: null,          // the application's { allow, ask, deny } rules (merged with the index's)
    agent: '',                  // the agent to start with (else the index's defaultAgent, the one marked default, the first)
    workspace: false,           // development only: true or the URL of the skill's scripts/workspace.mjs server
    codeActions: [],
    replyActions: [],
    defaults: {},
    systemPrompt: DEFAULT_SYSTEM_PROMPT,
    saveChats: true,
    resume: true,
    mount: document.body,
    ...options,
  };
  if (o.push === true) o.push = document.body;
  if (o.push === false) o.push = null;
  o.devWarnings = o.devWarnings === 'auto' ? isDevHost() : !!o.devWarnings;

  const namespace = `${String(o.appId).replace(/[^\w.-]/g, '_')}.ai`;
  const storage = safeStorage('localStorage');
  // `defaults` may be an object, a promise, or a (possibly async) function; the async part is applied by `ready`.
  const syncDefaults = isPlainObject(o.defaults) ? o.defaults : {};
  const store = createSettingsStore({ namespace, defaults: syncDefaults });
  const ctx = new ContextManager({ app: o.app, page: o.page, maxChars: store.get().maxContextChars });

  const listeners = new Map();
  const emit = (event, detail) => {
    for (const fn of [...(listeners.get(event) || [])]) {
      try { fn(detail); } catch (e) { console.error(`[ai-agent] "${event}" listener failed`, e); }
    }
  };
  const on = (event, fn) => {
    if (!listeners.has(event)) listeners.set(event, new Set());
    listeners.get(event).add(fn);
    return () => listeners.get(event)?.delete(fn);
  };

  const defaultPrompt = () => {
    const p = typeof o.systemPrompt === 'function' ? o.systemPrompt() : o.systemPrompt;
    return String(p || DEFAULT_SYSTEM_PROMPT);
  };

  // The system prompt as the next question would send it (the drawer composes it: agent, context, skills, tools…).
  const systemPrompt = async () => {
    const s = store.get();
    return drawer.session.systemFor(s, await drawer.session.baseSystemFor(s));
  };

  let drawer = null;

  /* Memory: notes kept between conversations — the app's file underneath, this browser's changes on top. */
  const memory = o.memory === false ? null : createMemoryStore({ namespace, storage });
  let saveTimer = null;
  const offMemory = memory ? memory.onChange(({ memories, change }) => {
    emit('memory', { memories, change });
    panel?.refreshMemory?.();
    // The app's own persistence (a file on its server, a user profile): every change made here, never the file load.
    if (typeof o.memorySave === 'function' && change?.type !== 'base') {
      clearTimeout(saveTimer);
      saveTimer = setTimeout(() => {
        Promise.resolve().then(() => o.memorySave(memory.export())).catch((e) => console.warn('[ai-agent] memorySave failed; the memories are still kept in this browser.', e));
      }, 400);
    }
  }) : () => {};

  /* Capabilities: tools, toolsets, skills, agents and the permission policy — given inline and/or loaded from the
     capability index (`capabilities`). The index part is replaced as a whole by reload(); inline parts stay. */
  const registerTools = (defs, { page = '' } = {}) => {
    const out = [];
    for (const def of Array.isArray(defs) ? defs : [defs]) {
      try { out.push(normalizeTool(def, { page })); } catch (e) { console.error(`[ai-agent] Tool skipped: ${e.message}`); }
    }
    return out;
  };
  const toolsOption = typeof o.tools === 'function' ? o.tools(o.host) : o.tools;     // a list, or (host) => a list
  const inline = readToolModule({ default: [...(Array.isArray(toolsOption) ? toolsOption : [toolsOption]).filter(Boolean), ...(Array.isArray(o.toolsets) ? o.toolsets : [])] }, { host: o.host });
  if (inline.problems.length && (o.tools?.length || o.toolsets?.length)) for (const pr of inline.problems) console.error(`[ai-agent] ${pr}`);
  const optionTools = new Map(registerTools(inline.tools).map((t) => [t.name, t]));   // inline + tools.register()
  const caps = {
    indexTools: [],
    toolsets: new Map(),
    inlineToolsets: inline.toolsets,
    indexToolsets: [],
    inlineSkills: [],
    indexSkills: [],
    builtinSkills: [],
    inlineAgents: [],
    indexAgents: [],
    inlinePolicy: normalizePolicy(o.permissions ?? undefined, []),
    indexPolicy: { allow: [], ask: [], deny: [] },
    defaultAgent: '',
    inlineProblems: [],
    problems: [],
    workspace: null,
    version: 0,
  };
  for (const def of Array.isArray(o.skills) ? o.skills : []) {
    const r = normalizeSkill(def);
    if (r.skill) caps.inlineSkills.push(r.skill);
    for (const pr of r.problems) caps.inlineProblems.push(`skill ${def?.name || '?'}: ${pr}`);
  }
  for (const def of Array.isArray(o.agents) ? o.agents : []) {
    const r = normalizeAgent(def);
    if (r.agent) caps.inlineAgents.push(r.agent);
    for (const pr of r.problems) caps.inlineProblems.push(`agent ${def?.name || '?'}: ${pr}`);
  }
  const appTools = new Map();
  /** Merge the index's tools with the inline ones (inline wins) and link the toolsets. */
  const rebuildTools = () => {
    appTools.clear();
    for (const t of caps.indexTools) appTools.set(t.name, t);
    for (const t of optionTools.values()) appTools.set(t.name, t);
    const linked = linkToolsets([...appTools.values()], [...caps.indexToolsets, ...caps.inlineToolsets]);
    caps.toolsets = linked.toolsets;
    for (const t of appTools.values()) {
      t.toolsets = linked.membership.get(t.name) || (t.toolset ? [t.toolset] : []);
      if (!t.group && t.toolsets.length) t.group = caps.toolsets.get(t.toolsets[0])?.title || '';
    }
    return linked.problems;
  };
  const byName = (lists) => { const m = new Map(); for (const list of lists) for (const x of list) m.set(x.name, x); return [...m.values()]; };
  const capabilities = {
    skills: () => [...byName([caps.indexSkills, caps.inlineSkills]), ...(caps.workspace ? caps.builtinSkills : [])],
    agents: () => {
      const list = byName([caps.indexAgents, caps.inlineAgents]);
      return list.length ? list : [implicitAgent(o.title)];
    },
    toolsets: () => caps.toolsets,
    policy: () => mergePolicies(caps.indexPolicy, caps.inlinePolicy),
    defaultAgent: () => o.agent || caps.defaultAgent,
    workspace: () => caps.workspace,
    fetchText: defaultFetchText,
    reload: () => reloadCapabilities(),
  };
  rebuildTools();
  const pageToolCache = new WeakMap();
  const toolRegistry = {
    all() {
      const page = ctx.page;
      let pageTools = [];
      if (page && Array.isArray(page.tools) && page.tools.length) {
        pageTools = pageToolCache.get(page);
        if (!pageTools) { pageTools = registerTools(page.tools, { page: page.id }); pageToolCache.set(page, pageTools); }
      }
      const merged = new Map(appTools);
      for (const t of pageTools) merged.set(t.name, t);
      return [...merged.values()];
    },
    get(name) { return toolRegistry.all().find((t) => t.name === name) || null; },
  };

  const getContextInfo = async () => {
    const s = store.get();
    ctx.setMaxChars(s.maxContextChars);
    const status = await drawer.computeStatus();
    const [texts, snapshot, system] = await Promise.all([drawer.session.contextTexts(s), ctx.snapshot(), systemPrompt()]);
    const set = drawer.session.toolMode(s) === 'native' ? drawer.session.toolSet(s) : null;
    const callable = set && set.active ? set.classes.callable : [];
    return {
      status: { state: status.state, currentHash: status.hash, syncedHash: status.syncedHash },
      appText: texts.appText, pageText: texts.pageText, viewText: texts.viewText, snapshot, system, share: texts.share, hasContent: ctx.hasContent,
      toolsJson: callable.length ? JSON.stringify(toolSpecs(callable)) : '',
    };
  };

  let relay = null;
  let panel = null;
  panel = createSettingsPanel({
    store, defaultPrompt, getContextInfo, relayHeaders: o.relayHeaders, theme: o.theme, title: o.title, mount: o.mount,
    isolate: o.isolateKeys !== false, warnTokens: o.contextWarnTokens, relayInfo: () => relay, devWarnings: o.devWarnings,
    getTools: () => toolRegistry.all(), pageId: () => ctx.page?.id,
    getPolicy: () => (drawer ? drawer.session.policy() : capabilities.policy()),
    getCapabilities: () => (drawer ? drawer.session.capabilityInfo() : null),
    memory,
    attachments: o.attachments !== false,
    vision: () => (drawer?.capture || drawer?.attachOn ? {
      method: drawer.capture ? drawer.capture.method() : 'none', shots: !!drawer.capture, uploads: !!drawer.attachOn,
      live: !!drawer.capture?.live(), stop: () => drawer.capture?.stop(),
      model: drawer.probeInfo?.model || '', modelSees: drawer.probeInfo?.vision,
    } : null),
  });
  let onDialogChange = () => {};
  drawer = new AgentDrawer({
    ctx, store, panel, emit, defaultPrompt,
    options: { ...o, storage, session: safeStorage('sessionStorage'), onDialogChange: () => onDialogChange(), toolRegistry, memoryStore: memory, caps: capabilities },
  });

  // Trailing debounce with a max wait: continuous updates (animation, simulation, live data) still refresh the flag
  // at least every debounceMaxMs.
  const changed = debounce(() => drawer.refreshStatus(), o.debounceMs, { maxWait: Math.max(0, Number(o.debounceMaxMs) || 0) });
  onDialogChange = () => changed();

  // A JSON file of the app (tool config, memory): an object, a URL, or a (possibly async) function.
  const loadJson = async (source) => {
    let v = typeof source === 'function' ? await source() : await source;
    if (typeof v === 'string') {
      const res = await fetch(new URL(v, globalThis.location?.href).href, { cache: 'no-store', credentials: 'same-origin' });
      if (!res.ok) throw new Error(`HTTP ${res.status} for ${v}`);
      v = await res.json();
    }
    return v;
  };

  // Async defaults, the tool config file, the memory file and the relay probe. Questions wait for this (drawer.ready);
  // nothing else does.
  const needsAsync = !isPlainObject(o.defaults) && o.defaults != null;

  /** Load (or reload) the capability index; replaces its tools, toolsets, skills, agents and policy. */
  const loadIndex = async ({ bust = '' } = {}) => {
    if (!o.capabilities) return null;
    const r = await loadCapabilities(o.capabilities, { host: o.host, bust });
    caps.indexTools = registerTools(r.tools);
    caps.indexToolsets = r.toolsets;
    caps.indexSkills = r.skills;
    caps.indexAgents = r.agents;
    caps.indexPolicy = r.permissions;
    caps.defaultAgent = r.defaultAgent;
    return r;
  };
  /** Rebuild after a load, check how the parts refer to each other, and report the problems. */
  const settleCapabilities = (loaded, { quiet = false } = {}) => {
    const problems = [...caps.inlineProblems, ...(loaded?.problems || []), ...rebuildTools()];
    problems.push(...checkReferences({ agents: capabilities.agents().filter((a) => !a.implicit), tools: [...appTools.values()], toolsets: caps.toolsets, skills: capabilities.skills(), defaultAgent: capabilities.defaultAgent() }));
    caps.problems = problems;
    caps.version += 1;
    if (problems.length && !quiet) console.warn(`[ai-agent] Capability problems:\n- ${problems.join('\n- ')}`);
    drawer.refreshAgents();
    panel.refreshTools?.();
    emit('capabilities', { problems: [...problems], version: caps.version });
    return problems;
  };
  /** Development: is the skill's workspace server there? Then its tools and the built-in authoring skill appear. */
  const connectWorkspace = async () => {
    if (!o.workspace) return;
    const info = await probeWorkspace(o.workspace === true ? DEFAULT_WORKSPACE_URL : String(o.workspace));
    if (!info) return;
    caps.workspace = { info, client: workspaceClient(info) };
    const loaded = await Promise.all(BUILTIN_SKILLS.filter((b) => b.when === 'workspace').map(async (b) => {
      const folderUrl = new URL(`./skills/${b.folder}/`, import.meta.url).href;
      try {
        return parseSkill(await defaultFetchText(`${folderUrl}SKILL.md`), { base: folderUrl, folder: b.folder, source: 'builtin' }).skill;
      } catch (e) {
        console.warn(`[ai-agent] The built-in skill "${b.folder}" could not be loaded.`, e);
        return null;
      }
    }));
    caps.builtinSkills = loaded.filter(Boolean);
    console.info(`[ai-agent] Workspace connected (${info.root || 'app'}; the agent may write in ${info.writable.join(', ') || 'nothing'}).`);
    emit('workspace', { ...info });
  };
  /** The tool config as settings defaults over `base`, and the names in it that are not tools (a likely mistake). */
  const readToolsConfig = async (source, base) => {
    const json = await loadJson(source);
    const patch = toolsConfigPatch(json);
    const unknown = Object.keys(json?.tools && typeof json.tools === 'object' ? json.tools : {}).filter((n) => !appTools.has(n));
    return {
      defaults: { ...base, ...patch, toolStates: { ...(base.toolStates || {}), ...(patch.toolStates || {}) } },
      problems: unknown.map((n) => `tool config: "${n}" is not a tool of this application (key it by the tool's name).`),
    };
  };
  /**
   * agent.capabilities.reload(): the index again, with fresh copies of every file and module (after a change), and
   * the tool config and memory file it names, so what the workspace just wrote takes effect without a page reload.
   */
  const reloadCapabilities = async () => {
    await ready;
    const loaded = await loadIndex({ bust: Date.now().toString(36) });
    rebuildTools();
    const extra = [];
    const toolsConfig = o.toolsConfig || loaded?.toolsConfig;
    if (toolsConfig) {
      try {
        const r = await readToolsConfig(toolsConfig, appliedDefaults);
        appliedDefaults = r.defaults;
        store.setDefaults(appliedDefaults);
        extra.push(...r.problems);
      } catch (e) {
        extra.push(`tool config: could not be loaded (${e?.message || e}).`);
      }
    }
    const memoryFile = o.memoryFile || loaded?.memory;
    if (memory && memoryFile) {
      try { memory.setBase(await loadJson(memoryFile)); } catch (e) { extra.push(`memory file: could not be loaded (${e?.message || e}).`); }
    }
    caps.inlineProblems = [...caps.inlineProblems.filter((p) => !p.startsWith('tool config:')), ...extra];
    return settleCapabilities(loaded, { quiet: true });
  };
  let appliedDefaults = syncDefaults;   // the settings defaults as last applied (async defaults, tool config, relay)

  const pendingAsync = needsAsync || o.relayProbe || o.toolsConfig || (memory && o.memoryFile) || o.capabilities || o.workspace;
  const ready = pendingAsync ? (async () => {
    let d = syncDefaults;
    if (needsAsync) {
      try {
        const v = await (typeof o.defaults === 'function' ? o.defaults() : o.defaults);
        if (isPlainObject(v)) d = v;
      } catch (e) {
        console.error('[ai-agent] The `defaults` option failed; using the built-in defaults.', e);
      }
    }
    // The capability index may name the tool config and the memory file, so it comes first.
    const [loaded] = await Promise.all([
      loadIndex().catch((e) => { console.error('[ai-agent] The capability index could not be loaded.', e); return null; }),
      connectWorkspace().catch((e) => { console.warn('[ai-agent] The workspace check failed.', e); }),
    ]);
    settleCapabilities(loaded);
    const toolsConfig = o.toolsConfig || loaded?.toolsConfig;
    const memoryFile = o.memoryFile || loaded?.memory;
    if (toolsConfig) {
      // The app's tool selection (e.g. ai-tools.json): an object, a URL, or a (possibly async) function.
      try {
        const r = await readToolsConfig(toolsConfig, d);
        d = r.defaults;
        if (r.problems.length && (o.capabilities || o.workspace)) {
          caps.inlineProblems.push(...r.problems);
          caps.problems.push(...r.problems);
          console.warn(`[ai-agent] ${r.problems.join('\n')}`);
        }
      } catch (e) {
        console.warn('[ai-agent] The toolsConfig could not be loaded; tools keep their built-in defaults.', e);
      }
    }
    if (memory && memoryFile) {
      // The app's memory file (e.g. ai-memory.json): the base that this browser's own memories sit on.
      try {
        memory.setBase(await loadJson(memoryFile));
      } catch (e) {
        console.warn('[ai-agent] The memoryFile could not be loaded; only the memories saved in this browser are used.', e);
      }
    }
    if (o.relayProbe) {
      const cfg = typeof o.relayProbe === 'object' ? o.relayProbe : { url: typeof o.relayProbe === 'string' ? o.relayProbe : '' };
      const url = cfg.url || d.relayUrl || '';
      if (!url) {
        console.warn('[ai-agent] relayProbe needs a relay address: relayProbe: "api/relay.php", or defaults.relayUrl.');
      } else {
        const headers = typeof o.relayHeaders === 'function' ? o.relayHeaders() : o.relayHeaders;
        relay = await probeRelay(url, { timeoutMs: cfg.timeoutMs, headers: headers && typeof headers === 'object' ? headers : {} });
        d = mergeSettings(d, relayDefaults(relay));
        store.setAdjust((settings) => adjustForRelay(settings, relay));
        emit('relay', relay);
      }
    }
    appliedDefaults = d;
    store.setDefaults(d);
  })() : Promise.resolve();
  if (!pendingAsync) settleCapabilities(null);
  drawer.ready = ready;

  const offSettings = store.onChange((settings) => {
    drawer.probeInfo = { key: '' };
    drawer.refreshSubtitle();
    drawer.refreshVision();
    drawer.refreshStatus();
    emit('settings', settings);
  });

  const api = {
    open: () => drawer.open(),
    close: () => drawer.close(),
    toggle: () => drawer.toggle(),
    isOpen: () => drawer.isOpen(),
    ask: (text) => drawer.ask(text),
    stop: () => drawer.stop(),
    newChat: () => drawer.newChat(),
    openSettings: (tab = 'model') => panel.open(tab),

    setApp(app) { ctx.setApp(app); changed(); },
    setPage(page) { ctx.setPage(page); drawer.refreshStatus(); },
    setContent(content) { ctx.setContent(content); changed(); },
    setView(view) { ctx.setView(view); },
    contextChanged() { changed(); },
    refreshContext: () => drawer.refreshStatus(),
    getContextStatus: () => ({ ...drawer.status }),
    relayInfo: () => (relay ? { ...relay } : null),
    onContextStatus(fn) {
      const off = on('context', fn);
      try { fn({ ...drawer.status }); } catch (e) { console.error('[ai-agent] onContextStatus listener failed', e); }
      return off;
    },
    rereadPage() { drawer.forceReread = true; return drawer.refreshStatus(); },
    systemPrompt,

    tools: {
      /** Every tool the application has now (app-wide + this page), with its state. */
      list() {
        const s = store.get();
        const policy = drawer.session.policy();
        return toolRegistry.all().map((t) => ({
          name: t.name, title: t.title, description: t.description, effect: t.effect, group: t.group, pages: [...t.pages],
          toolsets: [...(t.toolsets || [])], enabled: toolEnabled(t, s), available: toolAvailable(t, ctx.page?.id),
          permission: decidePermission(t, policy) || 'default', agent: drawer.session.agentHasTool(t.name),
        }));
      },
      /** Add or replace app-wide tools (definitions, lists or toolsets). */
      register(defs) {
        const r = readToolModule({ default: defs }, { host: o.host });
        for (const pr of r.problems) console.error(`[ai-agent] ${pr}`);
        for (const t of registerTools(r.tools)) optionTools.set(t.name, t);
        caps.inlineToolsets = [...caps.inlineToolsets.filter((ts) => !r.toolsets.some((x) => x.name === ts.name)), ...r.toolsets];
        rebuildTools();
        panel.refreshTools?.();
      },
      unregister(name) { optionTools.delete(name); caps.indexTools = caps.indexTools.filter((t) => t.name !== name); rebuildTools(); panel.refreshTools?.(); },
      /** The toolsets: [{ name, title, description, tools: [names] }]. */
      toolsets: () => [...caps.toolsets.values()].map((ts) => ({ ...ts, tools: [...ts.tools] })),
      /** The tools as MCP tool descriptors (name, title, description, inputSchema, annotations). */
      mcp: () => toolRegistry.all().map(toMcpTool),
      /** Turn a tool on or off for this user (saved like the other settings). */
      setEnabled(name, on) { drawer.setToolEnabled(name, on); },
      /** Run a tool directly (tests, scripted checks): arguments are validated; no confirmation, no on/off check. */
      async run(name, args = {}) {
        const t = toolRegistry.get(name);
        if (!t) throw new Error(`No tool named "${name}".`);
        const v = validateArgs(t, args);
        if (!v.ok) throw new Error(`Invalid arguments: ${v.errors.join('; ')}`);
        return serializeResult(await t.run(v.args, { host: o.host, agent: api, signal: undefined, call: { id: 'direct', name } }));
      },
      /** The current selection as the JSON an app ships as its toolsConfig (e.g. ai-tools.json). */
      exportConfig() { return exportToolsConfig(toolRegistry.all(), store.get()); },
    },

    /** The agents: configured workers the user picks from (the application's one implicit agent when none are defined). */
    agents: {
      list: () => capabilities.agents().map((a) => ({
        name: a.name, title: a.title, description: a.description, active: a.name === drawer.agent().name, implicit: !!a.implicit,
        tools: a.tools ? [...a.tools] : null, toolsets: a.toolsets ? [...a.toolsets] : null, skills: a.skills ? [...a.skills] : null,
        model: a.model, memory: a.memory, context: { ...a.context },
      })),
      /** The active agent's name. */
      current: () => drawer.agent().name,
      /** Switch agent (a conversation in progress is saved and a new one starts). false when there is no such agent. */
      use: (name) => drawer.useAgent(name),
    },

    /** Skills: instructions the agent loads when a request matches (Agent Skills format). */
    skills: {
      list: () => drawer.session.skillsForAgent().map((s) => ({ name: s.name, description: s.description, source: s.source, active: drawer.session.conv.skills.includes(s.name), allowedTools: [...s.allowedTools] })),
      /** Activate a skill in the current conversation (as /name or use_skill would). false when the agent has no such skill. */
      activate: (name) => !!drawer.session.activateSkill(name, { via: 'api' }),
      /** The skills active in the current conversation. */
      active: () => [...drawer.session.conv.skills],
    },

    /** The capability index: reload it after a change, and see what did not add up. */
    capabilities: {
      reload: () => reloadCapabilities(),
      problems: () => [...caps.problems],
      index: () => (typeof o.capabilities === 'string' ? new URL(o.capabilities, globalThis.location?.href).href : null),
    },

    /** The development workspace (scripts/workspace.mjs) when it is connected, else null. */
    workspace: () => (caps.workspace ? { ...caps.workspace.info } : null),

    /** Notes kept between conversations (null with `memory: false`). */
    memory: memory ? {
      list: () => memory.list(),
      add: (text) => memory.add(text, { source: 'user' }),
      update: (id, text) => memory.update(id, text),
      remove: (id) => memory.remove(id),
      clear: () => memory.clear(),
      /** The memories as the JSON an app ships as its memoryFile (e.g. ai-memory.json). */
      export: () => memory.export(),
      /** Add the memories of such a file; `{ replace: true }` makes them the whole memory. Returns how many were added. */
      import: (json, options) => memory.import(json, options),
    } : null,

    /**
     * Take a screenshot and put it in the composer for the next question (what the camera button does). With the
     * browser's screen capture this must be called from a click. Resolves to { width, height, source } or null.
     */
    async screenshot() {
      const shot = await drawer.attachScreenshot();
      return shot ? { width: shot.width, height: shot.height, source: shot.source } : null;
    },

    /**
     * Attach files to the next question, as if the user picked them with the + button: images go as pictures (for a
     * model that sees them), other files as their text. Accepts a File, a Blob, a FileList or an array. Resolves to one
     * summary per file ({ kind, name, … }), or null for a file that could not be attached (the chat says why).
     */
    attach(files) {
      const list = typeof Blob === 'function' && files instanceof Blob ? [files] : [...(files || [])];
      return drawer.attachFiles(list);
    },

    on,
    settings: {
      get: () => store.get(),
      save: (patch) => store.save(patch),
      reset: () => store.reset(),
      setKey: (providerId, key) => store.keys.set(providerId, key),
    },
    destroy() {
      changed.cancel();
      clearTimeout(saveTimer);
      offMemory();
      offSettings();
      drawer.destroy();
      panel.destroy();
      listeners.clear();
    },
  };
  api.ready = ready.then(() => api);
  drawer.api = api;
  drawer.refreshStatus();
  // `resume` may have reopened the drawer inside this call, before the app could register agent.on('open'):
  // replay that 'open' once, to listeners registered in the same tick. (Registering later? Check agent.isOpen().)
  if (drawer.resumedOpen) setTimeout(() => { if (drawer.isOpen()) emit('open', { resumed: true }); }, 0);
  return api;
}

/**
 * A content hook that reads what an element shows: its rendered text (tables keep their columns) plus the values of
 * visible form fields, which innerText does not include. The agent's own UI is always skipped.
 * Use it when the app has no better model of its screen; a hook over the app's own state is usually tighter.
 * @param {string|Element} target  CSS selector or element, e.g. 'main'
 * @param {{exclude?: string, fields?: boolean}} [options]
 * @returns {() => string}
 */
export function fromDom(target, { exclude = '', fields = true } = {}) {
  const skip = `.aia-scope, [data-aia-ignore], script, style, template${exclude ? `, ${exclude}` : ''}`;
  return () => {
    const root = typeof target === 'string' ? document.querySelector(target) : target;
    if (!root) return '';
    const parts = [];
    const walk = (el) => {
      for (const child of el.children) {
        if (child.matches(skip)) continue;
        if (child.querySelector('.aia-scope, [data-aia-ignore]') || (exclude && child.querySelector(exclude))) walk(child);
        else if (child.offsetParent !== null || getComputedStyle(child).position === 'fixed') parts.push(child.innerText);
      }
    };
    if (root.querySelector(skip)) walk(root); else parts.push(root.innerText);
    let text = parts.join('\n').replace(/[ \t]+\n/g, '\n').replace(/\n{3,}/g, '\n\n').trim();

    if (fields) {
      const lines = [];
      for (const field of root.querySelectorAll('input, textarea, select')) {
        if (field.closest(skip) || field.offsetParent === null) continue;
        const type = (field.getAttribute('type') || '').toLowerCase();
        if (['password', 'hidden', 'file', 'submit', 'button', 'reset', 'image'].includes(type)) continue;
        const label = field.getAttribute('aria-label')
          || (field.id && root.querySelector(`label[for="${CSS.escape(field.id)}"]`)?.innerText)
          || field.closest('label')?.innerText
          || field.getAttribute('placeholder') || field.name || field.id || field.tagName.toLowerCase();
        let value;
        if (type === 'checkbox' || type === 'radio') value = field.checked ? 'checked' : 'not checked';
        else if (field.tagName === 'SELECT') value = [...field.selectedOptions].map((x) => x.text).join(', ');
        else value = field.value;
        lines.push(`- ${String(label).replace(/\s+/g, ' ').trim().slice(0, 80)}: ${String(value ?? '').slice(0, 4000)}`);
      }
      if (lines.length) text += `\n\nForm fields:\n${lines.join('\n')}`;
    }
    return text;
  };
}
