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

/** One finger on the picture, in page coordinates: a tap, a scroll (the page moves with the finger), or after a long press a drag. */
export class Gesture {
  private start: { x: number; y: number; t: number } | null = null;
  private last: { x: number; y: number } | null = null;
  private mode: "none" | "scroll" | "drag" = "none";

  down(p: { x: number; y: number }, t: number): TkOp[] {
    this.start = { ...p, t };
    this.last = p;
    this.mode = "none";
    return [];
  }

  move(p: { x: number; y: number }, t: number): TkOp[] {
    if (!this.start || !this.last) return [];
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

  up(p: { x: number; y: number } | null): TkOp[] {
    const { start, mode } = this;
    this.start = this.last = null;
    this.mode = "none";
    if (!start) return [];
    if (mode === "none") return [{ k: "tap", x: start.x, y: start.y }];
    if (mode === "drag") return [{ k: "up", x: (p ?? start).x, y: (p ?? start).y }];
    return [];
  }

  /** The touch was taken away (another finger, the system): a drag lets go where it is, nothing else is sent. */
  cancel(): TkOp[] {
    const was = this.mode === "drag" && this.last;
    const at = this.last;
    this.start = this.last = null;
    this.mode = "none";
    return was && at ? [{ k: "up", x: at.x, y: at.y }] : [];
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
  text(tk: string, text: string, st: number): Promise<Reply>;
  rebind(tk: string): Promise<Reply>;
  input(tk: string, ops: TkOp[], st: number): void;
  ack(tk: string, seq: number): void;
  /** The computer's clock, as this phone reads it. */
  serverNow(): number;
  now(): number;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(t: unknown): void;
}

export interface TakeoverState {
  /** `pick`: choosing a pane; `starting`; `live`: frames come; `frozen`: nothing for a while, input off; `ended`. */
  phase: "pick" | "starting" | "live" | "frozen" | "ended";
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
}

const fresh = (project: string | null): TakeoverState => ({ phase: "pick", project, panes: null, tk: null, title: "", host: "", frame: null, message: null, text: null });

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
  private flushTimer: unknown = null;
  private watchTimer: unknown = null;
  private heardAt = 0;

  constructor(private port: TakeoverPort, private changed: (s: TakeoverState | null) => void, private t: (s: string) => string = (s) => s) {}

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
    if (this.phase() !== "starting") return;
    if (r.t !== "ack" || !r.tk) {
      this.patch({ phase: "pick", message: r.t === "error" ? r.message : this.t("Not connected.") });
      return;
    }
    this.heardAt = this.port.now();
    this.patch({ phase: "live", tk: r.tk, title: r.title ?? "", host: r.host ?? "" });
    this.watch();
  }

  /** A `tk_frame`, `tk_fhb` or `tk_end` from the computer. */
  onMessage(m: { type: string; tk?: string; seq?: number; data?: string; meta?: FrameMeta; reason?: string }) {
    const s = this.state;
    if (!s?.tk || m.tk !== s.tk || s.phase === "ended") return;
    if (m.type === "tk_end") return this.ended(endedText(m.reason ?? "", this.t));
    this.heardAt = this.port.now();
    if (m.type === "tk_frame" && typeof m.data === "string" && typeof m.seq === "number") {
      this.patch({ phase: "live", frame: { seq: m.seq, data: m.data, meta: m.meta ?? {} } });
    } else if (s.phase === "frozen") this.patch({ phase: "live" });
  }

  /** The frame `seq` is on the screen: the computer may send the next. */
  shown(seq: number) {
    if (this.state?.tk && this.state.phase !== "ended") this.port.ack(this.state.tk, seq);
  }

  /** Input, while the picture is live: anything else is dropped here, never kept for later. */
  input(ops: TkOp[]) {
    if (this.state?.phase !== "live" || !ops.length) return;
    this.queue.push(...ops);
    // A tap, a press, a release or a key goes now; moves and scrolls wait a moment for the ones that follow.
    if (ops.some((o) => o.k !== "move" && o.k !== "wheel")) this.flush();
    else if (this.flushTimer === null) this.flushTimer = this.port.setTimeout(() => this.flush(), FLUSH_MS);
  }

  private flush() {
    if (this.flushTimer !== null) this.port.clearTimeout(this.flushTimer);
    this.flushTimer = null;
    const ops = coalesce(this.queue.splice(0));
    if (ops.length && this.state?.tk && this.state.phase === "live") this.port.input(this.state.tk, ops, this.port.serverNow());
  }

  /** Text into the page (a password, a code): said as sent or not sent, never sent twice. */
  async type(text: string): Promise<boolean> {
    if (this.state?.phase !== "live" || !this.state.tk || !text) return false;
    this.flush();
    this.patch({ text: "sending" });
    const r = await this.port.text(this.state.tk, text, this.port.serverNow());
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
      this.patch({ phase: "live", frame: null });
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
      if (this.state?.phase === "live" && this.port.now() - this.heardAt > FREEZE_MS) {
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
