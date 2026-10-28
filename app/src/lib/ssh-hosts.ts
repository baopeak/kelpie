// Saved SSH hosts, keys, known hosts, port forwards and file transfer (the Hosts page).
// Backend: app/src-tauri/src/ssh_hosts.rs. A saved host is an alias in Kelpie's generated ssh
// config, so connecting to it is opening a remote project on it (lib/remote.ts): terminals and
// agents run there in tmux, files / git / search work, and sessions resume like local ones.
import { invoke } from "@tauri-apps/api/core";
import { createSignal } from "solid-js";
import { store } from "./store";
import { CORE_AGENTS, agentPresets, isAgentKind } from "./agents";
import { remoteCheck, remoteRoot, type HostCheck } from "./remote";
import { setSessionThemeResolver } from "./terminal-host";
import { t } from "./i18n";
import type { Project, SessionInfo, SessionKind, SessionTarget } from "./types";

export interface SshHost {
  id: string;
  label: string;
  alias: string;
  group: string;
  tags: string[];
  hostname: string;
  port: number | null;
  user: string;
  auth: "key" | "password";
  identityFile: string;
  jump: string;
  forwardAgent: boolean;
  keepalive: number;
  env: Record<string, string>;
  startup: string;
  folder: string;
  tmux: boolean;
  notes: string;
  color: string;
  /** Terminal colour theme of its panes (empty: the app's terminal theme). */
  terminalTheme: string;
  /** OS seen on the last check (`ubuntu`, `darwin`…), for its badge. */
  os: string;
  lastConnected: number;
}

export interface SshForward {
  id: string;
  hostId: string;
  label: string;
  kind: "local" | "remote" | "dynamic";
  bindAddress: string;
  bindPort: number;
  destHost: string;
  destPort: number;
  autoStart: boolean;
}

export interface Vault {
  version: number;
  hosts: SshHost[];
  forwards: SshForward[];
}

export interface SshKey {
  path: string;
  name: string;
  kind: string;
  bits: number;
  fingerprint: string;
  comment: string;
  public: string;
  encrypted: boolean;
  passphraseSaved: boolean;
}

export interface KnownHost { line: number; hosts: string; hashed: boolean; kind: string; fingerprint: string; marker: string }
export interface HostKeys { name: string; keys: { kind: string; fingerprint: string; line: string }[]; known: boolean; matches: boolean }
export interface AgentState { running: boolean; keys: string[]; error: string }
export interface ForwardState { running: boolean; error: string }

export const newHost = (over: Partial<SshHost> = {}): SshHost => ({
  id: "", label: "", alias: "", group: "", tags: [], hostname: "", port: null, user: "", auth: "key", identityFile: "", jump: "",
  forwardAgent: false, keepalive: 30, env: {}, startup: "", folder: "", tmux: true, notes: "", color: "", terminalTheme: "", os: "", lastConnected: 0, ...over,
});

export const newForward = (hostId: string): SshForward => ({ id: "", hostId, label: "", kind: "local", bindAddress: "", bindPort: 0, destHost: "localhost", destPort: 0, autoStart: false });

// ---------- backend ----------

export const sshPaths = () => invoke<{ config: string; known_hosts: string; ssh_dir: string }>("ssh_paths");
const vaultLoad = () => invoke<Vault>("ssh_vault_load");
const vaultSave = (vault: Vault) => invoke<Vault>("ssh_vault_save", { vault });
export const importSshConfig = () => invoke<SshHost[]>("ssh_import_config");
export const setSecret = (kind: "password" | "passphrase", target: string, value: string) => invoke<void>("ssh_secret_set", { kind, target, value });
export const hasSecret = (kind: "password" | "passphrase", target: string) => invoke<boolean>("ssh_secret_has", { kind, target });
export const listKeys = () => invoke<SshKey[]>("ssh_keys_list");
export const generateKey = (name: string, kind: string, passphrase: string, comment: string, savePassphrase: boolean) => invoke<string>("ssh_key_generate", { name, kind, passphrase, comment, savePassphrase });
export const importKey = (source: string, name: string, passphrase: string) => invoke<string>("ssh_key_import", { source, name, passphrase });
export const deleteKey = (path: string) => invoke<void>("ssh_key_delete", { path });
export const installKey = (alias: string, keyPath: string) => invoke<void>("ssh_key_install", { alias, keyPath });
export const agentList = () => invoke<AgentState>("ssh_agent_list");
export const agentAdd = (keyPath: string) => invoke<void>("ssh_agent_add", { keyPath });
export const knownHosts = () => invoke<KnownHost[]>("ssh_known_hosts_list");
export const removeKnownHosts = (lines: number[]) => invoke<void>("ssh_known_hosts_remove", { lines });
export const hostKeys = (alias: string) => invoke<HostKeys>("ssh_host_keys", { alias });
export const trustHost = (lines: string[]) => invoke<void>("ssh_host_trust", { lines });
export const transfer = (alias: string, upload: boolean, sources: string[], target: string) => invoke<void>("ssh_transfer", { alias, upload, sources, target });
export const forwardStart = (id: string) => invoke<void>("ssh_forward_start", { id });
export const forwardStop = (id: string) => invoke<void>("ssh_forward_stop", { id });
export const forwardStatus = () => invoke<Record<string, ForwardState>>("ssh_forward_status");

