// A web page an agent starts (a dev server, a studio, a dashboard) gets a tab of its own right away, in the
// background: it does not cover the agent or take the keyboard, and it stands out until it has been looked at. Only a
// port that really answers with a page counts; agents also hold ports that are not for people. A tab nobody looked at
// goes when its port closes, so a server the agent ran for a moment leaves nothing behind.
// An agent can also ask for a page itself (`kelpie_open_page`, the daemon's `page.open` event): it opens the same way, whatever
// the setting for automatic tabs says, and stays until the person closes it.
import { createEffect, createRoot } from "solid-js";
import { findPane } from "./layout";
import { isAgentKind } from "./agents";
import { browserPaneOnPort, browserPaneShowing, browserProbe, freshPages, isLocalUrl, markPageFresh, markPageSeen, setBrowserState } from "./browser";
import { portUrl, sessionPorts, uniquePorts, type Port } from "./processes";
import { sessionLabel, store } from "./store";
import { uid } from "./platform";
import { t } from "./i18n";
import type { OpenPageRequest } from "./types";

/** A server says it listens a moment before it answers. */
const PROBE_TRIES = 3;
const PROBE_GAP_MS = 1500;

/** Ports already considered, as `session:port`: a page the user closed is not opened again. */
const considered = new Set<string>();
/** The tabs opened here that have not been looked at yet. */
const unseen = new Map<string, { projectId: string; sessionId: string; port: number }>();

const key = (sessionId: string, port: number) => `${sessionId}:${port}`;
const holds = (sessionId: string, port: number) => {
  const held = sessionPorts[sessionId];
  return !!held && (!!held.stale || held.ports.some((p) => p.port === port));
};

/** Shows `url` in a new browser tab of the project, in the background, marked until it is looked at, with a notice that offers to
 *  show it. Returns the pane's id. */
function openInBackground(projectId: string, sessionName: string, url: string, title: string, page: string): string {
  const paneId = uid();
  setBrowserState(paneId, { title, url, design: false, notes: [] });
  const tab = store.addTab(projectId, { id: paneId, kind: "browser", url }, { background: true });
  markPageFresh(paneId);
  store.notify(
    t("{session} opened a web page: {page}", { session: sessionName, page }),
    { label: t("View"), run: () => store.focusPane(projectId, tab.id, paneId) },
    8000,
  );
  return paneId;
}

/** The browser pane of the project that already shows this page: the same address, or for a local server the same port. */
function paneShowing(projectId: string, url: URL) {
  return (isLocalUrl(url.href) && url.port && browserPaneOnPort(projectId, Number(url.port))) || browserPaneShowing(projectId, url.href);
}

/** A page an agent asked to show (`kelpie_open_page`): opened whatever the setting for automatic tabs says, but never twice in a
 *  project; then the person is told it is already open, and nothing takes the focus. */
export function openRequestedPage(req: OpenPageRequest) {
  let url: URL;
  try {
    url = new URL(req.url);
  } catch {
    return;
  }
  // The daemon checked the address; this window opens nothing else whatever it is sent.
  if ((url.protocol !== "http:" && url.protocol !== "https:") || url.username || url.password) return;
  const projectId = req.project_id;
  if (!store.state.projects.some((p) => p.id === projectId)) return;
  const session = store.sessions[req.session_id];
  const name = session ? sessionLabel(session) : t("An agent");
  const page = req.title || url.href;
  const shown = paneShowing(projectId, url);
  if (shown) {
    store.notify(
      t("{session} asked to show {page}, which is already open in a tab", { session: name, page }),
      { label: t("View"), run: () => store.focusPane(projectId, shown.tabId, shown.paneId) },
      8000,
    );
    return;
  }
  openInBackground(projectId, name, url.href, req.title ?? "", page);
}

async function openIfWebPage(sessionId: string, port: Port) {
  for (let i = 0; i < PROBE_TRIES; i++) {
    if (i) await new Promise((r) => setTimeout(r, PROBE_GAP_MS));
    if (!holds(sessionId, port.port)) return;
    const page = await browserProbe(port.host, port.port).catch(() => null);
    if (!page) continue;
    const session = store.sessions[sessionId];
    const projectId = session?.project_id;
    if (!projectId || !holds(sessionId, port.port) || store.state.settings.agentWebTabs === false) return;
    if (browserPaneOnPort(projectId, port.port)) return;
    const paneId = openInBackground(projectId, sessionLabel(session), portUrl(port), page.title ?? "", page.title ?? `${port.host}:${port.port}`);
    unseen.set(paneId, { projectId, sessionId, port: port.port });
    return;
  }
}

function sync() {
  for (const [sessionId, held] of Object.entries(sessionPorts)) {
    const kind = store.sessions[sessionId]?.kind;
    if (held.stale || !kind || !isAgentKind(kind)) continue;
    for (const port of uniquePorts(held.ports)) {
      if (considered.has(key(sessionId, port.port))) continue;
      considered.add(key(sessionId, port.port));
      void openIfWebPage(sessionId, port);
    }
  }
  for (const [paneId, page] of [...unseen]) {
    if (!freshPages().has(paneId)) unseen.delete(paneId);
    else if (!holds(page.sessionId, page.port)) {
      unseen.delete(paneId);
      markPageSeen(paneId);
      // The server may start again: its page then gets a tab again.
      considered.delete(key(page.sessionId, page.port));
      const tab = store.workspace(page.projectId).tabs.find((x) => findPane(x.root, paneId));
      if (tab) void store.closePane(page.projectId, tab.id, paneId);
    }
  }
}

if (typeof window !== "undefined") {
  createRoot(() => createEffect(sync));
  window.addEventListener("kelpie:open-page", (e) => openRequestedPage((e as CustomEvent<OpenPageRequest>).detail));
}
