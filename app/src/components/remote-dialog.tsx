// "Add remote project": an SSH host (from ~/.ssh/config or typed), a check of what it has
// (git, tmux, agent CLIs), and a folder on it chosen by browsing.
import { For, Show, createResource, createSignal, onMount } from "solid-js";
import { store } from "../lib/store";
import { CORE_AGENTS, agentPresets, kindInfo } from "../lib/agents";
import { remoteCheck, remoteFolders, remoteHosts, remoteRoot, splitHost, tmuxInstall, type HostCheck } from "../lib/remote";
import { Icon } from "./icons";
import { KeyHints } from "./key-hints";
import { openHosts } from "./hosts-page";
import { t, errorText } from "../lib/i18n";

export function openRemoteDialog() {
  store.setOverlay("remote");
}

export function RemoteDialog() {
  const [hosts] = createResource(() => remoteHosts().catch(() => [] as string[]));
  const [host, setHost] = createSignal("");
  const [check, setCheck] = createSignal<HostCheck | null>(null);
  const [checking, setChecking] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [folder, setFolder] = createSignal("");
  const [folders, setFolders] = createSignal<string[] | null>(null);
  const [tmux, setTmux] = createSignal(true);
  const [name, setName] = createSignal("");
  let hostInput!: HTMLInputElement;
  onMount(() => hostInput.focus());

  const browse = async (path: string) => {
    const { dest, port } = splitHost(host());
    setError(null);
    try {
      setFolders(await remoteFolders(port ? `${dest}:${port}` : dest, path));
      setFolder(path);
    } catch (e) {
      setError(String(e));
    }
  };
  const connect = async () => {
    if (!host().trim()) return;
    setChecking(true);
    setError(null);
    setCheck(null);
    try {
      const bins = agentPresets().length ? agentPresets().map((p) => p.bin) : CORE_AGENTS;
      const c = await remoteCheck(host().trim(), [...new Set(bins)]);
      setCheck(c);
      setTmux(c.tmux);
      await browse(c.home || "/");
    } catch (e) {
      setError(String(e));
    } finally {
      setChecking(false);
    }
  };
  const up = () => {
    const parts = folder().replace(/\/+$/, "").split("/");
    parts.pop();
    void browse(parts.join("/") || "/");
  };
  const add = () => {
    const { dest, port } = splitHost(host());
    const path = folder().trim();
    if (!dest || !path.startsWith("/")) return setError(t("Choose a folder (an absolute path on the host)"));
    const root = remoteRoot(dest, port, path);
    store.addProject(root, { name: name().trim() || `${path.split("/").filter(Boolean).pop() ?? "/"} · ${dest}`, remote: { dest, port, path: root.slice(root.indexOf("/", "ssh://".length)), tmux: tmux(), agents: agentsFound() } });
    store.setOverlay(null);
  };
  /** Agent kinds whose CLI the host has. */
  const agentsFound = () => (check()?.agents ?? []).map((bin) => agentPresets().find((p) => p.bin === bin)?.id ?? bin);

  return (
    <div class="overlay" onPointerDown={(e) => e.target === e.currentTarget && store.setOverlay(null)}>
      <div class="modal remote-dialog" onKeyDown={(e) => e.key === "Escape" && store.setOverlay(null)}>
        <h2>{t("Add remote project")}</h2>
        <div class="form-row">
          <label>{t("SSH host")}</label>
          <div class="inline-fields">
            <input ref={hostInput} class="text-input" list="kelpie-ssh-hosts" placeholder={t("Alias from ~/.ssh/config, or user@host[:port]")} value={host()}
              onInput={(e) => { setHost(e.currentTarget.value); setCheck(null); setFolders(null); }} onKeyDown={(e) => e.key === "Enter" && void connect()} />
            <datalist id="kelpie-ssh-hosts"><For each={hosts() ?? []}>{(h) => <option value={h} />}</For></datalist>
            <button class="btn primary" disabled={!host().trim() || checking()} onClick={() => void connect()}>{checking() ? t("Connecting…") : t("Connect")}</button>
          </div>
          <div class="form-note">{t("Kelpie uses your OpenSSH client and its config, keys and agent. Hosts that log in with a password: save them on the SSH hosts page.")} <a href="#" onClick={(e) => { e.preventDefault(); openHosts(); }}>{t("SSH hosts")}</a></div>
        </div>
        <Show when={check()}>
          {(c) => (
            <div class="remote-check">
              <span><Icon name="check" size={12} /> {c().os}</span>
              <span class={c().git ? "ok" : "warn"}>{c().git ? t("git ✓") : t("git missing")}</span>
              <span class={c().tmux ? "ok" : "muted-inline"}>{c().tmux ? t("tmux ✓") : t("tmux not installed")}</span>
              <span>{agentsFound().length ? t("agents: {list}", { list: agentsFound().map((k) => kindInfo(k).label).join(", ") }) : t("no agent CLIs found on its PATH")}</span>
            </div>
          )}
        </Show>
        <Show when={folders()}>
          <div class="form-row">
            <label>{t("Folder on the host")}</label>
            <div class="inline-fields">
              <button class="icon-btn" title={t("Parent folder")} onClick={up}><Icon name="up" size={14} /></button>
              <input class="text-input mono" value={folder()} onInput={(e) => setFolder(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && void browse(folder())} />
            </div>
            <div class="remote-folders">
              <For each={folders()} fallback={<div class="muted">{t("No folders here.")}</div>}>
                {(f) => (
                  <div class="remote-folder" onClick={() => void browse(`${folder().replace(/\/+$/, "")}/${f}`)}>
                    <Icon name="folder" size={13} /> {f}
                  </div>
                )}
              </For>
            </div>
          </div>
          <div class="form-row">
            <label>{t("Name (optional)")}</label>
            <input class="text-input" value={name()} placeholder={`${folder().split("/").filter(Boolean).pop() ?? ""} · ${splitHost(host()).dest}`} onInput={(e) => setName(e.currentTarget.value)} />
          </div>
          <label class="check-label" title={check()?.tmux ? "" : t("tmux is not installed on this host")}>
            <input type="checkbox" checked={tmux()} disabled={!check()?.tmux} onChange={(e) => setTmux(e.currentTarget.checked)} />
            {t("Keep sessions running on the host with tmux (they survive dropped connections and restarts)")}
          </label>
          <Show when={check() && !check()!.tmux}>
            <div class="form-note">
              {t("To keep sessions running, install tmux on the host, then connect again.")}{" "}
              <Show when={tmuxInstall(check()!)}>{(cmd) => <code>{cmd()}</code>}</Show>
            </div>
          </Show>
        </Show>
        <Show when={error()}><div class="form-note error">{errorText(error())}</div></Show>
        <div class="footer">
          <KeyHints items={[["Enter", t("Connect")], ["Esc", t("Cancel")]]} />
          <span style={{ flex: 1 }} />
          <button class="btn" onClick={() => store.setOverlay(null)}>{t("Cancel")}</button>
          <button class="btn primary" disabled={!folders() || !folder().startsWith("/")} onClick={add}>{t("Add project")}</button>
        </div>
      </div>
    </div>
  );
}
