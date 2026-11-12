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
import { browserEnv, Link, linkCodec, type AlertPrefs, type ChatMsg, type ChatQueueMsg, type ChatRequestInfo, type ChatResolved, type DeviceInfo, type Hello, type Outcome, type SessionRow, type SessionsMsg } from "./link";
import { browserCredentials, LockModel, type LockInfo, type Proof } from "./lock";
import { pair, type HostedPairing, type PairSocket } from "./pair";
import { currentPairing, forgetPairing, loadPairing, relayUrl, savePairing, seemsPaired } from "./pairing-store";
import { HOSTED } from "./hosted";
import type { Endpoint } from "./noise";
import { emptyView, ScreenSync, type Prompt, type ScreenFrame, type ScreenView } from "./screen-sync";
import { forgetHistory } from "./history";
import { discardKept, forgetGoneChats, goneChats, loadKept, saveKept, type GoneChat } from "./kept-messages";
import { browserReloadEnv, ShellReload, SHELL_CHANGED } from "./shell-reload";
import { labelOf } from "./inbox-order";
import { hadOldToken, store, wipeTypedText } from "./storage";
import { lockGrace, vibrate } from "./prefs";
import { computerBlock, isLocalHost, isTailscaleHost, livePath, pageMethod, screenWhy } from "./conn-path";
import { screenNow } from "./install-guide";
import { afterFailedPairing, movedTo, platformOf, relayMovedTo } from "./install-state";
import { Alerts, browserAlerts, readAlertLink, type AlertsState } from "./push";
import { locale, t } from "./i18n";
import { TakeoverSession, type FrameMeta, type TakeoverState } from "./takeover";
import { KIND_VIDEO, ScreenSession, type ScreenState } from "./screen";
import { NEW_SESSION_API, SCREEN_API, TAKEOVER_API } from "../../src/lib/attention";
import { newSessionRequest, optionsOutcome, startOutcome, type OptionsResult, type StartResult } from "./start-session";

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
/** The computer's Tailscale origin, when this page is somewhere else; on the hosted app, its relay's host when that is not this page's. */
export const [movedHost, setMovedHost] = createSignal<string | null>(null);
const movedHere = (endpoints: readonly Endpoint[]) => (HOSTED ? relayMovedTo(endpoints, location.host) : movedTo(endpoints, location.host));
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
/** The computer's feature level, from its last hello. */
export const [daemonApi, setDaemonApi] = createSignal(0);
/** This phone may take over the computer's browser panes here (a device given control, on this computer's HTTPS address, unlocked). */
export const canTakeOver = () => device()?.permission === "control" && daemonApi() >= TAKEOVER_API;
/** "+" in the inbox: this connection may type and the computer starts sessions for phones. */
export const canStartSessions = () => canType() && daemonApi() >= NEW_SESSION_API;
/** A pane taken over, or being picked (null: none). */
export const [takeover, setTakeover] = createSignal<TakeoverState | null>(null);
/** This phone may see the computer's screen (the same rule as a pane's takeover, at a computer that has it). */
export const canSeeScreen = () => device()?.permission === "control" && daemonApi() >= SCREEN_API;
/** The computer is also published over Tailscale, as its last hello said. */
export const [computerOnTailscale, setComputerOnTailscale] = createSignal(false);
/** Null when the computer's screen can be seen from here; otherwise the sentence that says why not (Settings). The computer's own reason comes first, once it
 *  has said anything on this page (a device kept from an earlier visit may say what no longer holds). */
export const screenWhyNow = () =>
  screenWhy({
    computer: daemonApi() > 0 ? computerBlock(device()?.control) : null,
    known: daemonApi() > 0, older: daemonApi() < SCREEN_API, hosted: HOSTED, permission: device()?.permission, route: route(), unlocked: lockShown().face === "open",
  }, t);
/** The computer's screen seen from here, or being picked (null: none). Its media goes to the view directly (`onScreenMedia`), not through a signal. */
export const [remoteScreen, setRemoteScreen] = createSignal<ScreenState | null>(null);
let screenMediaSink: ((bytes: Uint8Array) => void) | null = null;
/** The screen view takes the media messages while it is shown. */
export function onScreenMedia(sink: ((bytes: Uint8Array) => void) | null) {
  screenMediaSink = sink;
}
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
  return !isTailscaleHost(location.hostname) && !isLocalHost(location.hostname);
}

/** How this app was paired, from where its page comes (it never changes while the page lives). */
export const PAGE_METHOD = pageMethod(HOSTED, location);
/** The route the connection takes now, or took last, and its last round trip in ms. */
export const [route, setRoute] = createSignal<"direct" | "relay">("direct");
export const [rtt, setRtt] = createSignal<number | null>(null);
/** The way to the computer now, in the terms a person knows: the relay, Tailscale, this network or this computer. */
export const livePathNow = () => livePath(route(), PAGE_METHOD, location.hostname);

