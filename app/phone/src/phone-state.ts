// What the phone app knows and does: the connection and what it reports, the session list, the open
// session (a terminal's screen or a chat), and the few actions the views call. Views read signals from
// here and never touch the link directly.
import { batch, createSignal } from "solid-js";
import { ChatController } from "../../src/chat/chat-model";
import type { AnswerOutcome, ChatTransport } from "../../src/chat/transport";
import { keyAnswer } from "../../src/lib/attention";
import { adapterFor } from "./chat-adapters";
import { ChatFeed } from "./chat-feed";
import { NOTICE_SHOWN_MS, NoticeBoard, type Notice } from "./notice";
import { clearCache, loadList, loadScreen, saveList, saveScreen } from "./cache";
import { canAct, dataAge, deriveConn, type ConnState } from "./conn-state";
import { browserEnv, Link, type ChatMsg, type ChatResolved, type Hello, type Outcome, type SessionRow, type SessionsMsg } from "./link";
import { emptyView, ScreenSync, type Prompt, type ScreenFrame, type ScreenView } from "./screen-sync";
import { forgetHistory } from "./history";
import { loadToken, store, TOKEN_KEY, wipeTypedText } from "./storage";
import { t } from "./i18n";

// ---------- signals the views read ----------

const [paired, setPaired] = createSignal(!!loadToken());
export { paired };
export const [pairing, setPairing] = createSignal<string | null>(null);
export const [notPairedMessage, setNotPairedMessage] = createSignal<string | null>(null);
export const [sessions, setSessions] = createSignal<SessionRow[]>([]);
export const [projects, setProjects] = createSignal<Record<string, string>>({});
export const [device, setDevice] = createSignal<{ name: string; can_write: boolean } | null>(null);
export const [openId, setOpenId] = createSignal<string | null>(null);
export const [screen, setScreen] = createSignal<ScreenView>(emptyView());
export const [chat, setChat] = createSignal<ChatController | null>(null);
/** When the list on screen was saved by an earlier visit: set until the connection says anything. */
export const [paintedFrom, setPaintedFrom] = createSignal<number | null>(null);
export const [conn, setConn] = createSignal<ConnState>({ kind: "connecting" });
export const [now, setNow] = createSignal(Date.now());
export const [dataState, setDataState] = createSignal<"fresh" | "slow" | "stuck">("fresh");
export const [lastLiveAt, setLastLiveAt] = createSignal(0);
/** The one message about the open session that outlives what it is about (see notice.ts). */
export const [notice, setNotice] = createSignal<Notice | null>(null);
const board = new NoticeBoard();
let noticeTimer: ReturnType<typeof setTimeout> | undefined;

export function showNotice(session: string, text: string) {
  clearTimeout(noticeTimer);
  setNotice(board.show(session, text, Date.now()));
  noticeTimer = setTimeout(() => setNotice(null), NOTICE_SHOWN_MS);
}

export function dismissNotice() {
  clearTimeout(noticeTimer);
  board.dismiss();
  setNotice(null);
}

export const canType = () => !!device()?.can_write;
export const connected = () => canAct(conn());

// ---------- the link ----------

const syncs = new Map<string, ScreenSync>();
const chats = new ChatFeed();
let wantSession: string | null = null;
let listSaved = 0;

const link = new Link(browserEnv(), loadToken, {
  changed: () => recompute(),
  hello(h: Hello, epochChanged: boolean) {
    batch(() => {
      setDevice(h.device);
      setSessions(h.sessions);
      setProjects(h.projects);
      setPaintedFrom(null);
      if (epochChanged) {
        // A computer that restarted has other positions and numbers: start from nothing.
        for (const s of syncs.values()) s.reset();
        chats.forgetPositions();
      } else {
        for (const s of syncs.values()) s.resume();
      }
      const open = openId();
      if (open) setScreen(syncs.get(open)?.view ?? emptyView());
    });
    persistList();
    const wanted = wantSession && h.sessions.find((s) => s.id === wantSession);
    wantSession = null;
    if (wanted) openSession(wanted.id);
    else if (openId() && !h.sessions.some((s) => s.id === openId())) closeSession();
  },
  sessions(m: SessionsMsg) {
    const before = new Map(sessions().map((s) => [s.id, s.status]));
    batch(() => {
      setSessions(m.sessions);
      setProjects(m.projects ?? projects());
    });
    // A buzz when something starts needing you.
    if (m.sessions.some((s) => s.alive && s.status === "waiting" && before.get(s.id) !== "waiting")) navigator.vibrate?.(200);
    persistList();
    if (openId() && !m.sessions.some((s) => s.id === openId())) closeSession();
  },
  frame(m: ScreenFrame) {
    const sync = syncs.get(m.id);
    if (sync?.apply(m) && m.id === openId()) scheduleView();
  },
  chat(m: ChatMsg) {
    chats.apply(m);
  },
  chatResolved(m: ChatResolved) {
    chats.controller(m.id)?.resolvedElsewhere(m.request_id, !!m.dropped, { answeredBy: m.answered_by ?? (m.by === "desktop" ? "desktop" : undefined), at: m.at });
  },
  chatResync() {},
  unpaired() {
    store.del(TOKEN_KEY);
    void clearCache();
    wipeTypedText();
    forgetHistory();
    batch(() => {
      setPaired(false);
      setNotPairedMessage(t("This device is no longer paired. Pair it again from Kelpie → Settings → Mobile."));
      closeSession();
      setSessions([]);
    });
  },
  haveRev: (id) => syncs.get(id)?.haveRev() ?? null,
  chatSince: (id) => chats.since(id),
});

