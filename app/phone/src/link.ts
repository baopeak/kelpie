// The phone's one connection to Kelpie and the only place that knows the wire: phone protocol v2 as JSON
// over a WebSocket today (docs/architecture.md, "Phone protocol (v2)"); a later phase swaps the framing
// for kelpie-link without anything above this file changing.
//
// What lives here: the handshake (`{token, caps}` with caps `ack` and `hb`, so the daemon does not serve
// this client as a legacy page), answering and sending heartbeats, reconnecting with full-jitter backoff
// and on visibility / network triggers, resuming subscriptions, and the offline policy: nothing is queued,
// requests made while not connected are refused at once, and an answer whose reply was lost is retried
// once (the daemon lets the first answer win and recognises a repeat).
import { PHONE_API } from "../../src/lib/attention";
import { backoffMs, LagMeter, type ConnFacts, type Reach } from "./conn-state";
import type { ScreenFrame } from "./screen-sync";

// ---------- what the daemon sends ----------

export interface SessionRow {
  id: string;
  name: string | null;
  kind: string;
  title: string | null;
  status: "idle" | "working" | "waiting" | string;
  unread: boolean;
  alive: boolean;
  chat: boolean;
  project_id: string | null;
  cwd: string;
  last_activity: number;
  waiting_since: number | null;
}

export interface LoadStatus {
  level: number;
  reason: string[];
  since: number;
}

export interface Hello {
  v: number;
  api: number;
  daemon_epoch: string;
  rev: number;
  device: { name: string; can_write: boolean };
  sessions: SessionRow[];
  projects: Record<string, string>;
  load: LoadStatus;
  server_ts: number;
  hb?: { idle_s: number; pong_s: number; dead_s: number; legacy: boolean };
}

export interface SessionsMsg {
  rev: number;
  sessions: SessionRow[];
  projects: Record<string, string>;
}

export interface ChatMsg {
  id: string;
  mode: "reset" | "delta" | "more" | "live";
  seq: number;
  lines: string[];
}

/** The daemon's queue of messages for a chat (the whole state: the latest message replaces the last). `by` names the sender,
 *  `mine` says it is this device's, `kind` is `desktop` for a message from the computer's own window. */
export interface ChatQueueMsg {
  id: string;
  items: { qid: string; cid: string; by: string; mine: boolean; kind: "phone" | "desktop"; text: string }[];
  paused: boolean;
  held: boolean;
  in_flight: boolean;
  dropped?: { qid: string; cid: string; by: string; mine: boolean; text: string; reason: string }[];
}

export interface ChatResolved {
  id: string;
  request_id: string | number;
  by: "phone" | "desktop";
  dropped: boolean;
  answered_by?: string;
  at?: number;
}

export interface ReplyError {
  code: string;
  reason?: string | null;
  message: string;
  /** For `answer` / `chat_answer`: `stale`, `dup` or `refused`. */
  result?: "stale" | "dup" | "refused";
  answered_by?: string;
  at?: number;
}

/** What became of a request: the daemon's reply, or why there was none. */
export type Outcome =
  /** `qid`, `state` and `ahead` come with a `chat_send`: what the message was called, and where it stands. */
  | { t: "ack"; entered?: boolean; result?: string; qid?: string; state?: string; ahead?: number }
  | ({ t: "error" } & ReplyError)
  /** Not connected: nothing was sent (and nothing is kept to send later). */
  | { t: "offline" }
  /** Sent, and the connection dropped before the reply (or it was retried and still unsettled). */
  | { t: "unknown" };

export interface LinkHandlers {
  /** Any fact behind the connection state changed. */
  changed(): void;
  hello(h: Hello, epochChanged: boolean): void;
  sessions(m: SessionsMsg): void;
  frame(m: ScreenFrame): void;
  chat(m: ChatMsg): void;
  chatResolved(m: ChatResolved): void;
  chatQueue?(m: ChatQueueMsg): void;
  chatResync(id: string): void;
  /** The device was removed on the computer: forget the token. */
  unpaired(): void;
  /** The frame number to resume a followed terminal from. */
  haveRev(id: string): number | null;
  /** Where in a followed chat's log the phone is. */
  chatSince(id: string): number | null;
}

// ---------- environment (replaced in tests) ----------

export interface SocketLike {
  onopen: ((e: unknown) => void) | null;
  onmessage: ((e: { data: string }) => void) | null;
  onclose: ((e: { code: number }) => void) | null;
  onerror: ((e: unknown) => void) | null;
  readyState: number;
  send(data: string): void;
  close(): void;
}

