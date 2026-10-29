// Settings export and import (Settings → Session Daemon → Export and import settings). The window
// gathers its own sections (settings, quick commands, layouts, agent profiles) and the daemon's
// automations; the app core (app/src-tauri/src/config_transfer.rs) adds the SSH hosts and the plugin
// list and owns the file: format, size cap, field allowlists and the secret filter, the same both ways.
//
// An import is planned against the current configuration (the preview), planned again just before it
// is applied (refused if the counts moved), backed up, then applied hosts → app settings → automations.
// A step that fails undoes the ones before it. Imported automations are always off and their commands
// unapproved: the daemon never takes an approval from an editor, and the file has none.
//
// No store import here: the window passes a `TransferContext` (tests pass a fake one).
import { t } from "./i18n";
import type { AgentProfile, Automation, QuickCommand, SavedLayout, Settings } from "./types";
import type { SshHost, Vault } from "./ssh-hosts";

export const CONFIG_SECTIONS = ["settings", "quickCommands", "layouts", "automations", "sshHosts", "plugins", "agents"] as const;
export type ConfigSection = (typeof CONFIG_SECTIONS)[number];

/** Sections stored in the app state file (they save together). */
const APP_SECTIONS: readonly ConfigSection[] = ["settings", "quickCommands", "layouts", "agents"];

/** Settings never exported or imported: sections of their own, this computer's sign-ins and files,
 *  answers given here. (The app core refuses the same keys.) */
export const SETTINGS_EXCLUDED = ["profiles", "quickCommands", "layouts", "accounts", "activeAccounts", "sshConfig", "rootYoloOk", "skippedUpdate"];

export interface EnvEntry {
  name: string;
  /** Absent: the value was not exported (not asked for, or it looks like a secret). */
  value?: string;
}
export interface AgentEntry {
  kind: string;
  label: string;
  args: string[];
  permission?: string;
  env: EnvEntry[];
}
export type AutomationEntry = Omit<Automation, "env" | "last_run" | "last_session" | "last_error" | "next_run"> & { env: EnvEntry[] };
export interface PluginEntry {
  id: string;
  name: string;
  version?: string;
}
export type HostEntry = Omit<SshHost, "os" | "lastConnected">;

export interface ConfigSections {
  settings?: Record<string, unknown>;
  quickCommands?: QuickCommand[];
  layouts?: SavedLayout[];
  automations?: AutomationEntry[];
  sshHosts?: HostEntry[];
  plugins?: PluginEntry[];
  agents?: AgentEntry[];
}

export interface ConfigDoc {
  kind: "kelpie-config";
  version: 1;
  exportedAt: string | null;
  appVersion: string | null;
  sections: ConfigSections;
}

/** A checked file (`config_import_read`): its cleaned document, items left out, sections ignored. */
export interface ImportRead {
  doc: ConfigDoc;
  skipped: Partial<Record<ConfigSection, number>>;
  ignored: string[];
}

/** An automation as the daemon lists it. */
export type ListedAutomation = Automation & { command_hash?: string; command_approved?: boolean };

/** What the window offers an export or import (the real one is built from the store). */
export interface TransferContext {
  settings(): Settings;
  defaults: Settings;
  /** The whole app state, for the backup. */
  appState(): unknown;
  projects(): { id: string; path: string }[];
  canSave(): boolean;
  replaceSettings(next: Settings): void;
  saveNow(): Promise<void>;
  /** A connected daemon with the automation RPCs this needs (API level checked by the caller). */
  automationsSupported(): boolean;
  daemon<T>(method: string, params?: object): Promise<T>;
  invoke<T>(cmd: string, args?: object): Promise<T>;
  newId(): string;
}

// ---------- settings ----------

const isPlain = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);

/** `value` if it has the JSON shape of the default `def` (same scalar type, a list of strings, the known
 *  keys of an object; a `null` default takes null, text or an object of texts); otherwise undefined. */
