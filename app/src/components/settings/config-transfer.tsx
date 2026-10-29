// Settings → Session Daemon → Export and import settings (loaded when the row is shown). The file
// format, allowlists and secret filter are in the app core (config_transfer.rs); planning, backup and
// undo are in lib/config-transfer.ts. Native file dialogs pick the path; the end-to-end tests call
// `exportTo` / `previewImportFile` with a path instead (exposed on `window.__kelpie.configTransfer`).
import { For, Show, createSignal } from "solid-js";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { defaultSettings, store } from "../../lib/store";
import { daemon } from "../../lib/ipc";
import { errorText, msg, t } from "../../lib/i18n";
import { uid } from "../../lib/platform";
import { AUTOMATION_API } from "../../lib/automation-events";
import { loadVault } from "../../lib/ssh-hosts";
import { allViews } from "../../lib/terminal-host";
import { kindInfo } from "../../lib/agents";
import { CONFIG_SECTIONS, ImportFailed, applyImport, exportConfig, loadCurrent, planImport, readImportFile, type ConfigSection, type ImportPlan, type ImportRead, type TransferContext } from "../../lib/config-transfer";
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

const automationsSupported = () => store.connected() && store.daemonApi() >= AUTOMATION_API;

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
    newId: uid,
  };
}

const [exportSections, setExportSections] = createSignal<ConfigSection[]>([...CONFIG_SECTIONS]);
const [includeValues, setIncludeValues] = createSignal(false);
const [pending, setPending] = createSignal<{ path: string; read: ImportRead; plan: ImportPlan; chosen: ConfigSection[] } | null>(null);
const [message, setMessage] = createSignal<{ text: string; error?: boolean } | null>(null);
const [busy, setBusy] = createSignal(false);

/** Sections that can be exported now (automations need the daemon's RPCs). */
const available = (k: ConfigSection) => k !== "automations" || automationsSupported();

/** Writes the export: the ticked sections and the values switch, unless given. */
export async function exportTo(path: string, wanted = exportSections().filter(available), withValues = includeValues()) {
  const r = await exportConfig(path, wanted, withValues, appContext());
  setMessage({ text: t("Exported to {path}", { path: r.path }) });
  return r;
}

/** Reads a file and shows what importing it would do. */
export async function previewImportFile(path: string) {
  setMessage(null);
  setPending(null);
  const ctx = appContext();
  const read = await readImportFile(path, ctx);
  const inFile = CONFIG_SECTIONS.filter((k) => read.doc.sections[k] !== undefined && available(k));
  const plan = planImport(read, inFile, await loadCurrent(inFile, ctx), { defaults: ctx.defaults, newId: ctx.newId });
  setPending({ path, read, plan, chosen: inFile });
  return plan;
}

/** Imports the ticked sections of the file being previewed. */
export async function importPending() {
  const p = pending();
  if (!p) return null;
  // What the user saw for the ticked sections (each section's counts stand on their own).
  const sections = Object.fromEntries(Object.entries(p.plan.sections).filter(([k]) => p.chosen.includes(k as ConfigSection)));
  const result = await applyImport(p.read, p.chosen, { ...p.plan, sections }, appContext());
  for (const v of allViews()) v.applySettings(store.state.settings);
  if (p.chosen.includes("sshHosts")) await loadVault().catch(() => {});
  if (p.chosen.includes("automations")) window.dispatchEvent(new Event("kelpie:automations"));
  const lines = [t("Imported. The previous configuration was copied to {path}.", { path: result.backup })];
  if (result.plan.automations.length) lines.push(t("Imported automations are off; commands need approving on this computer."));
  if (p.chosen.includes("sshHosts")) lines.push(t("Saved passwords and key passphrases are not in the file: enter them again on the Hosts page."));
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

function note(k: ConfigSection, plan: ImportPlan): string | null {
  const changed = plan.sections.settings?.changed ?? [];
  if (k === "settings" && changed.length) return t("Changes: {names}", { names: changed.join(", ") });
  if (k === "automations") return t("They come in switched off; commands need approving on this computer.");
  if (k === "sshHosts") return t("No passwords or key passphrases: enter them again on the Hosts page.");
  if (k === "plugins") return t("Only a list: nothing is installed.");
  if (k === "agents" && plan.envNeeded.length) return t("Variables without a value here: {names}", { names: plan.envNeeded.flatMap((e) => e.names).join(", ") });
  return null;
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
  const inFile = () => CONFIG_SECTIONS.filter((k) => pending()?.plan.sections[k]);

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
        <div class="config-warning">{t("Values are written to the file as plain text: keep it private. Values that look like secrets are still left out.")}</div>
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
                    <input type="checkbox" checked={p().chosen.includes(k)} onChange={(e) => setPending({ ...p(), chosen: toggle(p().chosen, k, e.currentTarget.checked) })} /> {t(LABELS[k])}
                  </label>
                  <span class="muted-inline" data-part="counts">{summary(k, p().plan)}</span>
                  <Show when={note(k, p().plan)}>{(n) => <span class="muted-inline">{n()}</span>}</Show>
                </div>
              )}
            </For>
            <Show when={p().read.ignored.length}>
              <div class="muted-inline">{t("Ignored (not known to this version): {names}", { names: p().read.ignored.join(", ") })}</div>
            </Show>
            <div class="inline-fields">
              <button class="btn" disabled={busy()} onClick={() => setPending(null)}>{t("Cancel")}</button>
              <button class="btn primary" data-act="apply-import" disabled={busy() || !p().chosen.length} onClick={() => void run(importPending)}>{t("Import")}</button>
            </div>
          </div>
        )}
      </Show>
      <Show when={message()}>{(m) => <div class={m().error ? "config-error" : "config-result"} data-part="message">{m().text}</div>}</Show>
    </div>
  );
}
