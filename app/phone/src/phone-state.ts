// What the phone app knows and does: the connection and what it reports, the session list, the open
// session (a terminal's screen or a chat), and the few actions the views call. Views read signals from
// here and never touch the link directly.
import { batch, createSignal } from "solid-js";
import { ChatController } from "../../src/chat/chat-model";
import { fromPhoneMessage } from "../../src/chat/chat-queue";
import type { AnswerOutcome, ChatTransport, SendOutcome } from "../../src/chat/transport";
import { keyAnswer } from "../../src/lib/attention";
import { adapterFor } from "./chat-adapters";
import { ChatFeed } from "./chat-feed";
import { ChatSender, type PendingSend, type SendResult as ChatSendResult } from "./chat-send";
import { NOTICE_SHOWN_MS, NoticeBoard, refusalText, type Notice } from "./notice";
import { clearCache, loadList, loadScreen, saveList, saveScreen } from "./cache";
import { canAct, dataAge, deriveConn, type ConnState } from "./conn-state";
import { browserEnv, Link, linkCodec, type ChatMsg, type ChatQueueMsg, type ChatRequestInfo, type ChatResolved, type DeviceInfo, type Hello, type Outcome, type SessionRow, type SessionsMsg } from "./link";
import { browserCredentials, LockModel, type LockInfo, type Proof } from "./lock";
import { pair, type PairSocket } from "./pair";
import { currentPairing, forgetPairing, loadPairing, savePairing, seemsPaired } from "./pairing-store";
import { emptyView, ScreenSync, type Prompt, type ScreenFrame, type ScreenView } from "./screen-sync";
import { forgetHistory } from "./history";
import { discardKept, forgetGoneChats, goneChats, loadKept, saveKept, type GoneChat } from "./kept-messages";
import { labelOf } from "./inbox-order";
import { hadOldToken, store, wipeTypedText } from "./storage";
import { screenNow } from "./install-guide";
import { afterFailedPairing, movedTo, platformOf } from "./install-state";
import { Alerts, browserAlerts, readAlertLink, type AlertsState } from "./push";
import { locale, t } from "./i18n";

// ---------- signals the views read ----------

const [paired, setPaired] = createSignal(seemsPaired());
export { paired };
export const [pairing, setPairing] = createSignal<string | null>(null);
export const [notPairedMessage, setNotPairedMessage] = createSignal<string | null>(null);
export const [sessions, setSessions] = createSignal<SessionRow[]>([]);
export const [projects, setProjects] = createSignal<Record<string, string>>({});
export const [device, setDevice] = createSignal<DeviceInfo | null>(null);
/** While pairing: the six digits to type on the computer, and this phone's key fingerprint to check there. */
export const [pairShown, setPairShown] = createSignal<{ digits: string; key: string } | null>(null);
/** Another device was paired, changed or removed on the computer: said for a few seconds. */
export const [deviceNews, setDeviceNews] = createSignal<string | null>(null);
export const [openId, setOpenId] = createSignal<string | null>(null);
export const [screen, setScreen] = createSignal<ScreenView>(emptyView());
export const [chat, setChat] = createSignal<ChatController | null>(null);
/** The messages of the open chat that are on their way (see chat-send.ts). */
export const [chatPending, setChatPending] = createSignal<readonly PendingSend[]>([]);
/** The person pressed Stop and the agent has not stopped yet. */
export const [stopping, setStopping] = createSignal(false);
/** When the list on screen was saved by an earlier visit: set until the connection says anything. */
export const [paintedFrom, setPaintedFrom] = createSignal<number | null>(null);
export const [conn, setConn] = createSignal<ConnState>({ kind: "connecting" });
export const [now, setNow] = createSignal(Date.now());
export const [dataState, setDataState] = createSignal<"fresh" | "slow" | "stuck">("fresh");
export const [lastLiveAt, setLastLiveAt] = createSignal(0);
/** The computer's Tailscale origin, when this page is somewhere else. */
export const [movedHost, setMovedHost] = createSignal<string | null>(null);
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
/** The sender of each chat the phone has sent to: its messages stay with it while the person looks at something else. */
const senders = new Map<string, ChatSender>();
let wantSession: string | null = null;
/** The session asked for came from an alert about someone waiting: what it finds when it opens is worth saying. */
let wantFromAlert = false;
let listSaved = 0;

