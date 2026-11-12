// Taking over a browser pane of the computer from the phone (the daemon's `takeover` broker; the app streams the pane). What is here is plain
// logic, with its clock and its messages passed in, so that it is tested without a browser: where a touch lands on the page, what a gesture
// is (tap, scroll, a long press that drags), input merged and stamped with the computer's clock so the computer can drop what came too late, frames
// acknowledged once shown, and the stream counted lost after a few silent seconds. pane-takeover.tsx draws it.

/** What the app says about a frame: the page's viewport in CSS pixels, and which window it is (the pane, or a sign-in popup it opened). */
export interface FrameMeta {
  deviceWidth?: number;
  deviceHeight?: number;
  offsetTop?: number;
  pageScaleFactor?: number;
  window?: "pane" | "popup";
  /** The window's id: input mapped against this frame names it, and the computer drops input meant for a window it no longer shows. */
  window_id?: number;
}

export interface PaneRow {
  pane: string;
  title: string;
  host: string;
  project: string;
  project_name: string;
  held: boolean;
}

export type TkOp =
  | { k: "tap" | "down" | "move" | "up"; x: number; y: number }
  | { k: "wheel"; x: number; y: number; dx: number; dy: number }
  | { k: "key"; key: string };

/** No frame or heartbeat for this long: the picture is dimmed and input stops. */
export const FREEZE_MS = 3000;
/** A press held this long before it moves drags (a slider, a selection); a quicker one scrolls. */
export const LONG_PRESS_MS = 400;
/** A touch that moves less than this is a tap. */
export const DRAG_PX = 8;
/** Moves and scrolls are merged and sent at most this often. */
export const FLUSH_MS = 33;

export const KEYS = ["Enter", "Tab", "Backspace", "Escape", "ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown"] as const;

export interface Box {
  left: number;
  top: number;
  width: number;
  height: number;
}

/** Where an image of `natural` size is drawn inside `el` with `object-fit: contain`. */
export function drawnBox(el: Box, naturalWidth: number, naturalHeight: number): Box {
  if (!naturalWidth || !naturalHeight || !el.width || !el.height) return { ...el, width: 0, height: 0 };
  const scale = Math.min(el.width / naturalWidth, el.height / naturalHeight);
  const width = naturalWidth * scale;
  const height = naturalHeight * scale;
  return { left: el.left + (el.width - width) / 2, top: el.top + (el.height - height) / 2, width, height };
}

const round1 = (n: number) => Math.round(n * 10) / 10;

/** A point on the screen → CSS pixels of the page, or null outside the picture. */
export function toPage(clientX: number, clientY: number, box: Box, meta: FrameMeta): { x: number; y: number } | null {
  if (box.width <= 0 || box.height <= 0 || !meta.deviceWidth || !meta.deviceHeight) return null;
  const u = (clientX - box.left) / box.width;
  const v = (clientY - box.top) / box.height;
  if (u < 0 || v < 0 || u > 1 || v > 1) return null;
  return { x: round1(u * meta.deviceWidth), y: round1(v * meta.deviceHeight + (meta.offsetTop ?? 0)) };
}

/** One finger on the picture, in page coordinates: a tap, a scroll (the page moves with the finger), or after a long press a drag. Only the first
 *  finger counts. A second one gives the gesture up: a drag lets go where it is, nothing else is sent for it, and nothing at all is until every finger
 *  is off the picture. So a pinch never reaches the page. */
export class Gesture {
  private start: { x: number; y: number; t: number } | null = null;
  private last: { x: number; y: number } | null = null;
  private mode: "none" | "scroll" | "drag" = "none";
  private pointer: number | null = null;
  private fingers = new Set<number>();
  private blocked = false;

  down(id: number, p: { x: number; y: number }, t: number): TkOp[] {
    this.fingers.add(id);
    if (this.fingers.size > 1) {
      this.blocked = true;
      return this.giveUp();
    }
    if (this.blocked) return [];
    this.pointer = id;
    this.start = { ...p, t };
    this.last = p;
    this.mode = "none";
    return [];
  }

