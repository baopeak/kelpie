// Hosts that log in as root: Claude Code refuses to skip permission prompts there, so the daemon
// starts it asking for them (crates/kelpied/src/agents.rs). Before such a start Kelpie says why
// and offers to set up a regular user for Yolo mode (components/root-user-dialog.tsx).
import { createSignal } from "solid-js";
import { store } from "./store";
import { cachedCheck, checkHost, hostOf } from "./ssh-hosts";
import type { SessionTarget } from "./types";

/** Claude's flags that skip permission prompts (each refused as root). */
export function skipsPermissions(args: string[]): boolean {
  return args.some((a, i) =>
    a === "--dangerously-skip-permissions" || a === "--allow-dangerously-skip-permissions" || a === "--permission-mode=bypassPermissions" || (a === "--permission-mode" && args[i + 1] === "bypassPermissions"));
}

/** A login name `useradd` takes everywhere, and not root (the backend checks it again). */
export const validUserName = (n: string) => /^[a-z_][a-z0-9_-]{0,31}$/.test(n) && n !== "root";

/** "warn": a Claude start in Yolo mode on a root host waits for `answer`; "guide": setting up
 *  a regular user. */
export type RootUserDialog = { mode: "warn" | "guide"; hostId: string; projectId?: string; answer?: (start: boolean) => void };

const [dialog, setDialogSignal] = createSignal<RootUserDialog | null>(null);
export { dialog as rootUserDialog };

/** Warnings waiting for the dialog to close (another host's warning, or the guide). */
let waiting: (() => void)[] = [];

function setDialog(next: RootUserDialog | null) {
  setDialogSignal(next);
  if (next) return;
  const wake = waiting;
  waiting = [];
  wake.forEach((w) => w());
}

/** Shown on the user's own request: a start still waiting is cancelled, never left hanging. */
function show(next: RootUserDialog) {
  dialog()?.answer?.(false);
  setDialog(next);
}

export const openRootUserGuide = (hostId: string, projectId?: string) => show({ mode: "guide", hostId, projectId });

/** Closes the dialog; a waiting start goes ahead when `start`. */
export function closeRootUserDialog(start = false) {
  const d = dialog();
  setDialog(null);
  d?.answer?.(start);
}

/** From the warning to the guide (the start is cancelled). */
export function warnToGuide() {
  const d = dialog();
  if (d) show({ mode: "guide", hostId: d.hostId, projectId: d.projectId });
}

const pending = new Map<string, Promise<boolean>>();

/** Asked before starting `kind` with `args` on `target`: false when the user cancelled. Only a
 *  Claude start in Yolo mode on a saved host that logs in as root asks (once for the panes of a
 *  layout starting together); a host that cannot be checked starts as usual. */
export async function confirmRootStart(kind: string, args: string[], target: SessionTarget | undefined, projectId: string): Promise<boolean> {
  if (kind !== "claude" || !target || !skipsPermissions(args)) return true;
  const host = hostOf(target);
  if (!host || store.state.settings.rootYoloOk?.includes(host.id)) return true;
  const check = cachedCheck(host.id) ?? (await checkHost(host).catch(() => undefined));
  if (!check?.root) return true;
  let asked = pending.get(host.id);
  if (!asked) {
    asked = ask(host.id, projectId).finally(() => pending.delete(host.id));
    pending.set(host.id, asked);
  }
  return asked;
}

/** Shows the warning once the dialog is free: panes on two root hosts each get their answer. */
async function ask(hostId: string, projectId: string): Promise<boolean> {
  while (dialog()) await new Promise<void>((wake) => waiting.push(wake));
  return new Promise<boolean>((resolve) => setDialog({ mode: "warn", hostId, projectId, answer: resolve }));
}
