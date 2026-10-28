// In-app updates: checks GitHub Releases through the Rust core (`updater.rs`), offers the
// newer release in the status bar, and installs it now or once no agent is working. Sessions
// keep running through an install: the daemon runs from its own copy and the restarted app
// reconnects to it.
import { createEffect, createSignal, onCleanup } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { store } from "./store";
import type { SessionInfo } from "./types";

export interface UpdateInfo {
  version: string;
  current_version: string;
  notes?: string | null;
  date?: string | null;
}

/** First check after start (the app settles first), then every 6 hours. */
export const CHECK_DELAY_MS = 30_000;
export const CHECK_EVERY_MS = 6 * 60 * 60 * 1000;

/** The release to offer: a newer one the user did not choose to skip. */
export function offered(info: UpdateInfo | null, skipped?: string): UpdateInfo | null {
  return info && info.version !== skipped ? info : null;
}

/** Whether the connected daemon keeps running through an install: on macOS always (the bundle
 *  is swapped while it runs), on Windows when it runs from its copy (`…/daemon/<build>/`). */
export function daemonSurvivesUpdate(exe: string | undefined, mac: boolean): boolean {
  if (mac) return true;
  return !!exe && /[\\/]daemon[\\/][^\\/]+[\\/][^\\/]+$/.test(exe);
}

/** No session is working: the moment "install when idle" waits for. */
export function idle(sessions: SessionInfo[]): boolean {
  return !sessions.some((s) => s.alive && s.status === "working");
}

export { olderVersion } from "./version";

const [available, setAvailable] = createSignal<UpdateInfo | null>(null);
const [progress, setProgress] = createSignal<{ downloaded?: number; total?: number | null } | null>(null);
const [whenIdle, setWhenIdle] = createSignal(false);
export { available, progress, whenIdle };

/** Asks for a newer release. A manual check also offers a skipped version and reports errors. */
export async function checkForUpdate(manual = false): Promise<UpdateInfo | null> {
  try {
    const info = await invoke<UpdateInfo | null>("update_check");
    setAvailable(offered(info, manual ? undefined : store.state.settings.skippedUpdate));
    return info;
  } catch (e) {
    if (manual) throw e;
    return null;
  }
}

export async function installUpdate() {
  setWhenIdle(false);
  const stop = await listen<{ downloaded?: number; total?: number | null }>("update-progress", (e) => setProgress(e.payload));
  try {
    await invoke("update_install");
  } finally {
    stop();
    setProgress(null);
  }
}

export function installWhenIdle() {
  setWhenIdle(true);
}

export function skipUpdate() {
  const version = available()?.version;
  if (version) store.setSettings({ skippedUpdate: version });
  setAvailable(null);
}

/** Call once from inside the app's root: schedules checks while they are enabled and installs a
 *  postponed update as soon as nothing is working. */
export function startUpdater() {
  createEffect(() => {
    if (store.state.settings.updateChecks === false) return;
    const first = setTimeout(() => void checkForUpdate(), CHECK_DELAY_MS);
    const every = setInterval(() => void checkForUpdate(), CHECK_EVERY_MS);
    onCleanup(() => {
      clearTimeout(first);
      clearInterval(every);
    });
  });
  createEffect(() => {
    if (whenIdle() && available() && idle(Object.values(store.sessions))) void installUpdate().catch((e) => store.notify(String(e)));
  });
}
