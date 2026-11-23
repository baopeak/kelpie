// The daemon's window-tools policy (Settings → Agents → Programs agents start): whether agents may start programs and use their windows,
// and whether they may use the person's own desktop. Kept in policy.json by the daemon, read and written through `daemon.policy` /
// `daemon.set_policy`; Windows only. What needs no store lives here so a test can load it; the signal is in agent-window.ts.
import { WINDOW_AGENT_API } from "./attention";
import { isWindows } from "./platform";

export interface WindowPolicy {
  /** Agents may start programs and use their windows (`window_agents` on). */
  on: boolean;
  /** Agents may start and use programs on the person's own desktop (`window_visible`). */
  visible: boolean;
}

/** The policy from a `daemon.policy` / `daemon.set_policy` reply, or null when the reply has no window keys (an older daemon). */
export function toWindowPolicy(raw: unknown): WindowPolicy | null {
  const r = raw as Record<string, unknown> | null;
  if (!r || typeof r !== "object") return null;
  const mode = r.window_agents;
  if (mode !== "off" && mode !== "on") return null;
  return { on: mode === "on", visible: r.window_visible === true };
}

export interface WindowPatch {
  window_agents?: "off" | "on";
  window_visible?: boolean;
}

/** The patch for a change of one of the two switches. Turning the tools off also withdraws the visible desktop, so that turning them on
 *  again never brings back a permission the person no longer sees. */
export function windowPatch(change: { on?: boolean; visible?: boolean }): WindowPatch {
  const patch: WindowPatch = {};
  if (change.on !== undefined) patch.window_agents = change.on ? "on" : "off";
  if (change.visible !== undefined) patch.window_visible = change.visible;
  if (change.on === false && change.visible === undefined) patch.window_visible = false;
  return patch;
}

/** Whether Settings shows the window-tools rows: a daemon that has them, on Windows. */
export const windowToolsShown = (api: number, windows: boolean = isWindows) => windows && api >= WINDOW_AGENT_API;
