// The daemon's window-tools policy as this window last saw it (the pure part is agent-window-policy.ts).
import { createRoot, createSignal } from "solid-js";
import { policyGet } from "./ipc";
import { store } from "./store";
import { toWindowPolicy, windowToolsShown, type WindowPolicy } from "./agent-window-policy";

/** The daemon's window policy as last read or set (null: not read yet, or a daemon without it). */
export const [windowPolicy, setWindowPolicy] = createRoot(() => createSignal<WindowPolicy | null>(null));

export async function refreshWindowPolicy() {
  if (!windowToolsShown(store.daemonApi())) return;
  await policyGet().then((r) => setWindowPolicy(toWindowPolicy(r)), () => {});
}