function shaped(value: unknown, def: unknown): unknown {
  if (def === null) return value === null || typeof value === "string" || (isPlain(value) && Object.values(value).every((v) => typeof v === "string")) ? value : undefined;
  if (Array.isArray(def)) return Array.isArray(value) && value.every((v) => typeof v === "string") ? value : undefined;
  if (isPlain(def)) {
    if (!isPlain(value)) return undefined;
    const out: Record<string, unknown> = {};
    for (const [k, d] of Object.entries(def)) {
      if (!(k in value)) {
        out[k] = d;
        continue;
      }
      const v = shaped(value[k], d);
      if (v === undefined) return undefined;
      out[k] = v;
    }
    return out;
  }
  if (typeof def === "number") return typeof value === "number" && Number.isFinite(value) ? value : undefined;
  return typeof value === typeof def ? value : undefined;
}

const settingKeys = (defaults: Settings) => Object.keys(defaults).filter((k) => !SETTINGS_EXCLUDED.includes(k));

/** The settings an export writes: every known, portable key with a well-formed value. */
export function exportableSettings(s: Settings, defaults: Settings): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const k of settingKeys(defaults)) {
    const def = (defaults as unknown as Record<string, unknown>)[k];
    const v = shaped((s as unknown as Record<string, unknown>)[k] ?? def, def);
    out[k] = v === undefined ? def : v;
  }
  return out;
}

/** The settings of a file this version knows, with the right shape; the other keys are named in `skipped`. */
export function importableSettings(incoming: Record<string, unknown>, defaults: Settings): { values: Partial<Settings>; skipped: string[] } {
  const keys = settingKeys(defaults);
  const values: Record<string, unknown> = {};
  const skipped: string[] = [];
  for (const [k, v] of Object.entries(incoming)) {
    const ok = keys.includes(k) ? shaped(v, (defaults as unknown as Record<string, unknown>)[k]) : undefined;
    if (ok === undefined) skipped.push(k);
    else values[k] = ok;
  }
  return { values: values as Partial<Settings>, skipped };
}

// ---------- merging ----------

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b);
const samePath = (a: string, b: string) => a.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase() === b.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();

interface Merged<T> {
  list: T[];
  add: number;
  replace: number;
  /** Each incoming item as merged, with the local item it replaced. */
  pairs: { next: T; replaces: T | null }[];
}

/** Incoming items replace local ones with the same id, else the same name (keeping the local id);
 *  the rest are added (an empty id gets a new one). */
function mergeList<T>(local: T[], incoming: T[], key: { id: (x: T) => string; name: (x: T) => string; adopt: (inc: T, loc: T | null, id: string) => T }, newId: () => string): Merged<T> {
  const list = local.slice();
  const used = new Set<number>();
  const pairs: Merged<T>["pairs"] = [];
  let add = 0;
  let replace = 0;
  for (const inc of incoming) {
    const id = key.id(inc);
    const name = key.name(inc).trim();
    let at = id ? list.findIndex((l, i) => !used.has(i) && key.id(l) === id) : -1;
    if (at < 0 && name) at = list.findIndex((l, i) => !used.has(i) && key.name(l).trim() === name);
    if (at >= 0) {
      const next = key.adopt(inc, list[at], key.id(list[at]));
      pairs.push({ next, replaces: list[at] });
      list[at] = next;
      used.add(at);
      replace++;
    } else {
      const next = key.adopt(inc, null, id || newId());
      list.push(next);
      used.add(list.length - 1);
      pairs.push({ next, replaces: null });
      add++;
    }
  }
  return { list, add, replace, pairs };
}

/** Environment from a file over a local one: values replace; a name without a value keeps the local
 *  value, or is reported as needing one on this computer. */
