// The computer's screen seen from a phone, the window's side. The daemon's broker decides who sees it and the app's Rust side runs the
// helper (src-tauri/src/screen.rs); what this window does: show who sees the screen in the status bar with a Stop button
// (screen-indicator.tsx), say when it starts and ends, and give the helper the stop hotkey and the crisp-text switch from Settings.
import { invoke } from "@tauri-apps/api/core";
import { createEffect, createRoot, createSignal } from "solid-js";
import { onEvent } from "./ipc";
import { store } from "./store";
import { t } from "./i18n";

export interface Viewer {
  sc: string;
  device: string;
  title: string;
}

export const [viewers, setViewers] = createRoot(() => createSignal<readonly Viewer[]>([]));

/** Why a screen ended, said on the desktop; the reason codes are the daemon's and the helper's. */
export function endedText(reason: string, device: string): string | null {
  switch (reason) {
    case "desktop":
      return null;
    case "hotkey":
      return t("Stopped with the hotkey: {device} no longer sees this screen.", { device });
    case "closed":
      return t("The shared window was closed: {device} no longer sees it.", { device });
    case "revoked":
      return t("{device} lost control, or locked: it no longer sees this screen.", { device });
    case "idle":
      return t("{device} stopped seeing this screen after 10 minutes with nothing done.", { device });
    default:
      return t("{device} no longer sees this screen.", { device });
  }
}

/** The person pressed Stop. */
export async function stopScreen(sc: string) {
  await invoke("screen_stop", { sc }).catch(() => {});
}

let started = false;
/** Listens for the helper's starts and ends (once), and keeps it told what Settings says. */
export function initScreen() {
  if (started) return;
  started = true;
  void invoke<Viewer[]>("screen_viewers").then(setViewers).catch(() => {});
  void onEvent<Viewer>("screen-started", (v) => {
    setViewers((all) => [...all.filter((x) => x.sc !== v.sc), { sc: v.sc, device: v.device, title: v.title }]);
    store.notify(t("{device} sees this computer's screen: {title}", { device: v.device, title: v.title }));
  });
  void onEvent<{ sc: string; reason: string }>("screen-ended", (e) => {
    const was = viewers().find((v) => v.sc === e.sc);
    setViewers((all) => all.filter((v) => v.sc !== e.sc));
    const text = was && endedText(e.reason, was.device);
    if (text) store.notify(text);
  });
  void onEvent<{ note: string; hotkey?: string; encoder?: string }>("screen-note", (n) => {
    if (n.note === "hotkey_taken") store.notify(t("Another program holds {hotkey}, so it cannot stop the shared screen: use Stop in the status bar, or pick another hotkey in Settings → Mobile.", { hotkey: n.hotkey ?? "" }), undefined, 10000);
    if (n.note === "encoder" && n.encoder === "sw") store.notify(t("The shared screen uses the software encoder: smaller and slower, but it works."));
  });
  createRoot(() =>
    createEffect(() => {
      const hotkey = store.state.settings.screenStopHotkey;
      const tiles = store.state.settings.screenCrispText;
      void invoke("screen_prefs", { hotkey, tiles }).catch(() => {});
    }),
  );
}
