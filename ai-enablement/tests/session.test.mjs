// core/session.js without a browser: the parts the conformance vectors (execute-tool, tool-loop) do not pin — saved
// chats, switching agents, reset, the context flag, and a session with no view answering every card with its decline.

import test from 'node:test';
import assert from 'node:assert/strict';
import { AgentSession, defaultQuestion, confirmButtons, CARDS } from '../assets/ai-agent/core/session.js';
import { ContextManager } from '../assets/ai-agent/core/context.js';
import { sanitizeSettings } from '../assets/ai-agent/core/settings.js';
import { normalizeTool } from '../assets/ai-agent/core/tools.js';
import { parseAgent, implicitAgent } from '../assets/ai-agent/core/agents.js';

function memoryStorage() {
  const kv = new Map();
  return { getItem: (k) => (kv.has(k) ? kv.get(k) : null), setItem: (k, v) => kv.set(k, String(v)), removeItem: (k) => kv.delete(k), kv };
}

function makeSession({ agents = [], tools = [], storage = memoryStorage(), chat, settings = {} } = {}) {
  let s = sanitizeSettings(settings);
  const store = { get: () => s, save: (p) => { s = sanitizeSettings({ ...s, ...p, toolStates: { ...s.toolStates, ...(p.toolStates || {}) } }); }, keys: { get: () => '' } };
  const page = { id: 'editor', title: 'Editor', content: 'Hello.' };
  const ctx = new ContextManager({ page, now: () => 0 });
  const list = agents.map((text, i) => parseAgent(text, { file: `agents/a${i}.md` }).agent);
  const caps = {
    agents: () => (list.length ? list : [implicitAgent('AI agent')]), skills: () => [], toolsets: () => new Map(),
    policy: () => ({ allow: [], ask: [], deny: [] }), defaultAgent: () => '', workspace: () => null,
  };
  const registry = tools.map((t) => normalizeTool(t));
  const session = new AgentSession({
    ctx, store, caps, toolRegistry: { all: () => registry, get: (n) => registry.find((t) => t.name === n) || null },
    storage, namespace: 'test.ai', defaultPrompt: () => 'B', chat,
  });
  return { session, page, storage, store };
}

test('defaultQuestion: what a message with only attachments asks', () => {
  assert.equal(defaultQuestion('  Hi  '), 'Hi');
  assert.equal(defaultQuestion('', [{ id: 's' }]), 'Here is a screenshot of what I am looking at.');
  assert.equal(defaultQuestion('', [], [{ name: 'a' }]), 'Please look at the attached file.');
  assert.equal(defaultQuestion('', [], [{}, {}]), 'Please look at the attached files.');
  assert.equal(defaultQuestion('', [{ name: 'x.png' }]), 'Please look at the attached image.');
  assert.equal(defaultQuestion('', [{ name: 'x.png' }, {}]), 'Please look at what I attached.');
  assert.equal(defaultQuestion(''), '');
});

test('cards: buttons and the answer of a session without a view', async () => {
  assert.deepEqual(confirmButtons({ effect: 'write' }, '').map((b) => b.id), ['run', 'chat', 'skip']);
  assert.deepEqual(confirmButtons({ effect: 'write' }, 'ask').map((b) => b.id), ['run', 'skip']);
  assert.deepEqual(confirmButtons({ effect: 'destructive' }, '').map((b) => b.id), ['run', 'skip']);
  // No view: every card is declined, so a write tool never runs unattended.
  let ran = false;
  const { session } = makeSession({ tools: [{ name: 'set_x', description: 'd', effect: 'write', enabled: true, run: () => { ran = true; } }] });
  const r = await session.executeTool({ id: 'c', name: 'set_x', arguments: {} }, { reply: { raw: '', native: '', status: '' }, actions: [] });
  assert.equal(ran, false);
  assert.match(r.content, /declined to run set_x/);
  assert.equal(CARDS.confirm.decline, 'skip');
});

test('saved chats: persist keeps hashes not snapshot text; load brings the agent and skills back; delete', async () => {
  const agents = ['---\nname: a\ndescription: A.\n---\nA.', '---\nname: b\ndescription: B.\n---\nB.'];
  const { session, storage } = makeSession({ agents, chat: async ({ onEvent }) => { onEvent({ type: 'text', text: 'Answer.' }); return { label: 'L', model: 'm', toolCalls: [] }; } });
  session.agentName = 'b';
  const settings = session.store.get();
  const turn = await session.beginTurn('First question', { settings });
  const reply = { raw: '', native: '', status: '' };
  const result = await session.runModel({ settings, baseSystem: turn.baseSystem, messages: turn.messages, reply, actions: [], snapshotHash: turn.snapshot.hash });
  const end = session.endTurn({ turn, reply, result });
  assert.equal(end.kept, true);
  session.conv.skills = ['proofreading'];
  const id = session.persist();
  assert.ok(id);
  const saved = JSON.parse(storage.getItem('test.ai.chats'));
  assert.equal(saved[0].title, 'First question');
  assert.equal(saved[0].agent, 'b');
  assert.deepEqual(saved[0].skills, ['proofreading']);
  assert.equal(saved[0].messages[0].snapshot.text, undefined, 'snapshot text is not saved');
  assert.ok(saved[0].messages[0].snapshot.hash);

  session.agentName = 'a';
  session.newConversation({ save: false });
  assert.equal(session.conv.messages.length, 0);
  const loaded = session.loadChat(id);
  assert.equal(loaded.agentChanged, true);
  assert.equal(session.agentName, 'b');
  assert.deepEqual(session.conv.skills, ['proofreading']);
  assert.equal(session.conv.messages.length, 2);
  // A reopened chat has the hash but not the text: the next question re-reads the page.
  const status = await session.computeStatus();
  assert.equal(status.state, 'unread');

  session.deleteChat(id);
  assert.equal(session.readChats().length, 0);
  assert.equal(session.conv.id, null, 'the open conversation goes on, unsaved');
  assert.equal(session.loadChat('nope'), null);
});

test('switchAgent starts a new conversation; reset drops the transcript and the decisions', () => {
  const agents = ['---\nname: a\ndescription: A.\n---\nA.', '---\nname: b\ndescription: B.\n---\nB.'];
  const { session } = makeSession({ agents });
  session.conv.messages.push({ role: 'user', content: 'x' });
  session.allowedTools.add('set_x');
  assert.deepEqual(session.switchAgent('a'), { agent: session.agents()[0], changed: false });
  const r = session.switchAgent('b');
  assert.equal(r.changed, true);
  assert.equal(session.agent().name, 'b');
  assert.equal(session.conv.messages.length, 0);
  assert.equal(session.allowedTools.size, 0);
  assert.equal(session.switchAgent('zzz'), null);

  session.conv.messages.push({ role: 'user', content: 'y' });
  session.allowedTools.add('t');
  session.textTools.add('k');
  session.forceReread = true;
  session.reset();
  assert.equal(session.conv.messages.length, 0);
  assert.equal(session.allowedTools.size + session.textTools.size, 0);
  assert.equal(session.forceReread, false);
});

test('no storage: nothing is saved, nothing fails', () => {
  const { session } = makeSession({ storage: null });
  session.conv.messages.push({ role: 'user', content: 'x', at: 0 });
  assert.equal(session.persist(), null);
  assert.deepEqual(session.readChats(), []);
});
