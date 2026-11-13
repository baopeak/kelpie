// Agents using browser panes, the window's side. An agent's browser tool call goes from the daemon to the app's Rust, which hands it to
// this window as an `agent-browser-request` event. This window knows the layout and is the authority on which panes a session may use:
// it answers `tabs` itself and, for every other tool, picks the pane, refuses what the person did not allow (a pane not opened for the
// session nor shared in its project, a shared pane now on another site, one a phone controls, one in design mode), creates the pane's
// page hidden when nobody has opened it yet, checks all that again, and passes the call to Rust (`browser_agent_run`), which runs it in
// the page and answers the daemon. It also shows who is using a pane (banner with Stop, a tab badge, a notice for a background tab), and
// keeps the panes the person shares with agents (this window only, until it restarts).
import { createEffect, createRoot, createSignal } from "solid-js";
import { onEvent, policyGet, providerReply } from "./ipc";
import {
  browserAgentRun, browserAgentStop, browserLabel, browserMounted, browserOpen, browserPrivate, browserState, hasWebview, listenBrowserEvents, setBrowserState, type BrowserEvent,
} from "./browser";
import { findPane, hostOf } from "./takeover-panes";
import { findPane as paneIn } from "./layout";
import { anyCovering } from "./overlays";
import { held } from "./takeover";
import { sessionLabel, store } from "./store";
import { t } from "./i18n";
import { BROWSER_AGENT_API } from "./attention";
import {
  callContext, decidePrivate, planCall, sharedOrigins, toBrowserPolicy, usablePanes, type BrowserPolicy, type CallPlan, type CallPolicy, type PaneShare, type UsablePane,
} from "./agent-browser-panes";

/** A browser tool call of an agent session, as Rust passes it on from the daemon. The daemon checked the arguments and its policy. */
interface AgentRequest {
  preq: number;
  /** `restricted`: the session runs restricted (absent from an older daemon: not restricted). */
  session: { id: string; name?: string | null; kind?: string; cwd?: string; parent?: string | null; restricted?: boolean };
  tool: string;
  args: Record<string, unknown> | null;
  policy: CallPolicy | null;
}

/** Rust's notice that a session started or stopped acting in a pane. `reason` "left-site": a shared pane went to another site. */
interface LeaseEvent {
  label: string;
  session: string;
  name: string;
  active: boolean;
  reason?: string;
}

/** The session acting in a pane now, by pane id. */
export const [agentLeases, setAgentLeases] = createRoot(() => createSignal<Readonly<Record<string, { session: string; name: string }>>>({}));

/** Panes the person shares with agents, by pane id: the sites they showed when shared. Kept in this window only, never saved. */
export const [paneShares, setPaneShares] = createRoot(() => createSignal<Readonly<Record<string, PaneShare>>>({}));

/** Panes the person stopped agents in: not usable until shared again. */
const [stoppedPanes, setStoppedPanes] = createRoot(() => createSignal<ReadonlySet<string>>(new Set()));

/** The daemon's browser policy as last read or set (null: not read yet, or a daemon without it). */
export const [browserPolicy, setBrowserPolicy] = createRoot(() => createSignal<BrowserPolicy | null>(null));

/** Whether agents may use browser panes at all now: the share button is offered only then. */
export const browserAgentsOn = () => store.daemonApi() >= BROWSER_AGENT_API && (browserPolicy()?.mode ?? "off") !== "off";

export async function refreshBrowserPolicy() {
  if (store.daemonApi() < BROWSER_AGENT_API) return;
  await policyGet().then((r) => setBrowserPolicy(toBrowserPolicy(r)), () => {});
}

/** Whether a page created now for a pane opened for an agent is made private: only while browser control is on, so that with it off (the
 *  default) preview and dev-server tabs keep the person's sign-ins as before. A pane restored at start may mount before the window knows
 *  the daemon: this waits for it, 10 s at most (`decidePrivate`). */
export function privateForAgents(): Promise<boolean> {
  return decidePrivate(
    {
      connected: store.connected,
      api: store.daemonApi,
      mode: () => browserPolicy()?.mode ?? null,
      read: async () => {
        await refreshBrowserPolicy();
        return browserPolicy()?.mode ?? null;
      },
    },
    BROWSER_AGENT_API,
  );
}

const paneIdOf = (label: string) => (label.startsWith("browser-") ? label.slice("browser-".length) : null);

const without = <T>(all: Readonly<Record<string, T>>, key: string): Readonly<Record<string, T>> => {
  if (!(key in all)) return all;
  const next = { ...all };
  delete next[key];
  return next;
};

/** Where a pane is and the pane itself, if it is still in a layout. */
function locate(paneId: string) {
  const at = findPane(browserLabel(paneId), store.state.projects, store.workspace);
  if (!at) return null;
  const tab = store.workspace(at.projectId).tabs.find((x) => x.id === at.tabId);
  const pane = tab && paneIn(tab.root, paneId);
  return pane ? { ...at, pane } : null;
}

