// Remote projects over SSH (backend: app/src-tauri/src/remote.rs; sessions: the daemon runs
// `ssh -t`, see crates/kelpied/src/agents.rs). A remote project's `path` is its root
// `ssh://[user@]host[:port]/abs/path`; its `remote` field says how to reach it.
import { invoke } from "@tauri-apps/api/core";
import { store } from "./store";
import type { Project, SessionInfo, SessionTarget } from "./types";

export interface HostCheck {
  os: string;
  /** `/etc/os-release` ID (ubuntu, debian…); empty on macOS and the BSDs. */
  distro: string;
  home: string;
  /** The account it logs in as; `root` true when that is uid 0. */
  user: string;
  root: boolean;
  git: boolean;
  tmux: boolean;
  agents: string[];
}

export interface AddedUser {
  home: string;
  /** It was there already (it only got root's keys). */
  existed: boolean;
  /** sshd's AllowUsers / AllowGroups / DenyUsers / DenyGroups lines. */
  sshd: string[];
}

/** Package-manager command per `/etc/os-release` ID, for hosts without tmux. */
const TMUX_INSTALL: [RegExp, string][] = [
  [/^(ubuntu|debian|raspbian|linuxmint|pop|kali)$/, "sudo apt-get update && sudo apt-get install -y tmux"],
  [/^(fedora|rhel|centos|rocky|almalinux|ol|amzn)$/, "sudo dnf install -y tmux || sudo yum install -y tmux"],
  [/^(arch|manjaro|endeavouros)$/, "sudo pacman -S --noconfirm tmux"],
  [/^(opensuse.*|sles)$/, "sudo zypper install -y tmux"],
  [/^alpine$/, "sudo apk add tmux"],
];

/** The command that installs tmux on a checked host (typed into a terminal there), or null when
 *  its package manager is not known. */
export function tmuxInstall(c: Pick<HostCheck, "os" | "distro">): string | null {
  if (!c.distro && /^darwin/i.test(c.os)) return "brew install tmux";
  return TMUX_INSTALL.find(([re]) => re.test(c.distro.toLowerCase()))?.[1] ?? null;
}

export const remoteConfigure =(config: string | null) => invoke<void>("remote_configure", { config });
export const remoteHosts = () => invoke<string[]>("remote_hosts");
export const remoteCheck = (dest: string, agents: string[]) => invoke<HostCheck>("remote_check", { dest, agents });
export const remoteAddUser = (dest: string, name: string) => invoke<AddedUser>("remote_add_user", { dest, name });
export const remoteFolders = (dest: string, path: string) => invoke<string[]>("remote_folders", { dest, path });

export const isRemote = (p: Pick<Project, "remote"> | undefined | null) => !!p?.remote;

/** `host` as typed ("user@host:2222" or an alias) → destination and port. */
export function splitHost(host: string): { dest: string; port?: number } {
  const m = host.trim().match(/^(.*?)(?::(\d+))?$/);
  return { dest: m?.[1] ?? host.trim(), port: m?.[2] ? Number(m[2]) : undefined };
}

export function remoteRoot(dest: string, port: number | undefined, path: string): string {
  const clean = "/" + path.trim().replace(/\\/g, "/").replace(/^\/+|\/+$/g, "");
  return `ssh://${dest}${port ? `:${port}` : ""}${clean === "/" ? "/" : clean}`;
}

/** Kelpie's generated ssh config (saved hosts + an include of the user's config); every remote
 *  session reads it. Set at startup (`loadSshConfigPath`). */
let sshConfigPath: string | undefined;
export async function loadSshConfigPath() {
  sshConfigPath = (await invoke<{ config: string }>("ssh_paths")).config;
}

/** Where a project's sessions run by default (undefined: this computer). */
export const projectTarget = (p: Project): SessionTarget | undefined =>
  p.remote ? { dest: p.remote.dest, port: p.remote.port, path: p.remote.path, tmux: p.remote.tmux } : undefined;

/** Where a session runs (undefined: this computer). */
export const sessionTarget = (s: SessionInfo): SessionTarget | undefined =>
  s.remote ? { dest: s.remote.dest, port: s.remote.port, path: s.cwd, tmux: s.remote.tmux } : undefined;

/** The `remote` part of a session that runs on `t`. */
export function targetRemote(t: SessionTarget) {
  return { dest: t.dest, port: t.port, config: sshConfigPath ?? (store.state.settings.sshConfig?.trim() || undefined), tmux: t.tmux };
}

/** Environment that lets a session's ssh log in to `dest` with a saved password / key passphrase. */
export const targetEnv = (dest: string) => invoke<Record<string, string>>("ssh_session_env", { alias: dest }).catch(() => ({}) as Record<string, string>);

export const hostLabel = (p: Project) => (p.remote ? `${p.remote.dest}${p.remote.port ? `:${p.remote.port}` : ""}` : "");
