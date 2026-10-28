// xterm.js lifecycle for terminal panes.
//
// Policy (performance is the point of the whole app):
// - A view exists only for terminals the user has looked at. Visible views get a WebGL
//   renderer. Hidden ("parked") views unsubscribe from the daemon but keep their buffer, so
//   re-showing is a tiny delta replay; the most recently hidden ones also keep their WebGL
//   context (rebuilding one costs 100–400 ms per switch), older ones drop it because
//   browsers cap live contexts at ~16.
// - At most MAX_PARKED parked views are kept; older ones are disposed and rebuilt from the
//   daemon's snapshot when shown again.
// - The daemon owns the truth. Background sessions cost the UI nothing.
import { Terminal, type IMarker } from "@xterm/xterm";
import { createSignal } from "solid-js";
import { FitAddon } from "@xterm/addon-fit";
import { SearchAddon } from "@xterm/addon-search";
import { Unicode11Addon } from "@xterm/addon-unicode11";
import { WebLinksAddon } from "@xterm/addon-web-links";
import { WebglAddon } from "@xterm/addon-webgl";
import { readText, writeText } from "@tauri-apps/plugin-clipboard-manager";
import { openUrl } from "@tauri-apps/plugin-opener";
import { attachTerminal, b64ToBytes, daemon, detachTerminal, sendInput } from "./ipc";
import { isMac } from "./platform";
import { terminalThemeFor } from "./theme";
import { typedText } from "./typed-text";
import { osc52Text } from "./osc52";
import type { Settings } from "./types";

/** A session's own terminal theme id (its host's), over the app's; set by lib/ssh-hosts.ts. */
let themeOf: (sessionId: string) => string | undefined = () => undefined;
export const setSessionThemeResolver = (fn: (sessionId: string) => string | undefined) => (themeOf = fn);
const themeFor = (sessionId: string, s: Settings) => terminalThemeFor(s.theme, themeOf(sessionId) || s.terminalTheme);

const MAX_PARKED = 16;
/** Hidden views that keep their WebGL context (visible ones always have one). */
const MAX_HIDDEN_WEBGL = 6;

