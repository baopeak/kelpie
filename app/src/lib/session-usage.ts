// Context fill, tokens and cost per session. Terminal sessions of Claude Code, Codex and OMP are
// read from the agent's own transcript by the app core (`session_usage`), matched by the
// conversation id the daemon recorded — a session without an id has no numbers. Chat sessions
// carry theirs in the chat controller. Nothing is read for a session nobody looks at: a read
// follows the daemon's `session.turn` event (debounced) and only for a visible or working one.
import { createEffect, untrack } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import { store } from "./store";
import { sessionUsage } from "./ipc";
import { sessionHome } from "./accounts";
import { getController } from "../chat/mode-switch";
import { DANGER_AT, overCap, ringFraction } from "./usage-format";
import type { SessionInfo, SessionUsage } from "./types";

/** First daemon API level that sends `session.turn`. */
export const TURN_API = 9;
/** A read follows the event by this long, and several events share one. */
const DEBOUNCE_MS = 1000;
/** A visible session is read again when it comes into view and its reading is older than this. */
const VISIBLE_STALE_MS = 30_000;
/** Older daemons send no events: a working, visible session is read this often instead. */
const POLL_MS = 5000;
const CATCH_UP_MS = 2000;
const CATCH_UP_TRIES = 45;

/** Events that end or reshape a turn: read even when the session is out of sight. */
const TURN_END = new Set(["Stop", "StopFailure", "SessionStart", "PostCompact", "SubagentStop", "agent-turn-complete", "turn_end"]);
const TRANSCRIPT_KINDS = ["claude", "codex", "omp"];

/** What the ring and the tooltips show for a session. */
export interface UsageView {
  used: number;
  window: number | null;
  /** Share of the window in use; null when the window is unknown (OMP, an unpriced model). */
  fraction: number | null;
  tokensIn: number | null;
  tokensOut: number | null;
  cost: number | null;
  /** The agent's own cost rather than an estimate at API prices. */
  exact: boolean;
  model: string | null;
  /** The totals are still being counted in the background. */
  counting: boolean;
  /** The conversation these numbers belong to. */
  conv: string | null;
}

/** Readings by session id, with the conversation they belong to (a /clear starts another). */
const [readings, setReadings] = createStore<Record<string, { conv: string; u: SessionUsage }>>({});
/** `transcript_path` from the last hook of each session, with its conversation. */
const turnPaths = new Map<string, { conv: string; path: string }>();
const timers = new Map<string, ReturnType<typeof setTimeout>>();
const retries = new Map<string, ReturnType<typeof setTimeout>>();
/** Read counter per session: an older reply never replaces a newer one. */
const sequence = new Map<string, number>();
const lastRead = new Map<string, number>();

/** A terminal session whose transcript can be named exactly. */
function readable(s: SessionInfo): s is SessionInfo & { agent_session_id: string } {
  return !s.chat && !s.remote && TRANSCRIPT_KINDS.includes(s.kind) && !!s.agent_session_id && s.agent_session_id !== "latest";
}

export function usageView(s: SessionInfo): UsageView | null {
  if (s.chat) {
    const ctrl = getController(s);
    const u = ctrl.state.usage;
    if (!u) return null;
    return {
      used: u.used,
      window: u.size,
      fraction: ringFraction(u.used, u.size),
      tokensIn: null,
      tokensOut: null,
      cost: u.cost ?? null,
      exact: s.kind !== "claude",
      model: ctrl.state.model,
      counting: false,
      conv: ctrl.state.conversationId ?? "chat",
    };
  }
  const r = readings[s.id];
  if (!r || r.conv !== s.agent_session_id) return null;
  const u = r.u;
  return {
    used: u.context_used,
    window: u.context_window,
    fraction: ringFraction(u.context_used, u.context_window),
    tokensIn: u.totals || u.tokens_in ? u.tokens_in : null,
    tokensOut: u.totals || u.tokens_out ? u.tokens_out : null,
    cost: u.cost,
    exact: u.cost_exact,
    model: u.model,
    counting: !u.totals,
    conv: r.conv,
  };
}

// ---------- cost of a Kelpie session across its conversations ----------

/** What each conversation of a session has cost (`/clear` starts a new one; the cap counts them
 *  all). Kept across restarts. */
const COSTS_KEY = "kelpie-session-costs";
const MAX_COST_SESSIONS = 300;
const [costs, setCosts] = createStore<Record<string, Record<string, number>>>(
  (() => {
    try {
      return JSON.parse(localStorage.getItem(COSTS_KEY) ?? "{}");
    } catch {
      return {};
    }
  })(),
);
let costsSaveTimer: ReturnType<typeof setTimeout> | undefined;

