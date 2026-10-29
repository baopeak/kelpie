// Settings export and import (Settings → Session Daemon → Export and import settings). The window
// gathers its own sections (settings, quick commands, layouts, agent profiles) and the daemon's
// automations; the app core (app/src-tauri/src/config_transfer.rs) adds the SSH hosts and the plugin
// list and owns the file: format, size cap, field allowlists and the credential filter, the same both ways.
//
// An import is planned against the current configuration (the preview), planned again just before it
// is applied (refused if what it shows moved), backed up, then applied hosts → app settings → automations
// → forgotten host passwords. A step that fails undoes the ones before it; an automation the daemon
// refuses is skipped and reported. Everything that decides what programs run is shown old → new in the
// preview and needs a separate tick. Imported automations are always off, and the file never carries an
// approval.
//
// No store import here: the window passes a `TransferContext` (tests pass a fake one).
import { t } from "./i18n";
import transferKeys from "./settings-transfer.json";
import type { AgentProfile, Automation, QuickCommand, SavedLayout, Settings } from "./types";
import type { SshHost, Vault } from "./ssh-hosts";

export const CONFIG_SECTIONS = ["settings", "quickCommands", "layouts", "automations", "sshHosts", "plugins", "agents"] as const;
export type ConfigSection = (typeof CONFIG_SECTIONS)[number];

/** Sections stored in the app state file (they save together). */
const APP_SECTIONS: readonly ConfigSection[] = ["settings", "quickCommands", "layouts", "agents"];

/** Settings a file may carry, and settings it never does (sections of their own, this computer's sign-ins
 *  and files, answers given here). One list with the app core; every default setting is in exactly one. */
export const SETTINGS_EXPORTED: readonly string[] = transferKeys.exported;
export const SETTINGS_EXCLUDED: readonly string[] = transferKeys.excluded;

/** Settings that decide what programs run: shown old → new before an import. */
const RUN_SETTINGS = ["shellArgs", "agentPermissions", "agentsScripting"];

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
  /** Forgets the password saved for a host id (the OS credential store). */
  forgetPassword(hostId: string): Promise<unknown>;
  newId(): string;
}

// ---------- settings ----------

const isPlain = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);

/** `value` if it has the JSON shape of the default `def` (same scalar type, a list of strings, the known
 *  keys of an object; a `null` default takes null or text); otherwise undefined. */
function shaped(value: unknown, def: unknown): unknown {
  if (def === null) return value === null || typeof value === "string" ? value : undefined;
  if (Array.isArray(def)) return Array.isArray(value) && value.length <= 500 && value.every((v) => typeof v === "string") ? value : undefined;
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

const range = (min: number, max: number) => (v: unknown) => (typeof v === "number" && Number.isFinite(v) && v >= min && v <= max ? v : undefined);
const oneOf = (...allowed: unknown[]) => (v: unknown) => (allowed.includes(v) ? v : undefined);
const text = (max: number) => (v: unknown) => (typeof v === "string" && v.length <= max ? v : undefined);

/** Values the settings page allows, beyond the type: what an import may set. */
const RULES: Record<string, (v: unknown) => unknown> = {
  fontFamily: text(500),
  fontSize: range(8, 28),
  lineHeight: range(1, 2),
  scrollback: range(500, 100_000),
  uiScale: oneOf(0.9, 1, 1.1, 1.2),
  rightPanelWidth: range(200, 700),
  sidebarWidth: range(160, 600),
  costCap: range(0, 1_000_000),
  hibernateMinutes: range(1, 1440),
  projectFilter: oneOf("all", "running", "attention"),
  projectSort: oneOf("manual", "recent"),
  theme: oneOf("dark", "light"),
  density: oneOf("comfortable", "compact"),
  agentPermissions: oneOf("manual", "yolo"),
  keepAwake: oneOf("off", "agents", "always"),
  aiAgent: oneOf("claude", "codex"),
  autoWorktree: oneOf("ask", "always", "never"),
  mascotMotion: oneOf("on", "system", "off"),
  resumeSessions: oneOf("shown", "all", "off"),
  language: oneOf("system", "en", "vi"),
  terminalTheme: text(200),
  integrations: (v) => {
    if (!isPlain(v) || typeof v.linear !== "boolean") return undefined;
    const hosts = shaped(v.gitlabHosts ?? [], []);
    const jira = v.jira ?? null;
    if (hosts === undefined || !(jira === null || (isPlain(jira) && typeof jira.url === "string" && typeof jira.email === "string"))) return undefined;
    return { linear: v.linear, jira: jira === null ? null : { url: jira.url, email: jira.email }, gitlabHosts: hosts };
  },
};

const fieldOf = (o: Settings | Partial<Settings>, k: string) => (o as Record<string, unknown>)[k];

/** A setting's value if it is allowed (shape, then enum or range), else undefined. */
function allowedSetting(k: string, v: unknown, defaults: Settings): unknown {
  return RULES[k] ? RULES[k](v) : shaped(v, fieldOf(defaults, k));
}

/** The settings an export writes: the portable keys, each with an allowed value. */
export function exportableSettings(s: Settings, defaults: Settings): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const k of SETTINGS_EXPORTED) {
    const v = allowedSetting(k, fieldOf(s, k) ?? fieldOf(defaults, k), defaults);
    out[k] = v === undefined ? fieldOf(defaults, k) : v;
  }
  return out;
}