/** Whether a pane is on screen: in the project and tab shown, not behind another pane's maximize, and not hidden for a menu or dialog. */
function shownNow(projectId: string, tabId: string, paneId: string): boolean {
  if (store.state.activeProject !== projectId || store.activeTab(projectId)?.id !== tabId) return false;
  if (store.overlay() !== null || anyCovering()) return false;
  const zoomed = store.zoomed()[tabId];
  return !zoomed || zoomed === paneId;
}

/** The panes `sessionId` may use now. `parent`: the daemon's word on the session's parent, which counts for the first step (the window
 *  may not have the session's row yet). A restricted session gets only the panes opened for it or a session above it. */
function usableFor(sessionId: string, parent?: string | null, restricted = false): UsablePane[] {
  const sessions = (s: string) => {
    const row = store.sessions[s];
    if (s !== sessionId || parent === undefined) return row;
    return { project_id: row?.project_id ?? null, parent: parent ?? row?.parent };
  };
  return usablePanes(sessionId, store.state.projects, store.workspace, sessions, browserState, paneShares(), stoppedPanes(), { shown: shownNow, held: held(), restricted });
}

/** Where a page created for an agent goes until its pane is shown: off screen, at a common laptop size. */
const OFFSCREEN = { x: -20000, y: -20000, width: 1280, height: 800 };

/** Sessions already told about, per pane: the notice that a session uses a background tab comes once. */
const announced = new Set<string>();

/** The person sees which tab a session uses when it is not the one on screen, once per session and pane, with a way to look at it. */
function announce(sessionId: string, name: string, pane: UsablePane) {
  if (pane.shown) return;
  const key = `${sessionId}\n${pane.id}`;
  if (announced.has(key)) return;
  announced.add(key);
  store.notify(
    t("{session} is using the browser tab {title}", { session: name, title: pane.title || hostOf(pane.url) }),
    { label: t("View"), run: () => reveal(pane.id) },
    8000,
  );
}

function reveal(paneId: string) {
  const at = locate(paneId);
  if (!at) return;
  store.switchProject(at.projectId);
  store.focusPane(at.projectId, at.tabId, at.paneId);
}

/** Tauri's own error for a command this app does not have: an app older than the daemon's browser tools. */
const MISSING_COMMAND = /command browser_agent_run not found/i;

/** One call, start to end. Every path answers the daemon exactly once: here with a result or an error, or by Rust once
 *  `browser_agent_run` took the call, after which this window never answers it. */
async function onRequest(r: AgentRequest) {
  let settled = false;
  const reply = (result: unknown, error: string | null) => {
    if (settled) return;
    settled = true;
    void providerReply(r.preq, result, error).catch(() => {});
  };
  try {
    const raw = r.session as AgentRequest["session"] | string | null;
    const session = typeof raw === "string" ? { id: raw } : raw;
    if (!session?.id) return reply(null, "The request names no session.");
    const args = r.args ?? {};
    const usable = () => usableFor(session.id, session.parent, session.restricted === true);
    // Which pages are private is Rust's to say: it made them. Unanswered, none is said to be.
    let made: Record<string, boolean> | undefined;
    if (r.tool === "tabs") {
      const labels = usable().map((p) => p.label);
      made = await browserPrivate(labels).catch(() => Object.fromEntries(labels.map((l) => [l, false])));
    }
    const plan = (): CallPlan => planCall(r.tool, args, usable(), { exists: (id) => !!locate(id), design: (id) => !!browserState[id]?.design, made });
    const first = plan();
    if ("answer" in first) return reply(first.answer, null);
    if ("error" in first) return reply(null, first.error);

    const label = first.pane.label;
    // A pane in a tab nobody has opened since the app started has no page yet: it is created hidden, and showing its tab later shows it.
    // One opened for an agent (whichever session calls) gets a private page, as mounting it would: the daemon sent this call, so browser
    // control is on.
    if (!hasWebview(label)) {
      try {
        await browserOpen(label, first.pane.url, OFFSCREEN, { hidden: true, agent: !!locate(first.pane.id)?.pane.openedBy });
      } catch (e) {
        return reply(null, `The page of pane ${first.pane.id} could not be opened: ${String(e).replace(/^Error: /, "")}`);
      }
    }
    // The person may have stopped agents, stopped sharing, turned on design mode, closed the pane or handed it to a phone meanwhile.
    const again = plan();
    if ("error" in again) return reply(null, again.error);
    if (!("pane" in again) || again.pane.id !== first.pane.id) return reply(null, `Pane ${first.pane.id} changed while it opened; try again.`);
    const pane = again.pane;

    const known = store.sessions[session.id];
    const name = known ? sessionLabel(known) : session.name || t("An agent");
    try {
      await browserAgentRun(r.preq, label, r.tool, args, callContext(pane, r.policy, { id: session.id, name }));
    } catch (e) {
      // Rust did not take the call: the daemon must not wait for an answer.
      const text = String(e).replace(/^Error: /, "");
      return reply(null, MISSING_COMMAND.test(text) ? "This Kelpie app cannot use browser panes for agents yet: update Kelpie." : text);
    }
    settled = true;
    announce(session.id, name, pane);
  } catch (e) {
    reply(null, `The Kelpie window could not handle this call: ${String(e).replace(/^Error: /, "")}`.slice(0, 280));
  }
}

