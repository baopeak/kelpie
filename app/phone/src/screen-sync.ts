// A followed terminal session as the phone holds it: its text rows, built from the daemon's frames
// (phone protocol v2, "Screens are state, not a stream"). A frame says which acknowledged frame it
// follows (`base`), how far to scroll (`shift`) and which rows changed; the phone keeps the screens
// from the last one it acknowledged on, acknowledges every 2nd frame or within 250 ms, and carries
// on from the acknowledged one after a reconnect (`sub {have_rev}`).

export interface Prompt {
  keys: string[];
  question?: string | null;
  /** Answered by typing a line: the key needs Enter after it. */
  line: boolean;
  /** Keys whose choice outlasts this prompt: the computer's to make, not a phone's. */
  lasting?: string[];
}

/** What the daemon sends about a followed terminal session (`type: "screen"`). */
export interface ScreenFrame {
  id: string;
  rev: number;
  /** The acknowledged frame this one builds on; null: the whole view. */
  base: number | null;
  shift: number;
  rows: [number, string][];
  height: number;
  cursor: [number, number] | null;
  prompt: Prompt | null;
  waiting_since: number | null;
  title: string | null;
  server_ts: number;
  level: number;
}

/** What the phone shows of a session: the rows and the facts that came with the last frame. */
export interface ScreenView {
  rows: string[];
  cursor: [number, number] | null;
  prompt: Prompt | null;
  waitingSince: number | null;
  title: string | null;
}

/** Which question this is: a card for the next one must start afresh, not carry the last one's "Sent". */
export function promptKey(prompt: Prompt, waitingSince: number | null): string {
  return `${waitingSince ?? ""}|${prompt.question ?? ""}|${prompt.keys.join(",")}|${prompt.line ? 1 : 0}`;
}

export const ACK_EVERY_FRAMES = 2;
export const ACK_AFTER_MS = 250;
/** Screens held for one session. The daemon sends at most 4 frames past what it has heard acknowledged (`WINDOW_FRAMES` in
 *  `mobile/sync.rs`), and builds only on one of those, so twice that is never reached by a well-behaved daemon: this is the bound,
 *  whatever a frame says. Rows are shared between screens, so a screen costs little more than its changed rows. */
export const HELD_FRAMES = 8;

export const emptyView = (): ScreenView => ({ rows: [], cursor: null, prompt: null, waitingSince: null, title: null });

export interface SyncDeps {
  /** Tells the daemon the phone holds frame `rev` of session `id`. */
  ack(id: string, rev: number): void;
  /** Asks the daemon to follow again from what is held (`have_rev`), after a frame that cannot be applied. */
  refollow(id: string): void;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
}

export class ScreenSync {
  private held = new Map<number, ScreenView>();
  private acked: number | null = null;
  private unacked = 0;
  private timer: unknown = null;
  /** The newest applied frame. */
  view: ScreenView = emptyView();
  rev: number | null = null;

  constructor(readonly id: string, private deps: SyncDeps) {}

  /** The frame number to resume from: the last acknowledged one, when it is still held. */
  haveRev(): number | null {
    return this.acked !== null && this.held.has(this.acked) ? this.acked : null;
  }

  /** The daemon restarted (or the session is another run): positions and numbers mean nothing now. */
  reset() {
    this.held.clear();
    this.acked = null;
    this.unacked = 0;
    this.rev = null;
    this.view = emptyView();
    this.cancelAck();
  }

  /** A new connection to the same daemon run: carry on from the last screen acknowledged (the daemon
   *  builds on that one, not on what came after). */
  resume() {
    this.unacked = 0;
    this.cancelAck();
    const kept = this.acked !== null ? this.held.get(this.acked) : undefined;
    if (kept) {
      // All of it: the prompt and the title of a later frame are not what this screen says, and the daemon says `same` if the rows are.
      this.view = kept;
      this.rev = this.acked;
    } else {
      this.reset();
    }
  }

  /** Applies a frame; false when it follows one the phone does not hold (it asked to be sent the whole view). */
  apply(m: ScreenFrame): boolean {
    if (m.base !== null && !this.held.has(m.base)) {
      this.held.clear();
      this.acked = null;
      this.rev = null;
      this.deps.refollow(this.id);
      return false;
    }
    const next = (m.base === null ? [] : this.held.get(m.base)!.rows).slice(m.shift);
    next.length = m.height;
    for (let i = 0; i < next.length; i++) if (next[i] === undefined) next[i] = "";
    for (const [i, text] of m.rows) if (i < next.length) next[i] = text;
    // The daemon builds on `base` from now on: older screens are not needed. A whole frame says nothing about what it builds on:
    // it may have heard only an earlier acknowledgement, so nothing is let go for one, beyond the bound.
    if (m.base !== null) for (const k of [...this.held.keys()]) if (k < m.base) this.held.delete(k);
    const view: ScreenView = { rows: next, cursor: m.cursor, prompt: m.prompt, waitingSince: m.waiting_since, title: m.title };
    this.held.delete(m.rev);
    this.held.set(m.rev, view);
    // The first to arrive goes first.
    while (this.held.size > HELD_FRAMES) this.held.delete(this.held.keys().next().value!);
    this.view = view;
    this.rev = m.rev;
    if (++this.unacked >= ACK_EVERY_FRAMES) this.ackNow();
    else if (this.timer === null) this.timer = this.deps.setTimeout(() => this.ackNow(), ACK_AFTER_MS);
    return true;
  }

  ackNow() {
    this.cancelAck();
    if (this.rev !== null) {
      this.deps.ack(this.id, this.rev);
      this.acked = this.rev;
    }
    this.unacked = 0;
  }

  private cancelAck() {
    if (this.timer !== null) this.deps.clearTimeout(this.timer);
    this.timer = null;
  }

  dispose() {
    this.cancelAck();
  }
}
