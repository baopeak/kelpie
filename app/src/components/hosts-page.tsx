// Hosts page (Termius-style): saved SSH hosts with groups and tags, one-click terminals and
// agents on a host, its files (upload / download), keys, known hosts and port forwards.
import { For, Show, createEffect, createMemo, createResource, createSignal, onCleanup, onMount, type JSX } from "solid-js";
import { OsBadge } from "./host-target";
import { findTerminalTheme, terminalThemes } from "../lib/terminal-themes";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { store } from "../lib/store";
import * as ipc from "../lib/ipc";
import { kindInfo } from "../lib/agents";
import { tmuxInstall, type HostCheck } from "../lib/remote";
import { openRootUserGuide } from "../lib/root-user";
import {
  CLAUDE_INSTALL, agentAdd, agentList, checkHost, connectHost, deleteKey, forwardStart, forwardStatus, forwardStop, generateKey, hasSecret, hostAddress, hostById, hostKeys,
  hostProject, importKey, importSshConfig, installKey, isHostKeyError, knownHosts, listKeys, loadVault, newForward, newHost, parseDestination, removeForward, removeHost, removeKnownHosts, runOnHosts,
  saveForward, saveHost, saveVault, setSecret, transfer, trustHost, vault, type HostKeys, type SshForward, type SshHost,
} from "../lib/ssh-hosts";
import { Icon } from "./icons";
import { LoadBar } from "./load-bar";
import { t, errorText } from "../lib/i18n";
import { relTimeShort } from "../lib/platform";
import type { DirEntry } from "../lib/types";
import { confirmDialog } from "../lib/confirm-dialog";

type Tab = "hosts" | "keys" | "known" | "forwards";
const [tab, setTab] = createSignal<Tab>("hosts");
const [selected, setSelected] = createSignal<string | null>(null);
/** Host whose fingerprint check opens with the page (a launcher hit an unknown host key). */
const [trustFor, setTrustFor] = createSignal<string | null>(null);
/** Terminals and agents of a host open in the local project behind the page ("Open in"),
 *  instead of the host's own remote project. */
const [openInHere, setOpenInHere] = createSignal(false);
const localProject = () => {
  const p = store.activeProject();
  return p && !p.remote ? p : null;
};

export function openHosts(which: Tab = "hosts", hostId?: string, opts: { trust?: boolean } = {}) {
  setTab(which);
  if (hostId) setSelected(hostId);
  setTrustFor(opts.trust && hostId ? hostId : null);
  store.setOverlay("hosts");
}

const errText = (e: unknown) => errorText(String(e));

type Ask = { label: string; value: string; secret?: boolean; ok: (v: string) => void };

/** One-line question shown in place (no blocking browser prompt). */
function AskRow(props: { ask: Ask; onClose: () => void }) {
  const [v, setV] = createSignal(props.ask.value);
  let input!: HTMLInputElement;
  onMount(() => input.focus());
  const ok = () => {
    props.onClose();
    props.ask.ok(v());
  };
  return (
    <div class="inline-fields ask-row">
      <span class="muted-inline">{props.ask.label}</span>
      <input ref={input} class="text-input" data-field="ask" style={{ flex: 1, width: "auto" }} type={props.ask.secret ? "password" : "text"} autocomplete="off" value={v()}
        onInput={(e) => setV(e.currentTarget.value)} onKeyDown={(e) => { if (e.key === "Enter") ok(); if (e.key === "Escape") { e.preventDefault(); props.onClose(); } }} />
      <button class="btn primary" data-action="ask-ok" onClick={ok}>{t("OK")}</button>
      <button class="btn" onClick={props.onClose}>{t("Cancel")}</button>
    </div>
  );
}

export function HostsPage() {
  let el!: HTMLDivElement;
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && !e.defaultPrevented && document.activeElement?.closest(".hosts-page")) store.setOverlay(null);
  };
  onMount(() => {
    window.addEventListener("keydown", onKey);
    el.focus();
    // The vault may have changed since the app started (another window, a script).
    void loadVault().catch(() => {});
  });
  onCleanup(() => window.removeEventListener("keydown", onKey));
  const tabs: [Tab, string][] = [["hosts", t("Hosts")], ["keys", t("Keychain")], ["known", t("Known hosts")], ["forwards", t("Port forwarding")]];
  return (
    <div class="runs-page hosts-page" ref={el} tabIndex={-1}>
      <div class="board-head">
        <button class="settings-back" onClick={() => store.setOverlay(null)} title={t("Back (Esc)")}><Icon name="back" size={14} /></button>
        <h2>{t("SSH hosts")}</h2>
        <div class="seg hosts-tabs">
          <For each={tabs}>{([id, label]) => <button class={tab() === id ? "on" : ""} data-tab={id} onClick={() => setTab(id)}>{label}</button>}</For>
        </div>
        <span style={{ flex: 1 }} />
      </div>
      <Show when={tab() === "hosts"}><HostsTab /></Show>
      <Show when={tab() === "keys"}><div class="hosts-single"><KeysTab /></div></Show>
      <Show when={tab() === "known"}><div class="hosts-single"><KnownHostsTab /></div></Show>
      <Show when={tab() === "forwards"}><div class="hosts-single"><ForwardsTab /></div></Show>
    </div>
  );
}

// ---------- hosts ----------