/** The page's origin is plain HTTP and not Tailscale: anyone on the network could have swapped this page, so it may only read. */
export function plainHttpOrigin(): boolean {
  if (location.protocol !== "http:") return false;
  const host = location.hostname;
  const v4 = host.match(/^(\d+)\.(\d+)\.\d+\.\d+$/);
  const tailscale = (v4 && +v4[1] === 100 && +v4[2] >= 64 && +v4[2] < 128) || host.endsWith(".ts.net") || /^\[?fd7a:115c:a1e0:/i.test(host);
  const local = host === "localhost" || host === "127.0.0.1" || host === "[::1]";
  return !tailscale && !local;
}

/** How the app lock stands, for the views: whether what is behind it may be shown, and the state of its screen. */
export const [lockShown, setLockShown] = createSignal<{ clear: boolean; face: string; asking: boolean; error: string | null; covered: boolean }>({ clear: false, face: "unknown", asking: false, error: null, covered: false });

const lock = new LockModel({
  credentials: browserCredentials(),
  ask: (m) => link.lockAsk(m),
  lockNow: () => link.lockNow(),
  touch: () => link.lockTouch(),
  connected: () => link.ready,
  now: () => Date.now(),
  store,
}, () => setLockShown({ clear: lock.clear(), face: lock.face, asking: lock.asking, error: lock.error, covered: lock.covered }));
export const lockGo = () => lock.go();
setLockShown({ clear: lock.clear(), face: lock.face, asking: false, error: null, covered: false });

/** What the daemon said of each open request of the chats followed (by chat and request id). */
export const [requestInfo, setRequestInfo] = createSignal<Record<string, ChatRequestInfo>>({});
const infoKey = (chat: string, request: string | number) => `${chat}|${request}`;

/** What the daemon said of a card's request. An adapter names a card after the request in its own way: `approval-7`, `perm-7`, or the id as it is. */
export function requestInfoFor(chatId: string, cardId: string): ChatRequestInfo | undefined {
  return requestInfo()[infoKey(chatId, cardId)] ?? requestInfo()[infoKey(chatId, cardId.replace(/^(approval|perm)-/, ""))];
}

/** A dangerous approval the computer wants the passkey for: shown as a sheet, whose button is the gesture a ceremony needs. */
export const [recheckAsked, setRecheckAsked] = createSignal<{ command: string; go(): void; cancel(): void; error: string | null } | null>(null);

/** Asks the person to confirm with the passkey that `command` is what they approve. Null: they did not. */
function askRecheck(request: string, command: string): Promise<Proof | null> {
  return new Promise((resolve) => {
    const done = (p: Proof | null) => {
      setRecheckAsked(null);
      resolve(p);
    };
    const go = () => {
      void lock.recheck(request, command).then((p) => {
        if (p) done(p);
        else setRecheckAsked((cur) => cur && { ...cur, error: lock.error });
      });
    };
    setRecheckAsked({ command, go, cancel: () => done(null), error: null });
  });
}

const link = new Link(browserEnv(), () => {
  const p = currentPairing();
  return p ? linkCodec(p) : null;
}, {
  changed: () => recompute(),
  locked(info: LockInfo) {
    lock.told(info);
  },
  chatRequestInfo(m: ChatRequestInfo) {
    setRequestInfo((cur) => ({ ...cur, [infoKey(m.id, m.request_id)]: m }));
  },
  hello(h: Hello, epochChanged: boolean) {
    lock.sawServerClock(h.server_ts);
    lock.told(h.lock ?? { state: "none" });
    // What the computer made of this page's first message: visible on the page, since its traffic is sealed (scripts/e2e/mobile.py).
    document.documentElement.dataset.servedLegacy = String(h.hb?.legacy ?? false);
    batch(() => {
      setDevice(h.device);
      setSessions(h.sessions);
      setProjects(h.projects);
      setPaintedFrom(null);
      if (epochChanged) {
        // A computer that restarted has other positions and numbers: start from nothing.
        for (const s of syncs.values()) s.reset();
        chats.forgetPositions();
        // The queue and every id in it died with the old run: what waited there is shown as lost, not sent again.
        for (const s of senders.values()) s.epochChanged();
      } else {
        for (const s of syncs.values()) s.resume();
      }
      const open = openId();
      if (open) setScreen(syncs.get(open)?.view ?? emptyView());
    });
    forgetGoneChats(h.sessions.map((s) => s.id));
    refreshNotSent();
    persistList();
    void alerts.recheck(h.push);
    const asked = wantSession;
    wantSession = null;
    const wanted = asked && h.sessions.find((s) => s.id === asked);
    if (asked && wantFromAlert) tellHandled(asked);
    wantFromAlert = false;
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
    forgetGoneChats(m.sessions.map((s) => s.id));
    refreshNotSent();
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
    setRequestInfo((cur) => {
      const { [infoKey(m.id, m.request_id)]: _gone, ...rest } = cur;
      return rest;
    });
  },
  chatQueue(m: ChatQueueMsg) {
    const view = fromPhoneMessage(m);
    chats.controller(m.id)?.applyQueue(view);
    senders.get(m.id)?.snapshot(view);
  },
  chatResync() {},
  addrs(m) {
    // Kept for a client that can dial another address (a page's origin cannot change; the native app's can). What this page can do with
    // them is say so when the computer's Tailscale origin is not where it is.
    const known = m.endpoints ?? m.list.map((a) => ({ t: "lan", a }));
    setMovedHost(movedTo(known, location.host));
    const p = currentPairing();
    if (p && m.seq > p.addrsSeq) void savePairing({ ...p, endpoints: [...known, ...p.endpoints.filter((e) => e.t !== "lan" && e.t !== "ts")], addrsSeq: m.seq });
  },
  deviceChanged(m) {
    const name = m.name;
    tellDeviceNews(m.change === "added" ? t("A new device was paired with this computer: {name}.", { name }) : m.change === "removed" || m.change === "replaced" ? t("A device was removed from this computer: {name}.", { name }) : t("What {name} may do was changed on this computer.", { name }));
  },
  unpaired() {
    lock.forget();
    void alerts.forget();
    void forgetPairing();
    void clearCache();
    wipeTypedText();
    forgetHistory();
    forgetSenders();
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

// ---------- alerts ----------

export const [alertsState, setAlertsState] = createSignal<AlertsState>("off");
export const [alertsError, setAlertsError] = createSignal<string | null>(null);
const alerts = new Alerts(
  browserAlerts(
    { lang: () => locale(), ask: (m) => link.request(m, { write: false }) },
    { ios: platformOf(navigator.userAgent, navigator.maxTouchPoints ?? 0) === "ios", installed: () => matchMedia("(display-mode: standalone)").matches || (navigator as Navigator & { standalone?: boolean }).standalone === true, store },
  ),
  () => {
    setAlertsState(alerts.state);
    setAlertsError(alerts.error);
  },
);
export const turnAlertsOn = () => alerts.turnOn();
export const turnAlertsOff = () => alerts.turnOff();
export const testAlert = () => alerts.test();

/** A tap on an alert about someone waiting, against what the computer says now. The session opens either way, if it is still there. */
function tellHandled(id: string) {
  const row = sessions().find((s) => s.id === id);
  if (!row) tellDeviceNews(t("That session is gone."));
  else if (row.status !== "waiting") showNotice(id, t("Already handled: this session no longer waits for you."));
}

/** A running app, told by the service worker what a tap on an alert opens. */
function openFromAlert(navigate: string) {
  const { session, waited } = readAlertLink(new URL(navigate, location.href).hash);
  if (session) openWanted(session, waited);
}

/** A session asked for by a link, on a running app. What came through a system's own tap on an alert arrives the same way. */
function openWanted(session: string, waited: boolean) {
  if (!connected()) {
    wantSession = session;
    wantFromAlert = waited;
    return;
  }
  if (sessions().some((s) => s.id === session)) openSession(session);
  if (waited) tellHandled(session);
}

function tellDeviceNews(text: string) {
  setDeviceNews(text);
  setTimeout(() => setDeviceNews(null), 8000);
}

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
    ctrl = new ChatController(id, row.cwd, null, make, {
      transport: chatTransport(id),
      observer: true,
      onSent: (m) => senders.get(id)?.sentSeen(m.cid),
      // Also from the log replayed after a reconnect: a message cancelled on the desktop while this phone was offline.
      onUnsent: (m) => senders.get(id)?.unsentSeen(m.cid, m.reason, m.cancelledBy),
    });
    chats.open(id, ctrl);
  }
  watchSender(id, row.kind);
  setStopping(false);
  setChat(ctrl);
  link.follow(id, "chat");
}

/** The sender of chat `id`, made when the phone first looks at it; the views read its messages through `chatPending`. */
function watchSender(id: string, kind: string) {
  let sender = senders.get(id);
  if (!sender) {
    sender = new ChatSender(
      {
        send: async (cid, text) => sendOutcome(await link.chatSend(id, cid, text)),
        cancel: async (qid) => {
          const o = await link.chatCancel(id, qid);
          return o.t === "ack" ? "ok" : o.t === "error" && o.code === "too_late" ? "too_late" : "failed";
        },
        connected,
        now: () => Date.now(),
        newCid: () => globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`,
        // What the person still has to decide about outlives a reload (kept-messages.ts); what could not be kept says so.
        save: (kept) => {
          const row = sessions().find((s) => s.id === id);
          return saveKept(id, kept, row ? labelOf(row) : undefined);
        },
      },
      kind,
      () => chats.controller(id)?.state.commands ?? [],
      loadKept(id),
    );
    senders.set(id, sender);
  }
  const shown = sender;
  setChatPending(shown.pending);
  unwatchSender?.();
  unwatchSender = shown.subscribe(() => setChatPending(shown.pending));
}

let unwatchSender: (() => void) | undefined;

/** What the daemon said to a `chat_send`, as the sender reads it. */
export function sendOutcome(o: Outcome): SendOutcome {
  switch (o.t) {
    case "ack":
      if (o.result === "dup") return { result: "dup", qid: String(o.qid), state: (o.state ?? "queued") as "queued" };
      return { result: "ok", qid: String(o.qid), state: o.state === "sent" ? "sent" : "queued", ahead: o.ahead ?? 0 };
    case "error":
      return { result: "refused", code: o.code, message: o.message };
    case "offline":
      return { result: "offline" };
    default:
      return { result: "unknown" };
  }
}

/** The phone's way to a chat: it passes on the person's answers to the agent's questions and their messages (as text: the
 *  daemon builds the turn), takes messages out of the queue and stops the turn. It never writes to the agent itself. */
function chatTransport(id: string): ChatTransport {
  return {
    async answer(requestId, line): Promise<AnswerOutcome> {
      const first = await link.chatAnswer(id, requestId, line);
      if (first.t !== "error" || first.code !== "recheck") return chatOutcome(first);
      // Asked by the computer: what the person confirms is what the card shows, and the signature is over that.
      const proof = await askRecheck(String(requestId), requestInfo()[infoKey(id, requestId)]?.command ?? "");
      if (!proof) return { result: "refused", message: t("Not confirmed, so nothing was sent.") };
      return chatOutcome(await link.chatAnswer(id, requestId, line, proof));
    },
    deviceName: () => device()?.name,
    async queueOp(op, qid) {
      if (op === "cancel" && qid) await link.chatCancel(id, qid);
      else if (op === "resume") await link.chatResume(id);
    },
    async stop() {
      await link.chatStop(id);
    },
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
  if (o.t === "error") return { ok: false, reason: o.code === "prompt" ? "prompt" : "error", message: refusalText(o) };
  return { ok: false, reason: o.t };
}

export async function sendKey(id: string, key: string): Promise<SendResult> {
  if (!connected()) return { ok: false, reason: "offline" };
  const o = await link.key(id, key);
  if (o.t === "ack") return { ok: true, entered: true };
  if (o.t === "error") return { ok: false, reason: "error", message: refusalText(o) };
  return { ok: false, reason: o.t };
}

/** Answers the prompt a terminal session waits on with one of its keys. */
export async function answerPrompt(id: string, waitingSince: number | null, prompt: Prompt, key: string): Promise<Outcome> {
  if (!connected()) return { t: "offline" };
  const a = keyAnswer(prompt, key);
  // The daemon types text; only these two are keys.
  const body = a.data === "esc" || a.data === "enter" ? { key: a.data } : { text: a.data };
  const first = await link.answer(id, waitingSince, body, a.enter);
  if (first.t !== "error" || first.code !== "recheck") return first;
  const proof = await askRecheck(String(waitingSince ?? 0), screen().wait?.command ?? "");
  if (!proof) return { t: "error", code: "not_confirmed", message: t("Not confirmed, so nothing was sent.") };
  return link.answer(id, waitingSince, body, a.enter, proof);
}

/** Sends `text` to the open chat. */
export async function sendChat(text: string): Promise<ChatSendResult> {
  const id = openId();
  const sender = id ? senders.get(id) : undefined;
  if (!sender) return { accepted: false, reason: "offline" };
  return sender.send(text);
}

export const retryChat = (cid: string) => senders.get(openId() ?? "")?.retry(cid);
export const discardChat = (cid: string) => senders.get(openId() ?? "")?.discard(cid);
/** The text of a message of this phone back for the box; it leaves the queue (`null`: it was too late). */
export const takeChat = (cid: string) => senders.get(openId() ?? "")?.take(cid) ?? Promise.resolve(null);

/** Removes a message of anyone's from the open chat's queue. */
export async function cancelQueued(qid: string): Promise<void> {
  const id = openId();
  if (!id) return;
  const o = await link.chatCancel(id, qid);
  // Its own cancel is not news to it: the chip goes, without a "sent" or a "cancelled" in its place.
  if (o.t === "ack") senders.get(id)?.forgetQueued(qid);
}

/** Stops the agent's turn in the open chat: "Stopping…" shows until it is idle. */
export function stopChat() {
  const ctrl = chat();
  if (!ctrl) return;
  setStopping(true);
  ctrl.interrupt();
}

export function resumeChat() {
  chat()?.resumeQueue();
}

// ---------- pairing and start ----------

/** Pairs this phone with the computer the QR code's link came from. Null when paired; otherwise what to tell the person. */
export async function pairDevice(code: string, name: string): Promise<string | null> {
  setPairShown(null);
  const url = `${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/link`;
  const held = await loadPairing();
  const r = await pair(code, name, () => new WebSocket(url) as unknown as PairSocket, (digits, key) => setPairShown({ digits, key }), held);
  setPairShown(null);
  if (!r.ok) {
    if (!held) {
      if (afterFailedPairing(screenNow(false, false)) === "stay") return r.message;
      setPairing(null);
      setNotPairedMessage(r.message);
      return null;
    }
    // Nothing changed for the pairing it holds: back to it, with what went wrong.
    setPairing(null);
    setPaired(true);
    link.start();
    tellDeviceNews(r.message);
    return null;
  }
  const kept = await savePairing(r.pairing);
  setNotPairedMessage(kept ? null : t("This browser would not keep the pairing: it lasts until this page is closed."));
  setPairing(null);
  setPaired(true);
  link.start();
  return null;
}

/** Forgets this device's pairing (the computer still lists it until it is removed there). */
export function forgetDevice() {
  lock.forget();
  void alerts.forget();
  link.stop();
  void forgetPairing();
  void clearCache();
  wipeTypedText();
  forgetHistory();
  forgetSenders();
  closeSession();
  setSessions([]);
  setPaired(false);
}

/** The kept messages of chats the computer no longer lists (closed, or resumed as a new session): shown in the inbox under
 *  "Not sent", with their text, until the person lets them go (or a week after they last changed). */
export const [notSent, setNotSent] = createSignal<GoneChat[]>([]);

function refreshNotSent() {
  setNotSent(goneChats(sessions().map((s) => s.id)));
}

/** Lets a message from the "Not sent" list go, from the sender that still holds it on this page as well. */
export function discardNotSent(chat: string, cid: string) {
  const sender = senders.get(chat);
  if (sender?.pending.some((p) => p.cid === cid)) sender.discard(cid);
  else discardKept(chat, cid);
  refreshNotSent();
}

/** Nothing typed on this phone outlives its pairing: the messages on their way (and their kept copies, wiped with the drafts). */
function forgetSenders() {
  setNotSent([]);
  unwatchSender?.();
  unwatchSender = undefined;
  for (const s of senders.values()) s.dispose();
  senders.clear();
  setChatPending([]);
}

function readHash() {
  const params = new URLSearchParams(location.hash.slice(1));
  const code = params.get("p");
  const want = params.get("s");
  wantFromAlert = readAlertLink(location.hash).waited;
  // Neither stays in the address bar: a code is single use, and a session link carries no token.
  if (code || want) history.replaceState(null, "", location.pathname);
  return { code, want };
}

export function startApp() {
  store.del("kelpie-history"); // an earlier build kept sent text here
  void alerts.recheck(null); // Says at once if this page cannot have them; the look itself waits for the computer's hello.
  const { code, want } = readHash();
  wantSession = want;
  if (code) {
    link.stop();
    setPairing(code);
    setPaired(false);
  }
  const tick = () => {
    if (document.hidden) return;
    recompute();
    lock.tick();
    for (const s of senders.values()) s.prune();
  };
  setInterval(tick, 1000);
  navigator.serviceWorker?.addEventListener("message", (e: MessageEvent) => {
    if (e.data?.type === "kelpie-open" && typeof e.data.navigate === "string") openFromAlert(e.data.navigate);
  });
  window.addEventListener("online", () => link.setOnline(true));
  window.addEventListener("offline", () => link.setOnline(false));
  window.addEventListener("pageshow", (e) => e.persisted && link.kick());
  // What counts as the person being there, for the computer's idle time.
  for (const type of ["pointerdown", "keydown"]) document.addEventListener(type, () => lock.touched(), { passive: true, capture: true });
  document.addEventListener("visibilitychange", () => {
    // First: the system may take its picture of the app for the switcher at any moment from here.
    if (document.hidden) lock.left();
    else lock.back();
    link.setHidden(document.hidden);
    // Nothing goes out from a hidden page, not even a late acknowledgement.
    if (document.hidden) for (const s of syncs.values()) s.dispose();
    if (!document.hidden) recompute();
  });
  window.addEventListener("hashchange", () => {
    const waited = readAlertLink(location.hash).waited;
    const next = readHash();
    if (next.want) {
      openWanted(next.want, waited);
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
  // A phone paired with a token by an earlier version: the computer no longer takes those.
  if (hadOldToken()) setNotPairedMessage(t("This device was paired before end-to-end encryption. Pair it again from Kelpie → Settings → Mobile."));
  // The list as this browser last saw it, at once, until the connection says anything. (A page opened from a
  // pairing link connects only once that pairing is approved.)
  void loadPairing().then((p) => {
    if (code) return;
    if (!p) {
      setPaired(false);
      return;
    }
    setPaired(true);
    setMovedHost(movedTo(p.endpoints, location.host));
    // Nothing kept is painted while the lock may be on: the computer says, and until it has, the lock screen stands.
    void loadList().then((c) => {
      if (c && lock.clear() && !link.facts.ready && !sessions().length) {
        batch(() => {
          setSessions(c.sessions);
          setProjects(c.projects);
          setDevice(c.device);
          setPaintedFrom(c.at);
        });
      }
    });
    link.start();
  });
  recompute();
}
