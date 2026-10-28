// Settings → Plugins: install (folder or git URL), create, review and enable, disable, remove.
// Enabling shows everything the plugin would run; the approval covers that exact manifest, so
// an edited plugin turns itself off until it is reviewed again.
import { For, Show, createSignal, onMount } from "solid-js";
import { open } from "@tauri-apps/plugin-dialog";
import { revealEntry } from "../../lib/ipc";
import { msg, t, errorText } from "../../lib/i18n";
import {
  loadPlugins, pluginApprove, pluginCreate, pluginDir, pluginInstallFolder, pluginInstallGit, pluginRemove, plugins,
  type PluginInfo, type PluginManifest,
} from "../../lib/plugins";
import { Icon } from "../icons";
import { SettingRow } from "./setting-row";
import { confirmDialog } from "../../lib/confirm-dialog";

const COUNTS: [keyof NonNullable<PluginManifest["contributes"]>, string, string][] = [
  ["agents", msg("agent"), msg("agents")], ["commands", msg("command"), msg("commands")], ["quickCommands", msg("quick command"), msg("quick commands")],
  ["terminalThemes", msg("theme"), msg("themes")], ["layouts", msg("layout"), msg("layouts")], ["statusItems", msg("status item"), msg("status items")],
];

function summary(m: PluginManifest | null) {
  const c = m?.contributes ?? {};
  const parts = COUNTS.map(([k, one, many]) => [(c[k] as unknown[] | undefined)?.length ?? 0, one, many] as const).filter(([n]) => n).map(([n, one, many]) => `${n} ${t(n > 1 ? many : one)}`);
  return parts.join(" · ") || t("Adds nothing yet");
}

/** What enabling a plugin lets it run, in full. */
function Review(props: { plugin: PluginInfo; onDone: () => void }) {
  const c = () => props.plugin.manifest?.contributes ?? {};
  const [error, setError] = createSignal<string | null>(null);
  const runs = () => [
    ...(c().agents ?? []).flatMap((a) => [
      { what: t("Agent “{name}”", { name: a.label }), code: [a.bin, ...(a.launch ?? [])].join(" ") },
      // Chat view starts it with other arguments: what runs there is approved too.
      ...(a.acp?.length ? [{ what: t("Agent “{name}” in chat view", { name: a.label }), code: [a.bin, ...a.acp].join(" ") }] : []),
    ]),
    ...(c().commands ?? []).map((x) => ({ what: x.in === "agent" ? t("Command “{name}” (types into the focused agent)", { name: x.label }) : t("Command “{name}” (types into a terminal)", { name: x.label }), code: x.run })),
    ...(c().quickCommands ?? []).map((x) => ({ what: t("Quick command “{name}”", { name: x.label }), code: x.text })),
    ...(c().statusItems ?? []).map((x) => ({ what: t("Status item “{name}” — runs by itself every {n} s", { name: x.label ?? x.id, n: Math.max(5, x.intervalSeconds ?? 30) }), code: x.command })),
  ];
  const enable = async () => {
    try {
      await pluginApprove(props.plugin.id, props.plugin.hash);
      await loadPlugins();
      props.onDone();
    } catch (e) {
      setError(String(e));
    }
  };
  return (
    <div class="plugin-review" data-review={props.plugin.id}>
      <Show when={runs().length} fallback={<div class="muted-inline">{t("This plugin runs no programs.")}</div>}>
        <div class="plugin-review-head">{t("Enabling lets this plugin run these on this computer, with your permissions:")}</div>
        <For each={runs()}>{(r) => <div class="plugin-run"><div>{r.what}</div><code>{r.code}</code></div>}</For>
      </Show>
      <Show when={(c().terminalThemes?.length ?? 0) + (c().layouts?.length ?? 0)}>
        <div class="muted-inline">{t("Also adds {themes} terminal theme(s) and {layouts} layout(s).", { themes: c().terminalThemes?.length ?? 0, layouts: c().layouts?.length ?? 0 })}</div>
      </Show>
      <Show when={error()}><div class="form-note error">{errorText(error())}</div></Show>
      <div class="inline-fields">
        <button class="btn primary" onClick={() => void enable()}>{t("Enable plugin")}</button>
        <button class="btn" onClick={props.onDone}>{t("Cancel")}</button>
      </div>
    </div>
  );
}