  move(id: number, p: { x: number; y: number }, t: number): TkOp[] {
    if (this.blocked || id !== this.pointer || !this.start || !this.last) return [];
    if (this.mode === "none") {
      if (Math.hypot(p.x - this.start.x, p.y - this.start.y) < DRAG_PX) return [];
      this.mode = t - this.start.t >= LONG_PRESS_MS ? "drag" : "scroll";
      if (this.mode === "drag") {
        this.last = p;
        return [{ k: "down", x: this.start.x, y: this.start.y }, { k: "move", x: p.x, y: p.y }];
      }
    }
    const from = this.last;
    this.last = p;
    if (this.mode === "drag") return [{ k: "move", x: p.x, y: p.y }];
    return [{ k: "wheel", x: this.start.x, y: this.start.y, dx: round1(from.x - p.x), dy: round1(from.y - p.y) }];
  }

  /** A finger lifted, at `p` on the picture or outside it (null): a drag lets go where the finger last was on it. */
  up(id: number, p: { x: number; y: number } | null): TkOp[] {
    this.fingers.delete(id);
    if (this.blocked) {
      if (!this.fingers.size) this.blocked = false;
      return [];
    }
    if (id !== this.pointer) return [];
    const { start, mode } = this;
    const at = p ?? this.last ?? start;
    this.reset();
    if (!start || !at) return [];
    if (mode === "none") return [{ k: "tap", x: start.x, y: start.y }];
    if (mode === "drag") return [{ k: "up", x: at.x, y: at.y }];
    return [];
  }

  /** The system took the touch away: a drag lets go where it is, nothing else is sent. */
  cancel(id: number): TkOp[] {
    this.fingers.delete(id);
    if (this.blocked) {
      if (!this.fingers.size) this.blocked = false;
      return [];
    }
    return id === this.pointer ? this.giveUp() : [];
  }

  private giveUp(): TkOp[] {
    const at = this.mode === "drag" ? this.last : null;
    this.reset();
    return at ? [{ k: "up", x: at.x, y: at.y }] : [];
  }

  private reset() {
    this.start = this.last = null;
    this.mode = "none";
    this.pointer = null;
  }
}
/** Merges what waits to be sent: of moves in a row only the last, scrolls in a row added up. Taps, presses, releases and keys stay as they are, in order. */
export function coalesce(ops: TkOp[]): TkOp[] {
  const out: TkOp[] = [];
  for (const op of ops) {
    const prev = out[out.length - 1];
    if (op.k === "move" && prev?.k === "move") out[out.length - 1] = op;
    else if (op.k === "wheel" && prev?.k === "wheel") out[out.length - 1] = { ...prev, dx: round1(prev.dx + op.dx), dy: round1(prev.dy + op.dy) };
    else out.push(op);
  }
  return out;
}

/** What one request came to (the link's `Outcome`, as far as this needs it). */
export type Reply = { t: "ack"; result?: string; tk?: string; title?: string; host?: string; panes?: PaneRow[] } | { t: "error"; code: string; message: string } | { t: "offline" } | { t: "unknown" };

export interface TakeoverPort {
  panes(): Promise<Reply>;
  start(pane: string): Promise<Reply>;
  stop(tk: string): Promise<Reply>;
  /** `w`: the window id of the frame the input was mapped against (the computer drops what is meant for a window it no longer shows). */
  text(tk: string, text: string, st: number, w: number): Promise<Reply>;
  rebind(tk: string): Promise<Reply>;
  input(tk: string, ops: TkOp[], st: number, w: number): void;
  ack(tk: string, seq: number): void;
  /** The computer's clock, as this phone reads it. */
  serverNow(): number;
  now(): number;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(t: unknown): void;
}

export interface TakeoverState {
  /** `pick`: choosing a pane; `starting`; `live`: frames come; `frozen`: nothing for a while, input off; `hidden`: the computer does not show the
   * pane now (another tab in front, its window minimised or behind another), input off; `ended`. */
  phase: "pick" | "starting" | "live" | "frozen" | "hidden" | "ended";
  project: string | null;
  panes: PaneRow[] | null;
  tk: string | null;
  title: string;
  host: string;
  frame: { seq: number; data: string; meta: FrameMeta } | null;
  /** Why it ended, or why something was refused (the computer's words). */
  message: string | null;
  /** What became of the last text sent: it is never sent again by itself. */
  text: "sending" | "sent" | "not_sent" | null;
  /** The last frame could not be shown: the picture on screen is older than the page. */
  broken: boolean;
}

const fresh = (project: string | null): TakeoverState => ({ phase: "pick", project, panes: null, tk: null, title: "", host: "", frame: null, message: null, text: null, broken: false });

