// Browser pane takeover, the window's side. A phone with control takes over one of the browser panes to get past a sign-in; the daemon's broker
// decides who holds it, and Rust (src-tauri/src/browser/takeover.rs) streams the page and drives it. What this window does: tell the daemon which
// panes there are, bring the pane taken over to the front and keep it there, show who controls it with a Stop button (browser-pane.tsx), and say
// when it ends. The daemon's requests come as `provider-request` events and are answered with `provider_reply`.
import { invoke } from "@tauri-apps/api/core";
import { createEffect, createRoot, createSignal } from "solid-js";
import { onEvent } from "./ipc";
import { browserState } from "./browser";
import { store } from "./store";
import { t } from "./i18n";
import { browserLabelOf, findPane, listPanes, paneToBringBack, type HeldPane } from "./takeover-panes";

/** A pane a phone holds now, by pane id: the daemon's takeover id and the device's name. */
export interface Held {
  tk: string;
  device: string;
  /** When it started: of several holds only the newest is brought back to the front. */
  startedAt: number;
}

export const [held, setHeld] = createRoot(() => createSignal<Readonly<Record<string, Held>>>({}));

interface ProviderRequest {
  preq: number;
  method: string;
  params: Record<string, unknown>;
}

const answer = (preq: number, result: unknown) => invoke("provider_reply", { preq, result, error: null });
const refuse = (preq: number, error: string) => invoke("provider_reply", { preq, result: null, error });

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Starts streaming once the pane's page is open: a pane brought to the front from another tab opens it in about half a second. It tries for 3 s at
 *  most, well inside the 10 s the daemon waits: past that the daemon gives up and tells Rust to refuse the start. */
async function startWhenOpen(tk: string, label: string, device: string, settings: unknown): Promise<{ host: string }> {
  let last: unknown = null;
  for (let i = 0; i < 12; i++) {
    try {
      return await invoke<{ host: string }>("browser_takeover_start", { tk, label, banner: t("Controlled from {device}", { device }), settings });
    } catch (e) {
      last = e;
      if (!String(e).includes("no browser")) break;
      await sleep(250);
    }
  }
  throw last;
}

async function onRequest(r: ProviderRequest) {
  const projects = store.state.projects;
  if (r.method === "browser.panes") return answer(r.preq, { panes: listPanes(projects, store.workspace, browserState) });
  if (r.method !== "browser.takeover_start") return refuse(r.preq, "unknown request");
  const p = r.params as { tk: string; pane: string; device: string };
  const at = findPane(p.pane, projects, store.workspace);
  if (!at) return refuse(r.preq, t("That pane was closed on the computer."));
  // The pane is brought to the front: a hidden page sends no screencast frames (`keepInFront` brings it back once more if the person switches away).
  store.switchProject(at.projectId);
  store.focusPane(at.projectId, at.tabId, at.paneId);
  try {
    const started = await startWhenOpen(p.tk, p.pane, p.device, r.params);
    if (endedEarly.includes(p.tk)) return refuse(r.preq, "the phone's request ended before the pane was ready");
    setHeld((h) => ({ ...h, [at.paneId]: { tk: p.tk, device: p.device, startedAt: Date.now() } }));
    store.notify(t("{device} took over a browser pane", { device: p.device }));
    return answer(r.preq, { title: browserState[at.paneId]?.title || started.host, host: started.host });
  } catch (e) {
    return refuse(r.preq, String(e).replace(/^Error: /, ""));
  }
}

/** The person pressed Stop on the computer. The banner goes whatever Rust answers: a takeover it no longer knows has nothing left to stop. */
export async function stopTakeover(tk: string) {
  await invoke("browser_takeover_stop", { tk }).catch(() => {});
  ended({ tk, reason: "desktop" });
}

/** Takeovers that ended before the window had them as held (the daemon gave up while the start was finishing): a start that answers later is not shown. */
const endedEarly: string[] = [];

function ended(e: { tk: string; reason: string }) {
  const found = Object.entries(held()).find(([, x]) => x.tk === e.tk);
  if (!found) {
    endedEarly.push(e.tk);
    if (endedEarly.length > 16) endedEarly.shift();
    return;
  }
  const [pane, x] = found;
  setHeld((h) => {
    const next: Record<string, Held> = { ...h };
    delete next[pane];
    return next;
  });
  if (e.reason !== "desktop") store.notify(t("{device} no longer controls the browser pane", { device: x.device }));
}

/** A pane hidden behind another tab or project sends no screencast frames and takes no input (seen on WebView2: the phone is told it is hidden). Right
 *  after the person switches away from the newest held pane its tab is brought back, once per hold, with one toast; a second switch away stays. */
function keepInFront() {
  let wasInFront: Record<string, boolean> = {};
  const broughtBack = new Set<string>();
  createRoot(() =>
    createEffect(() => {
      const active = store.state.activeProject;
      const holds: (HeldPane & { at: NonNullable<ReturnType<typeof findPane>> })[] = [];
      for (const [paneId, h] of Object.entries(held())) {
        const at = findPane(browserLabelOf(paneId), store.state.projects, store.workspace);
        if (at) holds.push({ paneId, startedAt: h.startedAt, inFront: active === at.projectId && store.workspace(at.projectId).activeTab === at.tabId, at });
      }
      for (const id of broughtBack) if (!held()[id]) broughtBack.delete(id);
      const back = paneToBringBack(holds, wasInFront, broughtBack);
      wasInFront = Object.fromEntries(holds.map((h) => [h.paneId, h.inFront]));
      if (!back) return;
      broughtBack.add(back);
      // After this turn, not inside it: the switch re-runs this effect, which then finds the pane in front. The hold and the pane are looked up again
      // then: the takeover may have ended, or the pane gone, meanwhile.
      setTimeout(() => {
        if (!held()[back]) return;
        const at = findPane(browserLabelOf(back), store.state.projects, store.workspace);
        if (!at) return;
        store.switchProject(at.projectId);
        store.focusPane(at.projectId, at.tabId, at.paneId);
        store.notify(t("A phone controls this browser pane, so it was brought back to the front once. Switch away again and the phone sees that the pane is hidden and can do nothing in it."));
      }, 0);
    }),
  );
}

let started = false;
/** Listens for the daemon's requests (once). */
export function initTakeover() {
  if (started) return;
  started = true;
  keepInFront();
  void onEvent<ProviderRequest>("provider-request", (r) => void onRequest(r));
  void onEvent<{ tk: string; reason: string }>("takeover-ended", ended);
  void onEvent<{ event: string; params: { kind?: string } }>("daemon-event", (m) => {
    // Something of this user's tried to stand in for this window: the daemon refused it, and the person should know.
    if (m.event === "provider.refused") store.notify(t("A program on this computer tried to provide Kelpie's browser panes to phones in this window's place. It was refused."), undefined, 10000);
  });
}