export interface Env {
  socket(url: string): SocketLike;
  url(): string;
  now(): number;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(handle: unknown): void;
  random(): number;
  online(): boolean;
  /** Whether the computer's server answers a plain request, and with its own page. */
  reach(): Promise<Reach>;
}

export function browserEnv(): Env {
  return {
    socket: (url) => new WebSocket(url) as unknown as SocketLike,
    url: () => `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/ws`,
    now: () => Date.now(),
    setTimeout: (fn, ms) => setTimeout(fn, ms),
    clearTimeout: (h) => clearTimeout(h as number),
    random: () => Math.random(),
    online: () => navigator.onLine !== false,
    async reach() {
      const abort = new AbortController();
      const timer = setTimeout(() => abort.abort(), 3000);
      try {
        // A file the service worker leaves alone, so that the answer comes from the network.
        const r = await fetch(`/manifest.webmanifest?probe=${Date.now()}`, { cache: "no-store", signal: abort.signal });
        const type = r.headers.get("content-type") ?? "";
        return r.ok && !r.redirected && type.includes("manifest+json") ? "reachable" : "portal";
      } catch {
        return "unreachable";
      } finally {
        clearTimeout(timer);
      }
    },
  };
}

// ---------- timing ----------

/** Ask the daemon something if it has said nothing for this long (it pings a silent phone at 20 s). */
export const PING_AFTER_MS = 15_000;
/** How long a heartbeat of ours may stay unanswered before the connection is thrown away. */
export const PROBE_MS = 3000;
/** The least time between two "do it now" attempts that skip the backoff. */
export const KICK_GAP_MS = 2000;
/** A connection must last this long before the failure count starts over. */
const LASTED_MS = 5000;
const CONNECT_MS = 8000;
const HELLO_MS = 8000;
/** An answer is retried once if the connection dropped within this time of sending it. */
export const RETRY_WINDOW_MS = 60_000;
const REPLY_MS = 30_000;
const TICK_MS = 2000;
/** A restart's resync is given up on (and typing allowed) after this long. */
const RESYNC_MS = 5000;

type TimerName = "connect" | "hello" | "retry" | "tick" | "probe";

interface Pending {
  msg: Record<string, unknown>;
  resolve(o: Outcome): void;
  retry: boolean;
  firstAt: number;
  attempts: number;
  /** The connection dropped with this in flight: it is sent again after the next hello (once). */
  lost: boolean;
  timer: unknown;
}

interface Wanted {
  kind: "term" | "chat";
}

export class Link {
  readonly facts: ConnFacts = {
    online: true, ready: false, connecting: false, attempt: 0, nextAt: null, reach: "unknown", lastLiveAt: 0, replaced: false,
    unpaired: false, outdated: false, restartedAt: 0, rttMs: null, rttAt: 0, probeSince: null, lagMs: 0, lagAt: 0,
    resyncing: false, load: { level: 0, reasons: [] }, route: "direct",
  };
  epoch: string | null = null;
  device: { name: string; can_write: boolean } | null = null;
  /** The newest message time heard (ms epoch, phone clock). */
  lastIn = 0;
  readonly lag = new LagMeter();

  private ws: SocketLike | null = null;
  private readyAt = 0;
  private hideSubs = false;
  private stopped = true;
  private timers = { connect: null as unknown, hello: null as unknown, retry: null as unknown, tick: null as unknown, probe: null as unknown };
  private probedReachAt = 0;
  private lastKickAt = -Infinity;
  private probeSentAt = 0;
  private beat = 0;
  private reqSeq = 0;
  private pending = new Map<number, Pending>();
  /** Answers whose reply was lost with the connection: sent once more after the next hello, or given up on. */
  private lostList = new Set<Pending>();
  private wanted = new Map<string, Wanted>();
  private chatRetry = new Map<string, unknown>();
  /** After a restart: the followed sessions whose first state has not arrived yet. */
  private awaiting = new Set<string>();
  private resyncTimer: unknown = null;

  constructor(private env: Env, private token: () => string | null, private h: LinkHandlers) {}

  // ---------- life ----------

  start() {
    this.stopped = false;
    this.lastKickAt = this.env.now();
    this.facts.online = this.env.online();
    this.facts.unpaired = false;
    this.facts.replaced = false;
    this.open();
  }