function HostsTab() {
  const [query, setQuery] = createSignal("");
  const [editing, setEditing] = createSignal<SshHost | null>(null);
  const [picked, setPicked] = createSignal<string[]>([]);
  const [runText, setRunText] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const matches = (h: SshHost) => {
    const q = query().trim().toLowerCase();
    return !q || [h.label, h.alias, h.hostname, h.user, h.group, ...h.tags].some((x) => x.toLowerCase().includes(q));
  };
  /** `user@host[:port]` typed in the search box that no saved host matches: a new host to add. */
  const typed = () => (vault().hosts.some(matches) ? null : parseDestination(query()));
  const quickConnect = () => {
    const d = parseDestination(query());
    const only = vault().hosts.filter(matches);
    if (only.length === 1 && !typed()) return setSelected(only[0].id);
    if (d) setEditing(newHost({ user: d.user, hostname: d.hostname, port: d.port }));
  };
  /** Hosts by group ("" last), recently used first inside a group. */
  const groups = createMemo(() => {
    const map = new Map<string, SshHost[]>();
    for (const h of vault().hosts.filter(matches)) map.set(h.group, [...(map.get(h.group) ?? []), h]);
    return [...map.entries()].sort((a, b) => (a[0] === "" ? 1 : b[0] === "" ? -1 : a[0].localeCompare(b[0]))).map(([g, hs]) => [g, hs.sort((a, b) => b.lastConnected - a.lastConnected || a.label.localeCompare(b.label))] as const);
  });
  const current = () => hostById(selected() ?? "") ?? vault().hosts[0];
  const importConfig = async () => {
    setError(null);
    try {
      const found = (await importSshConfig()).filter((h) => !vault().hosts.some((x) => x.alias === h.alias));
      if (!found.length) return store.notify(t("No new hosts in the SSH config"));
      await saveVault({ ...vault(), hosts: [...vault().hosts, ...found] });
      store.notify(found.length === 1 ? t("Imported {n} host", { n: 1 }) : t("Imported {n} hosts", { n: found.length }));
    } catch (e) {
      setError(errText(e));
    }
  };
  const runPicked = async () => {
    const hosts = picked().map(hostById).filter((h): h is SshHost => !!h);
    if (!hosts.length || !runText().trim()) return;
    store.setOverlay(null);
    const failed = await runOnHosts(hosts, runText().trim());
    if (failed.length) store.notify(t("Could not connect to {list}", { list: failed.join(", ") }));
  };
  const snippets = () => (store.state.settings.quickCommands ?? []).filter((c) => c.action === "terminal");
  return (
    <div class="runs-body">
      <div class="runs-list hosts-list">
        <div class="quick-connect">
          <input class="text-input" data-field="quick-connect" placeholder={t("Find a host or ssh user@hostname…")} value={query()} onInput={(e) => setQuery(e.currentTarget.value)}
            onKeyDown={(e) => e.key === "Enter" && quickConnect()} />
          <button class="btn" data-action="quick-connect" disabled={!parseDestination(query()) && vault().hosts.filter(matches).length !== 1} onClick={quickConnect}>{t("Connect")}</button>
        </div>
        <div class="inline-fields">
          <button class="btn primary" data-action="new-host" onClick={() => setEditing(newHost())}><Icon name="plus" size={13} /> {t("New host")}</button>
          <button class="btn" title={t("Add the hosts of your SSH config")} onClick={() => void importConfig()}>{t("Import")}</button>
        </div>
        <Show when={error()}><div class="form-note error">{error()}</div></Show>
        <Show when={typed()}>
          {(d) => (
            <div class="runs-item host-item host-new" data-action="add-typed-host" onClick={quickConnect}>
              <div class="host-item-top"><OsBadge os="" size={22} /><span class="runs-item-name">{t("Add {dest}", { dest: `${d().user ? d().user + "@" : ""}${d().hostname}${d().port ? ":" + d().port : ""}` })}</span></div>
              <div class="run-task-meta">{t("Opens the new host form with this address")}</div>
            </div>
          )}
        </Show>
        <For each={groups()} fallback={<Show when={!typed()}><div class="muted">{t("No saved hosts yet.")}</div></Show>}>
          {([group, hosts]) => (
            <>
              <div class="section-label">{group || t("Ungrouped")}</div>
              <For each={hosts}>
                {(h) => (
                  <div class={`runs-item host-item${current()?.id === h.id ? " on" : ""}`} data-host={h.alias} onClick={() => { setSelected(h.id); setEditing(null); }}>
                    <div class="host-item-top">
                      <input type="checkbox" title={t("Select for running a command")} checked={picked().includes(h.id)} onClick={(e) => e.stopPropagation()}
                        onChange={(e) => setPicked(e.currentTarget.checked ? [...picked(), h.id] : picked().filter((x) => x !== h.id))} />
                      <OsBadge os={h.os} size={22} />
                      <span class="runs-item-name">{h.label}</span>
                      <span class="host-dot" style={{ background: h.color || "transparent" }} />
                    </div>
                    <div class="run-task-meta"><span class="mono">{hostAddress(h)}</span><For each={h.tags}>{(tag) => <span class="chip">{tag}</span>}</For></div>
                  </div>
                )}
              </For>
            </>
          )}
        </For>
        <Show when={picked().length}>
          <div class="hosts-run">
            <div class="form-note">{t("Run a command on {n} selected host(s), one terminal each", { n: picked().length })}</div>
            <Show when={snippets().length}>
              <select class="text-input" data-field="run-snippet" value="" onChange={(e) => { const c = snippets().find((x) => x.id === e.currentTarget.value); if (c) setRunText(c.text); e.currentTarget.value = ""; }}>
                <option value="">{t("Quick command…")}</option>
                <For each={snippets()}>{(c) => <option value={c.id}>{c.label}</option>}</For>
              </select>
            </Show>
            <input class="text-input mono" placeholder="uptime" value={runText()} onInput={(e) => setRunText(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && void runPicked()} /> {/* i18n-ignore */}
            <div class="inline-fields">
              <button class="btn primary" disabled={!runText().trim()} onClick={() => void runPicked()}>{t("Run")}</button>
              <button class="btn" onClick={() => setPicked([])}>{t("Clear selection")}</button>
            </div>
          </div>
        </Show>
      </div>
      <div class="runs-main">
        {/* Keyed by id: saving the host (e.g. its last connection time) must not reset the view. */}
        <Show when={current()?.id} keyed fallback={
          <div class="empty"><div class="welcome">
            <h1>{t("SSH hosts")}</h1>
            <div style={{ color: "var(--text-dim)" }}>{t("Save your servers once, then open a terminal or start Claude Code on any of them in one click. Sessions run in tmux on the server, so they survive dropped connections.")}</div>
            <div class="actions"><button class="btn primary" onClick={() => setEditing(newHost())}>{t("New host")}</button><button class="btn" onClick={() => void importConfig()}>{t("Import from SSH config")}</button></div>
          </div></div>
        }>
          {(id) => <HostDetail host={hostById(id) ?? current()!} onEdit={() => { const h = hostById(id)!; setEditing({ ...h, env: { ...h.env }, tags: [...h.tags] }); }} />}
        </Show>
      </div>
      {/* Termius-style drawer over the page: the form scrolls, Save / Connect stay at the bottom. */}
      <Show when={editing()} keyed>
        {(h) => (
          <div class="drawer-scrim" onPointerDown={(e) => e.target === e.currentTarget && setEditing(null)}>
            <HostEditor host={h} onDone={(id) => {
              setEditing(null);
              if (!id) return;
              setSelected(id);
              // Saved from an address typed in the box: the box would now hide the new host.
              if (parseDestination(query())) setQuery("");
            }} />
          </div>
        )}
      </Show>
    </div>
  );
}

function HostDetail(props: { host: SshHost; onEdit: () => void }) {
  const h = () => props.host;
  const [check, setCheck] = createSignal<HostCheck | null>(null);
  const [busy, setBusy] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [trust, setTrust] = createSignal(false);
  // Asked for by a launcher or the drawer's Connect (also when this host is already shown).
  createEffect(() => {
    if (trustFor() !== props.host.id) return;
    setTrust(true);
    setTrustFor(null);
  });
  const [files, setFiles] = createSignal(false);
  const here = localProject;
  const inHere = openInHere;
  const setInHere = setOpenInHere;
  /** Runs `fn`; a host-key failure opens the fingerprint check instead of an error. */
  const guarded = async (label: string, fn: () => Promise<unknown>) => {
    setBusy(label);
    setError(null);
    try {
      await fn();
    } catch (e) {
      if (isHostKeyError(e)) setTrust(true);
      else setError(errText(e));
    } finally {
      setBusy(null);
    }
  };
  const probe = () => guarded("check", async () => setCheck(await checkHost(h())));
  const open = (kind: string, prompt?: string) =>
    guarded(kind, async () => {
      const c = check() ?? (await checkHost(h()));
      setCheck(c);
      // Closed once started: an error (an agent without a folder there) shows on this page.
      await connectHost(h(), kind, { check: c, prompt, projectId: inHere() ? here()?.id : undefined });
      store.setOverlay(null);
    });
  const asProject = () => guarded("project", async () => {
    const id = await hostProject(h(), check() ?? undefined);
    store.setOverlay(null);
    store.switchProject(id);
  });
  const remove = async () => {
    if (!(await confirmDialog(t("Remove the host {name}? Its saved password and port forwards are removed too.", { name: h().label })))) return;
    await removeHost(h().id).catch((e) => setError(errText(e)));
    setSelected(null);
  };
  const agents = () => (check()?.agents ?? []).map((bin) => bin);
  const hasClaude = () => agents().includes("claude");
  return (
    <div class="run-detail host-detail" data-host-detail={h().alias}>
      <LoadBar when={busy()} />
      <div class="run-detail-head">
        <div style={{ flex: 1, "min-width": 0 }}>
          <h3><OsBadge os={h().os} size={24} /><span class="host-dot" style={{ background: h().color || "transparent" }} />{h().label}</h3>
          <div class="run-objective mono">{hostAddress(h())}{h().jump ? ` ${t("via")} ${h().jump}` : ""} · {t("alias")} <b>{h().alias}</b></div>
          <div class="run-task-meta">
            <span>{h().auth === "password" ? t("Password login") : h().identityFile ? t("Key {file}", { file: h().identityFile.split(/[\\/]/).pop() ?? "" }) : t("Key or agent")}</span>
            <Show when={h().lastConnected}><span>{t("connected {when}", { when: relTimeShort(h().lastConnected) })}</span></Show>
            <For each={h().tags}>{(tag) => <span class="chip">{tag}</span>}</For>
          </div>
        </div>
        <button class="btn" onClick={props.onEdit}>{t("Edit")}</button>
        <button class="btn" onClick={() => void remove()}>{t("Remove")}</button>
      </div>
      <Show when={here()}>
        {(p) => (
          <div class="host-open-in">
            <span class="muted-inline">{t("Open in")}</span>
            <div class="seg">
              <button class={inHere() ? "" : "on"} data-open-in="host" onClick={() => setInHere(false)}>{t("The host's own project")}</button>
              <button class={inHere() ? "on" : ""} data-open-in="current" onClick={() => setInHere(true)}>{t("Current project: {name}", { name: p().name })}</button>
            </div>
          </div>
        )}
      </Show>
      <div class="host-actions">
        <button class="btn primary" data-action="terminal" disabled={!!busy()} onClick={() => void open("shell")}><Icon name="terminal" size={13} /> {t("Terminal")}</button>
        <button class="btn primary" data-action="claude" disabled={!!busy() || (!!check() && !hasClaude())} title={check() && !hasClaude() ? t("Claude Code is not installed on this host") : ""} onClick={() => void open("claude")}>
          <Icon name="sparkle" size={13} /> {t("Claude Code here")}
        </button>
        <For each={agents().filter((a) => a !== "claude")}>
          {(a) => <button class="btn" disabled={!!busy()} onClick={() => void open(a)}>{kindInfo(a).label}</button>}
        </For>
        <button class={`btn${files() ? " active" : ""}`} data-action="files" onClick={() => setFiles(!files())}><Icon name="files" size={13} /> {t("Files")}</button>
        <button class="btn" disabled={!!busy()} onClick={() => void asProject()}><Icon name="folder" size={13} /> {t("Open as project")}</button>
        <button class="btn" data-action="check" disabled={!!busy()} onClick={() => void probe()}><Icon name="refresh" size={13} /> {busy() === "check" ? t("Connecting…") : t("Check connection")}</button>
      </div>
      <Show when={busy() && busy() !== "check"}><div class="form-note">{t("Connecting…")}</div></Show>
      <Show when={error()}><div class="form-note error" data-host-error>{error()}</div></Show>
      <Show when={trust()}><TrustPanel alias={h().alias} onDone={(ok) => { setTrust(false); if (ok) void probe(); }} /></Show>
      <Show when={check()}>
        {(c) => (
          <div class="remote-check" data-host-check>
            <span><Icon name="check" size={12} /> {c().os}</span>
            <Show when={c().root}>
              <span class="warn" data-host-root title={t("Claude Code refuses Yolo mode as root: on this host it asks for permissions.")}>{t("logs in as root")}</span>
            </Show>
            <span class={c().git ? "ok" : "warn"}>{c().git ? t("git ✓") : t("git missing")}</span>
            <span class={c().tmux ? "ok" : "muted-inline"} title={c().tmux ? "" : t("Without tmux a session ends when its connection drops; with it, sessions keep running on the host.")}>{c().tmux ? t("tmux ✓") : t("tmux not installed")}</span>
            <span>{agents().length ? t("agents: {list}", { list: agents().map((k) => kindInfo(k).label).join(", ") }) : t("no agent CLIs found on its PATH")}</span>
            <Show when={!hasClaude()}>
              <button class="btn" title={CLAUDE_INSTALL} onClick={() => void open("shell", CLAUDE_INSTALL)}>{t("Install Claude Code")}</button>
            </Show>
            <Show when={!c().tmux && tmuxInstall(c())}>
              {(cmd) => <button class="btn" title={cmd()} onClick={() => void open("shell", cmd())}>{t("Install tmux")}</button>}
            </Show>
            <Show when={c().root}>
              <button class="btn" data-action="root-user-guide" onClick={() => { store.setOverlay(null); openRootUserGuide(h().id, here()?.id); }}>{t("Set up a regular user…")}</button>
              <Show when={store.state.settings.rootYoloOk?.includes(h().id)}>
                <button class="btn" data-action="root-yolo-explain" title={t("Claude Code in Yolo mode on this host starts asking for permissions without saying why first.")}
                  onClick={() => store.setSettings({ rootYoloOk: (store.state.settings.rootYoloOk ?? []).filter((id) => id !== h().id) })}>{t("Explain again before Claude starts")}</button>
              </Show>
            </Show>
          </div>
        )}
      </Show>
      <Show when={!check() && !error() && !trust()}><div class="form-note">{h().folder ? t("Terminals and agents start in {folder}, inside tmux when the host has it.", { folder: h().folder }) : t("Terminals start in the login folder, inside tmux when the host has it. Agents need a start folder, or a project linked to its folder here.")}</div></Show>
      <Show when={files()}><FileBrowser host={h()} onHostKey={() => setTrust(true)} /></Show>
      <HostProjects host={h()} />
      <HostForwards host={h()} />
      <Show when={h().notes.trim()}><div class="host-notes">{h().notes}</div></Show>
    </div>
  );
}

/** Projects this host is linked to (offered first in their launchers), with the folder used in
 *  each; links never own the host. */
function HostProjects(props: { host: SshHost }) {
  const [folder, setFolder] = createSignal("");
  const links = () => store.state.projects.flatMap((p) => (p.hosts ?? []).filter((l) => l.hostId === props.host.id).map((l) => ({ project: p, path: l.path })));
  const here = () => {
    const p = store.activeProject();
    return p && !links().some((l) => l.project.id === p.id) ? p : null;
  };
  return (
    <div class="host-forwards" data-host-projects>
      <div class="section-label">{t("Used in projects")}</div>
      <For each={links()} fallback={<div class="muted-inline" style={{ "font-size": "12px" }}>{t("Not linked to any project. Any project can still run panes here; linking offers it first.")}</div>}>
        {(l) => (
          <div class="forward-row" data-host-link={l.project.id}>
            <span class="host-dot" style={{ background: l.project.color }} />
            <div style={{ flex: 1, "min-width": 0 }}>
              <div>{l.project.name}</div>
              <div class="mono muted-inline" style={{ "font-size": "11.5px" }}>{l.path || props.host.folder || t("the login folder")}</div>
            </div>
            <button class="btn" onClick={() => store.unlinkHost(l.project.id, props.host.id)}>{t("Unlink")}</button>
          </div>
        )}
      </For>
      <Show when={here()}>
        {(p) => (
          <div class="inline-fields">
            <input class="text-input mono" style={{ flex: 1, width: "auto" }} data-field="link-folder" placeholder={props.host.folder || t("the login folder")} value={folder()} onInput={(e) => setFolder(e.currentTarget.value)} />
            <button class="btn" data-action="link-current" onClick={() => { store.linkHost(p().id, { hostId: props.host.id, path: folder().trim() }); setFolder(""); }}>{t("Link to {name}", { name: p().name })}</button>
          </div>
        )}
      </Show>
    </div>
  );
}

/** Fingerprints of a host ssh does not know (or whose key changed), to compare and trust. */
function TrustPanel(props: { alias: string; onDone: (trusted: boolean) => void }) {
  const [keys] = createResource(() => props.alias, (a) => hostKeys(a).catch((e) => ({ error: errText(e) }) as unknown as HostKeys & { error: string }));
  const [error, setError] = createSignal<string | null>(null);
  const k = () => keys() as (HostKeys & { error?: string }) | undefined;
  const accept = async () => {
    const r = k();
    if (!r?.keys) return;
    try {
      if (r.known && !r.matches) {
        const stale = (await knownHosts()).filter((x) => x.hosts.split(",").includes(r.name)).map((x) => x.line);
        if (stale.length) await removeKnownHosts(stale);
      }
      await trustHost(r.keys.map((x) => x.line));
      props.onDone(true);
    } catch (e) {
      setError(errText(e));
    }
  };
  return (
    <div class={`trust-panel${k()?.known && !k()?.matches ? " changed" : ""}`} data-trust>
      <Show when={k()} fallback={<div class="form-note">{t("Reading the host's keys…")}</div>}>
        {(r) => (
          <Show when={!r().error} fallback={<div class="form-note error">{r().error}</div>}>
            <div class="trust-title">
              {r().known && !r().matches
                ? t("WARNING: the host key of {name} changed. This happens after a reinstall — or when someone intercepts the connection. Trust the new key only if you expected the change.", { name: r().name })
                : t("Kelpie has not connected to {name} before. Check that a fingerprint matches what the server's owner gave you:", { name: r().name })}
            </div>
            <For each={r().keys}>{(key) => <div class="mono trust-fp"><span class="muted-inline">{key.kind}</span> {key.fingerprint}</div>}</For>
            <div class="inline-fields">
              <button class="btn primary" data-action="trust" onClick={() => void accept()}>{r().known && !r().matches ? t("Replace the key and connect") : t("Trust and connect")}</button>
              <button class="btn" onClick={() => props.onDone(false)}>{t("Cancel")}</button>
            </div>
          </Show>
        )}
      </Show>
      <Show when={error()}><div class="form-note error">{error()}</div></Show>
    </div>
  );
}

function HostEditor(props: { host: SshHost; onDone: (id?: string) => void }) {
  const [h, setH] = createSignal<SshHost>(props.host);
  const [password, setPassword] = createSignal("");
  const [hasPassword] = createResource(() => props.host.id, (id) => (id ? hasSecret("password", id).catch(() => false) : Promise.resolve(false)));
  const [keys] = createResource(() => listKeys().catch(() => []));
  const [error, setError] = createSignal<string | null>(null);
  const set = <K extends keyof SshHost>(k: K, v: SshHost[K]) => setH({ ...h(), [k]: v });
  const envText = () => Object.entries(h().env).map(([k, v]) => `${k}=${v}`).join("\n");
  /** Saves the host (and its password); the saved id, or undefined after showing the error. */
  const save = async (): Promise<string | undefined> => {
    setError(null);
    try {
      const saved = await saveHost(h());
      if (h().auth === "password" && password()) await setSecret("password", saved.id, password());
      if (h().auth !== "password" && props.host.auth === "password") await setSecret("password", saved.id, "");
      return saved.id;
    } catch (e) {
      setError(errText(e));
    }
  };
  const browseKey = async () => {
    const f = await openDialog({ multiple: false, directory: false, title: t("Choose a private key") });
    if (typeof f === "string") set("identityFile", f);
  };
  const others = () => vault().hosts.filter((x) => x.id !== h().id);
  const [connecting, setConnecting] = createSignal(false);
  /** Saves, then opens a terminal where "Open in" says; a new host key opens its fingerprint
   *  check on the page instead. */
  const connect = async () => {
    const id = await save();
    const saved = id && hostById(id);
    if (!saved) return;
    setConnecting(true);
    try {
      await connectHost(saved, "shell", { projectId: openInHere() ? localProject()?.id : undefined });
      store.setOverlay(null);
    } catch (e) {
      if (!isHostKeyError(e)) return setError(errText(e));
      props.onDone(saved.id);
      openHosts("hosts", saved.id, { trust: true });
    } finally {
      setConnecting(false);
    }
  };
  /** One collapsed option row: its name, the current value, the field when opened. */
  const Opt = (o: { label: string; value: string; open?: boolean; children: JSX.Element }) => (
    <details class="drawer-opt" open={o.open}>
      <summary><span>{o.label}</span><span class="drawer-opt-value">{o.value}</span></summary>
      <div class="drawer-opt-body">{o.children}</div>
    </details>
  );
  let first!: HTMLInputElement;
  onMount(() => first.focus());
  return (
    <div class="host-drawer" data-host-editor onKeyDown={(e) => { if (e.key === "Escape") { e.preventDefault(); props.onDone(); } }}>
      <div class="drawer-head">
        <h3>{props.host.id ? t("Edit host") : t("New host")}</h3>
        <button class="icon-btn" title={t("Close (Esc)")} onClick={() => props.onDone()}><Icon name="close" size={13} /></button>
      </div>
      <div class="drawer-body">
        <div class="drawer-card">
          <div class="drawer-address">
            <OsBadge os={h().os} size={36} />
            <input ref={first} class="text-input mono" data-field="hostname" placeholder={t("IP or host name")} value={h().hostname} onInput={(e) => set("hostname", e.currentTarget.value)} />
          </div>
          <input class="text-input" data-field="label" value={h().label} placeholder={t("Name, e.g. Production API")} onInput={(e) => set("label", e.currentTarget.value)} />
          <div class="inline-fields">
            <input class="text-input" list="kelpie-host-groups" value={h().group} placeholder={t("Group")} onInput={(e) => set("group", e.currentTarget.value)} />
            <input class="text-input" value={h().tags.join(", ")} placeholder={t("Tags")} onInput={(e) => set("tags", e.currentTarget.value.split(",").map((s) => s.trim()).filter(Boolean))} />
          </div>
          <datalist id="kelpie-host-groups"><For each={[...new Set(vault().hosts.map((x) => x.group).filter(Boolean))]}>{(g) => <option value={g} />}</For></datalist>
        </div>
        <div class="drawer-card">
          <div class="drawer-port">{t("SSH on")} <input class="text-input mono" data-field="port" placeholder="22" value={h().port ?? ""} onInput={(e) => set("port", Number(e.currentTarget.value) || null)} /> {t("port")}</div>
          <div class="drawer-title">{t("Credentials")}</div>
          <input class="text-input mono" data-field="user" placeholder={t("Username")} value={h().user} onInput={(e) => set("user", e.currentTarget.value)} />
          <div class="seg">
            <button class={h().auth === "key" ? "on" : ""} onClick={() => set("auth", "key")}>{t("Key or agent")}</button>
            <button class={h().auth === "password" ? "on" : ""} data-auth="password" onClick={() => set("auth", "password")}>{t("Password")}</button>
          </div>
          <Show when={h().auth === "password"}>
            <input class="text-input" type="password" data-field="password" autocomplete="off" placeholder={hasPassword() ? t("Saved — type to replace") : t("Password")} value={password()} onInput={(e) => setPassword(e.currentTarget.value)} />
            <div class="form-note">{t("Kept in the system credential store, never in Kelpie's files.")}</div>
          </Show>
          <div class="inline-fields">
            <select class="text-input" style={{ flex: 1, width: "auto" }} value={h().identityFile} onChange={(e) => set("identityFile", e.currentTarget.value)}>
              <option value="">{t("Default keys and the agent")}</option>
              <For each={keys() ?? []}>{(k) => <option value={k.path}>{k.name} ({k.kind.replace("ssh-", "")})</option>}</For>
              <Show when={h().identityFile && !(keys() ?? []).some((k) => k.path === h().identityFile)}><option value={h().identityFile}>{h().identityFile}</option></Show>
            </select>
            <button class="btn" onClick={() => void browseKey()}>{t("Browse…")}</button>
          </div>
        </div>
        <div class="drawer-card drawer-opts">
          <Opt label={t("Start folder")} value={h().folder || t("the login folder")}>
            <input class="text-input mono" data-field="folder" value={h().folder} placeholder={t("the login folder")} onInput={(e) => set("folder", e.currentTarget.value)} />
          </Opt>
          <Opt label={t("Startup command")} value={h().startup || t("None")}>
            <input class="text-input mono" value={h().startup} placeholder="cd ~/app && git status" onInput={(e) => set("startup", e.currentTarget.value)} /> {/* i18n-ignore */}
          </Opt>
          <Opt label={t("Keep sessions in tmux")} value={h().tmux ? t("On") : t("Off")}>
            <label class="check-label"><input type="checkbox" checked={h().tmux} onChange={(e) => set("tmux", e.currentTarget.checked)} /> {t("Keep sessions in tmux")}</label>
          </Opt>
          <Opt label={t("Agent forwarding")} value={h().forwardAgent ? t("On") : t("Off")}>
            <label class="check-label"><input type="checkbox" checked={h().forwardAgent} onChange={(e) => set("forwardAgent", e.currentTarget.checked)} /> {t("Forward the SSH agent")}</label>
          </Opt>
          <Opt label={t("Jump host")} value={h().jump || t("None")}>
            <input class="text-input mono" list="kelpie-jump-hosts" placeholder={t("none (another saved host's alias, or user@host)")} value={h().jump} onInput={(e) => set("jump", e.currentTarget.value)} />
            <datalist id="kelpie-jump-hosts"><For each={others()}>{(x) => <option value={x.alias}>{x.label}</option>}</For></datalist>
          </Opt>
          <Opt label={t("Environment")} value={Object.keys(h().env).join(", ") || t("None")}>
            <textarea class="text-input mono" rows={2} style={{ height: "auto" }} placeholder={"LANG=C.UTF-8" /* i18n-ignore */} value={envText()}
              onInput={(e) => set("env", Object.fromEntries(e.currentTarget.value.split("\n").map((l) => l.split("=")).filter((p) => p[0]?.trim() && p.length > 1).map(([k, ...v]) => [k.trim(), v.join("=")])))} />
          </Opt>
          <Opt label={t("Keepalive (s)")} value={String(h().keepalive || t("Default"))}>
            <input class="text-input" style={{ width: "80px" }} value={h().keepalive} onInput={(e) => set("keepalive", Number(e.currentTarget.value) || 0)} />
          </Opt>
          <Opt label={t("Appearance")} value={findTerminalTheme(h().terminalTheme)?.label ?? t("App's terminal theme")}>
            <div class="inline-fields">
              <For each={["", "#5b8def", "#3fb950", "#d29922", "#f85149", "#a371f7", "#39c5cf"]}>
                {(c) => <button class={`host-swatch${h().color === c ? " on" : ""}`} style={{ background: c || "var(--bg-elev-2)" }} title={c || t("None")} onClick={() => set("color", c)} />}
              </For>
            </div>
            <select class="text-input" data-field="terminal-theme" value={h().terminalTheme} onChange={(e) => set("terminalTheme", e.currentTarget.value)}>
              <option value="">{t("App's terminal theme")}</option>
              <For each={terminalThemes()}>{(th) => <option value={th.id}>{th.label}</option>}</For>
            </select>
            <div class="form-note">{t("Panes on this host use this terminal theme, so they never look like another machine.")}</div>
          </Opt>
          <Opt label={t("SSH alias")} value={h().alias || t("from the name")}>
            <input class="text-input mono" value={h().alias} placeholder={t("from the name")} onInput={(e) => set("alias", e.currentTarget.value)} />
          </Opt>
          <Opt label={t("Notes")} value={h().notes.trim() ? "…" : t("None")}>
            <textarea class="text-input" rows={3} style={{ height: "auto" }} value={h().notes} onInput={(e) => set("notes", e.currentTarget.value)} />
          </Opt>
        </div>
      </div>
      <div class="drawer-foot">
        <Show when={error()}><div class="form-note error">{error()}</div></Show>
        <div class="inline-fields">
          <button class="btn" data-action="save-host" disabled={!h().hostname.trim()} onClick={() => void save().then((id) => id && props.onDone(id))}>{t("Save")}</button>
          <button class="btn primary drawer-connect" data-action="connect-host" disabled={!h().hostname.trim() || connecting()} onClick={() => void connect()}>{connecting() ? t("Connecting…") : t("Connect")}</button>
        </div>
      </div>
    </div>
  );
}

// ---------- files on a host ----------

function FileBrowser(props: { host: SshHost; onHostKey: () => void }) {
  const root = () => `ssh://${props.host.alias}/`;
  const [path, setPath] = createSignal(props.host.folder.trim() || "~");
  const [entries, setEntries] = createSignal<DirEntry[] | null>(null);
  const [picked, setPicked] = createSignal<string[]>([]);
  const [busy, setBusy] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [ask, setAsk] = createSignal<Ask | null>(null);
  const [listing, setListing] = createSignal(false);
  let box!: HTMLDivElement;
  const rel = (p: string) => p.replace(/^\/+/, "");
  const join = (name: string) => `${path().replace(/\/+$/, "")}/${name}`;
  const load = async (p: string) => {
    setError(null);
    setListing(true);
    try {
      // `~`: the login folder, from the host check.
      const abs = p === "~" ? (await checkHost(props.host)).home : p;
      setEntries(await ipc.listDir(root(), rel(abs)));
      setPath(abs || "/");
      setPicked([]);
    } catch (e) {
      if (isHostKeyError(e)) props.onHostKey();
      setError(errText(e));
    } finally {
      setListing(false);
    }
  };
  onMount(() => void load(path()));
  const run = async (label: string, fn: () => Promise<unknown>) => {
    setBusy(label);
    setError(null);
    try {
      await fn();
      await load(path());
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(null);
    }
  };
  const upload = (dirs: boolean) => run("upload", async () => {
    const f = await openDialog({ multiple: true, directory: dirs, title: dirs ? t("Upload folders") : t("Upload files") });
    const list = Array.isArray(f) ? f : typeof f === "string" ? [f] : [];
    if (list.length) await transfer(props.host.alias, true, list, path());
  });
  const download = () => run("download", async () => {
    const dest = await openDialog({ directory: true, title: t("Download to folder") });
    if (typeof dest === "string") {
      await transfer(props.host.alias, false, picked().map(join), dest);
      store.notify(t("Downloaded to {path}", { path: dest }));
    }
  });
  const mkdir = () => setAsk({ label: t("New folder name"), value: "", ok: (v) => v.trim() && void run("mkdir", () => ipc.createEntry(root(), rel(join(v.trim())), true)) });
  const rename = () => {
    const from = picked()[0];
    if (from) setAsk({ label: t("Rename {name} to", { name: from }), value: from, ok: (v) => v.trim() && v.trim() !== from && void run("rename", () => ipc.renameEntry(root(), rel(join(from)), rel(join(v.trim())))) });
  };
  const remove = async () => {
    if (!picked().length || !(await confirmDialog(t("Delete {n} item(s) on {host}? There is no trash on the host.", { n: picked().length, host: props.host.label })))) return;
    void run("delete", async () => { for (const n of picked()) await ipc.deleteEntry(root(), rel(join(n))); });
  };
  // Files dropped from Explorer / Finder onto the list upload into the current folder.
  onMount(() => {
    const un = getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type !== "drop") return;
      const { x, y } = event.payload.position;
      const ratio = window.devicePixelRatio || 1;
      const hit = document.elementFromPoint(x / ratio, y / ratio);
      const paths = event.payload.paths;
      if (hit && box.contains(hit) && paths.length) void run("upload", () => transfer(props.host.alias, true, paths, path()));
    });
    onCleanup(() => void un.then((f) => f()));
  });
  const fmt = (n: number) => (n < 1024 ? `${n} B` : n < 1 << 20 ? `${(n / 1024).toFixed(1)} KB` : n < 1 << 30 ? `${(n / (1 << 20)).toFixed(1)} MB` : `${(n / (1 << 30)).toFixed(1)} GB`);
  return (
    <div class="sftp" ref={box} data-sftp>
      <LoadBar when={listing() || busy()} />
      <div class="inline-fields">
        <button class="icon-btn" title={t("Parent folder")} onClick={() => void load(path().replace(/\/[^/]*\/?$/, "") || "/")}><Icon name="up" size={14} /></button>
        <input class="text-input mono" style={{ flex: 1, width: "auto" }} value={path()} onKeyDown={(e) => e.key === "Enter" && void load(e.currentTarget.value.trim() || "/")} />
        <button class="icon-btn" title={t("Refresh")} onClick={() => void load(path())}><Icon name="refresh" size={14} /></button>
      </div>
      <div class="inline-fields" style={{ "flex-wrap": "wrap" }}>
        <button class="btn" data-action="upload" disabled={!!busy()} onClick={() => void upload(false)}><Icon name="up" size={13} /> {t("Upload files")}</button>
        <button class="btn" disabled={!!busy()} onClick={() => void upload(true)}>{t("Upload folder")}</button>
        <button class="btn" data-action="download" disabled={!!busy() || !picked().length} onClick={() => void download()}><Icon name="down" size={13} /> {t("Download")}</button>
        <button class="btn" disabled={!!busy()} onClick={mkdir}>{t("New folder")}</button>
        <button class="btn" disabled={!!busy() || picked().length !== 1} onClick={rename}>{t("Rename")}</button>
        <button class="btn" disabled={!!busy() || !picked().length} onClick={remove}>{t("Delete")}</button>
        <Show when={busy()}><span class="muted-inline">{busy() === "upload" ? t("Uploading…") : busy() === "download" ? t("Downloading…") : t("Working…")}</span></Show>
      </div>
      <Show when={ask()} keyed>{(a) => <AskRow ask={a} onClose={() => setAsk(null)} />}</Show>
      <Show when={error()}><div class="form-note error">{error()}</div></Show>
      <div class="sftp-list">
        <Show when={entries()} fallback={<div class="muted">{t("Loading…")}</div>}>
          <For each={entries()} fallback={<div class="muted">{t("Empty folder. Drop files here to upload them.")}</div>}>
            {(e) => (
              <div class={`sftp-row${picked().includes(e.name) ? " on" : ""}`} data-entry={e.name}
                onClick={(ev) => setPicked(ev.ctrlKey || ev.metaKey ? (picked().includes(e.name) ? picked().filter((x) => x !== e.name) : [...picked(), e.name]) : [e.name])}
                onDblClick={() => e.is_dir && void load(join(e.name))}>
                <Icon name={e.is_dir ? "folder" : "files"} size={13} />
                <span class="sftp-name">{e.name}</span>
                <span class="muted-inline">{e.is_dir ? "" : fmt(e.size)}</span>
              </div>
            )}
          </For>
        </Show>
      </div>
      <div class="form-note">{t("Double-click a folder to open it; Ctrl+click selects several. Drop files from your computer here to upload them.")}</div>
    </div>
  );
}