export { link };

function recompute() {
  const at = Date.now();
  const next = deriveConn(link.facts, at);
  const silent = link.facts.probeSince !== null ? at - link.facts.probeSince : 0;
  batch(() => {
    setConn((cur) => (JSON.stringify(cur) === JSON.stringify(next) ? cur : next));
    setDataState(dataAge(link.facts.ready ? link.facts.lagMs : 0, silent));
    setLastLiveAt(link.facts.lastLiveAt);
    setNow(at);
  });
}

// ---------- rendering, batched ----------

let viewQueued = false;

/** Applies what the frames built to the screen once per animation frame; at most 10 times a second
 *  when the page is hidden or the phone is saving data. */
function scheduleView() {
  if (viewQueued) return;
  viewQueued = true;
  const run = () => {
    viewQueued = false;
    const id = openId();
    const sync = id ? syncs.get(id) : undefined;
    if (!sync) return;
    setScreen(sync.view);
    saveScreen(sync.id, { at: Date.now(), rows: sync.view.rows, title: sync.view.title });
  };
  const saveData = (navigator as Navigator & { connection?: { saveData?: boolean } }).connection?.saveData;
  if (document.hidden || saveData) setTimeout(run, 100);
  else requestAnimationFrame(run);
}

function persistList() {
  const at = Date.now();
  if (at - listSaved < 1000) return;
  listSaved = at;
  saveList({ at, sessions: sessions(), projects: projects(), device: device() });
}

// ---------- sessions ----------

function syncFor(id: string): ScreenSync {
  let s = syncs.get(id);
  if (!s) {
    s = new ScreenSync(id, {
      ack: (i, rev) => link.ack(i, rev),
      refollow: (i) => link.refollow(i),
      setTimeout: (fn, ms) => setTimeout(fn, ms),
      clearTimeout: (h) => clearTimeout(h as number),
    });
    syncs.set(id, s);
  }
  return s;
}

export function openSession(id: string, push = true) {
  if (openId() === id) return;
  if (openId()) leave();
  const row = sessions().find((s) => s.id === id);
  setOpenId(id);
  if (push) history.pushState({ s: id }, "");
  if (row?.chat) return void openChat(row);
  const sync = syncFor(id);
  setScreen(sync.view);
  // Until the first frame, the last screen this browser saw.
  if (!sync.view.rows.length) {
    void loadScreen(id).then((c) => {
      if (c && openId() === id && !syncs.get(id)?.view.rows.length) setScreen({ ...emptyView(), rows: c.rows, title: c.title });
    });
  }
  link.follow(id, "term");
}

async function openChat(row: SessionRow) {
  const id = row.id;
  let ctrl = chats.controller(id);
  if (!ctrl) {
    const make = await adapterFor(row.kind);
    if (openId() !== id) return;
    ctrl = new ChatController(id, row.cwd, null, make, { transport: chatTransport(id), observer: true });
    chats.open(id, ctrl);
  }
  setChat(ctrl);
  link.follow(id, "chat");
}

/** The phone's way to a chat: it can only pass on the person's answer to a question the agent asked. */
function chatTransport(id: string): ChatTransport {
  return {
    async answer(requestId, line): Promise<AnswerOutcome> {
      return chatOutcome(await link.chatAnswer(id, requestId, line));
    },
    deviceName: () => device()?.name,
  };
}

export function chatOutcome(o: Outcome): AnswerOutcome {
  switch (o.t) {
    case "ack":
      return { result: "ok" };
    case "error":
      if (o.result === "dup") return { result: "dup" };
      if (o.result === "stale") return { result: "stale", answeredBy: o.answered_by, at: o.at };
      return { result: "refused", message: o.message };
    case "offline":
      return { result: "refused", message: t("You are offline. Answer when the connection is back.") };
    default:
      return { result: "unknown" };
  }
}

