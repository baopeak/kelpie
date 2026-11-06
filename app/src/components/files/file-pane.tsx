// One pane of the Files tab: this computer or a saved host, with a path bar, sorting, filtering, selection,
// keys, a context menu and drag to the other pane. Listing and changes go through the daemon (`sftp.*`, `local.*`).
import { For, Show, createMemo, createSignal, onMount } from "solid-js";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import * as ipc from "../../lib/ipc";
import { t, errorText } from "../../lib/i18n";
import { confirmDialog } from "../../lib/confirm-dialog";
import { isHostKeyError, type SshHost } from "../../lib/ssh-hosts";
import { baseName, clickSelect, filterEntries, formatSize, formatTime, joinPath, modeString, parentPath, selectAll, sortEntries, type PaneEnd, type Selection, type SortKey } from "../../lib/file-pane";
import type { RemoteEntry } from "../../lib/types";
import { openContextMenu, type MenuItem } from "../context-menu";
import { AskRow, type Ask } from "../ask-row";
import { Icon } from "../icons";
import { LoadBar } from "../load-bar";

/** What the panes around a pane need from it. */
export interface PaneApi {
  end: () => PaneEnd;
  dir: () => string;
  picked: () => string[];
  reload: () => Promise<void>;
}

/** Drag data between panes. */
export const DRAG_TYPE = "application/x-kelpie-files";
export interface DragFiles {
  end: PaneEnd;
  dir: string;
  names: string[];
}

const hiddenKey = (end: PaneEnd) => `kelpie.files.hidden.${end === null ? "local" : "host"}`;
const readHidden = (end: PaneEnd) => {
  try {
    return localStorage.getItem(hiddenKey(end)) === "1";
  } catch {
    return false;
  }
};
const writeHidden = (end: PaneEnd, on: boolean) => {
  try {
    localStorage.setItem(hiddenKey(end), on ? "1" : "0");
  } catch {
    // Not remembered in a private window: still applies now.
  }
};

