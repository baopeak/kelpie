// The phone's one connection to Kelpie and the only place that knows the wire: phone protocol v2 as JSON messages
// (docs/architecture.md, "Phone protocol (v2)") inside kelpie-link, end-to-end encrypted, on the daemon's `/link`
// WebSocket. The framing is a `Codec`: Noise (`linkCodec`, noise.ts) in the app, plain JSON in the tests.
//
// What lives here: the handshake (a Noise IK reconnect with this phone's key, then `{caps}` with `ack` and `hb`, so
// the daemon does not serve this client as one that acknowledges nothing), answering and sending heartbeats,
// reconnecting with full-jitter backoff and on visibility / network triggers, resuming subscriptions, and the offline
// policy: nothing is queued, requests made while not connected are refused at once, and an answer whose reply was lost
// is retried once under the same `cid` (the daemon lets the first answer win and recognises a repeat).
//
// Two routes race: the page's own origin (the computer's Tailscale address) first, and the relay 250 ms later, or at once
// when the direct one has already failed; the first to complete its handshake wins and the other is closed. The relay
// carries the same sealed frames and cannot read them; what it says itself (`desktop_offline`) is a hint, and only ever
// ends a connection. On the relay the direct address is looked at every 30 s and when the network changes, and once it
// has answered for 10 s the link moves back to it with a new handshake, and picks up what it followed.
// The hosted app (hosted.ts) has no direct route: its page is the relay's, so the relay is its only way, and only the relay on the page's
// own host (another is not dialled: the computer moved to it, and this app must be paired again).
import { PHONE_API } from "../../src/lib/attention";
import { backoffMs, LagMeter, reachFromAnswer, whyNoAnswer, type ConnFacts, type Reach } from "./conn-state";
import { HEALTH_PATH, HOSTED, onHost } from "./hosted";
import type { Risk } from "./command-text";
import type { LockInfo } from "./lock";
import { sha256 } from "@noble/hashes/sha2.js";
import { Initiator, MODE_RECONNECT, unb64url, type Endpoint, type Session } from "./noise";
import type { Pairing } from "./pairing-store";
import type { NewSessionRequest } from "./start-session";
import type { ScreenFrame } from "./screen-sync";
import type { PaneRow, TkOp } from "./takeover";
import { MEDIA as SCREEN_MEDIA, type ScOp, type TargetRow } from "./screen";
import type { PathInfo, RelayInfo, ViaId } from "./screen-paths";

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

/** What this device may do on this connection: `capped` when the transport holds it below its record (a browser page over
 *  plain HTTP outside Tailscale may only read). */
export interface DeviceInfo {
  id?: string;
  name: string;
  can_write: boolean;
  permission?: "read" | "write" | "control";
  kind?: "app" | "pwa";
  capped?: boolean;
  /** Why this connection may not take control, as the computer says: `ok`, or the first reason that holds (absent from a computer below API 36). */
  control?: ControlReason;
  /** Why this connection may not see or control the computer's screen, as the computer says: `ok`, or the first reason that holds (from API level 41; the
   *  words are in conn-path.ts, and a reason this app does not know is read as none). */
  screen?: string;
}

/** What the computer says about control on this connection (`device.control`). A reason this app does not know is read as none. */
export type ControlReason = "ok" | "hosted_app" | "not_granted" | "plain_page" | "via_relay" | "lock_off" | "no_passkey" | "locked";

/** Hours with no alert, in this phone's time: `HH:MM` each, `from` after `to` spanning midnight. `tz_min` is the phone's offset east of UTC in minutes. */
export interface QuietHours {
  from: string;
  to: string;
  tz_min: number;
  /** "Needs you" still alerts during them (off: quiet hours hold every alert). */
  needs_you_through: boolean;
}

/** Which alerts this phone wants (`push_prefs`). `kinds` null: whatever the computer sends. */
export interface AlertPrefs {
  kinds: string[] | null;
  quiet: QuietHours | null;
}

/** What this phone may start (`new_options`): the computer's projects (never their folders) and its installed agents, each saying whether it runs
 *  without asking before it acts, as on the computer. `ready` false: the computer has not heard from its app since it started (`why` says so). */
export interface NewOptions {
  ready: boolean;
  projects: { id: string; name: string }[];
  agents: { kind: string; label: string; skips: boolean }[];
  why?: string;
}

/** Where the computer listens, newest `seq` wins. */
export interface AddrsMsg {
  /** Every way to reach the computer, the Tailscale origin first (`list` holds the direct addresses alone). */
  endpoints?: Endpoint[];
  seq: number;
  list: string[];
  /** Left out of a `partial` one. */
  hostname?: string;
  /** Sent to a page the relay hosts and to a locked phone: the relay alone (or none, when it is off), and nothing of the computer's other
   *  addresses. The phone takes the relay from it and keeps the rest of what it knows. */
  partial?: boolean;
}