// ---------- port forwards ----------

const [states, setStates] = createSignal<Record<string, { running: boolean; error: string }>>({});
const refreshStates = () => forwardStatus().then(setStates).catch(() => {});

function ForwardRow(props: { f: SshForward; onEdit?: () => void }) {
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const st = () => states()[props.f.id];
  const host = () => hostById(props.f.hostId);
  const toggle = async () => {
    setBusy(true);
    setError(null);
    try {
      if (st()?.running) await forwardStop(props.f.id);
      else await forwardStart(props.f.id);
    } catch (e) {
      setError(errText(e));
    } finally {
      setBusy(false);
      void refreshStates();
    }
  };
  const describe = () => {
    const f = props.f;
    const bind = `${f.bindAddress || "localhost"}:${f.bindPort}`;
    if (f.kind === "dynamic") return t("SOCKS proxy on {bind}", { bind });
    if (f.kind === "remote") return t("{host} port {bind} → {dest} here", { host: host()?.label ?? "?", bind, dest: `${f.destHost}:${f.destPort}` });
    return t("{bind} here → {dest} from {host}", { bind, dest: `${f.destHost}:${f.destPort}`, host: host()?.label ?? "?" });
  };
  return (
    <div class="forward-row" data-forward={props.f.id}>
      <LoadBar when={busy()} />
      <span class={`fw-state ${st()?.running ? "on" : st()?.error ? "err" : ""}`} />
      <div style={{ flex: 1, "min-width": 0 }}>
        <div>{props.f.label || describe()}</div>
        <div class="muted-inline" style={{ "font-size": "11.5px" }}>{props.f.label ? describe() : ""}{props.f.autoStart ? ` · ${t("starts with Kelpie")}` : ""}</div>
        <Show when={error() || (!st()?.running && st()?.error)}><div class="form-note error">{error() || st()?.error}</div></Show>
      </div>
      <button class={`btn${st()?.running ? "" : " primary"}`} data-action="toggle-forward" disabled={busy()} onClick={() => void toggle()}>{st()?.running ? t("Stop") : t("Start")}</button>
      <Show when={props.onEdit}><button class="btn" onClick={props.onEdit}>{t("Edit")}</button></Show>
    </div>
  );
}

