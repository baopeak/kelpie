// What a phone may start (`mobile.set_launch`): the projects and agents of this computer's launchers, each agent with the arguments and
// environment its profile gives it here, so that a session a phone starts is the one the desktop's own launchers would start. The daemon
// keeps it in memory and starts from it, also while this window is closed.
import { launchableAgents, type AgentPreset } from "./agents";
import type { Project, Settings } from "./types";

/** Launch arguments and environment of an agent profile, as every launcher of this app starts it: the profile's arguments, its permission mode
 *  (Claude's `--permission-mode`, Codex's `approval_policy`), the Yolo flags (`yolo`, the preset's) when Yolo is on and the profile makes no
 *  choice of its own (`withYolo: false` leaves them out: the chat view asks Allow / Deny itself), and the profile's environment with the
 *  account's (`account`) over it. */
export function profileLaunchOf(
  settings: Pick<Settings, "profiles" | "agentPermissions">,
  kind: string,
  yolo: string[],
  account: Record<string, string> | undefined,
  withYolo = true,
): { args: string[]; env?: Record<string, string> } {
  const p = settings.profiles.find((x) => x.kind === kind);
  const args = [...(p?.args ?? [])];
  if (p?.permission && kind === "claude") args.push("--permission-mode", p.permission);
  if (p?.permission && kind === "codex") args.push("-c", `approval_policy=${p.permission}`);
  if (withYolo && settings.agentPermissions === "yolo" && !p?.permission && yolo.length && !yolo.every((a) => args.includes(a))) args.push(...yolo);
  const env = { ...(p?.env ?? {}), ...(account ?? {}) };
  return { args, env: Object.keys(env).length ? env : undefined };
}

export interface LaunchCatalogue {
  projects: { id: string; name: string; path: string }[];
  agents: { kind: string; label: string; args: string[]; env: Record<string, string> }[];
}

/** What the catalogue is built from: the app's projects and the settings that decide which agents its launchers offer. */
export interface LaunchState {
  projects: Pick<Project, "id" | "name" | "path" | "remote">[];
  settings: { hiddenAgents?: string[]; registryAgents?: { id: string }[] };
}

/** The catalogue from the app's state. Projects on another machine (SSH) are left out: they need saved secrets and a dialog here. So are
 *  chat-only agents (saved from the ACP registry: they need this window's chat front-end) and agents hidden from the launchers. `profileLaunch`
 *  is the store's: the profile's arguments, the permission mode or Yolo, and the profile's and the active account's environment. */
export function buildLaunchCatalogue(state: LaunchState, presets: AgentPreset[], profileLaunch: (kind: string) => { args: string[]; env?: Record<string, string> }): LaunchCatalogue {
  const chatOnly = new Set((state.settings.registryAgents ?? []).map((a) => a.id));
  const kinds = launchableAgents(state.settings.hiddenAgents ?? [], presets).filter((k) => !chatOnly.has(k));
  return {
    projects: state.projects.filter((p) => !p.remote).map((p) => ({ id: p.id, name: p.name, path: p.path })),
    agents: kinds.map((kind) => {
      const launch = profileLaunch(kind);
      return { kind, label: presets.find((p) => p.id === kind)?.label ?? kind, args: launch.args, env: launch.env ?? {} };
    }),
  };
}