export interface Hello {
  v: number;
  api: number;
  daemon_epoch: string;
  rev: number;
  device: DeviceInfo;
  sessions: SessionRow[];
  projects: Record<string, string>;
  load: LoadStatus;
  server_ts: number;
  hb?: { idle_s: number; pong_s: number; dead_s: number; legacy: boolean };
  /** Alerts, where this connection may have them (absent over plain HTTP, and from a computer below `PHONE_API`). `prefs` and `events` from
   *  `ALERT_PREFS_API` on: this phone's own choices (null: everything) and the kinds the computer sends at all. */
  push?: { vapid: string | null; subscribed: boolean; prefs?: AlertPrefs | null; events?: string[] };
  /** The computer holds this phone locked: the rest says nothing of any session. */
  locked?: boolean;
  /** The computer reads padded chunks: this phone pads what it sends from now on (absent from a computer before the relay). */
  pad?: boolean;
  lock?: LockInfo;
  /** The computer is also published over Tailscale (absent from a computer below API 36). */
  ts?: boolean;
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

/** What the daemon makes of a chat's request: its label and the command, for the card. */
export interface ChatRequestInfo {
  id: string;
  request_id: string | number;
  command: string;
  risk: Risk;
}

/** A token for the relay, given inside the session (`relay`), and where the relay and this computer's room are. */
export interface RelayGrant {
  url: string;
  ticket: string;
  token: string;
  expires: number;
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
  /** `qid`, `state` and `ahead` come with a `chat_send`: what the message was called, and where it stands. `vapid` with a `push_info`: the key alerts are signed with.
   *  `prefs` with a `push_prefs`: the choices the computer keeps. `ready`, `projects`, `agents` and `why` with a `new_options`; `id` with a `new_session` (the session
   *  started), `dup` when that start was asked for before. */
  | { t: "ack"; entered?: boolean; result?: string; qid?: string; state?: string; ahead?: number; vapid?: string; prefs?: AlertPrefs | null; lock?: LockInfo; resume?: string; tk?: string; title?: string; host?: string; panes?: PaneRow[]; sc?: string; kind?: string; w?: number; h?: number; encoder?: string; t0?: number; targets?: TargetRow[]; paths?: PathInfo[]; relay?: RelayInfo | null; via?: string; ready?: boolean; projects?: NewOptions["projects"]; agents?: NewOptions["agents"]; why?: string; id?: string; dup?: boolean; sdp?: string; nonce?: string; ice_servers?: RTCIceServer[]; turn?: string | null }
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
  chatRequestInfo?(m: ChatRequestInfo): void;
  /** The computer locked this phone, or how its lock stands changed while it is locked. */
  locked?(lock: LockInfo): void;
  chatQueue?(m: ChatQueueMsg): void;
  chatResync(id: string): void;
  /** The computer's addresses (after a handshake, and when they change). */
  addrs?(m: AddrsMsg): void;
  /** A token for the relay, to keep with the pairing. */
  relay?(m: RelayGrant): void;
  /** Another device was paired, changed or removed on the computer. */
  deviceChanged?(m: { name: string; change: string; permission: string | null }): void;
  /** A takeover's frame, heartbeat or end (`tk_frame`, `tk_fhb`, `tk_end`). */
  takeover?(m: { type: string; tk?: string; seq?: number; data?: string; meta?: unknown; reason?: string; paused?: boolean }): void;
  /** The computer's screen: its state, stats, heartbeat, way or end (`sc_state`, `sc_stats`, `sc_fhb`, `sc_path`, `sc_end`). */
  screen?(m: { type: string; sc?: string; [k: string]: unknown }): void;
  /** A binary media message of the screen (video, a crisp tile, the cursor). */
  screenMedia?(bytes: Uint8Array): void;
  /** The computer said, inside the session, that the device was removed: forget the pairing. Nothing else does: a close code
   *  or a refusal before a session is not authenticated, and would let anyone on the path destroy the pairing. */
  unpaired(): void;
  /** The frame number to resume a followed terminal from. */
  haveRev(id: string): number | null;
  /** Where in a followed chat's log the phone is. */
  chatSince(id: string): number | null;
}

// ---------- environment (replaced in tests) ----------

export interface SocketLike {
  onopen: ((e: unknown) => void) | null;
  onmessage: ((e: { data: unknown }) => void) | null;
  onclose: ((e: { code: number }) => void) | null;
  onerror: ((e: unknown) => void) | null;
  readyState: number;
  send(data: Uint8Array | string): void;
  close(code?: number): void;
}

/** Turns protocol messages into socket messages and back, for one connection. */
export interface Codec {
  /** The socket opened: the handshake's first message, or null when there is none. */
  open(): Uint8Array | string | null;
  /** Whether the handshake is done (messages may go both ways). */
  ready(): boolean;
  /** One socket message: the whole protocol messages it completed (none while a handshake or a long message is under way).
   *  Throws when the connection is broken (a message that does not decrypt, a reply from another computer). */
  receive(data: unknown): unknown[];
  send(msg: Record<string, unknown>): (Uint8Array | string)[];
  /** What goes into the first sealed message besides `caps`. */
  greeting?(): Record<string, unknown>;
  /** Pads what this connection sends from now on, so that the carrier sees only a few sizes. */
  padding?(on: boolean): void;
}

/** SHA-256 of a base64url token's bytes, hex: how the computer and the relay know it. */
export function tokenHash(token: string): string {
  return Array.from(sha256(unb64url(token)), (b) => b.toString(16).padStart(2, "0")).join("");
}

/** kelpie-link: a Noise IK reconnect with this phone's key, then every message sealed. */
export function linkCodec(p: Pairing): Codec {
  let init: Initiator | null = null;
  let session: Session | null = null;
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  return {
    open() {
      init = new Initiator(MODE_RECONNECT, { priv: p.priv, pub: p.pub }, p.dpk, null);
      return init.first;
    },
    ready: () => session !== null,
    receive(data) {
      if (!(data instanceof ArrayBuffer)) throw new Error("not kelpie-link");
      const bytes = new Uint8Array(data);
      if (!session) {
        if (!init) throw new Error("no handshake");
        session = init.finish(bytes).session;
        init = null;
        return [];
      }
      const whole = session.open(bytes);
      if (!whole) return [];
      // A screen's media is binary and starts with a byte no JSON message does (crates/kelpie-screen/src/wire.rs).
      if (whole[0] === SCREEN_MEDIA) return [{ type: "sc_media", bytes: whole }];
      return [JSON.parse(dec.decode(whole))];
    },
    send(msg) {
      if (!session) throw new Error("not connected");
      return session.seal(enc.encode(JSON.stringify(msg)));
    },
    // Which relay token this phone holds, so that the computer gives it a new one only when it needs one.
    greeting: () => (p.relayToken ? { relay_token: tokenHash(p.relayToken) } : {}),
    padding(on) {
      if (session) session.pad = on;
    },
  };
}

export interface Env {
  socket(url: string): SocketLike;
  /** The direct route's socket address; null where there is none (the hosted app, whose page is the relay's). */
  url(): string | null;
  /** The one host a relay may be on (the hosted app's own); null: any. A relay elsewhere is not dialled, and said so (`relay: "elsewhere"`). */
  relayHost?(): string | null;
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
    socket: (url) => {
      const ws = new WebSocket(url);
      ws.binaryType = "arraybuffer";
      return ws as unknown as SocketLike;
    },
    url: () => (HOSTED ? null : `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/link`),
    relayHost: () => (HOSTED ? location.host : null),
    now: () => Date.now(),
    setTimeout: (fn, ms) => setTimeout(fn, ms),
    clearTimeout: (h) => clearTimeout(h as number),
    random: () => Math.random(),
    online: () => navigator.onLine !== false,
    async reach() {
      const abort = new AbortController();
      const timer = setTimeout(() => abort.abort(), 3000);
      const began = Date.now();
      try {
        // A file the service worker leaves alone, so that the answer comes from the network: the computer's manifest, or on the hosted app the relay's own word.
        const [path, type] = HOSTED ? [HEALTH_PATH, "text/plain"] : ["/manifest.webmanifest", "manifest+json"];
        const r = await fetch(`${path}?probe=${Date.now()}`, { cache: "no-store", signal: abort.signal });
        return reachFromAnswer(r.status, r.redirected, r.headers.get("content-type") ?? "", type);
      } catch {
        return whyNoAnswer(location.hostname, abort.signal.aborted, Date.now() - began);
      } finally {
        clearTimeout(timer);
      }
    },
  };
}

