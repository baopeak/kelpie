// A web page an agent starts (a dev server, a studio, a dashboard) gets a tab of its own right away, in the
// background: it does not cover the agent or take the keyboard, and it stands out until it has been looked at. Only a
// port that really answers with a page counts; agents also hold ports that are not for people. A tab nobody looked at
// goes when its port closes, so a server the agent ran for a moment leaves nothing behind.
// An agent can also ask for a page itself (`kelpie_open_page`, the daemon's `page.open` event): it opens the same way, whatever
// the setting for automatic tabs says, and stays until the person closes it.
import { createEffect, createRoot } from "solid-js";
import { findPane } from "./layout";
import { isAgentKind } from "./agents";
import { browserPaneOnPort, browserPaneShowing, browserProbe, freshPages, markPageFresh, markPageSeen, setBrowserState } from "./browser";
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
 *  show it. The pane is recorded as opened for `sessionId`, whose agent may then use it with the browser tools. Returns the pane's id. */
function openInBackground(projectId: string, sessionId: string, sessionName: string, url: string, title: string, page: string): string {
  const paneId = uid();
  setBrowserState(paneId, { title, url, design: false, notes: [] });
  const tab = store.addTab(projectId, { id: paneId, kind: "browser", url, openedBy: sessionId }, { background: true });
  markPageFresh(paneId);
  store.notify(
    t("{session} opened a web page: {page}", { session: sessionName, page }),
    { label: t("View"), run: () => store.focusPane(projectId, tab.id, paneId) },
    8000,
  );
  return paneId;
}

/** Not shown but changes how the text around it reads: bidirectional controls and zero-width characters (as the daemon strips them). */
const HIDDEN = /[؜᠎​-‏‪-‮⁠-⁩﻿]/g;
const cleanTitle = (title: string | null | undefined) => (title ?? "").replace(HIDDEN, "").trim();

/** A page an agent asked to show (`kelpie_open_page`): opened whatever the setting for automatic tabs says, but never twice in a
 *  project (a tab already on the same address: the person is told it is already open, and nothing takes the focus). A request this
 *  window cannot carry out is never dropped silently: the person is told what was asked and why nothing opened. */
export function openRequestedPage(req: OpenPageRequest) {
  const session = store.sessions[req.session_id];
  const name = session ? sessionLabel(session) : t("An agent");
  const title = cleanTitle(req.title);
  const refuse = (why: string) =>
    store.notify(t("{session} asked to open {page}, but {reason}", { session: name, page: String(req.url ?? "").replace(HIDDEN, ""), reason: why }), undefined, 8000);
  let url: URL;
  try {
    url = new URL(req.url);
  } catch {
    return refuse(t("it is not a web address"));
  }
  // The daemon checked the address; this window opens nothing else whatever it is sent.
  if (url.protocol !== "http:" && url.protocol !== "https:") return refuse(t("only http and https pages open in Kelpie"));
  if (url.username || url.password) return refuse(t("addresses with a user name or password are not opened"));
  const projectId = req.project_id;
  if (!store.state.projects.some((p) => p.id === projectId)) return refuse(t("its project is no longer in this window"));
  const page = title || url.href;
  // Only the same address: another page of the same server is another page, and gets its own tab.
  const shown = browserPaneShowing(projectId, url.href);
  if (shown) {
    // Asked for now: an automatic tab that showed it is no longer one to close when its server stops.
    unseen.delete(shown.paneId);
    store.notify(
      t("{session} asked to show {page}, which is already open in a tab", { session: name, page }),
      { label: t("View"), run: () => store.focusPane(projectId, shown.tabId, shown.paneId) },
      8000,
    );
    return;
  }
  // A tab that already showed the address stays the person's (above): only a new one is the agent's.
  openInBackground(projectId, req.session_id, name, url.href, title, page);
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
    const paneId = openInBackground(projectId, sessionId, sessionLabel(session), portUrl(port), page.title ?? "", page.title ?? `${port.host}:${port.port}`);
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