function mergeEnv(local: Record<string, string> | undefined, entries: EnvEntry[]): { env: Record<string, string>; needed: string[] } {
  const env: Record<string, string> = { ...(local ?? {}) };
  const needed: string[] = [];
  for (const e of entries) {
    if (e.value !== undefined) env[e.name] = e.value;
    else if (!(e.name in env)) needed.push(e.name);
  }
  return { env, needed };
}

// ---------- the plan (preview) ----------

export interface Current {
  settings: Settings;
  projects: { id: string; path: string }[];
  vault: Vault | null;
  automations: ListedAutomation[] | null;
  plugins: string[] | null;
}

export interface SectionPlan {
  /** Items (settings: keys) in the file. */
  total: number;
  add: number;
  /** Items replacing one with the same id or name (settings: values that change). */
  replace: number;
  /** Entries the file had that could not be used. */
  skipped: number;
  /** Settings: the keys whose value changes. */
  changed?: string[];
}

export interface ImportPlan {
  sections: Partial<Record<ConfigSection, SectionPlan>>;
  /** The whole new settings object, when an app-state section was chosen. */
  settings: Settings | null;
  vault: Vault | null;
  automations: { next: Automation; replaces: ListedAutomation | null }[];
  /** Listed plugins that are not installed here (the file has no source to install them from). */
  missingPlugins: PluginEntry[];
  /** Agent variables the file names without a value and this computer does not have. */
  envNeeded: { kind: string; names: string[] }[];
}