/** The settings of a file this version may take: `unknown` keys are not portable or not known here,
 *  `invalid` ones have a value the settings page does not allow; both are left out. */
export function importableSettings(incoming: Record<string, unknown>, defaults: Settings): { values: Partial<Settings>; unknown: string[]; invalid: string[] } {
  const values: Record<string, unknown> = {};
  const unknown: string[] = [];
  const invalid: string[] = [];
  for (const [k, v] of Object.entries(incoming)) {
    if (!SETTINGS_EXPORTED.includes(k)) {
      unknown.push(k);
      continue;
    }
    const ok = allowedSetting(k, v, defaults);
    if (ok === undefined) invalid.push(k);
    else values[k] = ok;
  }
  return { values: values as Partial<Settings>, unknown, invalid };
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

/** An argument list as one line, quoting what needs it. */
export const argvText = (a: readonly string[]) => a.map((x) => (/[\s"']/.test(x) || !x ? JSON.stringify(x) : x)).join(" ");

/** A layout's panes that start on another machine: `claude @ dest:path`. */
function remotePanes(t: SavedLayout["template"]): string[] {
  if (t.type === "split") return t.children.flatMap(remotePanes);
  return t.target ? [`${t.kind ?? "-"} @ ${t.target.dest}${t.target.port ? `:${t.target.port}` : ""} ${t.target.path}`] : [];
}

/** Where a host connects: `user@host:port via jump`. */
export const hostTarget = (h: Pick<SshHost, "user" | "hostname" | "port" | "jump">) =>
  `${h.user ? `${h.user}@` : ""}${h.hostname}${h.port ? `:${h.port}` : ""}${h.jump ? ` via ${h.jump}` : ""}`;

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
  /** Settings: the keys whose value changes, and those left out (not portable or not allowed). */
  changed?: string[];
  unknown?: string[];
  invalid?: string[];
}

/** A value that decides what runs on this computer, as it is (`before`, null: none) and as the import
 *  makes it. `field`: a setting name, `args`, `permission`, `env:<NAME>`, `text`, `kind`, `prompt`,
 *  `cwd`, `command` or `startup`. */
export interface RunChange {
  section: ConfigSection;
  item: string;
  field: string;
  before: string | null;
  after: string | null;
}

/** A replaced host that now connects somewhere else: its saved password is forgotten and the port
 *  forwards bound to it are removed (they are in the backup). */
export interface HostTargetChange {
  id: string;
  alias: string;
  before: string;
  after: string;
  forwards: number;
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
  runs: RunChange[];
  hostTargets: HostTargetChange[];
}

/** What the user agreed to in the preview; an apply that would do anything else is refused. */
export const planShown = (p: Pick<ImportPlan, "sections" | "runs" | "hostTargets">) => ({ sections: p.sections, runs: p.runs, hostTargets: p.hostTargets });

const show = (v: unknown): string | null => {
  const s = v === undefined || v === null ? "" : Array.isArray(v) ? argvText(v as string[]) : String(v);
  return s === "" ? null : s;
};

export function planImport(read: ImportRead, chosen: readonly ConfigSection[], cur: Current, opts: { defaults: Settings; newId: () => string }): ImportPlan {
  const s = read.doc.sections;
  const want = (k: ConfigSection) => chosen.includes(k) && s[k] !== undefined;
  const skippedIn = (k: ConfigSection) => read.skipped[k] ?? 0;
  const plan: ImportPlan = { sections: {}, settings: null, vault: null, automations: [], missingPlugins: [], envNeeded: [], runs: [], hostTargets: [] };
  const run = (section: ConfigSection, item: string, field: string, before: unknown, after: unknown) => {
    const [b, a] = [show(before), show(after)];
    if (b !== a) plan.runs.push({ section, item, field, before: b, after: a });
  };
  const projectFor = (id: string | null | undefined, cwd?: string) =>
    (id && cur.projects.some((p) => p.id === id) ? id : null) ?? (cwd ? cur.projects.find((p) => samePath(p.path, cwd))?.id ?? null : null);

  let next: Settings = JSON.parse(JSON.stringify(cur.settings));
  if (want("settings")) {
    const { values, unknown, invalid } = importableSettings(s.settings!, opts.defaults);
    const changed = Object.keys(values).filter((k) => !same(fieldOf(cur.settings, k), fieldOf(values, k)));
    next = { ...next, ...values };
    plan.sections.settings = { total: Object.keys(s.settings!).length, add: 0, replace: changed.length, skipped: unknown.length + invalid.length, changed, unknown, invalid };
    for (const k of RUN_SETTINGS) if (changed.includes(k)) run("settings", k, k, fieldOf(cur.settings, k), fieldOf(values, k));
  }
  if (want("quickCommands")) {
    const m = mergeList<QuickCommand>(next.quickCommands ?? [], s.quickCommands!, {
      id: (x) => x.id, name: (x) => x.label,
      adopt: (inc, _loc, id) => ({ id, label: inc.label, text: inc.text, action: inc.action, enter: inc.enter, projectId: projectFor(inc.projectId) }),
    }, opts.newId);
    next.quickCommands = m.list;
    plan.sections.quickCommands = { total: s.quickCommands!.length, add: m.add, replace: m.replace, skipped: skippedIn("quickCommands") };
    // What a click types and where ("$" a terminal, ">" the agent), and whether Enter follows.
    const typed = (q: QuickCommand) => `${q.action === "agent" ? ">" : "$"} ${q.text}${q.enter ? " ⏎" : ""}`;
    for (const { next: q, replaces } of m.pairs) run("quickCommands", q.label, "text", replaces && typed(replaces), typed(q));
  }
  if (want("layouts")) {
    const m = mergeList<SavedLayout>(next.layouts ?? [], s.layouts!, { id: (x) => x.id, name: (x) => x.name, adopt: (inc, _loc, id) => ({ id, name: inc.name, template: inc.template }) }, opts.newId);
    next.layouts = m.list;
    plan.sections.layouts = { total: s.layouts!.length, add: m.add, replace: m.replace, skipped: skippedIn("layouts") };
    // Opening a layout starts its panes; one on another machine connects where the file says.
    const remote = (l: SavedLayout | null) => (l ? remotePanes(l.template).join(", ") : null);
    for (const { next: l, replaces } of m.pairs) run("layouts", l.name, "remote", replaces && remote(replaces), remote(l));
  }
  if (want("agents")) {
    const profiles = (next.profiles ?? []).slice();
    let add = 0;
    let replace = 0;
    for (const a of s.agents!) {
      const at = profiles.findIndex((p) => p.kind === a.kind);
      const old = at >= 0 ? profiles[at] : undefined;
      const { env, needed } = mergeEnv(old?.env, a.env);
      const profile: AgentProfile = { kind: a.kind, label: a.label, args: a.args, ...(a.permission ? { permission: a.permission } : {}), ...(Object.keys(env).length ? { env } : {}) };
      if (old) {
        profiles[at] = profile;
        replace++;
      } else {
        profiles.push(profile);
        add++;
      }
      if (needed.length) plan.envNeeded.push({ kind: a.kind, names: needed });
      run("agents", a.label || a.kind, "args", old?.args, profile.args);
      run("agents", a.label || a.kind, "permission", old?.permission, profile.permission);
      for (const name of Object.keys(env).sort()) run("agents", a.label || a.kind, `env:${name}`, old?.env?.[name], env[name]);
    }
    next.profiles = profiles;
    plan.sections.agents = { total: s.agents!.length, add, replace, skipped: skippedIn("agents") };
  }
  if (APP_SECTIONS.some(want)) plan.settings = next;

  if (want("sshHosts") && cur.vault && s.sshHosts!.length) {
    const moved = (inc: SshHost, loc: SshHost | null) => !!loc && hostTarget(inc) !== hostTarget(loc);
    const m = mergeList<SshHost>(cur.vault.hosts, s.sshHosts!.map((h) => ({ ...h, os: "", lastConnected: 0 })), {
      id: (x) => x.id, name: (x) => x.alias,
      // A replaced host keeps what this computer recorded about it while it still connects to the same place.
      // Agent forwarding never comes from a file: off for a new host, or one that now connects elsewhere.
      adopt: (inc, loc, id) => {
        const kept = !!loc && !moved(inc, loc);
        return { ...inc, id, os: kept ? loc!.os : "", lastConnected: kept ? loc!.lastConnected : 0, forwardAgent: kept ? loc!.forwardAgent : false };
      },
    }, opts.newId);
    const changedIds = new Set<string>();
    for (const { next: h, replaces } of m.pairs) {
      if (replaces && moved(h, replaces)) {
        changedIds.add(h.id);
        plan.hostTargets.push({ id: h.id, alias: h.alias, before: hostTarget(replaces), after: hostTarget(h), forwards: cur.vault.forwards.filter((f) => f.hostId === h.id).length });
      }
      run("sshHosts", h.alias, "startup", replaces?.startup, h.startup);
    }
    // Port forwards of a host that now connects elsewhere would tunnel through the new destination.
    plan.vault = { ...cur.vault, hosts: m.list, forwards: cur.vault.forwards.filter((f) => !changedIds.has(f.hostId)) };
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
      const command = (a: Automation | null) => (a?.action === "shell" && a.shell ? argvText([a.shell.program, ...a.shell.argv]) : null);
      const agent = (a: Automation | null) => (a && a.action !== "shell" ? a.kind : null);
      run("automations", automation.name, "kind", agent(replaces), agent(automation));
      run("automations", automation.name, "command", command(replaces), command(automation));
      run("automations", automation.name, "args", replaces?.args, automation.args);
      run("automations", automation.name, "prompt", replaces?.prompt, automation.prompt);
      run("automations", automation.name, "cwd", replaces?.cwd, automation.cwd);
      for (const name of Object.keys(env).sort()) run("automations", automation.name, `env:${name}`, replaces?.env?.[name], env[name]);
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

/** For the preview: each section read on its own, so one that cannot be read (a damaged hosts file, an
 *  older daemon) is shown as unavailable with the reason instead of blocking the others. */
export async function loadForPreview(sections: readonly ConfigSection[], ctx: TransferContext): Promise<{ cur: Current; unavailable: Partial<Record<ConfigSection, string>> }> {
  const cur = await loadCurrent([], ctx);
  const unavailable: Partial<Record<ConfigSection, string>> = {};
  for (const k of sections) {
    try {
      const one = await loadCurrent([k], ctx);
      cur.vault ??= one.vault;
      cur.automations ??= one.automations;
      cur.plugins ??= one.plugins;
    } catch (e) {
      unavailable[k] = String(e).replace(/^Error:\s*/, "");
    }
  }
  return { cur, unavailable };
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
  /** Automations the daemon would not take here (an agent not installed, a script not there…). */
  refused: { name: string; error: string }[];
}

/** Applies the chosen sections of `read` if they still plan as `previewed`, and only with `runsChecked`
 *  when that changes what programs run. Every change made before a failure is undone; a backup of the
 *  previous configuration is kept either way. */
export async function applyImport(read: ImportRead, chosen: readonly ConfigSection[], previewed: Pick<ImportPlan, "sections" | "runs" | "hostTargets">, ctx: TransferContext, opts: { runsChecked: boolean }): Promise<ImportResult> {
  const planOpts = { defaults: ctx.defaults, newId: ctx.newId };
  const cur = await loadCurrent(chosen, ctx);
  const plan = planImport(read, chosen, cur, planOpts);
  const changed = () => new Error(t("The configuration changed since the preview; review the import again."));
  if (!same(planShown(plan), planShown(previewed))) throw changed();
  if (plan.runs.length && !opts.runsChecked) throw new Error(t("Check the commands this import would run, then tick the box to confirm."));
  if (plan.settings && !ctx.canSave()) throw new Error(t("App state cannot be saved in this session."));
  const backup = await ctx.invoke<string>("config_backup", { appState: ctx.appState(), automations: cur.automations ?? [] });
  const undo: (() => Promise<unknown>)[] = [];
  const refused: ImportResult["refused"] = [];
  try {
    if (plan.vault && cur.vault) {
      const before = cur.vault;
      const written = await ctx.invoke<Vault>("config_hosts_write", { expected: before, vault: plan.vault });
      undo.push(() => ctx.invoke("config_hosts_write", { expected: written, vault: before }));
    }
    if (plan.settings) {
      // Planned again from the settings as they are now: a change made while the backup was written is kept.
      const before: Settings = JSON.parse(JSON.stringify(ctx.settings()));
      const again = planImport(read, chosen, { ...cur, settings: before }, planOpts);
      if (!same(planShown(again), planShown(previewed))) throw changed();
      undo.push(async () => {
        ctx.replaceSettings(before);
        await ctx.saveNow();
      });
      ctx.replaceSettings(again.settings!);
      await ctx.saveNow();
    }
    for (const op of plan.automations) {
      const old = op.replaces;
      let saved: Automation;
      try {
        saved = await ctx.daemon<Automation>("automation.save", op.next);
      } catch (e) {
        // Refused by the daemon (an agent or script not on this computer…): skipped and reported. A daemon
        // that no longer answers fails the import instead.
        await ctx.daemon("automation.state").catch(() => {
          throw e;
        });
        refused.push({ name: op.next.name, error: String(e).replace(/^Error:\s*/, "") });
        continue;
      }
      if (old) {
        undo.push(async () => {
          await ctx.daemon("automation.save", old);
          // The user's own approval of exactly that command, given on this computer.
          if (old.command_approved && old.command_hash) await ctx.daemon("automation.approve", { id: old.id, hash: old.command_hash });
        });
      } else {
        undo.push(() => ctx.daemon("automation.delete", { id: saved.id }));
      }
    }
    // Last, once nothing else can fail: a host that now connects elsewhere loses this computer's password.
    for (const h of plan.hostTargets) await ctx.forgetPassword(h.id);
  } catch (e) {
    const failures: string[] = [];
    for (const u of undo.reverse()) await u().catch((x) => failures.push(String(x)));
    throw new ImportFailed(String(e).replace(/^Error:\s*/, ""), backup, failures);
  }
  return { backup, plan, refused };
}