// ---------- the vault ----------

const [vault, setVault] = createSignal<Vault>({ version: 1, hosts: [], forwards: [] });
export { vault };

export async function loadVault() {
  setVault(await vaultLoad());
  return vault();
}

/** Saves the whole vault; the backend fills ids and aliases and rejects unsafe values. */
export async function saveVault(next: Vault) {
  const saved = await vaultSave(next);
  setVault(saved);
  return saved;
}

export async function saveHost(edited: SshHost) {
  // What the app records itself (a check while the form was open) is kept, not reverted.
  const cur = edited.id ? hostById(edited.id) : undefined;
  const host = cur ? { ...edited, os: cur.os || edited.os, lastConnected: cur.lastConnected } : edited;
  const hosts = host.id ? vault().hosts.map((h) => (h.id === host.id ? host : h)) : [...vault().hosts, host];
  const saved = await saveVault({ ...vault(), hosts });
  // An edit can change who it logs in as: the last check no longer says.
  if (cur) forgetCheck(cur.id);
  // A new host is the one whose id was empty: the last one.
  return host.id ? saved.hosts.find((h) => h.id === host.id)! : saved.hosts[saved.hosts.length - 1];
}

export async function removeHost(id: string) {
  for (const f of vault().forwards.filter((f) => f.hostId === id)) await forwardStop(f.id).catch(() => {});
  await setSecret("password", id, "").catch(() => {});
  await saveVault({ ...vault(), hosts: vault().hosts.filter((h) => h.id !== id), forwards: vault().forwards.filter((f) => f.hostId !== id) });
}

export async function saveForward(f: SshForward) {
  const forwards = f.id ? vault().forwards.map((x) => (x.id === f.id ? f : x)) : [...vault().forwards, f];
  await saveVault({ ...vault(), forwards });
}

export async function removeForward(id: string) {
  await forwardStop(id).catch(() => {});
  await saveVault({ ...vault(), forwards: vault().forwards.filter((f) => f.id !== id) });
}

export const hostById = (id: string) => vault().hosts.find((h) => h.id === id);
export const hostByAlias = (alias: string) => vault().hosts.find((h) => h.alias === alias);
export const hostAddress = (h: SshHost) => `${h.user ? `${h.user}@` : ""}${h.hostname}${h.port && h.port !== 22 ? `:${h.port}` : ""}`;

/** "Host key verification failed" and friends: the user should compare fingerprints first. */
export const isHostKeyError = (e: unknown) => /host key verification failed|remote host identification has changed|no matching host key|host key for .* has changed/i.test(String(e));

// ---------- connecting ----------

/** The last check of each host since the app started, by host id (launchers read its agents). */
const [checks, setChecks] = createSignal<Record<string, HostCheck>>({});
export const cachedCheck = (hostId: string): HostCheck | undefined => checks()[hostId];
function forgetCheck(hostId: string) {
  const { [hostId]: _, ...rest } = checks();
  setChecks(rest);
}

/** What the host has (git, tmux, agent CLIs); records the connection time and its OS. */
export async function checkHost(host: SshHost): Promise<HostCheck> {
  const bins = agentPresets().length ? agentPresets().map((p) => p.bin) : CORE_AGENTS;
  const check = await remoteCheck(host.alias, [...new Set(bins)]);
  setChecks({ ...checks(), [host.id]: check });
  // The distro when it says (Linux), else the kernel name ("Darwin 23.1" → darwin).
  const os = check.distro || check.os.split(/\s+/)[0]?.toLowerCase() || "";
  void invoke<Vault>("ssh_host_touch", { id: host.id, os }).then(setVault, () => {});
  return check;
}