/** A fresh id for one write: the daemon answers a repeat of it with the first reply. */
function newCid(env: Env): string {
  return globalThis.crypto?.randomUUID?.() ?? `${env.now().toString(36)}-${Math.floor(env.random() * 1e12).toString(36)}`;
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
/** The relay is tried this long after the direct route, unless that one fails first. */
export const RELAY_STAGGER_MS = 250;
/** On the relay, how often the direct route is looked at, and how long it must answer before the link moves back to it. */
export const DIRECT_PROBE_MS = 30_000;
export const DIRECT_STABLE_MS = 10_000;
/** The relay's word that this computer said nothing it could read: the device may not use it. */
const RELAY_REFUSED = 4412;
/** How often a computer that says it does not know this device is asked again. */
const NOT_PAIRED_RETRY_MS = 60_000;
/** Connections in a row that opened, sent their first message and were closed with no reply, while the computer's server answers,
 *  before the phone says this computer may have been reset. A computer with new pairing keys drops such a message unanswered. */
const NO_REPLY_CLOSES = 3;
const HELLO_MS = 8000;
/** An answer is retried once if the connection dropped within this time of sending it. */
export const RETRY_WINDOW_MS = 60_000;
const REPLY_MS = 30_000;
const TICK_MS = 2000;
/** A restart's resync is given up on (and typing allowed) after this long. */
const RESYNC_MS = 5000;

type TimerName = "connect" | "hello" | "retry" | "tick" | "probe" | "stagger" | "direct";

export type Route = "direct" | "relay";

/** A socket being tried, until one completes its handshake. */
interface Attempt {
  ws: SocketLike;
  codec: Codec;
  route: Route;
  opened: boolean;
}

/** What the relay itself said, as opposed to a sealed frame it carried. */
function relayControl(data: unknown): "offline" | "online" | "pong" | null {
  if (typeof data !== "string") return null;
  if (data === "pong") return "pong";
  try {
    const t = (JSON.parse(data) as { t?: unknown }).t;
    return t === "desktop_offline" ? "offline" : t === "desktop_online" ? "online" : null;
  } catch {
    return null;
  }
}

/** The close a race loser is given: the computer then does not count it as a handshake left unfinished. */
export const ROUTE_LOST = 4000;

function closeQuietly(ws: SocketLike, code?: number) {
  ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null;
  try {
    ws.close(code);
  } catch {
    /* already gone */
  }
}

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
    unpaired: false, notPairedHere: null, outdated: false, restartedAt: 0, rttMs: null, rttAt: 0, probeSince: null, lagMs: 0, lagAt: 0,
    resyncing: false, load: { level: 0, reasons: [] }, route: "direct", relay: "none",
  };
  epoch: string | null = null;
  device: DeviceInfo | null = null;
  /** The newest message time heard (ms epoch, phone clock). */
  lastIn = 0;
  readonly lag = new LagMeter();

  private ws: SocketLike | null = null;
  private codec: Codec | null = null;
  /** The sockets being tried, while none has won. */
  private attempts: Attempt[] = [];
  /** What the routes that failed said, the most telling first (`attemptFailed`). */
  private failCode = 0;
  /** On the relay: a direct connection being made to move to, and since when the direct route answers. */
  private migration: Attempt | null = null;
  private directOkSince: number | null = null;
  private readyAt = 0;
  private hideSubs = false;
  private stopped = true;
  private timers = { connect: null as unknown, hello: null as unknown, retry: null as unknown, tick: null as unknown, probe: null as unknown, stagger: null as unknown, direct: null as unknown };
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

  /** This connection opened and sent the phone's first message. */
  private sentFirst = false;
  /** The secret of the window this page opened. Never kept. */
  private resume: string | null = null;
  /** The computer holds this phone locked: nothing is followed until it says otherwise, and what is wanted waits. */
  private lockedNow = false;
  private noReplyCloses = 0;

  /** `codec` makes the framing for one connection; null while this phone is not paired. `relayUrl`: the relay's address with this phone's ticket and token, when it has them. */
  constructor(private env: Env, private codecFor: () => Codec | null, private h: LinkHandlers, private relayUrl: () => string | null = () => null) {}

  // ---------- life ----------

  start() {
    this.stopped = false;
    this.lastKickAt = this.env.now();
    this.facts.online = this.env.online();
    this.facts.unpaired = false;
    this.facts.notPairedHere = null;
    this.facts.replaced = false;
    if (this.facts.relay === "refused") this.facts.relay = "unknown";
    this.noReplyCloses = 0;
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
      this.checkDirect();
      return;
    }
    if (now - this.lastKickAt < KICK_GAP_MS) {
      // Too soon to go at once. With nothing else on its way (the network was just gone, so no retry was counted down), it goes when the gap ends:
      // a page that is not left waiting for nothing.
      if (!this.facts.connecting && this.timers.retry === null) {
        const wait = this.lastKickAt + KICK_GAP_MS - now;
        this.facts.nextAt = now + wait;
        this.timers.retry = this.env.setTimeout(() => {
          this.timers.retry = null;
          this.kick();
        }, wait);
      }
      return;
    }
    this.lastKickAt = now;
    this.open();
  }

  /** A frozen picture asks: `kick`, except that a connection already being made is left to finish (within its own `CONNECT_MS`), so that a
   *  slow handshake (mobile data, a relay far away) is never started over every few seconds. */
  nudge() {
    if (this.facts.connecting && !this.facts.ready) return;
    this.kick();
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

  /** The network changed under the page (another Wi-Fi, mobile data): on the relay, the direct route may be back. */
  networkChanged() {
    this.directOkSince = null;
    this.checkDirect();
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
    if (!this.facts.ready || this.hideSubs || this.lockedNow) return;
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
      // `cid` names a write for the daemon: a retry of it (after a lost reply, over any connection) is answered, never done twice.
      const cid = opts.write === false || msg.cid !== undefined ? {} : { cid: newCid(this.env) };
      const p: Pending = { msg: { ...msg, ...cid, ...(opts.write === false ? {} : { epoch: this.epoch }) }, resolve, retry: !!opts.retry, firstAt: this.env.now(), attempts: 0, lost: false, timer: null };
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
    if (!this.ws || this.ws.readyState !== 1 || !this.codec?.ready()) return;
    try {
      for (const frame of this.codec.send(msg)) this.ws.send(frame);
    } catch {
      this.drop(1006);
    }
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
  answer(id: string, waitingSince: number | null, answer: { key: string } | { text: string }, enter: boolean, recheck?: Record<string, unknown>) {
    return this.request({ type: "answer", id, waiting_since: waitingSince, ...answer, enter, ...(recheck ? { recheck } : {}) }, { retry: true });
  }

  chatAnswer(id: string, requestId: string | number, line: string, recheck?: Record<string, unknown>) {
    return this.request({ type: "chat_answer", id, request_id: requestId, line, ...(recheck ? { recheck } : {}) }, { retry: true });
  }

  /** Sends a message to a chat. `cid` names it: if the connection drops with it in flight it is sent once more, with the
   *  same `cid`, after the next hello, and the daemon answers `dup` for a message it has (never a second one). */
  chatSend(id: string, cid: string, text: string) {
    return this.request({ type: "chat_send", id, cid, text }, { retry: true });
  }

  /** What this phone may start: the projects and installed agents of the computer's launch catalogue. */
  newOptions() {
    return this.request({ type: "new_options" });
  }

  /** Starts an agent in a project with the computer's settings for it. Sent once more after a dropped connection, with the same `cid`: the
   *  computer answers a repeat with the session it started (`dup`), never a second one. */
  newSession(req: NewSessionRequest) {
    return this.request({ type: "new_session", ...req }, { retry: true });
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

  /** A passkey ceremony's message (`lock_challenge`, `lock_enrol`, `lock_unlock`): what it says is the reply. */
  lockAsk(msg: Record<string, unknown>) {
    return this.request(msg, { write: false });
  }

  /** The person left the app. No reply is waited for: the page is on its way out. */
  lockNow() {
    this.post({ type: "lock_now" });
  }

  lockTouch() {
    this.post({ type: "lock_touch" });
  }

  // A pane taken over (takeover.ts). Input and acknowledgements need no reply; what they come to is in the frames.

  panes() {
    return this.request({ type: "panes" }, { write: false });
  }

  tkStart(pane: string) {
    return this.request({ type: "tk_start", pane }, { write: false });
  }

  tkStop(tk: string) {
    return this.request({ type: "tk_stop", tk }, { write: false });
  }

  tkRebind(tk: string) {
    return this.request({ type: "tk_rebind", tk }, { write: false });
  }

  /** `st`: the computer's clock as this phone reads it, so that it can drop what came too late. */
  /** `w`: the window id of the frame the input was mapped against. */
  tkText(tk: string, text: string, st: number, w: number) {
    return this.request({ type: "tk_text", tk, text, st, w }, { write: false });
  }

  tkInput(tk: string, ops: TkOp[], st: number, w: number) {
    this.post({ type: "tk_input", tk, ops, st, w });
  }

  tkAck(tk: string, seq: number) {
    this.post({ type: "tk_ack", tk, seq });
  }

  // The computer's screen (screen.ts). Input, acknowledgements and keyframe requests need no reply.

  screens() {
    return this.request({ type: "screens" }, { write: false });
  }

  /** `via`: the way chosen (the computer picks when it is left out). `recheck`: the person's Face ID over this screen, which a start through the relay needs.
   *  `p2p`: this phone can go peer to peer (Automatic may then start that way). */
  scStart(target: string, via?: ViaId | null, recheck?: Record<string, unknown>, p2p = false) {
    return this.request({ type: "sc_start", target, ...(via ? { via } : {}), ...(recheck ? { recheck } : {}), ...(p2p ? { p2p } : {}) }, { write: false });
  }

  scStop(sc: string) {
    return this.request({ type: "sc_stop", sc }, { write: false });
  }

  scRebind(sc: string) {
    return this.request({ type: "sc_rebind", sc }, { write: false });
  }

  scText(sc: string, text: string, st: number) {
    return this.request({ type: "sc_text", sc, text, st }, { write: false });
  }

  scInput(sc: string, ops: ScOp[], st: number) {
    this.post({ type: "sc_input", sc, ops, st });
  }

  scAck(sc: string, seq: number) {
    this.post({ type: "sc_ack", sc, seq });
  }

  scKey(sc: string) {
    this.post({ type: "sc_key", sc });
  }

  scResume(sc: string) {
    this.post({ type: "sc_resume", sc });
  }

  scSharpen(sc: string, rect: { x: number; y: number; w: number; h: number }) {
    this.post({ type: "sc_sharpen", sc, rect });
  }

  // The screen peer to peer (screen-rtc.ts): the offer and its answer, late candidates, and the way from now on.

  scRtcOffer(sc: string, sdp: string, relayOnly = false) {
    return this.request({ type: "sc_rtc_offer", sc, sdp, ...(relayOnly ? { relay_only: true } : {}) }, { write: false });
  }

  scRtcServers(sc: string, relayOnly = false) {
    return this.request({ type: "sc_rtc_servers", sc, ...(relayOnly ? { relay_only: true } : {}) }, { write: false });
  }

  scRtcIce(sc: string, cand: string) {
    this.post({ type: "sc_rtc_ice", sc, cand });
  }

  scVia(sc: string, via: ViaId) {
    return this.request({ type: "sc_via", sc, via }, { write: false });
  }

  /** The computer's clock as this phone reads it: this phone's, less the smallest difference seen lately. */
  serverNow(): number {
    return this.lag.serverNow(this.env.now());
  }

  /** The secret of the window this page opened: said in the first message of every connection from now on. */
  setResume(secret: string | null) {
    this.resume = secret;
  }

  // ---------- socket ----------

  private open() {
    this.clearTimer("retry");
    this.facts.nextAt = null;
    this.sentFirst = false;
    if (!this.codecFor()) return;
    this.detach();
    this.failCode = 0;
    this.facts.connecting = true;
    this.facts.ready = false;
    const relayAddress = this.relayUrl();
    if (!relayAddress) this.facts.relay = "none";
    else if (!this.relayAllowed(relayAddress)) this.facts.relay = "elsewhere";
    else if (this.facts.relay === "none" || this.facts.relay === "elsewhere") this.facts.relay = "unknown";
    // The computer said it will not serve this device through the relay: no use asking again until the link starts over.
    const relay = this.facts.relay === "refused" || this.facts.relay === "elsewhere" ? null : relayAddress;
    // No route completed its handshake in this long: the attempt is over.
    this.timers.connect = this.env.setTimeout(() => this.attempts.length && this.drop(1006), CONNECT_MS);
    const directUrl = this.env.url();
    const direct = directUrl !== null && this.tryRoute("direct", directUrl);
    if (relay && direct) this.timers.stagger = this.env.setTimeout(() => this.tryRelay(), RELAY_STAGGER_MS);
    else if (relay) this.tryRoute("relay", relay);
    if (!this.attempts.length) return this.closed(this.failCode || 1006, false);
    this.h.changed();
  }

  /** Whether a relay at `url` may be dialled from this page: on the hosted app only the relay that serves it. */
  private relayAllowed(url: string): boolean {
    const only = this.env.relayHost?.() ?? null;
    return only === null || onHost(url, only);
  }

  /** The relay's address, when there is one this page may dial. */
  private usableRelay(): string | null {
    const url = this.relayUrl();
    return url && this.relayAllowed(url) ? url : null;
  }

  /** The relay's turn in the race (or at once, when the direct route has failed). */
  private tryRelay() {
    this.clearTimer("stagger");
    const url = this.usableRelay();
    if (!url || this.ws || this.facts.relay === "refused" || this.attempts.some((a) => a.route === "relay")) return;
    if (!this.tryRoute("relay", url) && !this.attempts.length) this.closed(this.failCode || 1006, false);
  }

  /** Opens one route's socket; false when it could not even be made. The first whose handshake completes wins (`win`); the others are closed. */
  private tryRoute(route: Route, url: string): boolean {
    const codec = this.codecFor();
    if (!codec) return false;
    let ws: SocketLike;
    try {
      ws = this.env.socket(url);
    } catch {
      if (route === "relay") this.facts.relay = "down";
      this.failCode ||= 1006;
      return false;
    }
    const a: Attempt = { ws, codec, route, opened: false };
    this.attempts.push(a);
    ws.onopen = () => {
      if (!this.attempts.includes(a)) return;
      a.opened = true;
      if (route === "relay" && this.facts.relay !== "no_computer") this.facts.relay = "up";
      const first = codec.open();
      this.sentFirst = true;
      if (first === null) this.win(a);
      else ws.send(first);
    };
    ws.onmessage = (e) => {
      if (this.ws === ws) return this.onSocketMessage(ws, e.data);
      if (!this.attempts.includes(a)) return;
      const said = route === "relay" ? relayControl(e.data) : null;
      if (said === "offline") {
        // The relay has no computer for this room now: no use waiting on this route. What this route sent reached no computer, so it
        // was not left unanswered by one: a computer that is away is not one that was reset (the relay itself answering says nothing of it).
        this.facts.relay = "no_computer";
        this.sentFirst = false;
        return this.attemptFailed(a, route, 1006);
      }
      if (said) return;
      try {
        codec.receive(e.data);
      } catch {
        return this.attemptFailed(a, route, 1006);
      }
      if (codec.ready()) this.win(a);
    };
    ws.onclose = (e) => {
      if (this.ws === ws) return this.closed(e.code);
      if (this.attempts.includes(a)) this.attemptFailed(a, route, e.code, a.opened);
    };
    ws.onerror = () => {};
    return true;
  }

  /** One route failed before its handshake; when none is left, the connection failed with what the most telling of them said. */
  private attemptFailed(a: Attempt, route: Route, code: number, opened = false) {
    this.attempts = this.attempts.filter((x) => x !== a);
    closeQuietly(a.ws);
    if (route === "relay") {
      if (code === RELAY_REFUSED) this.facts.relay = "refused";
      else if (!opened && this.facts.relay !== "no_computer") this.facts.relay = "down";
    }
    // A code from the computer says more than a socket that just ended; the relay refusing this device says nothing of the computer.
    if (code !== RELAY_REFUSED && this.failCode < 4000 && (code >= 4000 || !this.failCode)) this.failCode = code;
    // The direct route is gone already: the relay need not wait its turn.
    if (route === "direct" && this.timers.stagger !== null) {
      this.clearTimer("stagger");
      const url = this.usableRelay();
      if (url && !this.ws && this.facts.relay !== "refused") this.tryRoute("relay", url);
    }
    if (this.attempts.length || this.ws) return this.h.changed();
    this.closed(this.failCode || code, false);
  }

  /** This route completed its handshake first: it is the connection. */
  private win(a: Attempt) {
    this.clearTimer("connect");
    this.clearTimer("stagger");
    for (const other of this.attempts) if (other !== a) closeQuietly(other.ws, ROUTE_LOST);
    this.attempts = [];
    this.ws = a.ws;
    this.codec = a.codec;
    this.facts.route = a.route;
    // The hello must come within this long of the handshake.
    this.timers.hello = this.env.setTimeout(() => this.ws === a.ws && this.drop(1006), HELLO_MS);
    this.greet();
    this.h.changed();
  }

  /** `caps`: this client acknowledges frames and answers heartbeats (without both the daemon serves it as one that does not), can use a
   *  relay token, and reads padded chunks. `resume`: it is the page that unlocked, so a flap asks nothing. */
  private greet() {
    this.post({ caps: ["ack", "hb", "relay", "pad"], ...(this.codec?.greeting?.() ?? {}), ...(this.resume ? { resume: this.resume } : {}) });
  }

  private onSocketMessage(ws: SocketLike, data: unknown) {
    if (this.facts.route === "relay") {
      const said = relayControl(data);
      // The computer left the relay: the session it had with this phone went with it.
      if (said === "offline") {
        this.facts.relay = "no_computer";
        return this.drop(1006);
      }
      if (said) return;
    }
    let msgs: unknown[];
    try {
      msgs = this.codec!.receive(data);
    } catch {
      // A message that does not decrypt (or a reply from another computer): this connection is over; a new one starts
      // with a new handshake.
      return this.drop(1006);
    }
    for (const m of msgs) {
      if (this.ws !== ws) return;
      if (m && typeof m === "object") this.onMessage(m);
    }
  }

  // ---------- moving back from the relay ----------

  /** On the relay: does the direct route answer? Once it has for `DIRECT_STABLE_MS`, the link moves to it. */
  private checkDirect() {
    if (!this.facts.ready || this.facts.route !== "relay" || this.migration || this.env.url() === null) return;
    this.clearTimer("direct");
    void this.env.reach().then((r) => {
      if (!this.facts.ready || this.facts.route !== "relay" || this.migration) return;
      if (r !== "reachable") {
        this.directOkSince = null;
        return this.scheduleDirect(DIRECT_PROBE_MS);
      }
      const now = this.env.now();
      if (this.directOkSince === null) {
        this.directOkSince = now;
        return this.scheduleDirect(DIRECT_STABLE_MS);
      }
      if (now - this.directOkSince >= DIRECT_STABLE_MS) this.migrate();
      else this.scheduleDirect(DIRECT_STABLE_MS - (now - this.directOkSince));
    });
  }

  private scheduleDirect(ms: number) {
    this.clearTimer("direct");
    if (this.facts.route === "relay" && this.env.url() !== null) this.timers.direct = this.env.setTimeout(() => this.checkDirect(), ms);
  }

  /** A new handshake on the direct route while the relay carries on; the relay's connection goes only once this one is made. */
  private migrate() {
    const codec = this.codecFor();
    const url = this.env.url();
    if (!codec || url === null) return;
    let ws: SocketLike;
    try {
      ws = this.env.socket(url);
    } catch {
      return;
    }
    const a: Attempt = { ws, codec, route: "direct", opened: false };
    this.migration = a;
    const giveUp = () => {
      if (this.migration !== a) return;
      this.migration = null;
      this.directOkSince = null;
      closeQuietly(ws);
      this.scheduleDirect(DIRECT_PROBE_MS);
    };
    const timer = this.env.setTimeout(giveUp, CONNECT_MS);
    ws.onopen = () => {
      if (this.migration !== a) return;
      const first = codec.open();
      if (first === null) this.moveTo(a, timer);
      else ws.send(first);
    };
    ws.onmessage = (e) => {
      if (this.ws === ws) return this.onSocketMessage(ws, e.data);
      if (this.migration !== a) return;
      try {
        codec.receive(e.data);
      } catch {
        return giveUp();
      }
      if (codec.ready()) this.moveTo(a, timer);
    };
    ws.onclose = (e) => {
      if (this.ws === ws) return this.closed(e.code);
      giveUp();
    };
    ws.onerror = () => {};
  }

  private moveTo(a: Attempt, timer: unknown) {
    this.env.clearTimeout(timer);
    this.migration = null;
    this.directOkSince = null;
    const old = this.ws;
    // What was in flight on the relay may not be answered there: an answer is sent again after the new hello, as after any drop.
    this.loseInFlight();
    this.clearTimer("probe");
    this.clearTimer("direct");
    this.facts.probeSince = null;
    this.ws = a.ws;
    this.codec = a.codec;
    this.facts.route = "direct";
    if (old) closeQuietly(old);
    this.timers.hello = this.env.setTimeout(() => this.ws === a.ws && this.drop(1006), HELLO_MS);
    this.greet();
    this.h.changed();
  }

  /** Stops listening to the current socket and closes it, with any being tried. */
  private detach() {
    for (const a of this.attempts) closeQuietly(a.ws);
    this.attempts = [];
    this.clearTimer("stagger");
    if (this.migration) closeQuietly(this.migration.ws);
    this.migration = null;
    const ws = this.ws;
    this.ws = null;
    this.codec = null;
    if (ws) closeQuietly(ws);
  }

  /** The connection is dead as far as this client is concerned. */
  private drop(code: number) {
    // Asked first: `detach` lets go of the codec, and a session given up on is still one the computer answered.
    const inSession = this.codec?.ready() ?? false;
    this.detach();
    this.closed(code, inSession);
  }

  private closed(code: number, inSession = this.codec?.ready() ?? false) {
    const sentFirst = this.sentFirst;
    this.down();
    // Before a session the computer says it does not know this device, or cannot confirm it: not a proof of anything, so the key
    // stays and the person decides. Asked again now and then: an answer forged on the way, or a record the computer could
    // not read, does not last. Not often enough to count as an attack at the computer.
    if (!inSession && (code === 4401 || code === 4402)) {
      this.facts.notPairedHere = code === 4401 ? "unknown" : "unconfirmed";
      this.timers.retry = this.env.setTimeout(() => this.open(), NOT_PAIRED_RETRY_MS);
      return this.h.changed();
    }
    // Closed with no reply and no reason, and more than once, by a computer that plainly answers otherwise.
    if (inSession || code >= 4000) this.noReplyCloses = 0;
    else if (sentFirst) this.noReplyCloses++;
    if (this.noReplyCloses >= NO_REPLY_CLOSES && this.facts.reach === "reachable" && this.facts.online) {
      this.facts.notPairedHere = "reset";
      // Whether the computer still answers at all: when it stops, it is off, not reset.
      this.findOutWhy();
      this.timers.retry = this.env.setTimeout(() => this.open(), NOT_PAIRED_RETRY_MS);
      return this.h.changed();
    }
    this.facts.notPairedHere = null;
    // Another window of this device took the connection: wait until this one is looked at.
    if (code === 4409) {
      this.facts.replaced = true;
      return this.h.changed();
    }
    // 4408 (changed / behind), 4410 (silent), 4411 (could not keep up), a 4401 in a session with no sealed word before it (which
    // proves nothing) and anything else: connect again.
    this.failed();
  }

  /** What ending a connection means for this client, whatever comes next. */
  private down() {
    this.ws = null;
    for (const a of this.attempts) closeQuietly(a.ws);
    this.attempts = [];
    if (this.migration) closeQuietly(this.migration.ws);
    this.migration = null;
    this.directOkSince = null;
    this.clearTimer("connect");
    this.clearTimer("hello");
    this.clearTimer("probe");
    this.clearTimer("tick");
    this.clearTimer("stagger");
    this.clearTimer("direct");
    const lasted = this.facts.ready && this.env.now() - this.readyAt > LASTED_MS;
    this.facts.ready = false;
    this.facts.connecting = false;
    this.facts.probeSince = null;
    this.awaiting.clear();
    this.clearResyncTimer();
    this.facts.resyncing = false;
    this.loseInFlight();
    if (lasted) this.facts.attempt = 0;
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
        this.facts.notPairedHere = null;
        this.noReplyCloses = 0;
        return this.onHello(m as Hello);
      // Sealed, so it is the computer's word: the device was removed.
      case "revoked":
        this.detach();
        this.down();
        this.facts.unpaired = true;
        this.h.unpaired();
        return this.h.changed();
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
      case "chat_request_info":
        this.h.chatRequestInfo?.(m as ChatRequestInfo);
        break;
      case "locked":
        this.lockedNow = true;
        this.h.locked?.(m.lock as LockInfo);
        break;
      case "chat_queue":
        this.h.chatQueue?.(m as ChatQueueMsg);
        break;
      case "chat_resync":
        this.refollow(m.id);
        this.h.chatResync(m.id);
        break;
      case "addrs":
        this.h.addrs?.(m as AddrsMsg);
        break;
      case "relay":
        if (typeof m.token === "string" && typeof m.url === "string" && typeof m.ticket === "string") this.h.relay?.(m as RelayGrant);
        break;
      case "device_changed":
        this.h.deviceChanged?.(m);
        break;
      case "tk_frame":
      case "tk_fhb":
      case "tk_end":
        this.h.takeover?.(m);
        // A frame changes nothing else the views show.
        if (m.type !== "tk_end") return;
        break;
      // The screen: media many times a second, which changes nothing else the views show.
      case "sc_media":
        this.h.screenMedia?.(m.bytes as Uint8Array);
        return;
      case "sc_state":
      case "sc_stats":
      case "sc_fhb":
      case "sc_path":
      case "sc_rtc_servers":
      case "sc_end":
        this.h.screen?.(m);
        if (m.type !== "sc_end") return;
        break;
      default:
        break;
    }
    this.h.changed();
  }

  private onHello(hello: Hello) {
    this.clearTimer("hello");
    const now = this.env.now();
    this.device = hello.device;
    this.facts.ready = true;
    this.facts.connecting = false;
    this.facts.reach = "reachable";
    this.facts.outdated = hello.api < PHONE_API;
    this.facts.load = { level: hello.load?.level ?? 0, reasons: hello.load?.reason ?? [] };
    this.facts.lastLiveAt = now;
    this.facts.probeSince = null;
    this.readyAt = now;
    // The computer pads what it sends to this phone, and says it reads padded chunks: from now on both directions do.
    if (hello.pad) this.codec?.padding?.(true);
    this.awaiting.clear();
    this.clearResyncTimer();
    // Locked, it says nothing of any session. What depends on the computer's run waits for the hello that does: that is the one that finds a restart.
    this.lockedNow = !!hello.locked;
    if (hello.locked) {
      this.facts.resyncing = false;
      this.armTick();
      this.h.locked?.(hello.lock ?? { state: "unlock" });
      return this.h.changed();
    }
    const epochChanged = this.epoch !== null && this.epoch !== hello.daemon_epoch;
    this.epoch = hello.daemon_epoch;
    if (epochChanged) this.facts.restartedAt = now;
    this.armTick();
    if (this.facts.route === "relay") this.scheduleDirect(DIRECT_PROBE_MS);
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
      p.resolve({ t: "ack", entered: m.entered, result: m.result, qid: m.qid, state: m.state, ahead: m.ahead, vapid: m.vapid, prefs: m.prefs, lock: m.lock, resume: m.resume, tk: m.tk, title: m.title, host: m.host, panes: m.panes, sc: m.sc, kind: m.kind, w: m.w, h: m.h, encoder: m.encoder, t0: m.t0, targets: m.targets, paths: m.paths, relay: m.relay, via: m.via, ready: m.ready, projects: m.projects, agents: m.agents, why: m.why, id: m.id, dup: m.dup, sdp: m.sdp, nonce: m.nonce, ice_servers: m.ice_servers, turn: m.turn });
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