/** How the app lock stands, for the views: whether what is behind it may be shown, and the state of its screen. */
export const [lockShown, setLockShown] = createSignal<{ clear: boolean; face: string; asking: boolean; error: string | null; covered: boolean }>({ clear: false, face: "unknown", asking: false, error: null, covered: false });
/** What the computer last said of the lock (its idle time and the re-check of dangerous approvals), for Settings. */
export const [lockInfo, setLockInfo] = createSignal<LockInfo>({ state: "unknown" });

const lock = new LockModel({
  credentials: browserCredentials(),
  ask: (m) => link.lockAsk(m),
  lockNow: () => {
    // Leaving the app locks it, and that ends a screen at once (the computer does the same on `lock_now`).
    remoteScreens.locked();
    link.lockNow();
  },
  touch: () => link.lockTouch(),
  grace: () => lockGrace.get() * 1000,
  setResume: (secret) => link.setResume(secret),
  connected: () => link.ready,
  now: () => Date.now(),
  store,
}, () => {
  setLockShown({ clear: lock.clear(), face: lock.face, asking: lock.asking, error: lock.error, covered: lock.covered });
  setLockInfo(lock.lastInfo());
});
export const lockGo = () => lock.go();
setLockShown({ clear: lock.clear(), face: lock.face, asking: false, error: null, covered: false });

/** Seeing or controlling the computer asks for the passkey again after any time out of sight, even within the lock's grace (owner rule 2026-11-12):
 *  the lock screen comes back, and what was asked for opens once it is unlocked. */
export const openScreenPicker = () => lock.whenFresh(() => void remoteScreens.openPicker());
export const openTakeoverPicker = (project: string | null) => lock.whenFresh(() => void takeovers.openPicker(project));
/** Locks the app now, from Settings: the computer is told, and the lock screen asks for the passkey. */
export const lockAppNow = () => lock.relock();

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

/** The relay was turned on, off or moved: the kept shell is fetched again, for the CSP that names it (sw.ts). */
function refreshShell() {
  try {
    navigator.serviceWorker?.controller?.postMessage({ type: "kelpie-refresh-shell" });
  } catch {
    /* no worker on this page */
  }
}

/** Loads the page again when its shell changed under it: a kept shell with another CSP, or a new version's worker taking it over (main.tsx). */
export const shellReload = new ShellReload(browserReloadEnv());

/** The relay, for the installed app on its HTTPS origin, once the computer gave this phone a token. */
const relayRoute = () => {
  const p = currentPairing();
  return p ? relayUrl(p, Date.now(), location.protocol === "https:") : null;
};

const link = new Link(browserEnv(), () => {
  const p = currentPairing();
  return p ? linkCodec(p) : null;
}, {
  changed: () => recompute(),
  locked(info: LockInfo) {
    lock.told(info);
    // The computer ended any screen this phone saw when it locked: no grace (owner decision 2026-11-06).
    remoteScreens.locked();
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
      setDaemonApi(h.api);
      setComputerOnTailscale(!!h.ts);
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
    void alerts.heard(h.push);
    const asked = wantSession;
    wantSession = null;
    const wanted = asked && h.sessions.find((s) => s.id === asked);
    if (asked && wantFromAlert) tellHandled(asked);
    wantFromAlert = false;
    if (wanted) openSession(wanted.id);
    else if (openId() && !h.sessions.some((s) => s.id === openId())) closeSession();
    // A pane held when the connection dropped: this connection asks for it again. The screen likewise.
    void takeovers.reconnected();
    void remoteScreens.reconnected();
  },
  takeover(m) {
    takeovers.onMessage({ ...m, meta: m.meta as FrameMeta | undefined });
  },
  screen(m) {
    remoteScreens.onMessage(m as Parameters<ScreenSession["onMessage"]>[0]);
  },
  screenMedia(bytes) {
    // Only video says frames are coming; a cursor or a tile does not.
    remoteScreens.media(bytes[1] === KIND_VIDEO);
    screenMediaSink?.(bytes);
  },
  sessions(m: SessionsMsg) {
    const before = new Map(sessions().map((s) => [s.id, s.status]));
    batch(() => {
      setSessions(m.sessions);
      setProjects(m.projects ?? projects());
    });
    // A buzz when something starts needing you, unless the person turned it off.
    if (vibrate.get() && m.sessions.some((s) => s.alive && s.status === "waiting" && before.get(s.id) !== "waiting")) navigator.vibrate?.(200);
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
    setMovedHost(movedHere(known));
    const p = currentPairing();
    // `endpoints` is the whole list, the relay's included (or left out, when it was turned off); a computer from before sends the direct addresses only.
    if (p && m.seq > p.addrsSeq) {
      const relayBefore = JSON.stringify(p.endpoints.find((e) => e.t === "relay") ?? null);
      const endpoints = m.endpoints ? known : [...known, ...p.endpoints.filter((e) => e.t !== "lan" && e.t !== "ts")];
      void savePairing({ ...p, endpoints, addrsSeq: m.seq });
      if (JSON.stringify(endpoints.find((e) => e.t === "relay") ?? null) !== relayBefore) refreshShell();
    }
  },
  relay(m) {
    const p = currentPairing();
    if (!p) return;
    const was = p.endpoints.find((e) => e.t === "relay");
    void savePairing({ ...p, relayToken: m.token, relayExpires: m.expires, endpoints: [...p.endpoints.filter((e) => e.t !== "relay"), { t: "relay", a: m.url, r: m.ticket }] });
    if (was?.a !== m.url) refreshShell();
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
    takeovers.close();
    remoteScreens.close();
    batch(() => {
      setPaired(false);
      setNotPairedMessage(t("This device is no longer paired. Pair it again from Kelpie → Settings → Mobile."));
      closeSession();
      setSessions([]);
    });
  },
  haveRev: (id) => syncs.get(id)?.haveRev() ?? null,
  chatSince: (id) => chats.since(id),
}, relayRoute);