function noteCost(id: string, conv: string, cost: number) {
  if (costs[id]?.[conv] === cost) return;
  setCosts(id, (prev) => ({ ...prev, [conv]: cost }));
  clearTimeout(costsSaveTimer);
  costsSaveTimer = setTimeout(() => {
    try {
      const entries = Object.entries(costs).slice(-MAX_COST_SESSIONS);
      localStorage.setItem(COSTS_KEY, JSON.stringify(Object.fromEntries(entries)));
    } catch {
      /* storage unavailable: the sums restart with the app */
    }
  }, 3000);
}

/** Everything the session's conversations have cost, or null when none has a price. */
export function sessionCost(s: SessionInfo): number | null {
  const m = costs[s.id];
  const all = m ? Object.values(m) : [];
  return all.length ? all.reduce((a, b) => a + b, 0) : null;
}

/** Over the cost cap (the setting is off at 0). */
export const isOverCap = (s: SessionInfo) => overCap(sessionCost(s), store.state.settings.costCap);

async function load(id: string, attempt = 0) {
  const s = store.sessions[id];
  if (!s || !readable(s)) return;
  const conv = s.agent_session_id;
  lastRead.set(id, Date.now());
  const mine = (sequence.get(id) ?? 0) + 1;
  sequence.set(id, mine);
  const hook = turnPaths.get(id);
  const r = await sessionUsage({
    kind: s.kind,
    agentSessionId: conv,
    transcriptPath: hook?.conv === conv ? hook.path : undefined,
    cwd: s.cwd,
    home: sessionHome(s),
  }).catch(() => null);
  // The pane moved on to another conversation while the file was read: this belongs to the old one.
  if (store.sessions[id]?.agent_session_id !== conv || sequence.get(id) !== mine) return;
  if (r) setReadings(id, reconcile({ conv, u: r }));
  else setReadings(produce((m) => delete m[id]));
  // Still counting in the background: look again, one timer per session.
  if (r && !r.totals && attempt < CATCH_UP_TRIES && !retries.has(id)) {
    retries.set(
      id,
      setTimeout(() => {
        retries.delete(id);
        void load(id, attempt + 1);
      }, CATCH_UP_MS),
    );
  }
}

/** Reads session `id` soon; calls within the delay share one read. */
export function refreshUsage(id: string, delay = DEBOUNCE_MS) {
  if (timers.has(id)) return;
  timers.set(
    id,
    setTimeout(() => {
      timers.delete(id);
      void load(id);
    }, delay),
  );
}

function onTurn(p: { session: string; event: string; transcript_path?: string | null; agent_session_id?: string | null }) {
  const s = store.sessions[p.session];
  if (!s) return;
  if (p.transcript_path && p.agent_session_id) turnPaths.set(s.id, { conv: p.agent_session_id, path: p.transcript_path });
  if (readable(s) && (TURN_END.has(p.event) || s.status === "working" || store.isSessionVisible(s.id))) refreshUsage(s.id);
}

/** Menus, the 85 % hint and the cost cap's reactions load on first use (not needed at startup). */
export function openUsageMenu(e: MouseEvent, s: SessionInfo) {
  void import("./usage-alerts").then((m) => m.openMenu(e, s));
}

/** The numbers shown for `s` were read from the transcript of the conversation it runs now. */
export const hasExactReading = (s: SessionInfo) => !s.chat && readings[s.id]?.conv === s.agent_session_id;

export function initSessionUsage() {
  window.addEventListener("kelpie:session-turn", (e) => onTurn((e as CustomEvent).detail));
  // A session coming into view gets its first reading (and a fresh one after a while away).
  createEffect(() => {
    for (const s of Object.values(store.sessions)) {
      if (!readable(s) || !store.isSessionVisible(s.id)) continue;
      if (Date.now() - (lastRead.get(s.id) ?? 0) > VISIBLE_STALE_MS) refreshUsage(s.id, 0);
    }
  });
  // An older daemon sends no `session.turn`: poll what is on screen and working.
  setInterval(() => {
    if (store.daemonApi() >= TURN_API || document.hidden) return;
    for (const s of Object.values(store.sessions)) if (readable(s) && s.status === "working" && store.isSessionVisible(s.id)) refreshUsage(s.id, 0);
  }, POLL_MS);
  // What each session's conversations cost, summed per session for the cap.
  createEffect(() => {
    for (const s of Object.values(store.sessions)) {
      const v = usageView(s);
      if (v?.conv && v.cost !== null) untrack(() => noteCost(s.id, v.conv!, v.cost!));
    }
  });
  // The hint at 85 % and the cap's reactions live in a lazy module: it is only asked when one is due.
  createEffect(() => {
    const cap = store.state.settings.costCap;
    let due = false;
    for (const s of Object.values(store.sessions)) {
      const v = usageView(s);
      const full = v?.fraction != null && v.fraction >= DANGER_AT;
      if (full || overCap(sessionCost(s), cap)) {
        due = true;
        // Read here so a change of any of these asks again.
        void [s.status, store.isSessionVisible(s.id), store.state.settings.costCapEsc];
      }
    }
    if (due) void import("./usage-alerts").then((m) => m.run());
  });
}
