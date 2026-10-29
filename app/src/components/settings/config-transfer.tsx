// Settings → Session Daemon → Export and import settings (loaded when the row is shown). The file
// format, allowlists and credential filter are in the app core (config_transfer.rs); planning, backup and
// undo are in lib/config-transfer.ts. Native file dialogs pick the path; the end-to-end tests call
// `exportTo` / `previewImportFile` with a path instead (exposed on `window.__kelpie.configTransfer`).
import { For, Show, createMemo, createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { store } from "../../lib/store";
import { defaultSettings } from "../../lib/default-settings";
import { daemon } from "../../lib/ipc";
import { errorText, msg, t } from "../../lib/i18n";
import { uid } from "../../lib/platform";
import { AUTOMATION_API } from "../../lib/automation-events";
import { loadVault } from "../../lib/ssh-hosts";
import { allViews } from "../../lib/terminal-host";
import { kindInfo } from "../../lib/agents";
import { CONFIG_SECTIONS, ImportFailed, applyImport, exportConfig, loadForPreview, planImport, readImportFile, type ConfigSection, type Current, type ImportPlan, type ImportRead, type RunChange, type TransferContext } from "../../lib/config-transfer";
import "./config-transfer.css";

const LABELS: Record<ConfigSection, string> = {
  settings: msg("Settings"),
  quickCommands: msg("Quick commands"),
  layouts: msg("Saved layouts"),
  automations: msg("Automations"),
  sshHosts: msg("SSH hosts"),
  plugins: msg("Plugin list"),
  agents: msg("Agent arguments and environment"),
};

/** What each run-deciding field is (setting names are shown as they are). */
const FIELDS: Record<string, string> = {
  args: msg("arguments"),
  permission: msg("permission mode"),
  text: msg("typed ($ terminal, > agent prompt, ⏎ Enter)"),
  kind: msg("agent"),
  prompt: msg("prompt"),
  cwd: msg("folder"),
  command: msg("command"),
  startup: msg("startup command"),
  remote: msg("panes on another machine"),
  target: msg("connects to"),
  identityFile: msg("key file"),
  when: msg("when it runs"),
  deliver: msg("when it finishes"),
  action: msg("does"),
  name: msg("name"),
  label: msg("name"),
  notes: msg("notes"),
  template: msg("panes"),
  trigger: msg("trigger"),
  hostname: msg("host name"),
  content: msg("text"),
};
/** `env:NAME` → "variable NAME", `args:--flag` → "arguments --flag", known fields translated, the rest as they are. */
const fieldLabel = (f: string) => {
  if (f.startsWith("env:")) return t("variable {name}", { name: f.slice(4) });
  const [base, detail] = f.includes(":") ? [f.slice(0, f.indexOf(":")), f.slice(f.indexOf(":") + 1)] : [f, ""];
  const label = FIELDS[base] ? t(FIELDS[base]) : base;
  return detail ? `${label} ${detail}` : label;
};

const automationsSupported = () => store.connected() && store.daemonApi() >= AUTOMATION_API;

/** Forgets a host's saved password in the OS credential store. */
let forgetPassword = (hostId: string): Promise<unknown> => invoke("ssh_secret_set", { kind: "password", target: hostId, value: "" });

/** End-to-end tests only (this module is reachable from them through `window.__kelpie`, which dev and
 *  KELPIE_E2E builds alone expose): records forgotten passwords instead of touching the OS store. */
export function replacePasswordStore(forget: (hostId: string) => Promise<unknown>) {
  forgetPassword = forget;
}

/** The window's side of an export or import. */
export function appContext(): TransferContext {
  return {
    settings: () => store.state.settings,
    defaults: defaultSettings,
    appState: () => JSON.parse(JSON.stringify(store.state)),
    projects: () => store.state.projects.map((p) => ({ id: p.id, path: p.path })),
    canSave: store.canPersist,
    replaceSettings: store.replaceSettings,
    saveNow: store.saveNow,
    automationsSupported,
    daemon: (method, params) => daemon(method, params ?? {}),
    invoke: (cmd, args) => invoke(cmd, args as Record<string, unknown> | undefined),
    forgetPassword: (hostId) => forgetPassword(hostId),
    newId: uid,
  };
}

interface Pending {
  path: string;
  read: ImportRead;
  cur: Current;
  /** Sections in the file that cannot be imported now, with the reason. */
  unavailable: Partial<Record<ConfigSection, string>>;
  chosen: ConfigSection[];
}

const [exportSections, setExportSections] = createSignal<ConfigSection[]>([...CONFIG_SECTIONS]);
const [includeValues, setIncludeValues] = createSignal(false);
const [pending, setPending] = createSignal<Pending | null>(null);
/** The user ticked "I've checked these commands" for the preview as it stands. */
const [runsChecked, setRunsChecked] = createSignal(false);
const [message, setMessage] = createSignal<{ text: string; error?: boolean } | null>(null);
const [busy, setBusy] = createSignal(false);

/** Sections that can be exported now (automations need the daemon's RPCs). */
const available = (k: ConfigSection) => k !== "automations" || automationsSupported();

/** The preview's plan of some sections (ids of new items are only placeholders here). */
const planOf = (p: Pending, sections: readonly ConfigSection[]) => planImport(p.read, sections, p.cur, { defaults: defaultSettings, newId: () => "" });
/** Every section that can be imported (for the counts), and the ticked ones (what Import would do). */
const fullPlan = createMemo(() => {
  const p = pending();
  return p && planOf(p, CONFIG_SECTIONS.filter((k) => p.read.doc.sections[k] !== undefined && !p.unavailable[k]));
});
const chosenPlan = createMemo(() => {
  const p = pending();
  return p && planOf(p, p.chosen);
});

/** Writes the export: the ticked sections and the values switch, unless given. */
export async function exportTo(path: string, wanted = exportSections().filter(available), withValues = includeValues()) {
  const r = await exportConfig(path, wanted, withValues, appContext());
  const lines = [t("Exported to {path}", { path: r.path })];
  if (r.foreign?.length) lines.push(t("Not exported, they use a network or device path: {names}", { names: r.foreign.map((f) => `${f.name} (${t(LABELS[f.section])})`).join(", ") }));
  setMessage({ text: lines.join("\n") });
  return r;
}

/** Reads a file and shows what importing it would do. */
export async function previewImportFile(path: string) {
  setMessage(null);
  setPending(null);
  setRunsChecked(false);
  const ctx = appContext();
  const read = await readImportFile(path, ctx);
  const inFile = CONFIG_SECTIONS.filter((k) => read.doc.sections[k] !== undefined);
  const { cur, unavailable } = await loadForPreview(inFile, ctx);
  setPending({ path, read, cur, unavailable, chosen: inFile.filter((k) => !unavailable[k]) });
  return chosenPlan();
}

/** Imports the ticked sections of the file being previewed. */
export async function importPending() {
  const p = pending();
  const shown = chosenPlan();
  if (!p || !shown) return null;
  const result = await applyImport(p.read, p.chosen, shown, appContext(), { runsChecked: runsChecked() });
  for (const v of allViews()) v.applySettings(store.state.settings);
  if (p.chosen.includes("sshHosts")) await loadVault().catch(() => {});
  if (p.chosen.includes("automations")) window.dispatchEvent(new Event("kelpie:automations"));
  const lines = [t("Imported. The previous configuration was copied to {path}.", { path: result.backup })];
  if (result.plan.automations.length) lines.push(t("Imported automations are off. Their commands need approving here, unless this computer already approved exactly the same command."));
  for (const r of result.refused) lines.push(t("Not imported, refused here: {name} ({error})", { name: r.name, error: errorText(r.error) }));
  if (p.chosen.includes("sshHosts")) lines.push(t("Saved passwords and key passphrases are not in the file: enter them again on the Hosts page."));
  for (const h of result.plan.hostTargets) lines.push(t("{alias} now connects to {target}: its saved password was forgotten.", { alias: h.alias, target: h.after }));
  for (const k of CONFIG_SECTIONS) lines.push(...fillNotes(k, result.plan));
  for (const e of result.plan.envNeeded) lines.push(t("{agent} needs values for: {names}", { agent: kindInfo(e.kind).label, names: e.names.join(", ") }));
  if (result.plan.missingPlugins.length) lines.push(t("Plugins to install from their source: {names}", { names: result.plan.missingPlugins.map((x) => x.name).join(", ") }));
  setPending(null);
  setMessage({ text: lines.join("\n") });
  return result;
}

function summary(k: ConfigSection, plan: ImportPlan): string {
  const s = plan.sections[k];
  if (!s) return "";
  const parts = [
    k === "settings"
      ? t("{n} of {total} change", { n: s.replace, total: s.total })
      : k === "plugins"
        ? t("{total} listed, {n} not installed here", { total: s.total, n: s.add })
        : t("{total} in the file: {add} new, {replace} replacing one with the same name", { total: s.total, add: s.add, replace: s.replace }),
  ];
  if (s.skipped) parts.push(t("{n} unusable entries left out", { n: s.skipped }));
  return parts.join(" · ");
}

/** Masked values the file had: taken from this computer, or (without a value here) left empty or the
 *  item left out. */
const fillNotes = (k: ConfigSection, plan: ImportPlan) => [
  ...plan.keptLocal
    .filter((f) => f.section === k)
    .map((f) => t("Masked in the file, this computer's value is kept: {item} ({fields})", { item: f.item, fields: f.fields.map(fieldLabel).join(", ") })),
  ...plan.fillIn
    .filter((f) => f.section === k)
    .map((f) =>
      f.leftOut
        ? t("Left out, masked in the file and not set here: {item} ({fields})", { item: f.item, fields: f.fields.map(fieldLabel).join(", ") })
        : t("Masked in the file and not set here, left empty: {item} ({fields}). Fill it in after the import.", { item: f.item, fields: f.fields.map(fieldLabel).join(", ") }),
    ),
  ...plan.argsNotImported
    .filter((f) => f.section === k)
    .map((f) =>
      f.section === "settings"
        ? t("Default shell: arguments not imported, a hidden value couldn't be filled in here (re-enter it in Terminal settings)")
        : f.section === "automations"
          ? t("{item}: arguments not imported, a hidden value couldn't be filled in here (re-enter it on the Automations page)", { item: f.item })
          : t("{item}: arguments not imported, a hidden value couldn't be filled in here (re-enter it in Agent settings)", { item: f.item }),
    ),
];

/** Items the file had that use a network or device path (never brought in). */
const foreignNotes = (k: ConfigSection, read: ImportRead) => {
  const names = (read.foreign ?? []).filter((f) => f.section === k).map((f) => f.name);
  return names.length ? [t("Left out, a network or device path: {names}", { names: names.join(", ") })] : [];
};

function notes(k: ConfigSection, plan: ImportPlan, read: ImportRead): string[] {
  return [...sectionNotes(k, plan), ...foreignNotes(k, read), ...fillNotes(k, plan)];
}

function sectionNotes(k: ConfigSection, plan: ImportPlan): string[] {
  const s = plan.sections.settings;
  switch (k) {
    case "settings":
      return [
        s?.changed?.length ? t("Changes: {names}", { names: s.changed.join(", ") }) : "",
        s?.invalid?.length ? t("Left out, value not allowed: {names}", { names: s.invalid.join(", ") }) : "",
        s?.unknown?.length ? t("Left out, not a setting this version takes: {names}", { names: s.unknown.join(", ") }) : "",
      ];
    case "automations":
      return [t("They come in switched off. Commands need approving here, unless this computer already approved exactly the same command.")];
    case "sshHosts":
      return [
        t("No passwords or key passphrases: enter them again on the Hosts page."),
        ...plan.hostTargets.map((h) =>
          h.forwards
            ? t("Connection target changed: {alias}, {before} → {after}. Its saved password will be forgotten and its {n} port forwards removed.", { alias: h.alias, before: h.before, after: h.after, n: h.forwards })
            : t("Connection target changed: {alias}, {before} → {after}. Its saved password will be forgotten.", { alias: h.alias, before: h.before, after: h.after }),
        ),
      ];
    case "plugins":
      return [t("Only a list: nothing is installed.")];
    case "agents":
      return [plan.envNeeded.length ? t("Variables without a value here: {names}", { names: plan.envNeeded.flatMap((e) => e.names).join(", ") }) : ""];
    default:
      return [];
  }
}

function RunRow(props: { change: RunChange }) {
  return (
    <div class="config-run" data-section={props.change.section} data-field={props.change.field}>
      <span class="config-run-what">{t(LABELS[props.change.section])} · {props.change.item} · {fieldLabel(props.change.field)}</span>
      <code class="config-run-before">{props.change.before ?? t("(none)")}</code>
      <span class="config-run-arrow">→</span>
      <code class="config-run-after">{props.change.after ?? t("(none)")}</code>
    </div>
  );
}

export default function ConfigTransfer() {
  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      setMessage({ text: e instanceof ImportFailed ? e.message : errorText(e), error: true });
    } finally {
      setBusy(false);
    }
  };
  const pickExport = () => run(async () => {
    const stamp = new Date().toISOString().slice(0, 10);
    const path = await save({ title: t("Export settings"), defaultPath: `kelpie-settings-${stamp}.json`, filters: [{ name: "JSON", extensions: ["json"] }] });
    if (path) await exportTo(path);
  });
  const pickImport = () => run(async () => {
    const path = await open({ title: t("Import settings"), multiple: false, directory: false, filters: [{ name: "JSON", extensions: ["json"] }] });
    if (typeof path === "string") await previewImportFile(path);
  });
  const toggle = (list: ConfigSection[], k: ConfigSection, on: boolean) => (on ? [...list.filter((x) => x !== k), k] : list.filter((x) => x !== k));
  const choose = (k: ConfigSection, on: boolean) => {
    const p = pending();
    if (!p) return;
    // What runs changes with the sections: the check is asked again.
    setRunsChecked(false);
    setPending({ ...p, chosen: CONFIG_SECTIONS.filter((x) => toggle(p.chosen, k, on).includes(x)) });
  };
  const inFile = () => CONFIG_SECTIONS.filter((k) => pending()?.read.doc.sections[k] !== undefined);
  const runs = () => chosenPlan()?.runs ?? [];
  const ready = () => !busy() && !!pending()?.chosen.length && (!runs().length || runsChecked());

  return (
    <div class="config-transfer">
      <div class="config-sections" data-part="export-sections">
        <For each={CONFIG_SECTIONS}>
          {(k) => (
            <label class="check-label" title={available(k) ? undefined : t("Needs a connected session daemon of this version")}>
              <input type="checkbox" data-section={k} disabled={!available(k)} checked={available(k) && exportSections().includes(k)} onChange={(e) => setExportSections(toggle(exportSections(), k, e.currentTarget.checked))} /> {t(LABELS[k])}
            </label>
          )}
        </For>
      </div>
      <label class="check-label">
        <input type="checkbox" data-part="include-values" checked={includeValues()} onChange={(e) => setIncludeValues(e.currentTarget.checked)} /> {t("Include values of agent environment variables")}
      </label>
      <Show when={includeValues()}>
        <div class="config-warning">{t("Values are written to the file as plain text. Values of variables with names like TOKEN, KEY or PASSWORD, and values that look like credentials, are left out; this filter can miss some, so check the file before you share it.")}</div>
      </Show>
      <div class="inline-fields">
        <button class="btn" data-act="export" disabled={busy() || !exportSections().some(available)} onClick={() => void pickExport()}>{t("Export…")}</button>
        <button class="btn" data-act="import" disabled={busy()} onClick={() => void pickImport()}>{t("Import…")}</button>
      </div>
      <Show when={pending()}>
        {(p) => (
          <div class="config-preview" data-part="import-preview">
            <div class="muted-inline">{t("From {path}", { path: p().path })}{p().read.doc.appVersion ? ` · Kelpie ${p().read.doc.appVersion}` : ""}</div>
            <For each={inFile()}>
              {(k) => (
                <div class="config-preview-row" data-section={k}>
                  <label class="check-label">
                    <input type="checkbox" disabled={!!p().unavailable[k]} checked={p().chosen.includes(k)} onChange={(e) => choose(k, e.currentTarget.checked)} /> {t(LABELS[k])}
                  </label>
                  <Show when={p().unavailable[k]} fallback={<span class="muted-inline" data-part="counts">{summary(k, fullPlan()!)}</span>}>
                    {(why) => <span class="config-error" data-part="unavailable">{t("Cannot be imported now: {reason}", { reason: why() })}</span>}
                  </Show>
                  <For each={p().unavailable[k] ? [] : notes(k, fullPlan()!, p().read).filter(Boolean)}>{(n) => <span class="muted-inline" data-part="note">{n}</span>}</For>
                </div>
              )}
            </For>
            <Show when={p().read.ignored.length}>
              <div class="muted-inline">{t("Ignored (not known to this version): {names}", { names: p().read.ignored.join(", ") })}</div>
            </Show>
            <Show when={runs().length}>
              <div class="config-runs" data-part="runs">
                <div class="config-runs-head">{t("These decide what programs run on this computer. Check every value:")}</div>
                <For each={runs()}>{(c) => <RunRow change={c} />}</For>
                <label class="check-label config-runs-check">
                  <input type="checkbox" data-part="runs-checked" checked={runsChecked()} onChange={(e) => setRunsChecked(e.currentTarget.checked)} /> {t("I've checked these commands; they will run on this computer.")}
                </label>
              </div>
            </Show>
            <div class="inline-fields">
              <button class="btn" disabled={busy()} onClick={() => setPending(null)}>{t("Cancel")}</button>
              <button class="btn primary" data-act="apply-import" disabled={!ready()} onClick={() => void run(importPending)}>{t("Import")}</button>
            </div>
          </div>
        )}
      </Show>
      <Show when={message()}>{(m) => <div class={m().error ? "config-error" : "config-result"} data-part="message">{m().text}</div>}</Show>
    </div>
  );
}