  stop() {
    this.stopped = true;
    this.clearTimers();
    this.detach();
    this.facts.ready = false;
    this.facts.connecting = false;
    this.facts.nextAt = null;
    this.failPending();
    this.h.changed();
  }

  get ready(): boolean {
    return this.facts.ready;
  }

  /** The page was shown, or the network came back: connect now (skipping the backoff, at most every 2 s)
   *  or, when connected, check at once that the other end still answers. */
  kick() {
    if (this.stopped || this.facts.unpaired) return;
    const now = this.env.now();
    if (this.facts.replaced) {
      this.facts.replaced = false;
      this.open();
      return;
    }
    if (this.facts.ready) {
      if (now - this.lastIn > 1000) this.probe();
      return;
    }
    if (now - this.lastKickAt < KICK_GAP_MS) return;
    this.lastKickAt = now;
    this.open();
  }

  /** The browser says the network is gone or back. */
  setOnline(online: boolean) {
    this.facts.online = online;
    if (!online) {
      // No point waiting for a socket to notice, or trying while there is no network.
      this.clearTimer("retry");
      this.facts.nextAt = null;
      if (this.facts.ready) this.drop(1006);
    } else {
      this.facts.reach = "unknown";
      this.kick();
    }
    this.h.changed();
  }

  /** Hidden page: let go of every subscription but the session list (which is always pushed) and stop
   *  checking the connection. Shown again: follow them again from where the phone is. */
  setHidden(hidden: boolean) {
    if (this.hideSubs === hidden) return;
    this.hideSubs = hidden;
    if (hidden) {
      // The resync's time limit runs while the page is shown: it is armed again on show, and must not open the gate meanwhile.
      this.clearResyncTimer();
      for (const id of this.wanted.keys()) this.post({ type: "unsub", id });
    } else {
      this.kick();
      if (this.awaiting.size) this.armResyncTimer();
      for (const [id, w] of this.wanted) this.subscribe(id, w);
    }
  }

  private armResyncTimer() {
    this.clearResyncTimer();
    this.resyncTimer = this.env.setTimeout(() => this.finishResync(), RESYNC_MS);
  }

  // ---------- subscriptions ----------

  follow(id: string, kind: "term" | "chat") {
    const w = { kind };
    this.wanted.set(id, w);
    this.subscribe(id, w);
  }

  unfollow(id: string) {
    this.settled(id);
    if (!this.wanted.delete(id)) return;
    this.post({ type: "unsub", id });
    this.cancelChatRetry(id);
  }

  /** Asks again for a chat's log (the daemon said to, or the first ask was refused). */
  refollow(id: string) {
    const w = this.wanted.get(id);
    if (w) this.subscribe(id, w);
  }

  private subscribe(id: string, w: Wanted) {
    if (!this.facts.ready || this.hideSubs) return;
    if (w.kind === "term") {
      void this.request({ type: "sub", id, height: 200, have_rev: this.h.haveRev(id), focus: true }, { write: false }).then((o) => o.t === "error" && this.settled(id));
      return;
    }
    this.cancelChatRetry(id);
    void this.request({ type: "chat_sub", id, since_seq: this.h.chatSince(id) }, { write: false }).then((o) => {
      // Asked again too soon, or the daemon is busy sending other logs: it says when with `chat_resync` (busy),
      // and the too-fast case is just a little later.
      if (o.t === "error" && o.code !== "too_fast") this.settled(id);
      if (o.t === "error" && o.code === "too_fast" && this.wanted.has(id)) {
        this.chatRetry.set(id, this.env.setTimeout(() => this.refollow(id), 1100));
      }
    });
  }

  private cancelChatRetry(id: string) {
    const t = this.chatRetry.get(id);
    if (t !== undefined) this.env.clearTimeout(t);
    this.chatRetry.delete(id);
  }

  // ---------- requests ----------

  /** Sends a request and resolves with what the daemon says. Refused at once when not connected: nothing is kept for later.
   *  `retry`: if the connection drops before the reply, send it once more after the next hello within a minute
   *  (only for requests the daemon treats as first-wins). */
  request(msg: Record<string, unknown>, opts: { retry?: boolean; write?: boolean } = {}): Promise<Outcome> {
    return new Promise((resolve) => {
      if (!this.facts.ready || !this.ws) return resolve({ t: "offline" });
      const p: Pending = { msg: { ...msg, ...(opts.write === false ? {} : { epoch: this.epoch }) }, resolve, retry: !!opts.retry, firstAt: this.env.now(), attempts: 0, lost: false, timer: null };
      this.transmit(p);
    });
  }