export function PluginSettings() {
  const [reviewing, setReviewing] = createSignal<string | null>(null);
  const [gitUrl, setGitUrl] = createSignal("");
  const [newId, setNewId] = createSignal("");
  const [newName, setNewName] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [message, setMessage] = createSignal<{ text: string; error?: boolean } | null>(null);
  const [folder, setFolder] = createSignal("");
  onMount(() => {
    void loadPlugins();
    void pluginDir().then(setFolder).catch(() => {});
  });

  const act = async (what: () => Promise<string | void>) => {
    setBusy(true);
    setMessage(null);
    try {
      const text = await what();
      await loadPlugins();
      if (text) setMessage({ text });
    } catch (e) {
      setMessage({ text: String(e), error: true });
    } finally {
      setBusy(false);
    }
  };
  const installFolder = async () => {
    const dir = await open({ directory: true, multiple: false, title: t("Plugin folder (with kelpie-plugin.json)") });
    if (typeof dir !== "string") return;
    await act(async () => {
      try {
        return t("Installed {id}. Review it to enable it.", { id: await pluginInstallFolder(dir) });
      } catch (e) {
        if (!String(e).includes("already installed") || !(await confirmDialog(t("{error}. Replace it with this folder?", { error: String(e) })))) throw e;
        return t("Updated {id}.", { id: await pluginInstallFolder(dir, true) });
      }
    });
  };
  const toggle = (p: PluginInfo) => (p.approved ? void act(async () => void (await pluginApprove(p.id, null))) : setReviewing(reviewing() === p.id ? null : p.id));

  return (
    <>
      <SettingRow id="plugins" wide>
        <div class="plugin-list">
          <Show when={plugins().length} fallback={<div class="muted-inline">{t("No plugins installed.")}</div>}>
            <For each={plugins()}>
              {(p) => (
                <div class="plugin-card" data-plugin={p.id}>
                  <div class="plugin-line">
                    <div class="plugin-main">
                      <div class="plugin-name">
                        {p.manifest?.name ?? p.id} <span class="muted-inline">{p.manifest?.version ? `v${p.manifest.version} · ` : ""}{p.id}</span>
                      </div>
                      <Show when={p.manifest?.description}><div class="muted-inline">{p.manifest!.description}</div></Show>
                      <div class="muted-inline" style={{ "font-size": "11.5px" }}>{summary(p.manifest)}</div>
                    </div>
                    <span class={`plugin-state ${p.error ? "bad" : p.approved ? "on" : p.changed ? "warn" : ""}`}>
                      {p.error ? t("Invalid") : p.approved ? t("Enabled") : p.changed ? t("Changed — review") : t("Off")}
                    </span>
                    <Show when={!p.error}>
                      <button class="btn" disabled={busy()} onClick={() => toggle(p)}>{p.approved ? t("Disable") : t("Review & enable")}</button>
                    </Show>
                    <button class="icon-btn" title={t("Show files")} onClick={() => void revealEntry(p.dir, p.file || "kelpie-plugin.json").catch(() => {})}><Icon name="folder" size={13} /></button>
                    <button class="icon-btn" title={t("Remove plugin")} disabled={busy()}
                      onClick={async () => (await confirmDialog(t("Remove plugin “{name}”? Its folder goes to the trash.", { name: p.manifest?.name ?? p.id }))) && void act(() => pluginRemove(p.id))}>
                      <Icon name="close" size={13} />
                    </button>
                  </div>
                  <Show when={p.error}><div class="form-note error">{p.error}</div></Show>
                  <Show when={reviewing() === p.id && !p.approved && !p.error}><Review plugin={p} onDone={() => setReviewing(null)} /></Show>
                </div>
              )}
            </For>
          </Show>
          <Show when={message()}>{(m) => <div class={m().error ? "form-note error" : "muted-inline"} data-plugin-message>{m().text}</div>}</Show>
          <div class="inline-fields">
            <button class="btn" disabled={busy()} onClick={() => void installFolder()}>{t("Install from folder…")}</button>
            <button class="btn" disabled={busy()} onClick={() => void act(async () => void (await loadPlugins()))}><Icon name="refresh" size={13} /> {t("Reload")}</button>
            <Show when={folder()}><button class="btn" onClick={() => void revealEntry(folder(), "").catch(() => {})}>{t("Open plugins folder")}</button></Show>
          </div>
        </div>
      </SettingRow>
      <SettingRow id="plugin-install-git" wide>
        <div class="inline-fields">
          <input class="text-input" placeholder="https://github.com/you/kelpie-plugin.git" value={gitUrl()} onInput={(e) => setGitUrl(e.currentTarget.value)} />
          <button class="btn" disabled={busy() || !gitUrl().trim()} onClick={() => void act(async () => { const id = await pluginInstallGit(gitUrl().trim()); setGitUrl(""); return t("Installed {id}. Review it to enable it.", { id }); })}>
            {busy() ? t("Working…") : t("Install")}
          </button>
        </div>
      </SettingRow>
      <SettingRow id="plugin-create" wide>
        <div class="inline-fields">
          <input class="text-input" placeholder={t("id, e.g. my-tools")} value={newId()} onInput={(e) => setNewId(e.currentTarget.value.toLowerCase())} />
          <input class="text-input" placeholder={t("Name")} value={newName()} onInput={(e) => setNewName(e.currentTarget.value)} />
          <button class="btn" disabled={busy() || !newId().trim()} onClick={() => void act(async () => {
            const dir = await pluginCreate(newId().trim(), newName().trim() || newId().trim());
            setNewId("");
            setNewName("");
            void revealEntry(dir, "kelpie-plugin.json").catch(() => {});
            return t("Created {dir}. Edit kelpie-plugin.json, then Reload and enable it.", { dir });
          })}>{t("Create")}</button>
        </div>
      </SettingRow>
    </>
  );
}