export { link };

const takeovers = new TakeoverSession({
  panes: () => link.panes(),
  start: (pane) => link.tkStart(pane),
  stop: (tk) => link.tkStop(tk),
  text: (tk, text, st, w) => link.tkText(tk, text, st, w),
  rebind: (tk) => link.tkRebind(tk),
  input: (tk, ops, st, w) => link.tkInput(tk, ops, st, w),
  ack: (tk, seq) => link.tkAck(tk, seq),
  serverNow: () => link.serverNow(),
  now: () => Date.now(),
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (id) => clearTimeout(id as ReturnType<typeof setTimeout>),
}, setTakeover, t);
export { takeovers };

const remoteScreens = new ScreenSession({
  targets: () => link.screens(),
  start: (target) => link.scStart(target),
  stop: (sc) => link.scStop(sc),
  text: (sc, text, st) => link.scText(sc, text, st),
  rebind: (sc) => link.scRebind(sc),
  resume: (sc) => link.scResume(sc),
  input: (sc, ops, st) => link.scInput(sc, ops, st),
  ack: (sc, seq) => link.scAck(sc, seq),
  keyframe: (sc) => link.scKey(sc),
  serverNow: () => link.serverNow(),
  now: () => Date.now(),
  setTimeout: (fn, ms) => setTimeout(fn, ms),
  clearTimeout: (id) => clearTimeout(id as ReturnType<typeof setTimeout>),
}, setRemoteScreen, t);
export { remoteScreens };

// ---------- alerts ----------

export const [alertsState, setAlertsState] = createSignal<AlertsState>("off");
export const [alertsError, setAlertsError] = createSignal<string | null>(null);
/** This phone's alert choices as the computer keeps them, and the kinds it sends at all (null: it did not say). */
export const [alertPrefs, setAlertPrefs] = createSignal<AlertPrefs | null>(null);
export const [alertEvents, setAlertEvents] = createSignal<string[] | null>(null);
const alerts = new Alerts(
  browserAlerts(
    { lang: () => locale(), ask: (m) => link.request(m, { write: false }) },
    { ios: platformOf(navigator.userAgent, navigator.maxTouchPoints ?? 0) === "ios", installed: () => matchMedia("(display-mode: standalone)").matches || (navigator as Navigator & { standalone?: boolean }).standalone === true, store },
  ),
  () => {
    batch(() => {
      setAlertsState(alerts.state);
      setAlertsError(alerts.error);
      setAlertPrefs(alerts.prefs);
      setAlertEvents(alerts.events);
    });
  },
);
export const turnAlertsOn = () => alerts.turnOn();
export const chooseAlerts = (p: AlertPrefs) => alerts.choose(p);
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

/** A session asked for by a link, on a running app. What came through a system's own tap on an alert arrives the same way. A locked hello counts as connected, and says nothing of any
 *  session, so while locked the list here is whatever it was before: the hello that unlocks opens it, and judges it from what is true then. */