export function planImport(read: ImportRead, chosen: readonly ConfigSection[], cur: Current, opts: { defaults: Settings; newId: () => string }): ImportPlan {
  const s = read.doc.sections;
  const want = (k: ConfigSection) => chosen.includes(k) && s[k] !== undefined;
  const skippedIn = (k: ConfigSection) => read.skipped[k] ?? 0;
  const plan: ImportPlan = { sections: {}, settings: null, vault: null, automations: [], missingPlugins: [], envNeeded: [] };
  const projectFor = (id: string | null | undefined, cwd?: string) =>
    (id && cur.projects.some((p) => p.id === id) ? id : null) ?? (cwd ? cur.projects.find((p) => samePath(p.path, cwd))?.id ?? null : null);

  let next: Settings = JSON.parse(JSON.stringify(cur.settings));
  if (want("settings")) {
    const { values, skipped } = importableSettings(s.settings!, opts.defaults);
    const changed = Object.keys(values).filter((k) => !same((cur.settings as unknown as Record<string, unknown>)[k], (values as Record<string, unknown>)[k]));
    next = { ...next, ...values };
    plan.sections.settings = { total: Object.keys(s.settings!).length, add: 0, replace: changed.length, skipped: skipped.length, changed };
  }
  if (want("quickCommands")) {
    const m = mergeList<QuickCommand>(next.quickCommands ?? [], s.quickCommands!, {
      id: (x) => x.id, name: (x) => x.label,
      adopt: (inc, _loc, id) => ({ id, label: inc.label, text: inc.text, action: inc.action, enter: inc.enter, projectId: projectFor(inc.projectId) }),
    }, opts.newId);
    next.quickCommands = m.list;
    plan.sections.quickCommands = { total: s.quickCommands!.length, add: m.add, replace: m.replace, skipped: skippedIn("quickCommands") };
  }
  if (want("layouts")) {
    const m = mergeList<SavedLayout>(next.layouts ?? [], s.layouts!, { id: (x) => x.id, name: (x) => x.name, adopt: (inc, _loc, id) => ({ id, name: inc.name, template: inc.template }) }, opts.newId);
    next.layouts = m.list;
    plan.sections.layouts = { total: s.layouts!.length, add: m.add, replace: m.replace, skipped: skippedIn("layouts") };
  }
  if (want("agents")) {
    const profiles = (next.profiles ?? []).slice();
    let add = 0;
    let replace = 0;
    for (const a of s.agents!) {
      const at = profiles.findIndex((p) => p.kind === a.kind);
      const { env, needed } = mergeEnv(at >= 0 ? profiles[at].env : undefined, a.env);
      const profile: AgentProfile = { kind: a.kind, label: a.label, args: a.args, ...(a.permission ? { permission: a.permission } : {}), ...(Object.keys(env).length ? { env } : {}) };
      if (at >= 0) {
        profiles[at] = profile;
        replace++;
      } else {
        profiles.push(profile);
        add++;
      }
      if (needed.length) plan.envNeeded.push({ kind: a.kind, names: needed });
    }
    next.profiles = profiles;
    plan.sections.agents = { total: s.agents!.length, add, replace, skipped: skippedIn("agents") };
  }
  if (APP_SECTIONS.some(want)) plan.settings = next;

  if (want("sshHosts") && cur.vault && s.sshHosts!.length) {
    const m = mergeList<SshHost>(cur.vault.hosts, s.sshHosts!.map((h) => ({ ...h, os: "", lastConnected: 0 })), {
      id: (x) => x.id, name: (x) => x.alias,
      // A replaced host keeps what this computer recorded about it.
      adopt: (inc, loc, id) => ({ ...inc, id, os: loc?.os ?? "", lastConnected: loc?.lastConnected ?? 0 }),
    }, opts.newId);
    plan.vault = { ...cur.vault, hosts: m.list };
    plan.sections.sshHosts = { total: s.sshHosts!.length, add: m.add, replace: m.replace, skipped: skippedIn("sshHosts") };
  }

  if (want("automations") && cur.automations) {
    const m = mergeList<ListedAutomation>(cur.automations, s.automations!.map((a) => ({ ...a, env: {} }) as unknown as ListedAutomation), {
      id: (x) => x.id, name: (x) => x.name, adopt: (inc, _loc, id) => ({ ...inc, id }),
    }, opts.newId);
    s.automations!.forEach((entry, i) => {
      const { next: merged, replaces } = m.pairs[i];
      // The file's variables; one without a value keeps the replaced automation's value, if it had one.
      const env: Record<string, string> = {};
      for (const e of entry.env) {
        const v = e.value ?? replaces?.env?.[e.name];
        if (v !== undefined) env[e.name] = v;
      }
      const automation: Automation = {
        ...(entry as unknown as Automation),
        id: merged.id,
        env,
        enabled: false,
        project_id: projectFor(entry.project_id, entry.cwd),
        last_run: 0, last_session: null, last_error: null, next_run: 0,
      };
      plan.automations.push({ next: automation, replaces });
    });
    plan.sections.automations = { total: s.automations!.length, add: m.add, replace: m.replace, skipped: skippedIn("automations") };
  }

  if (want("plugins") && cur.plugins) {
    plan.missingPlugins = s.plugins!.filter((p) => !cur.plugins!.includes(p.id));
    plan.sections.plugins = { total: s.plugins!.length, add: plan.missingPlugins.length, replace: 0, skipped: skippedIn("plugins") };
  }
  return plan;
}

// ---------- reading, exporting, applying ----------

/** The configuration an import works against. A read that fails stops the import: it is never taken for
 *  an empty configuration that the import would then save over. */
export async function loadCurrent(chosen: readonly ConfigSection[], ctx: TransferContext): Promise<Current> {
  const cur: Current = { settings: JSON.parse(JSON.stringify(ctx.settings())), projects: ctx.projects(), vault: null, automations: null, plugins: null };
  if (chosen.includes("sshHosts")) cur.vault = await ctx.invoke<Vault>("config_hosts_current");
  if (chosen.includes("automations")) {
    if (!ctx.automationsSupported()) throw new Error(t("Automations need a connected session daemon of this version."));
    const state = await ctx.daemon<{ error: string | null }>("automation.state");
    if (state.error) throw new Error(state.error);
    cur.automations = await ctx.daemon<ListedAutomation[]>("automation.list");
  }
  if (chosen.includes("plugins")) cur.plugins = (await ctx.invoke<{ id: string }[]>("plugin_list")).map((p) => p.id);
  return cur;
}

