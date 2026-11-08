// What the agent knows about the application, and what is on screen right now.
//
// Three layers, each fed by a hook the host application provides:
//
//   app      What the application is and does. Static-ish. Goes into the system prompt on every request.
//   page     Which page/view the user is on and what that page is for. Goes into the system prompt too.
//   content  The live content of the screen (the document, the table rows, the form values…). This is the part
//            that is fingerprinted: its hash is compared with the hash of the last snapshot the model received,
//            and the snapshot is only re-sent when they differ (see core/conversation.js).
//   view     Small, volatile UI state (cursor, selection, scroll, active tab). Sent fresh with every question but
//            deliberately NOT hashed, so moving the cursor never makes the page look "changed".
//
// Hooks may be plain values, functions, or async functions. Pure module: no DOM access here (see fromDom in
// ../ai-agent.js for the DOM-scraping helper).

import { hashText, stableStringify } from './hash.js';

export const DEFAULT_MAX_CONTEXT_CHARS = 24000;
const VIEW_MAX_CHARS = 2000;

async function resolve(value) {
  return typeof value === 'function' ? value() : value;
}

function humanize(key) {
  const s = String(key).replace(/([a-z0-9])([A-Z])/g, '$1 $2').replace(/[_-]+/g, ' ').trim();
  return s.charAt(0).toUpperCase() + s.slice(1);
}

/**
 * Turn an app/page descriptor into readable prompt text. Strings pass through; objects become "Label: value"
 * lines, arrays become bullet lists, nested objects become indented JSON.
 */
export function describe(value, skip = []) {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value.trim();
  if (typeof value !== 'object') return String(value);
  const lines = [];
  for (const [key, v] of Object.entries(value)) {
    if (skip.includes(key) || v === undefined || v === null || v === '' || typeof v === 'function') continue;
    if (Array.isArray(v)) {
      if (!v.length) continue;
      lines.push(`${humanize(key)}:`);
      for (const item of v) lines.push(`- ${typeof item === 'object' ? stableStringify(item, 0) : String(item)}`);
    } else if (typeof v === 'object') {
      lines.push(`${humanize(key)}:\n${stableStringify(v, 2)}`);
    } else {
      lines.push(`${humanize(key)}: ${String(v).trim()}`);
    }
  }
  return lines.join('\n');
}

/** Screen content -> text. Strings are sent as written; anything else as stable (sorted-key) JSON. */
export function serializeContent(value) {
  if (value === null || value === undefined) return '';
  if (typeof value === 'string') return value;
  if (typeof value === 'object' && typeof value.text === 'string' && Object.keys(value).length === 1) return value.text;
  return stableStringify(value, 2);
}

/** Keep the head of oversized content, and say plainly that it was cut. */
export function clip(text, maxChars) {
  const s = String(text ?? '');
  if (!(maxChars > 0) || s.length <= maxChars) return { text: s, truncated: false };
  const keep = Math.max(0, maxChars - 160);
  return {
    text: `${s.slice(0, keep)}\n…[truncated: the first ${keep.toLocaleString('en-US')} of ${s.length.toLocaleString('en-US')} characters are shown]`,
    truncated: true,
  };
}

export class ContextManager {
  /** `now`: the clock that stamps snapshots (tests and conformance vectors pass a fixed one). */
  constructor({ app = null, page = null, maxChars = DEFAULT_MAX_CONTEXT_CHARS, now = () => Date.now() } = {}) {
    this.app = app;
    this.page = page;
    this.maxChars = maxChars;
    this.now = now;
  }

  setApp(app) { this.app = app; }

  /** page: { id, title, purpose, description?, content?: hook, view?: hook, ...anything else descriptive } */
  setPage(page) { this.page = page || null; }

  /** Replace only the content hook of the current page. */
  setContent(content) { this.page = { ...(this.page || { id: 'page', title: 'Page' }), content }; }

  /** Replace only the view-state hook of the current page. */
  setView(view) { this.page = { ...(this.page || { id: 'page', title: 'Page' }), view }; }

  setMaxChars(n) { if (Number.isFinite(n) && n > 0) this.maxChars = n; }

  get hasContent() { return !!(this.page && this.page.content !== undefined && this.page.content !== null); }

  async appText() {
    try { return describe(await resolve(this.app)); } catch (e) { return `[app context hook failed: ${e?.message || e}]`; }
  }

  /** Page descriptor text (id, title, purpose, …), without the content/view hooks themselves. */
  async pageText() {
    const p = this.page;
    if (!p) return '';
    const meta = { ...p };
    delete meta.content;
    delete meta.view;
    delete meta.tools;      // the page's tools go to the model as tools, not as page description
    try {
      const resolved = {};
      for (const [k, v] of Object.entries(meta)) resolved[k] = typeof v === 'function' ? await v() : v;
      return describe(resolved);
    } catch (e) {
      return `[page context hook failed: ${e?.message || e}]`;
    }
  }

  /**
   * Capture the screen content now.
   * @returns {Promise<{pageId, pageTitle, text, chars, totalChars, truncated, hash, empty, at, error?}>}
   */
  async snapshot() {
    const p = this.page || {};
    const pageId = String(p.id ?? 'page');
    const pageTitle = String((typeof p.title === 'function' ? await p.title() : p.title) ?? pageId);
    let full = '';
    let error;
    try {
      full = serializeContent(await resolve(p.content));
    } catch (e) {
      error = String(e?.message || e);
      full = `[The screen content could not be read: ${error}]`;
    }
    const { text, truncated } = clip(full, this.maxChars);
    return {
      pageId,
      pageTitle,
      text,
      chars: text.length,
      totalChars: full.length,
      truncated,
      // The fingerprint covers the whole content (not only the part that fits), plus which page it came from.
      hash: hashText(`${pageId}\n${pageTitle}\n${full}`),
      empty: full.trim() === '',
      at: new Date(this.now()).toISOString(),
      error,
    };
  }

  /** Volatile view state, sent with each question and never hashed. */
  async viewText() {
    const p = this.page;
    if (!p || p.view === undefined || p.view === null) return '';
    try {
      const v = await resolve(p.view);
      if (v === null || v === undefined || v === '') return '';
      return clip(typeof v === 'string' ? v : describe(v), VIEW_MAX_CHARS).text;
    } catch (e) {
      return `[view state hook failed: ${e?.message || e}]`;
    }
  }
}