function openWanted(session: string, waited: boolean) {
  if (!connected() || !lock.clear()) {
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
    setRoute(link.facts.route);
    setRtt(link.facts.rttMs);
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
        newCid,
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

/** A name for one write the computer recognises when it comes again (a chat message, a start). */
const newCid = () => globalThis.crypto?.randomUUID?.() ?? `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 12)}`;

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

/** What this phone may start now (the "+" sheet). */
export async function loadNewOptions(): Promise<OptionsResult> {
  return optionsOutcome(await link.newOptions());
}

/** Starts `kind` in project `projectId` with the computer's settings for it and an optional first prompt, and opens the new session. Nothing is
 *  kept to send later: offline, it says so. */
export async function startSession(projectId: string, kind: string, prompt: string): Promise<StartResult> {
  const r = startOutcome(await link.newSession(newSessionRequest(projectId, kind, prompt, newCid())));
  if (r.ok) openSession(r.id);
  return r;
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
  // The relay only for the installed app on its HTTPS origin: a plain-HTTP page is not let in there.
  const relaySocket = location.protocol === "https:" ? (relay: string) => new WebSocket(relay) as unknown as PairSocket : undefined;
  // The hosted app pairs through its relay alone, and with one computer: another one's pairing goes only once the person says so.
  const hosted: HostedPairing | undefined = HOSTED
    ? {
        origin: location.origin,
        confirm: (question) => confirm(question),
        leave: async () => {
          lock.forget();
          await alerts.forget();
        },
      }
    : undefined;
  const r = await pair(code, name, () => new WebSocket(url) as unknown as PairSocket, (digits, key) => setPairShown({ digits, key }), held, relaySocket, hosted);
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
  // What this app kept of the computer it left (its list, screens, drafts and messages) is nothing to the new one.
  if (HOSTED && held && !sameKey(held.dpk, r.pairing.dpk)) {
    void clearCache();
    wipeTypedText();
    forgetHistory();
    forgetSenders();
    closeSession();
    setSessions([]);
  }
  const kept = await savePairing(r.pairing);
  setNotPairedMessage(kept ? null : t("This browser would not keep the pairing: it lasts until this page is closed."));
  setPairing(null);
  setPaired(true);
  link.start();
  // What the hosted app trusts, said once the pairing is made (and from the menu at any time).
  if (HOSTED) setTrustOpen(true);
  return null;
}

const sameKey = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((x, i) => x === b[i]);

/** The sheet that says what the hosted app trusts. */
export const [trustOpen, setTrustOpen] = createSignal(false);

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

/** Forgets the list and the screens this phone saved, and with `drafts` what was typed here too; the pairing stays. */
export async function clearKept(drafts: boolean): Promise<void> {
  await clearCache();
  if (drafts) {
    wipeTypedText();
    forgetHistory();
    refreshNotSent();
  }
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
    if (e.data?.type === SHELL_CHANGED) shellReload.shellChanged();
  });
  // A page keeps the CSP it loaded with: one that refuses the relay loads again, from the shell the worker kept for it (shell-reload.ts).
  document.addEventListener("securitypolicyviolation", (e) => shellReload.violation(e.blockedURI, relayRoute()));
  window.addEventListener("online", () => link.setOnline(true));
// Another network under the page: on the relay, the computer's own address may answer again.
(navigator as Navigator & { connection?: EventTarget }).connection?.addEventListener?.("change", () => link.networkChanged());
  window.addEventListener("offline", () => link.setOnline(false));
  window.addEventListener("pageshow", (e) => e.persisted && link.kick());
  // What counts as the person being there, for the computer's idle time.
  for (const type of ["pointerdown", "keydown"]) document.addEventListener(type, () => lock.touched(), { passive: true, capture: true });
  document.addEventListener("visibilitychange", () => {
    // First: the system may take its picture of the app for the switcher at any moment from here.
    if (document.hidden) lock.left();
    else lock.back();
    // Leaving the app ends a screen or a takeover at once, whatever the lock's grace (owner rule 2026-11-06). With no grace the lock
    // does it (the computer stops them on `lock_now`, and the screen says it ended when the phone locked).
    if (document.hidden && lockGrace.get() > 0) {
      remoteScreens.close();
      takeovers.close();
    }
    // A session asked for while out of sight (a tap on an alert) opens on the way back: within the grace no new hello comes to open it.
    if (!document.hidden && wantSession && connected() && lock.clear()) {
      const want = wantSession;
      const waited = wantFromAlert;
      wantSession = null;
      wantFromAlert = false;
      openWanted(want, waited);
    }
    link.setHidden(document.hidden);
    // Nothing goes out from a hidden page, not even a late acknowledgement.
    if (document.hidden) for (const s of syncs.values()) s.dispose();
    if (!document.hidden) recompute();
    if (!document.hidden) shellReload.visible();
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
    setMovedHost(movedHere(p.endpoints));
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