function dropLease(paneId: string) {
  setAgentLeases((all) => without(all, paneId));
}

function onLease(e: LeaseEvent) {
  const paneId = paneIdOf(e.label);
  if (!paneId) return;
  if (!e.active) {
    dropLease(paneId);
    // A shared pane the agent's action took to another site: it is no longer shared, and the person decides whether to share it again.
    if (e.reason === "left-site" && paneShares()[paneId]) {
      setPaneShares((all) => without(all, paneId));
      store.notify(t("A page you shared with agents went to another site, so agents can no longer use it. Share it again to let them."), { label: t("View"), run: () => reveal(paneId) }, 10000);
    }
    return;
  }
  // A call that started before the person stopped agents or stopped sharing may still report its start: the pane is not theirs any more.
  const at = locate(paneId);
  if (!at || stoppedPanes().has(paneId) || (!at.pane.openedBy && !paneShares()[paneId])) {
    void browserAgentStop(e.label).catch(() => {});
    return;
  }
  const known = store.sessions[e.session];
  setAgentLeases((all) => ({ ...all, [paneId]: { session: e.session, name: known ? sessionLabel(known) : e.name || t("An agent") } }));
}

/** Lets agents of the pane's project use it, on the site it shows now. A pane the person stopped agents in becomes usable again. */
export function sharePane(paneId: string) {
  const at = locate(paneId);
  const origins = sharedOrigins(browserState[paneId]?.url || at?.pane.url);
  if (!origins.length) return;
  setPaneShares((all) => ({ ...all, [paneId]: { origins } }));
  setStoppedPanes((s) => {
    if (!s.has(paneId)) return s;
    const next = new Set(s);
    next.delete(paneId);
    return next;
  });
}

/** Stops sharing a pane. The session acting in it is stopped too, unless the pane was opened for it (or a session above it). */
export function unsharePane(paneId: string) {
  setPaneShares((all) => without(all, paneId));
  const lease = agentLeases()[paneId];
  if (!lease || usableFor(lease.session).some((p) => p.id === paneId)) return;
  void browserAgentStop(browserLabel(paneId)).catch(() => {});
  dropLease(paneId);
}

/** The banner's Stop: the agent's use of the pane ends now, and the pane is no longer one agents may use (not opened for anyone, not
 *  shared) until the person shares it. The window decides which panes agents may use, so this holds whatever Rust answers. */
export function stopAgentsInPane(paneId: string) {
  setStoppedPanes((s) => new Set(s).add(paneId));
  void browserAgentStop(browserLabel(paneId)).catch(() => {});
  dropLease(paneId);
  const at = locate(paneId);
  if (at?.pane.openedBy) store.updatePane(at.projectId, at.tabId, paneId, { openedBy: undefined });
  setPaneShares((all) => without(all, paneId));
}

/** A pane that is not mounted (its tab is not shown) still loads pages and changes its title when an agent uses it: what `tabs` lists,
 *  the tab shows and the pane opens at after a restart follow it. A mounted pane handles its own events. */
function followUnmounted(e: BrowserEvent) {
  if (browserMounted(e.label) || (e.kind !== "load" && e.kind !== "title")) return;
  const paneId = paneIdOf(e.label);
  const at = paneId ? locate(paneId) : null;
  if (!paneId || !at) return;
  if (!browserState[paneId]) setBrowserState(paneId, { title: "", url: "", design: false, notes: [] });
  if (e.kind === "title") setBrowserState(paneId, "title", e.title ?? "");
  if (e.kind === "load" && e.url) {
    setBrowserState(paneId, "url", e.url);
    if (at.pane.url !== e.url) store.updatePane(at.projectId, at.tabId, paneId, { url: e.url });
  }
}

let started = false;
/** Listens for agents' browser calls and Rust's lease notices (once), and reads the policy whenever the daemon connects. */
export function initAgentBrowser() {
  if (started) return;
  started = true;
  listenBrowserEvents();
  window.addEventListener("kelpie:browser-event", (e) => followUnmounted((e as CustomEvent<BrowserEvent>).detail));
  void onEvent<AgentRequest>("agent-browser-request", (r) => void onRequest(r));
  void onEvent<LeaseEvent>("agent-browser-lease", onLease);
  createRoot(() =>
    createEffect(() => {
      if (store.connected() && store.daemonApi() >= BROWSER_AGENT_API) void refreshBrowserPolicy();
    }),
  );
}
