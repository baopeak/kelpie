// The one answer to "can I trust what I see, and can I act?": a single connection state, derived from
// facts the link collects, shown by an always-visible indicator and used to decide what is disabled.

/** Why the last connection attempt failed, as far as a same-origin fetch can tell. */
export type Reach =
  /** Not known yet. */
  | "unknown"
  /** The computer's server answered (so the socket failed for another reason). */
  | "reachable"
  /** Something answered with a page that is not Kelpie's: a captive portal wants a sign-in. */
  | "portal"
  /** Nothing answered. */
  | "unreachable";

export interface ConnFacts {
  /** `navigator.onLine`. */
  online: boolean;
  /** The socket is open and the daemon's hello was received. */
  ready: boolean;
  /** The socket is being opened (first connect, or between attempts). */
  connecting: boolean;
  /** Reconnect attempts since the last connection that lasted. */
  attempt: number;
  /** When the next attempt starts (ms epoch), if one is scheduled. */
  nextAt: number | null;
  reach: Reach;
  /** When this page last had a live connection (ms epoch), 0 if never. */
  lastLiveAt: number;
  /** Another window of this device took the connection. */
  replaced: boolean;
  /** The computer said, inside the session, that this device was removed. */
  unpaired: boolean;
  /** Before any session the computer answered that it does not know this device, or cannot confirm its pairing now, or keeps not
   *  answering it at all. That is no proof of anything (anyone on the path can do it), so the key stays until the person forgets it. */
  notPairedHere: NotPairedHere | null;
  /** The daemon is older than this client needs. */
  outdated: boolean;
  /** The daemon has restarted since this page last saw it (set for a few seconds after the hello). */
  restartedAt: number;
  /** Last measured round trip, ms, and when. */
  rttMs: number | null;
  rttAt: number;
  /** A heartbeat of ours is still unanswered since this time (ms epoch), if any. */
  probeSince: number | null;
  /** How far behind the newest frame was when it arrived, ms, and when. */
  lagMs: number;
  lagAt: number;
  /** The computer's load (`load.status`). */
  load: { level: number; reasons: string[] };
  /** The daemon restarted: what the phone follows is being sent again, and has not all arrived yet. */
  resyncing: boolean;
  /** The route in use; "relay" arrives with the relay (a later phase). */
  route: "direct" | "relay";
}

/** `reset`: the computer's server answers and it has said nothing to this device's handshakes, which is what a computer with new
 *  pairing keys does. */
export type NotPairedHere = "unknown" | "unconfirmed" | "reset";

export type ConnState =
  | { kind: "connecting" }
  | { kind: "live" }
  | { kind: "degraded"; why: "slow" | "late" }
  | { kind: "reconnecting"; attempt: number; nextInS: number | null }
  | { kind: "no_network" }
  | { kind: "sign_in" }
  | { kind: "computer_offline"; since: number }
  | { kind: "relay_unreachable" }
  | { kind: "restarted"; resyncing: boolean }
  | { kind: "busy"; level: number; reasons: string[] }
  | { kind: "replaced" }
  | { kind: "unpaired" }
  | { kind: "not_paired_here"; why: NotPairedHere }
  | { kind: "outdated" };

export const SLOW_RTT_MS = 800;
export const SLOW_LAG_MS = 2000;
export const NOT_RESPONDING_MS = 5000;
/** A measurement older than this says nothing about now. */
const FRESH_MS = 20_000;
/** How long "the computer restarted" stays on after the hello. */
export const RESTARTED_SHOWN_MS = 6000;

export function deriveConn(f: ConnFacts, now: number): ConnState {
  if (f.unpaired) return { kind: "unpaired" };
  if (f.notPairedHere) return { kind: "not_paired_here", why: f.notPairedHere };
  if (f.outdated) return { kind: "outdated" };
  if (f.replaced) return { kind: "replaced" };
  if (f.ready) {
    if (now - f.restartedAt < RESTARTED_SHOWN_MS || f.resyncing) return { kind: "restarted", resyncing: f.resyncing };
    const lateFor = f.probeSince !== null ? now - f.probeSince : 0;
    if (lateFor > SLOW_RTT_MS) return { kind: "degraded", why: "late" };
    if (f.rttMs !== null && f.rttMs > SLOW_RTT_MS && now - f.rttAt < FRESH_MS) return { kind: "degraded", why: "slow" };
    if (f.lagMs > SLOW_LAG_MS && now - f.lagAt < FRESH_MS / 2) return { kind: "degraded", why: "slow" };
    if (f.load.level >= 1) return { kind: "busy", level: f.load.level, reasons: f.load.reasons };
    return { kind: "live" };
  }
  if (!f.online) return { kind: "no_network" };
  if (f.attempt === 0 && f.connecting && f.lastLiveAt === 0) return { kind: "connecting" };
  if (f.reach === "portal") return { kind: "sign_in" };
  if (f.reach === "unreachable" && f.attempt >= 2) {
    return f.route === "relay" ? { kind: "relay_unreachable" } : { kind: "computer_offline", since: f.lastLiveAt };
  }
  return { kind: "reconnecting", attempt: Math.max(f.attempt, 1), nextInS: f.nextAt === null ? null : Math.max(0, Math.ceil((f.nextAt - now) / 1000)) };
}

/** Whether typing, keys and answers may be sent now. Allowed while the socket is open and authenticated and
 *  the picture is current: `live`, `degraded` (a slow link still delivers) and `busy` (a busy computer
 *  must not lock the person out). After a restart only once what the phone follows has been sent again,
 *  because ids and positions were new. Every other state refuses: nothing is queued, and the reply box
 *  stays editable so a draft is kept. */
export function canAct(s: ConnState): boolean {
  if (s.kind === "restarted") return !s.resyncing;
  return s.kind === "live" || s.kind === "degraded" || s.kind === "busy";
}

/** What the phone is showing is the last thing it knew, not the computer as it is. */
export function isStale(s: ConnState): boolean {
  return !canAct(s);
}

/** Full-jitter reconnect delay: the first retry within 200 ms, then up to 1 s, doubling to 30 s. */
export function backoffMs(attempt: number, random: () => number): number {
  const cap = attempt <= 1 ? 200 : Math.min(30_000, 1000 * 2 ** (attempt - 2));
  return Math.floor(random() * cap);
}

/** How far behind the daemon's clock a message arrived, without knowing how far the two clocks
 *  differ: the smallest difference seen lately is taken as "no delay". */
export class LagMeter {
  private samples: number[] = [];

  constructor(private keep = 24) {}

  /** `serverTs` is the daemon's wall clock when it made the message, `receivedAt` the phone's when it came. Returns the lag in ms. */
  add(serverTs: number, receivedAt: number): number {
    const diff = receivedAt - serverTs;
    this.samples.push(diff);
    if (this.samples.length > this.keep) this.samples.shift();
    return diff - Math.min(...this.samples);
  }
}

/** How old what is shown is, for the data-age chip: fine, slow (amber) or not responding (red). */
export function dataAge(lagMs: number, silentMs: number): "fresh" | "slow" | "stuck" {
  const age = Math.max(lagMs, silentMs);
  return age > NOT_RESPONDING_MS ? "stuck" : age > SLOW_LAG_MS ? "slow" : "fresh";
}