const agentIds = (c: HostCheck) => c.agents.map((bin) => agentPresets().find((p) => p.bin === bin)?.id ?? bin);

/** Agent kinds found on a host, or the core ones until it has been checked. */
export const hostAgents = (hostId: string): string[] => {
  const c = cachedCheck(hostId);
  return c ? agentIds(c) : CORE_AGENTS;
};

/** The remote project for a host's folder (created on first use). */
export async function hostProject(host: SshHost, check?: HostCheck): Promise<string> {
  const c = check ?? (await checkHost(host));
  const folder = host.folder.trim() || c.home || "/";
  const root = remoteRoot(host.alias, undefined, folder);
  const path = root.slice(root.indexOf("/", "ssh://".length));
  const leaf = path.split("/").filter(Boolean).pop() ?? "/";
  return store.addProject(root, { name: `${host.label} · ${leaf}`, remote: { dest: host.alias, path, tmux: host.tmux && c.tmux, agents: agentIds(c) } });
}

/** Saved hosts linked to a project with their folder there (links to removed hosts are skipped). */
export function projectHosts(p: Project | null | undefined): { host: SshHost; path: string }[] {
  return (p?.hosts ?? []).flatMap((l) => {
    const host = hostById(l.hostId);
    return host ? [{ host, path: l.path }] : [];
  });
}

/** Where panes on `host` start for `project`: `path`, else the folder linked there, else the
 *  host's start folder, else its login folder. Checks the host first (a new host key fails
 *  here, before any terminal asks about it). */
export async function hostTarget(host: SshHost, project?: Project | null, path?: string): Promise<SessionTarget> {
  const c = cachedCheck(host.id) ?? (await checkHost(host));
  const linked = project?.hosts?.find((l) => l.hostId === host.id)?.path;
  return { dest: host.alias, path: (path ?? "").trim() || (linked ?? "").trim() || host.folder.trim() || c.home || "/", tmux: host.tmux && c.tmux };
}

/** The folder an agent of `project` starts in on `host`: `path`, else the project's own folder
 *  there. Empty: none chosen yet. Agents never fall back to the host's start or login folder:
 *  that folder is not this project's (as root it is /root). */
export function agentFolder(host: SshHost, project: Project | null | undefined, path?: string): string {
  const own = project?.remote?.dest === host.alias ? project.remote.path : "";
  return (path ?? "").trim() || (project?.hosts?.find((l) => l.hostId === host.id)?.path ?? "").trim() || own;
}

/** The saved host a session or target runs on (undefined: this computer or an unsaved host). */
export const hostOf = (t: { dest: string } | null | undefined) => (t ? hostByAlias(t.dest) : undefined);

/** A pane's host as a saved layout keeps it. Layouts apply to any project, so the folder is kept
 *  only when it is not the one this project uses there anyway (empty: each project's own). */
export function layoutTarget(project: Project | undefined, t: SessionTarget): SessionTarget {
  const host = hostOf(t);
  if (!host) return t;
  const usual = project?.hosts?.find((l) => l.hostId === host.id)?.path || host.folder || cachedCheck(host.id)?.home || "";
  return t.path === usual ? { ...t, path: "" } : t;
}

/** Where a saved layout's pane on a host starts in `project` (an unsaved destination: as saved). */
export const layoutTargetFor = (project: Project | undefined, t: SessionTarget): Promise<SessionTarget> => {
  const host = hostOf(t);
  return host ? hostTarget(host, project, t.path) : Promise.resolve(t);
};

/** Agents of other projects already running in this folder of the host (they would share files). */
export function agentsAt(target: SessionTarget, projectId: string): SessionInfo[] {
  return Object.values(store.sessions).filter((s) => s.alive && isAgentKind(s.kind) && s.project_id !== projectId && s.remote?.dest === target.dest && s.cwd === target.path);
}

/** Opens a terminal (or an agent) on the host: in `projectId` when given (the host's folder
 *  linked there), else in the host's own remote project. A new shell types the host's startup
 *  command. */