  private transmit(p: Pending) {
    const req = ++this.reqSeq;
    p.attempts++;
    p.lost = false;
    this.pending.set(req, p);
    p.timer = this.env.setTimeout(() => {
      // No reply for a long while: the answer, if any, is not coming. Check the connection too.
      if (this.pending.get(req) === p) {
        this.pending.delete(req);
        p.resolve({ t: "unknown" });
        if (!this.hideSubs) this.probe();
      }
    }, REPLY_MS);
    this.post({ ...p.msg, req });
  }

  private post(msg: Record<string, unknown>) {
    if (this.ws && this.ws.readyState === 1) this.ws.send(JSON.stringify(msg));
  }

  // typed senders: the rest of the app never builds a message by hand

  list() {
    this.post({ type: "list" });
  }

  ack(id: string, rev: number) {
    this.post({ type: "ack", id, rev });
  }

  input(id: string, text: string, force = false) {
    return this.request({ type: "input", id, text, enter: true, ...(force ? { force: true } : {}) });
  }

  key(id: string, key: string) {
    return this.request({ type: "key", id, key });
  }

  /** Answers the prompt a terminal session waits on (`waiting_since` names which one). */
  answer(id: string, waitingSince: number | null, answer: { key: string } | { text: string }, enter: boolean) {
    return this.request({ type: "answer", id, waiting_since: waitingSince, ...answer, enter }, { retry: true });
  }

  chatAnswer(id: string, requestId: string | number, line: string) {
    return this.request({ type: "chat_answer", id, request_id: requestId, line }, { retry: true });
  }

  /** Sends a message to a chat. `cid` names it: if the connection drops with it in flight it is sent once more, with the
   *  same `cid`, after the next hello, and the daemon answers `dup` for a message it has (never a second one). */
  chatSend(id: string, cid: string, text: string) {
    return this.request({ type: "chat_send", id, cid, text }, { retry: true });
  }

  chatCancel(id: string, qid: string) {
    return this.request({ type: "chat_cancel", id, qid });
  }

  chatStop(id: string) {
    return this.request({ type: "chat_stop", id });
  }

  chatResume(id: string) {
    return this.request({ type: "chat_resume", id });
  }

  // ---------- socket ----------

  private open() {
    this.clearTimer("retry");
    this.facts.nextAt = null;
    const token = this.token();
    if (!token) return;
    this.detach();
    this.facts.connecting = true;
    this.facts.ready = false;
    let ws: SocketLike;
    try {
      ws = this.env.socket(this.env.url());
    } catch {
      this.facts.connecting = false;
      this.failed();
      return;
    }
    this.ws = ws;
    this.timers.connect = this.env.setTimeout(() => this.ws === ws && this.drop(1006), CONNECT_MS);
    ws.onopen = () => {
      if (this.ws !== ws) return;
      this.clearTimer("connect");
      // `caps`: this client acknowledges frames and answers heartbeats; without both the daemon serves it as a legacy page.
      ws.send(JSON.stringify({ token, caps: ["ack", "hb"] }));
      this.timers.hello = this.env.setTimeout(() => this.ws === ws && this.drop(1006), HELLO_MS);
    };
    ws.onmessage = (e) => {
      if (this.ws !== ws) return;
      let m: any;
      try {
        m = JSON.parse(e.data);
      } catch {
        return;
      }
      this.onMessage(m);
    };
    ws.onclose = (e) => {
      if (this.ws !== ws) return;
      this.closed(e.code);
    };
    ws.onerror = () => {};
    this.h.changed();
  }

  /** Stops listening to the current socket and closes it. */
  private detach() {
    const ws = this.ws;
    this.ws = null;
    if (ws) {
      ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null;
      try {
        ws.close();
      } catch {
        /* already gone */
      }
    }
  }

  /** The connection is dead as far as this client is concerned. */
  private drop(code: number) {
    this.detach();
    this.closed(code);
  }

  private closed(code: number) {
    this.ws = null;
    this.clearTimer("connect");
    this.clearTimer("hello");
    this.clearTimer("probe");
    this.clearTimer("tick");
    const lasted = this.facts.ready && this.env.now() - this.readyAt > LASTED_MS;
    this.facts.ready = false;
    this.facts.connecting = false;
    this.facts.probeSince = null;
    this.awaiting.clear();
    this.clearResyncTimer();
    this.facts.resyncing = false;
    this.loseInFlight();
    if (lasted) this.facts.attempt = 0;
    if (code === 4401) {
      this.facts.unpaired = true;
      this.h.unpaired();
      return this.h.changed();
    }
    // Another window of this device took the connection: wait until this one is looked at.
    if (code === 4409) {
      this.facts.replaced = true;
      return this.h.changed();
    }
    // 4408 (changed / behind), 4410 (silent), 4411 (could not keep up) and anything else: connect again.
    this.failed();
  }