function leave() {
  const id = openId();
  if (!id) return;
  link.unfollow(id);
  chats.close(id);
}

export function closeSession() {
  leave();
  batch(() => {
    setOpenId(null);
    setChat(null);
    setScreen(emptyView());
  });
}

// ---------- actions ----------

export type SendResult = { ok: true; entered: boolean } | { ok: false; reason: "offline" | "unknown" | "prompt" | "error"; message?: string };

/** Types `text` and Enter into a terminal session. Nothing is sent, and nothing kept, unless connected. */
export async function sendText(id: string, text: string, force = false): Promise<SendResult> {
  if (!connected()) return { ok: false, reason: "offline" };
  const o = await link.input(id, text, force);
  if (o.t === "ack") return { ok: true, entered: o.entered !== false };
  if (o.t === "error") return { ok: false, reason: o.code === "prompt" ? "prompt" : "error", message: o.message };
  return { ok: false, reason: o.t };
}

export async function sendKey(id: string, key: string): Promise<SendResult> {
  if (!connected()) return { ok: false, reason: "offline" };
  const o = await link.key(id, key);
  if (o.t === "ack") return { ok: true, entered: true };
  if (o.t === "error") return { ok: false, reason: "error", message: o.message };
  return { ok: false, reason: o.t };
}

/** Answers the prompt a terminal session waits on with one of its keys. */
export async function answerPrompt(id: string, waitingSince: number | null, prompt: Prompt, key: string): Promise<Outcome> {
  if (!connected()) return { t: "offline" };
  const a = keyAnswer(prompt, key);
  // The daemon types text; only these two are keys.
  const body = a.data === "esc" || a.data === "enter" ? { key: a.data } : { text: a.data };
  return link.answer(id, waitingSince, body, a.enter);
}

// ---------- pairing and start ----------

export async function pairDevice(code: string, name: string): Promise<string | null> {
  try {
    const r = await fetch("/pair", { method: "POST", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ code, name: name.trim() || t("Phone") }) });
    const body = (await r.json().catch(() => ({}))) as { token?: string; error?: string };
    if (!r.ok || !body.token) return body.error || `HTTP ${r.status}`;
    store.set(TOKEN_KEY, body.token);
    setNotPairedMessage(null);
    setPairing(null);
    setPaired(true);
    link.start();
    return null;
  } catch (e) {
    return String((e as Error)?.message ?? e);
  }
}

/** Forgets this device's pairing (the computer still lists it until it is removed there). */
export function forgetDevice() {
  link.stop();
  store.del(TOKEN_KEY);
  void clearCache();
  wipeTypedText();
  forgetHistory();
  closeSession();
  setSessions([]);
  setPaired(false);
}

function readHash() {
  const params = new URLSearchParams(location.hash.slice(1));
  const code = params.get("p");
  const want = params.get("s");
  // Neither stays in the address bar: a code is single use, and a session link carries no token.
  if (code || want) history.replaceState(null, "", location.pathname);
  return { code, want };
}

export function startApp() {
  store.del("kelpie-history"); // an earlier build kept sent text here
  const { code, want } = readHash();
  wantSession = want;
  if (code) {
    link.stop();
    setPairing(code);
    setPaired(false);
  }
  const tick = () => !document.hidden && recompute();
  setInterval(tick, 1000);
  window.addEventListener("online", () => link.setOnline(true));
  window.addEventListener("offline", () => link.setOnline(false));
  window.addEventListener("pageshow", (e) => e.persisted && link.kick());
  document.addEventListener("visibilitychange", () => {
    link.setHidden(document.hidden);
    // Nothing goes out from a hidden page, not even a late acknowledgement.
    if (document.hidden) for (const s of syncs.values()) s.dispose();
    if (!document.hidden) recompute();
  });
  window.addEventListener("hashchange", () => {
    const next = readHash();
    if (next.want) {
      const s = sessions().find((x) => x.id === next.want);
      if (s) openSession(s.id);
    } else if (next.code) {
      link.stop();
      closeSession();
      setPairing(next.code);
      setPaired(false);
    }
  });
  window.addEventListener("popstate", (e) => {
    const id = (e.state as { s?: string } | null)?.s;
    if (id && sessions().some((s) => s.id === id)) openSession(id, false);
    else closeSession();
  });
  // The list as this browser last saw it, at once, until the connection says anything. (A page opened from a
  // pairing link connects only once that pairing is approved.)
  if (loadToken() && !code) {
    void loadList().then((c) => {
      if (c && !link.facts.ready && !sessions().length) {
        batch(() => {
          setSessions(c.sessions);
          setProjects(c.projects);
          setDevice(c.device);
          setPaintedFrom(c.at);
        });
      }
    });
    link.start();
  }
  recompute();
}
