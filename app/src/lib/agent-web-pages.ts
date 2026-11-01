// A web page an agent starts (a dev server, a studio, a dashboard) gets a tab of its own right away, in the
// background: it does not cover the agent or take the keyboard, and it stands out until it has been looked at. Only a
// port that really answers with a page counts; agents also hold ports that are not for people. A tab nobody looked at
// goes when its port closes, so a server the agent ran for a moment leaves nothing behind.
import { createEffect, createRoot } from "solid-js";
import { findPane } from "./layout";
import { isAgentKind } from "./agents";
import { browserPaneOnPort, browserProbe, freshPages, markPageFresh, markPageSeen, setBrowserState } from "./browser";
import { portUrl, sessionPorts, uniquePorts, type Port } from "./processes";
import { sessionLabel, store } from "./store";
import { uid } from "./platform";
import { t } from "./i18n";

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
    const paneId = uid();
    const url = portUrl(port);
    setBrowserState(paneId, { title: page.title ?? "", url, design: false, notes: [] });
    const tab = store.addTab(projectId, { id: paneId, kind: "browser", url }, { background: true });
    markPageFresh(paneId);
    unseen.set(paneId, { projectId, sessionId, port: port.port });
    store.notify(
      t("{session} opened a web page: {page}", { session: sessionLabel(session), page: page.title ?? `${port.host}:${port.port}` }),
      { label: t("View"), run: () => store.focusPane(projectId, tab.id, paneId) },
      8000,
    );
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

if (typeof window !== "undefined") createRoot(() => createEffect(sync));
