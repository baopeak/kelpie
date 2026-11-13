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

/** The daemon's bounds on a catalogue, in bytes for the texts (`crates/kelpied/src/mobile/launch.rs` keeps the same numbers). The daemon
 *  refuses a whole catalogue that breaks one, and then phones can start nothing: the catalogue is made to fit before it is sent. */
export const LAUNCH_LIMITS = { projects: 500, agents: 64, args: 64, projectId: 128, projectName: 256, path: 4096, kind: 64, label: 128, envKey: 128 } as const;

/** Control characters, as Rust's `char::is_control` has them (Unicode Cc). */
const CONTROL = /[\u0000-\u001f\u007f-\u009f]/;
const bytes = (s: string) => new TextEncoder().encode(s).length;
/** A text the daemon takes as it is: not empty, within `most` bytes, without control characters. */
const plain = (s: string, most: number) => s.length > 0 && bytes(s) <= most && !CONTROL.test(s);
/** A name in an agent's environment as the daemon takes it: plain, and without "=" (the system would read it as another variable). */
const envName = (s: string) => plain(s, LAUNCH_LIMITS.envKey) && !s.includes("=");
/** A name as the daemon takes it: control characters dropped, cut at `most` bytes on a character. */
function fitName(s: string, most: number): string {
  let out = "";
  let used = 0;
  for (const ch of s.replace(new RegExp(CONTROL.source, "g"), "")) {
    used += bytes(ch);
    if (used > most) break;
    out += ch;
  }
  return out;
}

/** The catalogue from the app's state. Projects on another machine (SSH) are left out: they need saved secrets and a dialog here. So are
 *  chat-only agents (saved from the ACP registry: they need this window's chat front-end) and agents hidden from the launchers. `profileLaunch`
 *  is the store's: the profile's arguments, the permission mode or Yolo, and the profile's and the active account's environment.
 *
 *  It is made to fit the daemon's bounds (`LAUNCH_LIMITS`), so that one odd project does not keep phones from starting anything: a name or a
 *  label too long or with control characters is trimmed; a project whose id or folder could not be used, and an agent whose kind could not,
 *  with more arguments than allowed or with an environment name the daemon refuses, is left out (its arguments and environment are never cut:
 *  that would start it otherwise than the desktop does); past the most projects or agents, the rest are left out. */
export function buildLaunchCatalogue(state: LaunchState, presets: AgentPreset[], profileLaunch: (kind: string) => { args: string[]; env?: Record<string, string> }): LaunchCatalogue {
  const chatOnly = new Set((state.settings.registryAgents ?? []).map((a) => a.id));
  const kinds = launchableAgents(state.settings.hiddenAgents ?? [], presets).filter((k) => !chatOnly.has(k) && plain(k, LAUNCH_LIMITS.kind));
  const L = LAUNCH_LIMITS;
  return {
    projects: state.projects
      .filter((p) => !p.remote && plain(p.id, L.projectId) && plain(p.path, L.path))
      .slice(0, L.projects)
      .map((p) => ({ id: p.id, name: fitName(p.name, L.projectName), path: p.path })),
    agents: kinds
      .map((kind) => {
        const launch = profileLaunch(kind);
        return { kind, label: fitName(presets.find((p) => p.id === kind)?.label ?? kind, L.label), args: launch.args, env: launch.env ?? {} };
      })
      .filter((a) => a.args.length <= L.args && Object.keys(a.env).every(envName))
      .slice(0, L.agents),
  };
}