function ForwardEditor(props: { f: SshForward; onDone: () => void }) {
  const [f, setF] = createSignal<SshForward>(props.f);
  const [error, setError] = createSignal<string | null>(null);
  const set = <K extends keyof SshForward>(k: K, v: SshForward[K]) => setF({ ...f(), [k]: v });
  const save = async () => {
    try {
      await saveForward(f());
      props.onDone();
    } catch (e) {
      setError(errText(e));
    }
  };
  const del = async () => {
    await removeForward(f().id).catch((e) => setError(errText(e)));
    props.onDone();
  };
  return (
    <div class="forward-editor">
      <div class="form-row"><label>{t("Host")}</label>
        <select class="text-input" value={f().hostId} onChange={(e) => set("hostId", e.currentTarget.value)}>
          <For each={vault().hosts}>{(h) => <option value={h.id}>{h.label}</option>}</For>
        </select>
      </div>
      <div class="form-row"><label>{t("Type")}</label>
        <div class="seg" style={{ width: "360px" }}>
          <button class={f().kind === "local" ? "on" : ""} onClick={() => set("kind", "local")}>{t("Local (-L)")}</button>
          <button class={f().kind === "remote" ? "on" : ""} onClick={() => set("kind", "remote")}>{t("Remote (-R)")}</button>
          <button class={f().kind === "dynamic" ? "on" : ""} onClick={() => set("kind", "dynamic")}>{t("SOCKS (-D)")}</button>
        </div>
      </div>
      <div class="form-row"><label>{f().kind === "remote" ? t("Listen on the host") : t("Listen here")}</label>
        <div class="inline-fields">
          <input class="text-input mono" placeholder="localhost" value={f().bindAddress} onInput={(e) => set("bindAddress", e.currentTarget.value)} /> {/* i18n-ignore */}
          <span>:</span>
          <input class="text-input mono" data-field="bind-port" style={{ width: "80px" }} placeholder="8080" value={f().bindPort || ""} onInput={(e) => set("bindPort", Number(e.currentTarget.value) || 0)} />
        </div>
      </div>
      <Show when={f().kind !== "dynamic"}>
        <div class="form-row"><label>{f().kind === "remote" ? t("Send to (from here)") : t("Destination (from the host)")}</label>
          <div class="inline-fields">
            <input class="text-input mono" data-field="dest-host" value={f().destHost} onInput={(e) => set("destHost", e.currentTarget.value)} />
            <span>:</span>
            <input class="text-input mono" data-field="dest-port" style={{ width: "80px" }} placeholder="80" value={f().destPort || ""} onInput={(e) => set("destPort", Number(e.currentTarget.value) || 0)} />
          </div>
        </div>
      </Show>
      <div class="form-row"><label>{t("Name (optional)")}</label><input class="text-input" value={f().label} onInput={(e) => set("label", e.currentTarget.value)} /></div>
      <div class="form-row"><label /><label class="check-label"><input type="checkbox" checked={f().autoStart} onChange={(e) => set("autoStart", e.currentTarget.checked)} /> {t("Start when Kelpie opens")}</label></div>
      <Show when={error()}><div class="form-note error">{error()}</div></Show>
      <div class="inline-fields">
        <button class="btn primary" data-action="save-forward" disabled={!f().hostId || !f().bindPort} onClick={() => void save()}>{t("Save")}</button>
        <button class="btn" onClick={props.onDone}>{t("Cancel")}</button>
        <Show when={f().id}><button class="btn" onClick={() => void del()}>{t("Delete")}</button></Show>
      </div>
    </div>
  );
}