/** Why a takeover ended, in the phone's words; the reason codes are the daemon's. */
export function endedText(reason: string, t: (s: string) => string): string {
  switch (reason) {
    case "desktop":
      return t("Stopped on the computer.");
    case "closed":
      return t("The pane was closed on the computer.");
    case "revoked":
      return t("Control was taken away, or this phone locked.");
    case "idle":
      return t("Stopped after 10 minutes with nothing typed.");
    case "link_lost":
      return t("The connection was lost for too long.");
    case "app_closed":
    case "app":
      return t("The Kelpie app on the computer closed.");
    default:
      return t("Stopped.");
  }
}

/** One takeover from the phone's side, from picking a pane to its end. `changed` is called with each new state. */
export class TakeoverSession {
  state: TakeoverState | null = null;
  private queue: TkOp[] = [];
  /** The window the queued input was mapped against. */
  private queueWindow = 0;
  /** The last frame acknowledged. */
  private ackedSeq = 0;
  private flushTimer: unknown = null;
  private watchTimer: unknown = null;
  private heardAt = 0;

  private port: TakeoverPort;
  private changed: (s: TakeoverState | null) => void;
  private t: (s: string) => string;

  // Fields set in the body rather than as parameter properties: this file also runs as plain JavaScript once its types are stripped (scripts/e2e).
  constructor(port: TakeoverPort, changed: (s: TakeoverState | null) => void, t: (s: string) => string = (s) => s) {
    this.port = port;
    this.changed = changed;
    this.t = t;
  }

  private set(next: TakeoverState | null) {
    this.state = next;
    this.changed(next);
  }

  /** Read anew after an `await`: what was checked before may have changed meanwhile. */
  private phase(): TakeoverState["phase"] | null {
    return this.state?.phase ?? null;
  }

  private patch(p: Partial<TakeoverState>) {
    if (this.state) this.set({ ...this.state, ...p });
  }

  /** Shows the panes (those of `project` first). */
  async openPicker(project: string | null) {
    if (this.state?.tk && this.state.phase !== "ended") return;
    this.set(fresh(project));
    const r = await this.port.panes();
    if (this.phase() !== "pick") return;
    if (r.t === "ack") {
      const panes = [...(r.panes ?? [])].sort((a, b) => Number(b.project === project) - Number(a.project === project));
      this.patch({ panes });
    } else this.patch({ panes: [], message: r.t === "error" ? r.message : this.t("Not connected.") });
  }

  async pick(pane: string) {
    if (!this.state || this.state.phase !== "pick") return;
    this.patch({ phase: "starting", message: null });
    const r = await this.port.start(pane);
    if (this.phase() !== "starting") {
      // Closed while the computer was starting it (the person left the app): the pane would stay taken with nobody there.
      if (r.t === "ack" && r.tk) void this.port.stop(r.tk);
      return;
    }
    if (r.t !== "ack" || !r.tk) {
      this.patch({ phase: "pick", message: r.t === "error" ? r.message : this.t("Not connected.") });
      return;
    }
    this.heardAt = this.port.now();
    this.patch({ phase: "live", tk: r.tk, title: r.title ?? "", host: r.host ?? "" });
    this.watch();
  }

  /** A `tk_frame`, `tk_fhb` or `tk_end` from the computer. */
  onMessage(m: { type: string; tk?: string; seq?: number; data?: string; meta?: FrameMeta; reason?: string; paused?: boolean; hidden?: boolean }) {
    const s = this.state;
    if (!s?.tk || m.tk !== s.tk || s.phase === "ended") return;
    if (m.type === "tk_end") return this.ended(endedText(m.reason ?? "", this.t));
    this.heardAt = this.port.now();
    if (m.type === "tk_frame" && typeof m.data === "string" && typeof m.seq === "number") {
      // The same picture as the one shown: the page does not load it again, so nothing would acknowledge it. It is shown already.
      const same = s.frame?.data === m.data;
      // A frame does not end `hidden`: the heartbeat that says the pane shows again does.
      this.patch({ phase: s.phase === "hidden" ? "hidden" : "live", frame: { seq: m.seq, data: m.data, meta: m.meta ?? {} } });
      if (same) this.shown(m.seq);
    } else if (m.type === "tk_fhb") {
      // `paused`: the computer waits for an acknowledgement it never got (a frame lost on the way, or not shown), so the picture is stuck and input is
      // not taken. It is asked to go on from the last frame acknowledged, which brings a fresh one. Any acknowledgement resumes a paused hold, so one
      // that never acknowledged a frame still says 0. `hidden`: the computer does not show the pane, so nothing sent would reach it.
      if (m.paused === true) this.port.ack(s.tk, this.ackedSeq);
      const phase = m.hidden === true ? "hidden" : m.paused === true ? "frozen" : "live";
      if (phase !== "live") this.queue = [];
      if (phase !== s.phase) this.patch({ phase });
    } else if (s.phase === "frozen") this.patch({ phase: "live" });
  }