export const readImportFile = (path: string, ctx: Pick<TransferContext, "invoke">) => ctx.invoke<ImportRead>("config_import_read", { path });

/** Writes the chosen sections to `path`. Values of agent variables only with `includeEnvValues`. */
export async function exportConfig(path: string, wanted: readonly ConfigSection[], includeEnvValues: boolean, ctx: TransferContext) {
  const s = ctx.settings();
  const sections: Record<string, unknown> = {};
  if (wanted.includes("settings")) sections.settings = exportableSettings(s, ctx.defaults);
  if (wanted.includes("quickCommands")) sections.quickCommands = s.quickCommands ?? [];
  if (wanted.includes("layouts")) sections.layouts = s.layouts ?? [];
  if (wanted.includes("agents")) sections.agents = s.profiles ?? [];
  if (wanted.includes("automations")) {
    if (!ctx.automationsSupported()) throw new Error(t("Automations need a connected session daemon of this version."));
    sections.automations = await ctx.daemon<Automation[]>("automation.list");
  }
  return ctx.invoke<{ path: string; counts: Partial<Record<ConfigSection, number>> }>("config_export", { path, sections, wanted, includeEnvValues, exportedAt: new Date().toISOString() });
}

export class ImportFailed extends Error {
  constructor(public reason: string, public backup: string, public undoFailures: string[]) {
    super(undoFailures.length ? t("The import failed ({error}) and not every change could be undone; the previous configuration is in {path}.", { error: reason, path: backup }) : t("The import failed and nothing was changed: {error}", { error: reason }));
  }
}

export interface ImportResult {
  backup: string;
  plan: ImportPlan;
}

/** Applies the chosen sections of `read` if they still plan as `previewed`. Every change made before a
 *  failure is undone; a backup of the previous configuration is kept either way. */
export async function applyImport(read: ImportRead, chosen: readonly ConfigSection[], previewed: ImportPlan, ctx: TransferContext): Promise<ImportResult> {
  const cur = await loadCurrent(chosen, ctx);
  const plan = planImport(read, chosen, cur, { defaults: ctx.defaults, newId: ctx.newId });
  if (!same(plan.sections, previewed.sections)) throw new Error(t("The configuration changed since the preview; review the import again."));
  if (plan.settings && !ctx.canSave()) throw new Error(t("App state cannot be saved in this session."));
  const backup = await ctx.invoke<string>("config_backup", { appState: ctx.appState(), automations: cur.automations ?? [] });
  const undo: (() => Promise<unknown>)[] = [];
  try {
    if (plan.vault && cur.vault) {
      const before = cur.vault;
      const written = await ctx.invoke<Vault>("config_hosts_write", { expected: before, vault: plan.vault });
      undo.push(() => ctx.invoke("config_hosts_write", { expected: written, vault: before }));
    }
    if (plan.settings) {
      const before = cur.settings;
      undo.push(async () => {
        ctx.replaceSettings(before);
        await ctx.saveNow();
      });
      ctx.replaceSettings(plan.settings);
      await ctx.saveNow();
    }
    for (const op of plan.automations) {
      const old = op.replaces;
      if (old) {
        await ctx.daemon("automation.save", op.next);
        undo.push(async () => {
          await ctx.daemon("automation.save", old);
          // The user's own approval of exactly that command, given on this computer.
          if (old.command_approved && old.command_hash) await ctx.daemon("automation.approve", { id: old.id, hash: old.command_hash });
        });
      } else {
        const saved = await ctx.daemon<Automation>("automation.save", op.next);
        undo.push(() => ctx.daemon("automation.delete", { id: saved.id }));
      }
    }
  } catch (e) {
    const failures: string[] = [];
    for (const u of undo.reverse()) await u().catch((x) => failures.push(String(x)));
    throw new ImportFailed(String(e).replace(/^Error:\s*/, ""), backup, failures);
  }
  return { backup, plan };
}