function usePolling() {
  void refreshStates();
  const timer = setInterval(() => void refreshStates(), 2000);
  onCleanup(() => clearInterval(timer));
}

function HostForwards(props: { host: SshHost }) {
  usePolling();
  const [editing, setEditing] = createSignal<SshForward | null>(null);
  const mine = () => vault().forwards.filter((f) => f.hostId === props.host.id);
  return (
    <div class="host-forwards">
      <div class="section-label">{t("Port forwarding")}<span class="spacer" /><button class="btn" data-action="new-forward" onClick={() => setEditing(newForward(props.host.id))}><Icon name="plus" size={12} /> {t("Add")}</button></div>
      <Show when={editing()} keyed>{(f) => <ForwardEditor f={f} onDone={() => setEditing(null)} />}</Show>
      <For each={mine()} fallback={<div class="muted-inline" style={{ "font-size": "12px" }}>{t("No port forwards for this host.")}</div>}>{(f) => <ForwardRow f={f} onEdit={() => setEditing({ ...f })} />}</For>
    </div>
  );
}

function ForwardsTab() {
  usePolling();
  const [editing, setEditing] = createSignal<SshForward | null>(null);
  return (
    <div class="hosts-section">
      <div class="inline-fields">
        <div class="form-note" style={{ flex: 1 }}>{t("Forwards run while Kelpie is open; the ones marked to start with Kelpie come back when it opens.")}</div>
        <button class="btn primary" disabled={!vault().hosts.length} onClick={() => setEditing(newForward(vault().hosts[0]?.id ?? ""))}><Icon name="plus" size={13} /> {t("New forward")}</button>
      </div>
      <Show when={editing()} keyed>{(f) => <ForwardEditor f={f} onDone={() => setEditing(null)} />}</Show>
      <For each={vault().forwards} fallback={<div class="muted">{t("No port forwards yet.")}</div>}>{(f) => <ForwardRow f={f} onEdit={() => setEditing({ ...f })} />}</For>
    </div>
  );
}