export async function connectHost(host: SshHost, kind: SessionKind = "shell", opts: { prompt?: string; check?: HostCheck; projectId?: string } = {}) {
  const prompt = opts.prompt ?? (kind === "shell" && host.startup.trim() ? host.startup : undefined);
  const project = opts.projectId ? store.state.projects.find((p) => p.id === opts.projectId) : undefined;
  if (project && !project.remote) {
    if (isAgentKind(kind) && !agentFolder(host, project)) throw new Error(t("Link {project} to its folder on {host} first: agents do not start in the login folder", { project: project.name, host: host.label }));
    const target = await hostTarget(host, project);
    store.switchProject(project.id);
    return store.newSession(project.id, kind, { prompt, target });
  }
  if (isAgentKind(kind) && !host.folder.trim()) throw new Error(t("Set a start folder for {host}, or open it in a project linked to its folder there: agents do not start in the login folder", { host: host.label }));
  const projectId = await hostProject(host, opts.check);
  store.switchProject(projectId);
  return store.newSession(projectId, kind, { prompt });
}

/** `ssh user@host -p 2222`, `user@host:2222` or a bare address typed in the search box. */
export function parseDestination(text: string): { user: string; hostname: string; port: number | null } | null {
  const m = text.trim().match(/^(ssh\s+)?(?:([A-Za-z0-9._-]+)@)?([A-Za-z0-9][A-Za-z0-9.-]*)(?::(\d{1,5}))?(?:\s+-p\s*(\d{1,5}))?$/);
  // A bare word is a search, unless it says `ssh` or has a user, a dot or a port.
  if (!m || !(m[1] || m[2] || /[.:]/.test(text) || m[5])) return null;
  const port = Number(m[4] ?? m[5]) || null;
  if (port && port > 65535) return null;
  return { user: m[2] ?? "", hostname: m[3], port: port && port !== 22 ? port : null };
}

/** Badge for the OS seen on a host: a short name and its brand colour. */
export function osBadge(os: string): { label: string; short: string; color: string } | null {
  const known: Record<string, [string, string]> = {
    ubuntu: ["Ubuntu", "#E95420"], debian: ["Debian", "#A80030"], fedora: ["Fedora", "#51A2DA"], centos: ["CentOS", "#932279"],
    rhel: ["Red Hat", "#EE0000"], rocky: ["Rocky", "#10B981"], almalinux: ["AlmaLinux", "#0F4266"], alpine: ["Alpine", "#0D597F"],
    arch: ["Arch", "#1793D1"], amzn: ["Amazon Linux", "#FF9900"], opensuse: ["openSUSE", "#73BA25"], "opensuse-leap": ["openSUSE", "#73BA25"],
    raspbian: ["Raspberry Pi OS", "#C51A4A"], linuxmint: ["Mint", "#87CF3E"], nixos: ["NixOS", "#5277C3"], darwin: ["macOS", "#555555"],
    freebsd: ["FreeBSD", "#AB2B28"], linux: ["Linux", "#333333"],
  };
  if (!os) return null;
  const [label, color] = known[os] ?? [os, "#5b6170"];
  return { label, short: label.replace(/[^A-Za-z]/g, "").slice(0, 2), color };
}

/** Termius' "run snippet on hosts": one terminal per host, each typing `command`. */
export async function runOnHosts(hosts: SshHost[], command: string) {
  const failed: string[] = [];
  for (const h of hosts) {
    try {
      await connectHost(h, "shell", { prompt: command });
    } catch {
      failed.push(h.label);
    }
  }
  return failed;
}

/** Forwards marked auto-start, once the vault is loaded. */
export async function startAutoForwards() {
  for (const f of vault().forwards.filter((f) => f.autoStart)) await forwardStart(f.id).catch(() => {});
}

/** Loads the vault and starts the auto-start forwards (app start). */
export async function initSshHosts() {
  // Panes on a saved host take its terminal theme.
  setSessionThemeResolver((id) => hostOf(store.sessions[id]?.remote)?.terminalTheme || undefined);
  await loadVault().catch(() => {});
  await startAutoForwards();
}

/** Official installer for Claude Code on Linux / macOS hosts (typed into a terminal there). */
export const CLAUDE_INSTALL = "curl -fsSL https://claude.ai/install.sh | bash"; // i18n-ignore