  /** The frame `seq` is on the screen: the computer may send the next. */
  shown(seq: number) {
    if (!this.state?.tk || this.state.phase === "ended") return;
    this.ackedSeq = Math.max(this.ackedSeq, seq);
    this.port.ack(this.state.tk, seq);
    if (this.state.broken) this.patch({ broken: false });
  }

  /** The frame `seq` could not be decoded: it is acknowledged all the same, so that the next one comes, and the screen says the picture is behind. */
  failed(seq: number) {
    if (!this.state?.tk || this.state.phase === "ended") return;
    this.ackedSeq = Math.max(this.ackedSeq, seq);
    this.port.ack(this.state.tk, seq);
    this.patch({ broken: true });
  }

  private windowNow(): number {
    return this.state?.frame?.meta.window_id ?? 0;
  }

  /** Input, while the picture is live: anything else is dropped here, never kept for later. */
  input(ops: TkOp[]) {
    if (this.state?.phase !== "live" || !ops.length) return;
    // Input mapped against another window's picture goes on its own, with that window's id.
    const w = this.windowNow();
    if (this.queue.length && w !== this.queueWindow) this.flush();
    this.queueWindow = w;
    this.queue.push(...ops);
    // A tap, a press, a release or a key goes now; moves and scrolls wait a moment for the ones that follow.
    if (ops.some((o) => o.k !== "move" && o.k !== "wheel")) this.flush();
    else if (this.flushTimer === null) this.flushTimer = this.port.setTimeout(() => this.flush(), FLUSH_MS);
  }

  private flush() {
    if (this.flushTimer !== null) this.port.clearTimeout(this.flushTimer);
    this.flushTimer = null;
    const ops = coalesce(this.queue.splice(0));
    if (ops.length && this.state?.tk && this.state.phase === "live") this.port.input(this.state.tk, ops, this.port.serverNow(), this.queueWindow);
  }

  /** Text into the page (a password, a code): said as sent or not sent, never sent twice. */
  async type(text: string): Promise<boolean> {
    if (this.state?.phase !== "live" || !this.state.tk || !text) return false;
    this.flush();
    this.patch({ text: "sending" });
    const r = await this.port.text(this.state.tk, text, this.port.serverNow(), this.windowNow());
    const ok = r.t === "ack" && r.result === "sent";
    this.patch({ text: ok ? "sent" : "not_sent" });
    return ok;
  }

  async stop() {
    const tk = this.state?.tk;
    if (tk && this.state?.phase !== "ended") {
      this.ended(this.t("Stopped."));
      await this.port.stop(tk);
    }
  }

  /** The connection is back after a handshake: the hold is asked for again, and is gone if the computer let it go. */
  async reconnected() {
    const tk = this.state?.tk;
    if (!tk || this.state?.phase === "ended" || this.state?.phase === "pick") return;
    const r = await this.port.rebind(tk);
    if (this.state?.tk !== tk) return;
    if (r.t === "ack") {
      this.heardAt = this.port.now();
      this.patch({ phase: "live" });
    } else if (r.t === "error") this.ended(endedText("link_lost", this.t));
  }

  close() {
    void this.stop();
    this.unwatch();
    this.set(null);
  }

  private ended(message: string) {
    this.queue = [];
    this.unwatch();
    this.patch({ phase: "ended", message, frame: null });
  }

  /** The picture is dimmed after `FREEZE_MS` without a word from the computer. */
  private watch() {
    this.unwatch();
    const look = () => {
      this.watchTimer = this.port.setTimeout(look, 500);
      if ((this.state?.phase === "live" || this.state?.phase === "hidden") && this.port.now() - this.heardAt > FREEZE_MS) {
        this.queue = [];
        this.patch({ phase: "frozen" });
      }
    };
    this.watchTimer = this.port.setTimeout(look, 500);
  }

  private unwatch() {
    if (this.watchTimer !== null) this.port.clearTimeout(this.watchTimer);
    this.watchTimer = null;
    if (this.flushTimer !== null) this.port.clearTimeout(this.flushTimer);
    this.flushTimer = null;
  }
}