// ---------- keys ----------

function KeysTab() {
  const [keys, { refetch }] = createResource(() => listKeys().catch(() => []));
  const [agent, { refetch: refetchAgent }] = createResource(() => agentList().catch((e) => ({ running: false, keys: [], error: String(e) })));
  const [gen, setGen] = createSignal<{ name: string; kind: string; passphrase: string; comment: string; save: boolean } | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [installFor, setInstallFor] = createSignal<string | null>(null);
  const [ask, setAsk] = createSignal<Ask | null>(null);
  const act = async (fn: () => Promise<unknown>, done?: string) => {
    setError(null);
    try {
      await fn();
      if (done) store.notify(done);
      void refetch();
      void refetchAgent();
    } catch (e) {
      setError(errText(e));
    }
  };
  const doImport = async () => {
    const f = await openDialog({ multiple: false, directory: false, title: t("Choose a private key") });
    if (typeof f !== "string") return;
    setAsk({
      label: t("Name for the key in ~/.ssh"), value: f.split(/[\\/]/).pop() ?? "imported_key",
      ok: (name) => name.trim() && setAsk({ label: t("Passphrase (leave empty if it has none)"), value: "", secret: true, ok: (pass) => void act(() => importKey(f, name.trim(), pass), t("Key imported")) }),
    });
  };
  const savePassphrase = (path: string) =>
    setAsk({ label: t("Passphrase of this key (kept in the system credential store; empty forgets it)"), value: "", secret: true, ok: (p) => void act(() => setSecret("passphrase", path, p), p ? t("Passphrase saved") : t("Passphrase forgotten")) });
  return (
    <div class="hosts-section">
      <div class="inline-fields">
        <button class="btn primary" data-action="generate-key" onClick={() => setGen({ name: "id_ed25519_kelpie", kind: "ed25519", passphrase: "", comment: "", save: true })}><Icon name="plus" size={13} /> {t("Generate key")}</button>
        <button class="btn" onClick={() => void doImport()}>{t("Import key…")}</button>
        <span style={{ flex: 1 }} />
        <span class="muted-inline">{agent()?.running ? t("SSH agent: {n} key(s) loaded", { n: agent()!.keys.length }) : t("SSH agent not running")}</span>
      </div>
      <Show when={ask()} keyed>{(a) => <AskRow ask={a} onClose={() => setAsk(null)} />}</Show>
      <Show when={gen()}>
        {(g) => (
          <div class="forward-editor">
            <div class="form-row"><label>{t("File name")}</label><input class="text-input mono" data-field="key-name" value={g().name} onInput={(e) => setGen({ ...g(), name: e.currentTarget.value })} /></div>
            <div class="form-row"><label>{t("Type")}</label>
              <div class="seg" style={{ width: "260px" }}>
                <For each={[["ed25519", "Ed25519"], ["rsa", "RSA 4096"], ["ecdsa", "ECDSA"]]}>{([k, l]) => <button class={g().kind === k ? "on" : ""} onClick={() => setGen({ ...g(), kind: k })}>{l}</button>}</For>
              </div>
            </div>
            <div class="form-row"><label>{t("Passphrase")}</label><input class="text-input" type="password" autocomplete="off" value={g().passphrase} placeholder={t("optional")} onInput={(e) => setGen({ ...g(), passphrase: e.currentTarget.value })} /></div>
            <div class="form-row"><label>{t("Comment")}</label><input class="text-input" value={g().comment} placeholder={t("e.g. you@laptop")} onInput={(e) => setGen({ ...g(), comment: e.currentTarget.value })} /></div>
            <div class="form-row"><label /><label class="check-label"><input type="checkbox" checked={g().save} onChange={(e) => setGen({ ...g(), save: e.currentTarget.checked })} /> {t("Save the passphrase so Kelpie can unlock the key")}</label></div>
            <div class="inline-fields">
              <button class="btn primary" data-action="create-key" onClick={() => void act(async () => { await generateKey(g().name.trim(), g().kind, g().passphrase, g().comment, g().save); setGen(null); }, t("Key created"))}>{t("Create")}</button>
              <button class="btn" onClick={() => setGen(null)}>{t("Cancel")}</button>
            </div>
          </div>
        )}
      </Show>
      <Show when={error()}><div class="form-note error">{error()}</div></Show>
      <For each={keys() ?? []} fallback={<div class="muted">{t("No keys in ~/.ssh yet.")}</div>}>
        {(k) => (
          <div class="key-row" data-key={k.name}>
            <Icon name="target" size={16} />
            <div style={{ flex: 1, "min-width": 0 }}>
              <div><b>{k.name}</b> <span class="muted-inline">{k.kind.replace("ssh-", "")} {k.bits ? `· ${k.bits}` : ""} {k.comment ? `· ${k.comment}` : ""}</span></div>
              <div class="mono muted-inline" style={{ "font-size": "11.5px" }}>{k.fingerprint}</div>
              <div class="muted-inline" style={{ "font-size": "11.5px" }}>{k.encrypted ? (k.passphraseSaved ? t("passphrase saved") : t("protected by a passphrase")) : t("no passphrase")}</div>
            </div>
            <button class="btn" onClick={() => void writeText(k.public).then(() => store.notify(t("Public key copied")))}>{t("Copy public key")}</button>
            <button class="btn" disabled={!vault().hosts.length} onClick={() => setInstallFor(installFor() === k.path ? null : k.path)}>{t("Install on host…")}</button>
            <Show when={k.encrypted}><button class="btn" onClick={() => savePassphrase(k.path)}>{t("Passphrase…")}</button></Show>
            <button class="btn" onClick={() => void act(() => agentAdd(k.path), t("Added to the SSH agent"))}>{t("Add to agent")}</button>
            <button class="btn" onClick={async () => (await confirmDialog(t("Move {name} and {name}.pub to the trash?", { name: k.name }))) && void act(() => deleteKey(k.path))}>{t("Delete")}</button>
            <Show when={installFor() === k.path}>
              <div class="key-install">
                <For each={vault().hosts}>{(h) => <button class="btn" onClick={() => void act(() => installKey(h.alias, k.path), t("Key installed on {host}", { host: h.label }))}>{h.label}</button>}</For>
              </div>
            </Show>
          </div>
        )}
      </For>
      <Show when={agent() && !agent()!.running && agent()!.error}><div class="form-note">{t("The SSH agent is not running. On Windows, start the \"OpenSSH Authentication Agent\" service to use it.")}</div></Show>
    </div>
  );
}