export function FilePane(props: {
  side: "left" | "right";
  end: PaneEnd;
  start?: string;
  hosts: SshHost[];
  label: (alias: string) => string;
  active: boolean;
  onFocus: () => void;
  onEnd: (end: PaneEnd) => void;
  /** Copy these names of `dir` to the other pane. */
  onCopy: (from: DragFiles) => void;
  /** Files dropped from the other pane. */
  onDropFiles: (from: DragFiles, toDir: string) => void;
  onPermissions: (entries: RemoteEntry[], dir: string) => void;
  onEdit?: (path: string, entry: RemoteEntry, how: "kelpie" | "external") => void;
  onSwitchPane: () => void;
  onHostKey: () => void;
  api: (api: PaneApi) => void;
  ref: (el: HTMLDivElement) => void;
}) {
  const [dir, setDir] = createSignal<string>("");
  const [typed, setTyped] = createSignal("");
  const [entries, setEntries] = createSignal<RemoteEntry[] | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [listing, setListing] = createSignal(false);
  const [hidden, setHidden] = createSignal(readHidden(props.end));
  const [filter, setFilter] = createSignal("");
  const [sort, setSort] = createSignal<{ key: SortKey; asc: boolean }>({ key: "name", asc: true });
  const [sel, setSel] = createSignal<Selection>({ picked: [], anchor: null });
  const [ask, setAsk] = createSignal<Ask | null>(null);
  const host = () => props.end;
  const shown = createMemo(() => sortEntries(filterEntries(entries() ?? [], filter()), sort().key, sort().asc));
  const order = () => shown().filter((e) => e.readable_name).map((e) => e.name);
  const byName = (name: string) => entries()?.find((e) => e.name === name);
  const pickedEntries = () => sel().picked.map(byName).filter((e): e is RemoteEntry => !!e);

  const load = async (path: string) => {
    setError(null);
    setListing(true);
    try {
      const end = host();
      let abs = path;
      if (end !== null && (path === "~" || path === "")) abs = await ipc.sftpRealpath(end);
      const list = end === null ? await ipc.localList(abs, hidden()) : await ipc.sftpList(end, abs, hidden());
      setEntries(list);
      setDir(abs);
      setTyped(abs);
      setSel({ picked: [], anchor: null });
    } catch (e) {
      if (isHostKeyError(e)) props.onHostKey();
      setError(errorText(String(e)));
    } finally {
      setListing(false);
    }
  };
  const reload = () => load(dir());
  props.api({ end: host, dir, picked: () => sel().picked, reload });

  onMount(async () => {
    if (props.start) return void load(props.start);
    if (host() !== null) return void load("~");
    // This computer starts in Downloads (the home folder when there is none).
    const home = await ipc.localHome().catch(() => "");
    const downloads = joinPath(null, home, "Downloads");
    await load(downloads);
    if (error() && home) await load(home);
  });

  const open = (e: RemoteEntry) => {
    const path = joinPath(host(), dir(), e.name);
    if (e.kind === "dir" || (host() === null && dir() === "")) return void load(path);
    if (e.kind === "link") {
      // A link to a folder opens through its target; a link to a file edits like a file.
      const end = host();
      if (end === null) return void load(path);
      void ipc.sftpStat(end, path).then((s) => (s.stat?.kind === "dir" ? ipc.sftpRealpath(end, path).then(load) : props.onEdit?.(path, e, "kelpie"))).catch((err) => setError(errorText(String(err))));
      return;
    }
    props.onEdit?.(path, e, "kelpie");
  };

  const run = async (fn: () => Promise<unknown>) => {
    setError(null);
    try {
      await fn();
      await reload();
    } catch (e) {
      setError(errorText(String(e)));
    }
  };

  const mkdir = () =>
    setAsk({
      label: t("New folder name"), value: "", ok: (v) => {
        const name = v.trim();
        if (!name) return;
        const end = host();
        void run(() => (end === null ? ipc.createEntry(dir(), name, true) : ipc.sftpMkdir(end, joinPath(end, dir(), name))));
      },
    });

  const rename = () => {
    const from = sel().picked[0];
    if (!from) return;
    setAsk({
      label: t("Rename {name} to", { name: from }), value: from, ok: (v) => {
        const to = v.trim();
        if (!to || to === from) return;
        const end = host();
        void run(() => (end === null ? ipc.renameEntry(dir(), from, to) : ipc.sftpRename(end, joinPath(end, dir(), from), joinPath(end, dir(), to))));
      },
    });
  };

  const remove = async () => {
    const names = sel().picked;
    if (!names.length) return;
    const end = host();
    if (end === null) {
      if (!(await confirmDialog(t("Move {n} item(s) to the trash?", { n: names.length })))) return;
      return void run(async () => { for (const n of names) await ipc.deleteEntry(dir(), n); });
    }
    if (!(await confirmDialog(t("Delete {n} item(s) on {host}? There is no trash on the host.", { n: names.length, host: props.label(end) })))) return;
    await run(() => ipc.xferStart({ kind: "delete", src: { dest: end }, items: names.map((n) => joinPath(end, dir(), n)) }));
  };

  const copyOver = () => sel().picked.length && props.onCopy({ end: host(), dir: dir(), names: sel().picked });

  const menu = (ev: MouseEvent, e: RemoteEntry) => {
    if (!sel().picked.includes(e.name)) setSel({ picked: [e.name], anchor: e.name });
    const one = sel().picked.length === 1;
    const path = joinPath(host(), dir(), e.name);
    const isFile = e.kind !== "dir";
    const items: MenuItem[] = [
      { label: t("Open"), run: () => open(e) },
      ...(one && isFile && props.onEdit ? [{ label: t("Edit in Kelpie"), run: () => props.onEdit!(path, e, "kelpie") }, { label: t("Open with external editor"), run: () => props.onEdit!(path, e, "external") }] : []),
      null,
      { label: t("Copy to the other pane"), run: () => copyOver() },
      null,
      ...(one ? [{ label: t("Rename"), run: rename }] : []),
      ...(host() !== null ? [{ label: t("Permissions…"), run: () => props.onPermissions(pickedEntries(), dir()) }] : []),
      { label: t("Copy path"), run: () => void writeText(path) },
      ...(host() === null ? [{ label: t("Show in folder"), run: () => void ipc.revealEntry(dir(), e.name) }] : []),
      null,
      { label: t("Delete"), run: () => void remove(), danger: true },
    ];
    openContextMenu(ev, items);
  };

  const onKey = (ev: KeyboardEvent) => {
    if (ask() || (ev.target as HTMLElement).tagName === "INPUT") return;
    const mod = ev.ctrlKey || ev.metaKey;
    if (mod && ev.key.toLowerCase() === "a") {
      ev.preventDefault();
      setSel(selectAll(order()));
    } else if (ev.key === "Enter") {
      const e = byName(sel().picked[0] ?? "");
      if (e) open(e);
    } else if (ev.key === "Backspace") {
      ev.preventDefault();
      void load(parentPath(host(), dir()));
    } else if (ev.key === "F2") {
      ev.preventDefault();
      rename();
    } else if (ev.key === "Delete") {
      void remove();
    } else if (ev.key === "F5") {
      ev.preventDefault();
      copyOver();
    } else if (ev.key === "Tab") {
      ev.preventDefault();
      props.onSwitchPane();
    } else if (ev.key === "ArrowDown" || ev.key === "ArrowUp") {
      ev.preventDefault();
      const o = order();
      const at = o.indexOf(sel().anchor ?? "");
      const next = o[Math.max(0, Math.min(o.length - 1, at + (ev.key === "ArrowDown" ? 1 : -1)))];
      if (next) setSel(clickSelect(sel(), o, next, { range: ev.shiftKey }));
    }
  };

  const header = (key: SortKey, label: string) => (
    <button class={`files-col ${key}${sort().key === key ? " on" : ""}`} onClick={() => setSort((s) => ({ key, asc: s.key === key ? !s.asc : true }))}>
      {label}{sort().key === key ? (sort().asc ? " ▲" : " ▼") : ""}
    </button>
  );

  const ends = () => [{ value: "", label: t("This computer") }, ...props.hosts.map((h) => ({ value: h.alias, label: h.label }))];

  return (
    <div class={`file-pane${props.active ? " active" : ""}`} data-pane={props.side} ref={props.ref} onFocusIn={props.onFocus} onPointerDown={props.onFocus}
      onDragOver={(ev) => ev.dataTransfer?.types.includes(DRAG_TYPE) && ev.preventDefault()}
      onDrop={(ev) => {
        const raw = ev.dataTransfer?.getData(DRAG_TYPE);
        if (!raw) return;
        ev.preventDefault();
        const from = JSON.parse(raw) as DragFiles;
        if (from.end === host() && from.dir === dir()) return;
        props.onDropFiles(from, dir());
      }}>
      <LoadBar when={listing()} />
      <div class="file-pane-bar">
        <select class="select" data-field="pane-end" value={host() ?? ""} onChange={(e) => props.onEnd(e.currentTarget.value || null)}>
          <For each={ends()}>{(o) => <option value={o.value}>{o.label}</option>}</For>
        </select>
        <button class="icon-btn" title={t("Parent folder")} onClick={() => void load(parentPath(host(), dir()))}><Icon name="up" size={14} /></button>
        <button class="icon-btn" title={t("Home folder")} onClick={() => void (host() === null ? ipc.localHome().then(load) : load("~"))}><Icon name="folder" size={14} /></button>
        <Show when={host() === null && navigator.userAgent.includes("Windows")}>
          <button class="icon-btn" title={t("Drives")} onClick={() => void load("")}><Icon name="server" size={14} /></button>
        </Show>
        <input class="text-input mono file-pane-path" data-field="pane-path" value={typed()} onInput={(e) => setTyped(e.currentTarget.value)}
          onKeyDown={(e) => e.key === "Enter" && void load(e.currentTarget.value.trim() || (host() === null ? "" : "/"))} />
        <button class="icon-btn" title={t("Refresh")} onClick={() => void reload()}><Icon name="refresh" size={14} /></button>
      </div>
      <div class="file-pane-bar">
        <input class="text-input file-pane-filter" placeholder={t("Filter")} value={filter()} onInput={(e) => setFilter(e.currentTarget.value)} />
        <label class="check-inline" title={t("Names that start with a dot, and hidden files")}>
          <input type="checkbox" data-field="show-hidden" checked={hidden()} onChange={(e) => { setHidden(e.currentTarget.checked); writeHidden(host(), e.currentTarget.checked); void reload(); }} /> {t("Hidden")}
        </label>
        <button class="btn" data-action="new-folder" onClick={mkdir}>{t("New folder")}</button>
      </div>
      <Show when={ask()} keyed>{(a) => <AskRow ask={a} onClose={() => setAsk(null)} />}</Show>
      <Show when={error()}><div class="form-note error" data-pane-error>{error()}</div></Show>
      <div class="files-head">
        {header("name", t("Name"))}
        {header("size", t("Size"))}
        {header("mtime", t("Modified"))}
        <Show when={entries()?.some((e) => e.mode != null)}>{header("mode", t("Permissions"))}</Show>
        <Show when={host() !== null}>{header("owner", t("Owner"))}</Show>
      </div>
      <div class="files-list" tabIndex={0} onKeyDown={onKey} data-files-list>
        <Show when={entries()} fallback={<div class="muted">{t("Loading…")}</div>}>
          <For each={shown()} fallback={<div class="muted">{filter() ? t("Nothing matches the filter.") : t("Empty folder. Drop files here to copy them in.")}</div>}>
            {(e) => (
              <div class={`files-row${sel().picked.includes(e.name) ? " on" : ""}${e.readable_name ? "" : " unreadable"}`} data-entry={e.name}
                draggable={e.readable_name}
                title={e.readable_name ? (e.link_target ? `→ ${e.link_target}` : "") : t("This name is not readable text on the host, so Kelpie cannot copy it")}
                onDragStart={(ev) => {
                  if (!sel().picked.includes(e.name)) setSel({ picked: [e.name], anchor: e.name });
                  ev.dataTransfer?.setData(DRAG_TYPE, JSON.stringify({ end: host(), dir: dir(), names: sel().picked } satisfies DragFiles));
                }}
                onClick={(ev) => e.readable_name && setSel(clickSelect(sel(), order(), e.name, { toggle: ev.ctrlKey || ev.metaKey, range: ev.shiftKey }))}
                onDblClick={() => e.readable_name && open(e)}
                onContextMenu={(ev) => e.readable_name && menu(ev, e)}>
                <span class="files-name">
                  <Icon name={e.kind === "dir" || (host() === null && dir() === "") ? "folder" : "files"} size={13} />
                  <span>{e.name}</span>
                  <Show when={e.kind === "link"}><span class="muted-inline" title={e.link_target ?? ""}>↗</span></Show>
                </span>
                <span class="files-size">{e.kind === "dir" ? "" : formatSize(e.size)}</span>
                <span class="files-time">{formatTime(e.mtime)}</span>
                <Show when={entries()?.some((x) => x.mode != null)}><span class="files-mode mono">{modeString(e.mode, e.kind)}</span></Show>
                <Show when={host() !== null}><span class="files-owner">{e.owner ?? ""}{e.group ? `:${e.group}` : ""}</span></Show>
              </div>
            )}
          </For>
        </Show>
      </div>
      <div class="file-pane-foot muted-inline">
        {sel().picked.length ? t("{n} selected", { n: sel().picked.length }) : t("{n} items", { n: entries()?.length ?? 0 })}
        <Show when={dir()}>{" · "}{baseName(dir()) || dir()}</Show>
      </div>
    </div>
  );
}
