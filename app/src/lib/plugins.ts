// Plugins (Settings → Plugins): folders with an `kelpie-plugin.json` manifest that add agents,
// palette commands, quick commands, terminal themes, layouts and status-bar items. The app
// (app/src-tauri/src/plugins.rs) lists them and keeps approvals; only approved plugins — the
// exact manifest the user reviewed — contribute anything here.
import { createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { store } from "./store";
import { t } from "./i18n";
import { daemon } from "./ipc";
import { allPanes } from "./layout";
import { commands } from "./commands";
import { loadAgentPresets } from "./agents";
import { setPluginThemes, type TerminalTheme } from "./terminal-themes";
import { allViews } from "./terminal-host";
import { runQuickCommand, typeInto } from "./quick-commands";
import type { LayoutTemplate, QuickCommand, SavedLayout } from "./types";

export interface PluginAgent { id: string; label: string; bin: string; launch?: string[]; yolo?: string[]; resumeLatest?: string[]; prompt?: string; acp?: string[] }
export interface PluginCommand { id: string; label: string; run: string; in?: "terminal" | "tab" | "split" | "agent"; enter?: boolean }
export interface PluginQuickCommand { label: string; text: string; action?: "terminal" | "agent"; enter?: boolean }
export interface PluginStatusItem { id: string; label?: string; command: string; intervalSeconds?: number; timeoutSeconds?: number }

export interface PluginManifest {
  id: string;
  name: string;
  version?: string;
  description?: string;
  author?: string;
  contributes?: {
    agents?: PluginAgent[];
    commands?: PluginCommand[];
    quickCommands?: PluginQuickCommand[];
    terminalThemes?: { id: string; label: string; dark?: boolean; colors: string[] }[];
    layouts?: { name: string; template: LayoutTemplate }[];
    statusItems?: PluginStatusItem[];
  };
}

export interface PluginInfo {
  id: string;
  dir: string;
  manifest: PluginManifest | null;
  hash: string;
  approved: boolean;
  changed: boolean;
  error: string | null;
  /** The manifest's file name (`kelpie-plugin.json`, or `erka-plugin.json` from before the rename). */
  file: string;
}

export const pluginList = () => invoke<PluginInfo[]>("plugin_list");
export const pluginDir = () => invoke<string>("plugin_dir");
export const pluginInstallFolder = (src: string, replace = false) => invoke<string>("plugin_install_folder", { src, replace });
export const pluginInstallGit = (url: string) => invoke<string>("plugin_install_git", { url });
export const pluginRemove = (id: string) => invoke<void>("plugin_remove", { id });
export const pluginApprove = (id: string, hash: string | null) => invoke<void>("plugin_approve", { id, hash });
export const pluginCreate = (id: string, name: string) => invoke<string>("plugin_create", { id, name });
export const pluginStatus = (id: string, item: string, cwd: string | null) => invoke<{ ok: boolean; text: string; error: string }>("plugin_status", { id, item, cwd });

const [plugins, setPlugins] = createSignal<PluginInfo[]>([]);
const [loaded, setLoaded] = createSignal(false);
export { plugins, loaded as pluginsLoaded };

/** Plugins whose contributions are in use. */
export const activePlugins = () => plugins().filter((p) => p.approved && !p.error && p.manifest);

type Contributes = NonNullable<PluginManifest["contributes"]>;

const contributions = <K extends keyof Contributes>(key: K): { plugin: PluginInfo; item: NonNullable<Contributes[K]>[number] }[] =>
  activePlugins().flatMap((p) => ((p.manifest!.contributes?.[key] ?? []) as NonNullable<Contributes[K]>[number][]).map((item) => ({ plugin: p, item })));

/** The daemon's id for a plugin's agent (session kind). */
export const pluginAgentId = (pluginId: string, agentId: string) => `${pluginId}-${agentId}`;

export const pluginQuickCommands = (): QuickCommand[] =>
  contributions("quickCommands").map(({ plugin, item }, i) => ({
    id: `plugin:${plugin.id}:${i}`,
    label: item.label,
    text: item.text,
    action: item.action === "agent" ? "agent" : "terminal",
    enter: item.enter ?? true,
    projectId: null,
  }));

function validTemplate(t: any): t is LayoutTemplate {
  if (t?.type === "leaf") return t.kind === null || t.kind === undefined || typeof t.kind === "string";
  return t?.type === "split" && (t.dir === "row" || t.dir === "col") && Array.isArray(t.children) && t.children.length > 0 && t.children.length <= 12 && t.children.every(validTemplate);
}

export const pluginLayouts = (): (SavedLayout & { plugin: string })[] =>
  contributions("layouts")
    .filter(({ item }) => validTemplate(item.template))
    .map(({ plugin, item }, i) => ({ id: `plugin:${plugin.id}:${i}`, name: item.name, template: normalizeTemplate(item.template), plugin: plugin.manifest!.name }));

function normalizeTemplate(t: LayoutTemplate): LayoutTemplate {
  if (t.type === "leaf") return { type: "leaf", kind: t.kind ?? null };
  const sizes = Array.isArray(t.sizes) && t.sizes.length === t.children.length && t.sizes.every((n) => n > 0) ? t.sizes : t.children.map(() => 1 / t.children.length);
  return { type: "split", dir: t.dir, sizes, children: t.children.map(normalizeTemplate) };
}

/** Saved layouts followed by the ones plugins add. */
export const allLayouts = (): SavedLayout[] => [...(store.state.settings.layouts ?? []), ...pluginLayouts()];

export const pluginStatusItems = () => contributions("statusItems");

// ---------- palette commands ----------

/** `${projectPath}`, `${projectName}`, `${branch}` and `${file}` in a command. */
export function expandVariables(text: string): string {
  const p = store.activeProject();
  const tab = p ? store.activeTab(p.id) : undefined;
  const pane = tab && allPanes(tab.root).find((x) => x.id === tab.focus);
  const root = p?.remote?.path ?? p?.path ?? "";
  const sep = root.includes("\\") ? "\\" : "/";
  const vars: Record<string, string> = {
    projectPath: root,
    projectName: p?.name ?? "",
    branch: (p && store.projectInfo[p.id]?.branch) ?? "",
    file: pane?.path ? `${root.replace(/[\\/]+$/, "")}${sep}${pane.path.replace(/\//g, sep)}` : "",
  };
  return text.replace(/\$\{(\w+)\}/g, (m, name: string) => vars[name] ?? m);
}

export async function runPluginCommand(c: PluginCommand) {
  const project = store.activeProject();
  if (!project) return store.notify(t("Open a project first"));
  const text = expandVariables(c.run);
  const enter = c.enter ?? true;
  if (c.in === "tab" || c.in === "split") {
    const s = await store.newSession(project.id, "shell", { where: c.in === "tab" ? "tab" : "right" });
    if (s) await typeInto(s, text, enter, 60);
    return;
  }
  await runQuickCommand({ id: "", label: c.label, text, action: c.in === "agent" ? "agent" : "terminal", enter, projectId: null });
}

const PREFIX = "plugin.";

function applyCommands() {
  for (let i = commands.length - 1; i >= 0; i--) if (commands[i].id.startsWith(PREFIX)) commands.splice(i, 1);
  for (const { plugin, item } of contributions("commands")) {
    commands.push({ id: `${PREFIX}${plugin.id}.${item.id}`, label: `${plugin.manifest!.name}: ${item.label}`, run: () => void runPluginCommand(item), when: () => !!store.state.activeProject });
  }
}

// ---------- agents ----------

let agentsSent = "";

/** Tells the daemon about plugin agents (it keeps them for resuming after a restart). Not
 *  before the first read of the plugin folder: an empty list then would drop the daemon's copy. */
export async function syncPluginAgents(force = false) {
  if (!store.connected() || !loaded()) return;
  const agents = contributions("agents").map(({ plugin, item }) => ({
    id: pluginAgentId(plugin.id, item.id),
    label: item.label,
    bin: item.bin,
    launch: item.launch ?? [],
    yolo: item.yolo ?? [],
    resume_latest: item.resumeLatest ?? [],
    prompt: item.prompt ?? "typed",
    acp: item.acp ?? [],
  }));
  // Agents saved from the ACP registry travel in the same list: `agent.set_custom` replaces it whole.
  for (const a of store.state.settings.registryAgents ?? []) {
    if (!agents.some((x) => x.id === a.id)) agents.push({ id: a.id, label: a.label, bin: a.bin, launch: [], yolo: [], resume_latest: [], prompt: "typed", acp: a.acp });
  }
  const key = JSON.stringify(agents);
  if (key === agentsSent && !force) return;
  try {
    await daemon("agent.set_custom", { agents });
    agentsSent = key;
  } catch (e) {
    store.notify(t("Plugin agents: {error}", { error: String(e) }));
  }
  await loadAgentPresets();
}

// ---------- loading ----------

function apply() {
  applyCommands();
  setPluginThemes(
    contributions("terminalThemes").map(({ plugin, item }): TerminalTheme => ({
      id: `${plugin.id}.${item.id}`,
      label: `${item.label} (${plugin.manifest!.name})`,
      dark: item.dark ?? true,
      colors: item.colors as TerminalTheme["colors"],
    })),
  );
  for (const v of allViews()) v.applySettings(store.state.settings);
}

/** Reads the plugin folder and applies what approved plugins contribute. */
export async function loadPlugins() {
  try {
    setPlugins(await pluginList());
  } catch (e) {
    console.warn("plugins:", e);
    setPlugins([]);
  }
  setLoaded(true);
  apply();
  await syncPluginAgents();
}