  /** Schedules the next attempt. */
  private failed() {
    if (this.stopped) return;
    this.facts.attempt++;
    this.findOutWhy();
    // Offline: the `online` event starts the next attempt; counting down would be a lie.
    if (!this.facts.online) {
      this.facts.nextAt = null;
      return this.h.changed();
    }
    const wait = backoffMs(this.facts.attempt, () => this.env.random());
    this.facts.nextAt = this.env.now() + wait;
    this.timers.retry = this.env.setTimeout(() => this.open(), wait);
    this.h.changed();
  }

  /** After a failure: does the computer's server answer at all, and with its own page? At most every 10 s. */
  private findOutWhy() {
    const now = this.env.now();
    if (now - this.probedReachAt < 10_000 || !this.facts.online) return;
    this.probedReachAt = now;
    void this.env.reach().then((r) => {
      this.facts.reach = r;
      this.h.changed();
    });
  }

  // ---------- messages ----------

  private onMessage(m: any) {
    const now = this.env.now();
    this.lastIn = now;
    if (this.facts.ready) this.facts.lastLiveAt = now;
    if (this.facts.probeSince !== null) {
      this.facts.probeSince = null;
      this.clearTimer("probe");
    }
    if (typeof m.server_ts === "number") {
      this.facts.lagMs = this.lag.add(m.server_ts, now);
      this.facts.lagAt = now;
    }
    if (m.req !== undefined && this.settle(m)) return;
    switch (m.type) {
      case "hello":
        return this.onHello(m as Hello);
      case "hb":
        if (m.echo) {
          // The answer to ours: the round trip.
          if (m.n === this.beat && this.probeSentAt) {
            this.facts.rttMs = now - this.probeSentAt;
            this.facts.rttAt = now;
            this.probeSentAt = 0;
          }
        } else {
          this.post({ type: "hb", echo: true, n: m.n });
        }
        break;
      case "load.status":
        this.facts.load = { level: m.level, reasons: m.reason ?? [] };
        break;
      case "sessions":
        this.h.sessions(m as SessionsMsg);
        break;
      case "screen":
        this.h.frame(m as ScreenFrame);
        this.settled(m.id);
        break;
      case "chat":
        this.h.chat(m as ChatMsg);
        this.settled(m.id);
        break;
      case "chat_resolved":
        this.h.chatResolved(m as ChatResolved);
        break;
      case "chat_queue":
        this.h.chatQueue?.(m as ChatQueueMsg);
        break;
      case "chat_resync":
        this.refollow(m.id);
        this.h.chatResync(m.id);
        break;
      default:
        break;
    }
    this.h.changed();
  }

  private onHello(hello: Hello) {
    this.clearTimer("hello");
    const now = this.env.now();
    const epochChanged = this.epoch !== null && this.epoch !== hello.daemon_epoch;
    this.epoch = hello.daemon_epoch;
    this.device = hello.device;
    this.facts.ready = true;
    this.facts.connecting = false;
    this.facts.reach = "reachable";
    this.facts.outdated = hello.api < PHONE_API;
    this.facts.load = { level: hello.load?.level ?? 0, reasons: hello.load?.reason ?? [] };
    this.facts.lastLiveAt = now;
    this.facts.probeSince = null;
    this.readyAt = now;
    this.awaiting.clear();
    this.clearResyncTimer();
    if (epochChanged) this.facts.restartedAt = now;
    this.armTick();
    // The hello comes first; then the subscriptions pick up where the phone is.
    this.h.hello(hello, epochChanged);
    // A restart seen while the page is hidden counts too: nothing is re-followed until the page is shown, and
    // nothing may be sent until what it follows has arrived (the 5 s limit starts when it is shown).
    if (epochChanged) {
      this.awaiting = new Set(this.wanted.keys());
      if (this.awaiting.size && !this.hideSubs) this.armResyncTimer();
    }
    this.facts.resyncing = this.awaiting.size > 0;
    for (const [id, w] of this.wanted) this.subscribe(id, w);
    this.resendLost(epochChanged);
    this.h.changed();
  }