// ---------- known hosts ----------

function KnownHostsTab() {
  const [list, { refetch }] = createResource(() => knownHosts().catch(() => []));
  const [query, setQuery] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);
  const shown = () => (list() ?? []).filter((k) => !query().trim() || k.hosts.toLowerCase().includes(query().trim().toLowerCase()) || k.fingerprint.includes(query().trim()));
  const remove = async (line: number, name: string) => {
    if (!(await confirmDialog(t("Forget the key of {name}? ssh asks to trust it again on the next connection.", { name })))) return;
    try {
      await removeKnownHosts([line]);
      void refetch();
    } catch (e) {
      setError(errText(e));
    }
  };
  return (
    <div class="hosts-section">
      <input class="text-input" placeholder={t("Search known hosts")} value={query()} onInput={(e) => setQuery(e.currentTarget.value)} />
      <Show when={error()}><div class="form-note error">{error()}</div></Show>
      <For each={shown()} fallback={<div class="muted">{t("No known hosts.")}</div>}>
        {(k) => (
          <div class="key-row" data-known={k.line}>
            <div style={{ flex: 1, "min-width": 0 }}>
              <div class="mono">{k.hashed ? t("(hashed host name)") : k.hosts} <Show when={k.marker}><span class="chip">{k.marker}</span></Show></div>
              <div class="mono muted-inline" style={{ "font-size": "11.5px" }}>{k.kind} {k.fingerprint}</div>
            </div>
            <button class="btn" onClick={() => void remove(k.line, k.hashed ? k.fingerprint : k.hosts)}>{t("Forget")}</button>
          </div>
        )}
      </For>
    </div>
  );
}