/** Device reports xterm emits on its own (DA, DSR/CPR, OSC colour replies, focus events). */
export function isTerminalReply(data: string): boolean {
  return /^(\x1b\[[?>=]?[\d;]*[cnR]|\x1b\][\d;]+;[^\x07\x1b]*(\x07|\x1b\\)|\x1b\[[IO])+$/.test(data);
}

/** Set by the keymap: returns true when the event is an app shortcut xterm must not eat. */
let isAppShortcut: (e: KeyboardEvent, fromTerminal: boolean) => boolean = () => false;
export const setAppShortcutMatcher = (fn: (e: KeyboardEvent, fromTerminal: boolean) => boolean) => (isAppShortcut = fn);
/** True while a palette/dialog is open: keys must not reach the terminal underneath. */
let overlayOpen: () => boolean = () => false;
export const setOverlayCheck = (fn: () => boolean) => (overlayOpen = fn);
/** Opens a Ctrl/Cmd-clicked link (the app routes local dev-server URLs to a browser pane). */
let openLink: (uri: string) => void = (uri) => void openUrl(uri);
export const setLinkOpener = (fn: (uri: string) => void) => (openLink = fn);
/** Other sessions that receive what is typed into a session (a tab with broadcast input on). */
let inputFanout: (sessionId: string) => number[] = () => [];
export const setInputFanout = (fn: (sessionId: string) => number[]) => (inputFanout = fn);

export class TermView {
  readonly el = document.createElement("div");
  readonly term: Terminal;
  readonly fit = new FitAddon();
  readonly search = new SearchAddon();
  private webgl: WebglAddon | null = null;
  handle: number | null = null;
  /** Subscribed to the daemon (set at the start of attach so effects never double-attach). */
  attached = false;
  /** Stream offset of the last byte written into this buffer (for delta re-attach). */
  lastSeq: number | null = null;
  private opened = false;
  private attachGen = 0;
  private suppressReplies = 0;
  /** Live output that raced ahead of the attach reply, with its stream offset. */
  private early: { seq: number; data: Uint8Array }[] | null = null;
  /** Detach/attach run strictly in order per view (a remount parks then re-attaches). */
  private op: Promise<unknown> = Promise.resolve();
  private sessionSize: [number, number] = [0, 0];
  private resizeObs: ResizeObserver | null = null;
  private resizeTimer = 0;
  focused = false;
  /** Lines where the user pressed Enter (prompts sent to a shell or an agent), oldest first. */
  private marks: IMarker[] = [];
  /** Scrolled away from the bottom, and how many lines arrived since (for "jump to bottom"). */
  private readonly scrollState = createSignal({ away: false, newLines: 0 });
  readonly scrolledAway = this.scrollState[0];
  private awayBase = 0;

  constructor(readonly sessionId: string, private settings: Settings) {
    this.el.className = "term-host";
    this.term = new Terminal({
      fontFamily: settings.fontFamily,
      fontSize: settings.fontSize,
      lineHeight: settings.lineHeight,
      scrollback: settings.scrollback,
      cursorBlink: settings.cursorBlink,
      cursorStyle: "bar",
      cursorWidth: 2,
      allowProposedApi: true,
      theme: themeFor(sessionId, settings),
      macOptionIsMeta: true,
      drawBoldTextInBrightColors: false,
      rescaleOverlappingGlyphs: true,
      customGlyphs: true,
      fastScrollSensitivity: 5,
      smoothScrollDuration: 0,
      scrollOnUserInput: true,
    });
    this.term.loadAddon(this.fit);
    this.term.loadAddon(this.search);
    this.term.loadAddon(new Unicode11Addon());
    this.term.unicode.activeVersion = "11";
    // Links open only on Ctrl/Cmd+click: terminal text never triggers actions implicitly.
    this.term.loadAddon(new WebLinksAddon((e, uri) => {
      if (isMac ? e.metaKey : e.ctrlKey) openLink(uri);
    }));
    this.term.onData((d) => this.input(d));
    this.term.onBinary((d) => this.input(d, true));
    // A program's copy (OSC 52; Claude Code's fullscreen mode copies its mouse selection so):
    // taken only from the terminal the user is in, and never from replayed output.
    this.term.parser.registerOscHandler(52, (data) => {
      const text = osc52Text(data);
      if (text != null && this.suppressReplies === 0 && this.term.element?.contains(document.activeElement)) void writeText(text);
      return true;
    });
    this.term.attachCustomKeyEventHandler((e) => this.onKey(e));
    this.term.textarea?.addEventListener("focus", () => (this.focused = true));
    this.term.onScroll(() => this.updateScroll());
    this.term.onWriteParsed(() => this.scrollState[0]().away && this.updateScroll());
  }

  private updateScroll() {
    const b = this.term.buffer.active;
    const away = b.viewportY < b.baseY;
    const was = this.scrollState[0]();
    if (away && !was.away) this.awayBase = b.baseY;
    const newLines = away ? Math.max(0, b.baseY - this.awayBase) : 0;
    if (away !== was.away || newLines !== was.newLines) this.scrollState[1]({ away, newLines });
  }

  scrollToBottom() {
    this.term.scrollToBottom();
    this.updateScroll();
  }

  /** Scrolls to the previous (-1) or next (1) line where a prompt was sent; past the last one,
   *  back to the bottom. */
  jumpPrompt(dir: -1 | 1) {
    this.marks = this.marks.filter((m) => !m.isDisposed && m.line >= 0);
    const b = this.term.buffer.active;
    // A long command wraps over several rows: jump to its first one.
    const starts = this.marks.map((m) => {
      let y = m.line;
      while (y > 0 && b.getLine(y)?.isWrapped) y--;
      return y;
    });
    // A jump leaves the prompt one row below the top: that row is the current position.
    const here = b.viewportY + 1;
    const target = dir < 0 ? [...starts].reverse().find((y) => y < here) : starts.find((y) => y > here);
    if (target != null) this.term.scrollToLine(Math.max(0, target - 1));
    else if (dir > 0) this.term.scrollToBottom();
    this.updateScroll();
  }

  /** Remembers the cursor line when the user submits a line (not in a full-screen program,
   *  whose screen has no history to jump through). */
  private markPrompt() {
    if (this.term.buffer.active.type !== "normal") return;
    const last = this.marks[this.marks.length - 1];
    const line = this.term.buffer.active.baseY + this.term.buffer.active.cursorY;
    if (last && !last.isDisposed && last.line === line) return;
    const m = this.term.registerMarker(0);
    if (!m) return;
    this.marks.push(m);
    if (this.marks.length > 1000) this.marks.shift()?.dispose();
  }

  private input(data: string, binary = false) {
    if (this.handle == null) return;
    // While a raw replay is written, xterm re-answers old queries; drop only those replies,
    // never the user's keystrokes.
    if (this.suppressReplies > 0 && isTerminalReply(data)) return;
    void sendInput(this.handle, data, binary);
    // Keystrokes and pastes only: a terminal's own replies belong to that terminal.
    if (!isTerminalReply(data)) {
      if (data === "\r") this.markPrompt();
      for (const h of inputFanout(this.sessionId)) void sendInput(h, data, binary);
    }
  }

  write(data: Uint8Array | string) {
    this.term.write(data);
  }

  private onKey(e: KeyboardEvent): boolean {
    if (overlayOpen()) return false;
    if (e.type !== "keydown") return true;
    if (isAppShortcut(e, true)) return false;
    const key = e.key.toLowerCase();
    if (!isMac && e.ctrlKey && !e.altKey) {
      if (key === "c" && (e.shiftKey || this.term.hasSelection())) {
        if (this.term.hasSelection()) {
          void writeText(this.term.getSelection());
          this.term.clearSelection();
        }
        e.preventDefault();
        return false;
      }
      if (key === "v") {
        e.preventDefault();
        void this.pasteFromClipboard();
        return false;
      }
    }
    if (isMac && e.metaKey && key === "v") {
      e.preventDefault();
      void this.pasteFromClipboard();
      return false;
    }
    if (isMac && e.metaKey && key === "c") {
      if (this.term.hasSelection()) void writeText(this.term.getSelection());
      return false;
    }
    // Shift+Enter → ESC CR, the "newline without submit" agents (Claude Code, OMP) accept.
    if (key === "enter" && e.shiftKey && !e.ctrlKey && !e.metaKey && !e.altKey) {
      e.preventDefault();
      this.input("\x1b\r");
      return false;
    }
    return true;
  }

  async pasteFromClipboard() {
    const text = await readText().catch(() => "");
    if (text) this.paste(text);
  }

  /** Every paste into a terminal (clipboard, drops, inserted paths, quick commands, prompts
   *  Kelpie types) goes through here, cleaned by `typedText`. */
  paste(text: string) {
    this.term.paste(typedText(text));
  }

  mount(parent: HTMLElement) {
    parent.appendChild(this.el);
    if (!this.opened) {
      this.term.open(this.el);
      this.opened = true;
    }
    this.enableWebgl();
    this.resizeObs = new ResizeObserver(() => {
      clearTimeout(this.resizeTimer);
      this.resizeTimer = window.setTimeout(() => this.fitNow(), 40);
    });
    this.resizeObs.observe(parent);
    this.fitNow();
  }

  /** Leaves the pane. The element moves to a hidden holder instead of leaving the document:
   *  detaching a live WebGL canvas costs Chromium 100–200 ms of synchronous work. The WebGL
   *  context stays until `releaseWebgl` (see MAX_HIDDEN_WEBGL). */
  unmount() {
    this.resizeObs?.disconnect();
    this.resizeObs = null;
    parkingLot().appendChild(this.el);
  }

  /** In a pane on screen (not in the parking lot). */
  get visible() {
    return this.el.isConnected && this.el.parentElement !== lot;
  }

  get hasWebgl() {
    return this.webgl !== null;
  }

  private enableWebgl() {
    if (!this.settings.webgl || this.webgl) return;
    try {
      const addon = new WebglAddon();
      addon.onContextLoss(() => {
        addon.dispose();
        if (this.webgl === addon) this.webgl = null;
      });
      this.term.loadAddon(addon);
      this.webgl = addon;
    } catch {
      this.webgl = null; // DOM renderer fallback
    }
  }

  releaseWebgl() {
    this.webgl?.dispose();
    this.webgl = null;
  }

  /** Window shown again: a context the GPU dropped meanwhile comes back. */
  restoreWebgl() {
    if (this.visible) this.enableWebgl();
  }

  fitNow() {
    if (!this.visible) return;
    const dims = this.fit.proposeDimensions();
    if (!dims || !Number.isFinite(dims.cols) || !Number.isFinite(dims.rows) || dims.cols < 2 || dims.rows < 1) return;
    if (dims.cols !== this.term.cols || dims.rows !== this.term.rows) this.term.resize(dims.cols, dims.rows);
    if (this.handle != null && (this.sessionSize[0] !== dims.cols || this.sessionSize[1] !== dims.rows)) {
      this.sessionSize = [dims.cols, dims.rows];
      void daemon("session.resize", { handle: this.handle, cols: dims.cols, rows: dims.rows }).catch(() => {});
    }
  }

  /** Subscribes to the daemon session; replays only what this buffer is missing. */
  attach(handle: number): Promise<void> {
    // Window hidden: subscribe once it is shown again (see the visibilitychange listener).
    if (document.hidden) {
      waitingForShow.set(this, handle);
      return Promise.resolve();
    }
    const gen = ++this.attachGen;
    this.attached = true;
    const run = this.op.then(() => this.doAttach(handle, gen));
    this.op = run.catch(() => {});
    return run;
  }

  private async doAttach(handle: number, gen: number) {
    if (gen !== this.attachGen) return;
    if (this.handle !== handle) this.lastSeq = null; // different stream (daemon restarted)
    this.handle = handle;
    this.early = [];
    const reply = await attachTerminal(handle, this.lastSeq, (seq, data) => {
      if (gen !== this.attachGen) return;
      if (this.early) {
        this.early.push({ seq, data });
        return;
      }
      this.write(data);
      this.lastSeq = seq;
    }).catch((e) => {
      console.warn("attach failed", e);
      if (gen === this.attachGen) this.attached = false;
      return null;
    });
    if (!reply || gen !== this.attachGen) return;
    const bytes = b64ToBytes(reply.data);
    if (reply.mode === "reset") {
      this.term.reset();
      if (reply.cols > 0 && (reply.cols !== this.term.cols || reply.rows !== this.term.rows)) this.term.resize(reply.cols, reply.rows);
    }
    this.sessionSize = [reply.cols, reply.rows];
    if (bytes.length) {
      if (reply.raw) {
        // Replayed bytes may contain terminal queries; the app already got its answers.
        this.suppressReplies++;
        this.term.write(bytes, () => this.suppressReplies--);
      } else {
        this.term.write(bytes);
      }
    }
    const early = this.early;
    this.early = null;
    this.lastSeq = reply.seq;
    // Chunks covered by the reply (seq <= reply.seq) were already included in it.
    for (const chunk of early ?? []) {
      if (chunk.seq <= reply.seq) continue;
      this.write(chunk.data);
      this.lastSeq = chunk.seq;
    }
    this.fitNow();
  }

  detach(): Promise<void> {
    this.attachGen++;
    this.attached = false;
    this.early = null;
    const handle = this.handle;
    const run = this.op.then(() => (handle != null ? detachTerminal(handle).catch(() => {}) : undefined));
    this.op = run;
    return run;
  }

  focus() {
    this.term.focus();
  }

  dispose() {
    this.attachGen++;
    this.unmount();
    this.term.dispose();
  }

  applySettings(s: Settings) {
    this.settings = s;
    Object.assign(this.term.options, {
      fontFamily: s.fontFamily,
      fontSize: s.fontSize,
      lineHeight: s.lineHeight,
      cursorBlink: s.cursorBlink,
      scrollback: s.scrollback,
      theme: themeFor(this.sessionId, s),
    });
    // Renderer switch applies now (hidden views keep their context, so no remount does it).
    if (!s.webgl) this.releaseWebgl();
    else if (this.visible) this.enableWebgl();
    this.fitNow();
  }
}

/** Hidden holder for parked terminal elements (kept in the document, never rendered). */
let lot: HTMLDivElement | null = null;
function parkingLot(): HTMLDivElement {
  if (!lot || !lot.isConnected) {
    lot = document.createElement("div");
    lot.className = "term-parking";
    lot.style.display = "none";
    document.body.appendChild(lot);
  }
  return lot;
}

const views = new Map<string, TermView>();
const parked: string[] = [];

export function getView(sessionId: string, settings: Settings): TermView {
  let v = views.get(sessionId);
  if (!v) {
    v = new TermView(sessionId, settings);
    views.set(sessionId, v);
  }
  const i = parked.indexOf(sessionId);
  if (i >= 0) parked.splice(i, 1);
  return v;
}

export const peekView = (sessionId: string) => views.get(sessionId);
export const allViews = () => [...views.values()];

/** Pane hidden: stop rendering and streaming but keep the buffer for a fast return. */
export function parkView(sessionId: string) {
  const v = views.get(sessionId);
  if (!v) return;
  v.unmount();
  void v.detach();
  if (!parked.includes(sessionId)) parked.push(sessionId);
  while (parked.length > MAX_PARKED) disposeView(parked.shift()!);
  // Oldest hidden views give their WebGL context back first.
  const withGl = parked.filter((id) => views.get(id)?.hasWebgl);
  for (const id of withGl.slice(0, Math.max(0, withGl.length - MAX_HIDDEN_WEBGL))) views.get(id)?.releaseWebgl();
}

export function disposeView(sessionId: string) {
  const v = views.get(sessionId);
  if (!v) return;
  void v.detach();
  v.dispose();
  views.delete(sessionId);
  const i = parked.indexOf(sessionId);
  if (i >= 0) parked.splice(i, 1);
}

// A hidden window (minimized or covered) gets its timers throttled to as little as once a
// minute, and xterm parses incoming output on timers: streams that keep arriving pile up and
// the window takes seconds to catch up when shown. So on-screen views unsubscribe while the
// window is hidden, like parked ones, and replay only what they missed when it is shown.
const waitingForShow = new Map<TermView, number>();
document.addEventListener("visibilitychange", () => {
  if (document.hidden) {
    for (const v of views.values()) {
      if (!v.visible || !v.attached || v.handle == null) continue;
      waitingForShow.set(v, v.handle);
      void v.detach();
    }
    return;
  }
  const waiting = [...waitingForShow];
  waitingForShow.clear();
  for (const [v, handle] of waiting) {
    // Disposed or parked meanwhile: its pane re-attaches it when shown.
    if (views.get(v.sessionId) !== v || !v.visible || v.attached) continue;
    // The newest handle asked for (a daemon reconnect while hidden asks again).
    void v.attach(handle);
  }
  for (const v of views.values()) v.restoreWebgl();
});

/** Daemon reconnected: handles may have changed, buffers must be rebuilt from snapshots. */
export function resetStreams() {
  for (const v of views.values()) {
    v.lastSeq = null;
    v.handle = null;
    v.attached = false;
  }
}