  /** The first state of a followed session arrived (or it cannot come): a restart's resync may be over. */
  private settled(id: string) {
    if (!this.awaiting.delete(id)) return;
    if (!this.awaiting.size) this.finishResync();
  }

  private finishResync() {
    this.awaiting.clear();
    this.clearResyncTimer();
    if (this.facts.resyncing) {
      this.facts.resyncing = false;
      this.h.changed();
    }
  }

  private clearResyncTimer() {
    if (this.resyncTimer !== null) this.env.clearTimeout(this.resyncTimer);
    this.resyncTimer = null;
  }

  /** A reply to one of our requests. */
  private settle(m: any): boolean {
    const p = this.pending.get(m.req);
    if (!p) return m.type === "ack" || m.type === "error";
    this.pending.delete(m.req);
    this.env.clearTimeout(p.timer);
    if (m.type === "error") {
      const { code, reason, message, result, answered_by, at } = m;
      p.resolve({ t: "error", code, reason, message, result, answered_by, at });
    } else {
      p.resolve({ t: "ack", entered: m.entered, result: m.result, qid: m.qid, state: m.state, ahead: m.ahead });
    }
    this.h.changed();
    return true;
  }

  // ---------- losing and retrying ----------

  /** The connection dropped: requests that wait for a reply cannot be told apart from lost ones. A first-wins
   *  answer is kept for one retry after the next hello; the rest are `unknown` (the caller asks the person). */
  private loseInFlight() {
    const now = this.env.now();
    for (const [req, p] of [...this.pending]) {
      this.env.clearTimeout(p.timer);
      this.pending.delete(req);
      if (p.retry && p.attempts < 2 && now - p.firstAt < RETRY_WINDOW_MS) {
        p.lost = true;
        this.lostList.add(p);
        p.timer = this.env.setTimeout(() => this.giveUp(p), Math.max(0, RETRY_WINDOW_MS - (now - p.firstAt)));
      } else {
        p.resolve({ t: "unknown" });
      }
    }
  }

  private giveUp(p: Pending) {
    if (this.lostList.delete(p)) p.resolve({ t: "unknown" });
  }

  private resendLost(epochChanged: boolean) {
    for (const p of [...this.lostList]) {
      this.lostList.delete(p);
      this.env.clearTimeout(p.timer);
      // After a restart the prompt is another one: an answer meant for the old one must not be typed.
      if (epochChanged || this.env.now() - p.firstAt >= RETRY_WINDOW_MS) p.resolve({ t: "unknown" });
      else this.transmit(p);
    }
  }

  private failPending() {
    for (const [req, p] of [...this.pending]) {
      this.env.clearTimeout(p.timer);
      this.pending.delete(req);
      p.resolve({ t: "unknown" });
    }
    for (const p of [...this.lostList]) {
      this.env.clearTimeout(p.timer);
      p.resolve({ t: "unknown" });
    }
    this.lostList.clear();
  }

  // ---------- liveness ----------

  private armTick() {
    this.clearTimer("tick");
    this.timers.tick = this.env.setTimeout(() => {
      this.timers.tick = null;
      if (!this.facts.ready) return;
      // Nothing is checked while the page is hidden: the browser may not even run this.
      if (!this.hideSubs && this.facts.probeSince === null && this.env.now() - this.lastIn > PING_AFTER_MS) this.probe();
      this.h.changed();
      this.armTick();
    }, TICK_MS);
  }

  /** Asks the daemon for a heartbeat; no word within 3 s and the connection is thrown away and made again. */
  private probe() {
    if (!this.facts.ready || this.facts.probeSince !== null) return;
    const now = this.env.now();
    this.facts.probeSince = now;
    this.probeSentAt = now;
    this.post({ type: "hb", n: ++this.beat });
    this.timers.probe = this.env.setTimeout(() => {
      this.timers.probe = null;
      if (this.facts.probeSince !== null) {
        this.lastKickAt = this.env.now();
        this.drop(1006);
      }
    }, PROBE_MS);
    this.h.changed();
  }

  // ---------- timers ----------

  private clearTimer(name: TimerName) {
    const t = this.timers[name];
    if (t !== null) this.env.clearTimeout(t);
    this.timers[name] = null;
  }

  private clearTimers() {
    for (const n of Object.keys(this.timers) as TimerName[]) this.clearTimer(n);
    for (const id of [...this.chatRetry.keys()]) this.cancelChatRetry(id);
  }
}
