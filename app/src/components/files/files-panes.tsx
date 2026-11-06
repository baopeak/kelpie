// The Files tab of a host: two panes side by side (this computer and the host, or two hosts), copy between them by
// drag, buttons, the menu or F5, drops from Explorer / Finder, and the transfer queue below. Every copy is a job of
// the daemon's queue (`xfer.start`), so it outlives the window.
import { For, Show, createEffect, createSignal, on, onCleanup, onMount } from "solid-js";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { t, errorText } from "../../lib/i18n";
import * as ipc from "../../lib/ipc";
import { store } from "../../lib/store";
import { vault, type SshHost } from "../../lib/ssh-hosts";
import { joinPath, type PaneEnd } from "../../lib/file-pane";
import { filesViewClosed, filesViewOpened, initTransfers, isFinished, jobList } from "../../lib/transfers";
import type { RemoteEntry, XferRoute, XferStart } from "../../lib/types";
import { FilePane, type DragFiles, type PaneApi } from "./file-pane";
import { TransferQueue } from "./transfer-queue";
import { PermissionsDialog } from "./permissions-dialog";
import { EditSessions } from "./edit-sessions";
import { editExternal, editInKelpie, initRemoteEdit } from "../../lib/remote-edit";
import { Icon } from "../icons";

type Side = "left" | "right";

export function FilesPanes(props: { host: SshHost; onHostKey: () => void }) {
  initTransfers();
  initRemoteEdit();
  // Conflict questions show while a Files view is open (elsewhere from the status bar).
  filesViewOpened();
  onCleanup(filesViewClosed);
  const [ends, setEnds] = createSignal<Record<Side, PaneEnd>>({ left: null, right: props.host.alias });
  const [active, setActive] = createSignal<Side>("right");
  const [error, setError] = createSignal<string | null>(null);
  const [perms, setPerms] = createSignal<{ dest: string; dir: string; entries: RemoteEntry[] } | null>(null);
  /** Host to host: Auto (direct when the first host logs in to the second by itself) or always through this computer. */
  const [route, setRoute] = createSignal<XferRoute>("auto");
  const hostToHost = () => ends().left !== null && ends().right !== null && store.daemonApi() >= ipc.ROUTE_API;
  const apis: Partial<Record<Side, PaneApi>> = {};
  const boxes: Partial<Record<Side, HTMLDivElement>> = {};
  const other = (s: Side): Side => (s === "left" ? "right" : "left");
  const label = (alias: string) => vault().hosts.find((h) => h.alias === alias)?.label ?? alias;
  const tooOld = () => store.daemonApi() > 0 && store.daemonApi() < ipc.XFER_API;

  const copy = async (from: DragFiles, to: Side, toDir?: string) => {
    setError(null);
    const dst = ends()[to];
    if (from.end === null && dst === null) return setError(t("Copies on this computer are for Explorer or Finder"));
    const dir = toDir ?? apis[to]?.dir() ?? "";
    if (!dir) return setError(t("Open a folder in the other pane first"));
    const job: XferStart = {
      kind: "copy",
      src: from.end === null ? {} : { dest: from.end },
      dst: dst === null ? {} : { dest: dst },
      items: from.names.map((n) => joinPath(from.end, from.dir, n)),
      dst_dir: dir,
      conflict: "ask",
      ...(from.end !== null && dst !== null && hostToHost() ? { route: route() } : {}),
    };
    try {
      await ipc.xferStart(job);
    } catch (e) {
      setError(errorText(String(e)));
    }
  };
  // A host's file: in Kelpie's editor (a remote project), or a private copy in an external one, uploaded on save.
  const edit = async (alias: string, path: string, how: "kelpie" | "external") => {
    setError(null);
    try {
      const host = vault().hosts.find((h) => h.alias === alias);
      if (how === "kelpie" && host) await editInKelpie(host, path);
      else await editExternal(alias, path);
    } catch (e) {
      setError(errorText(String(e)));
    }
  };
  const copyFrom = (side: Side) => {
    const api = apis[side];
    if (api && api.picked().length) void copy({ end: api.end(), dir: api.dir(), names: api.picked() }, other(side));
  };

  // A job that finishes refreshes the panes showing its target (or, for a delete or chmod, its folder).
  // Jobs that had already finished when the view opened are not news.
  const seen = new Set<string>(jobList().filter(isFinished).map((j) => j.id));
  createEffect(on(jobList, (list) => {
    for (const j of list) {
      if (!isFinished(j) || seen.has(j.id)) continue;
      seen.add(j.id);
      const end = j.kind === "copy" ? j.dst.dest ?? null : j.src.dest ?? null;
      for (const side of ["left", "right"] as Side[]) if (ends()[side] === end) void apis[side]?.reload();
    }
  }));

  // Files dropped from Explorer / Finder: onto a host's pane they upload into its folder.
  onMount(() => {
    const un = getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type !== "drop") return;
      const { x, y } = event.payload.position;
      const ratio = window.devicePixelRatio || 1;
      const hit = document.elementFromPoint(x / ratio, y / ratio);
      const side = (["left", "right"] as Side[]).find((s) => hit && boxes[s]?.contains(hit));
      if (!side || !event.payload.paths.length) return;
      const dst = ends()[side];
      const dir = apis[side]?.dir();
      if (dst === null) return setError(t("Copies on this computer are for Explorer or Finder"));
      if (!dir) return;
      void ipc.xferStart({ kind: "copy", src: {}, dst: { dest: dst }, items: event.payload.paths, dst_dir: dir, conflict: "ask" }).catch((e) => setError(errorText(String(e))));
    });
    onCleanup(() => void un.then((f) => f()));
  });

  const pane = (side: Side) => (
    // Keyed on the end: picking another host makes a fresh pane.
    <For each={[ends()[side]]}>
      {(end) => (
        <FilePane side={side} end={end} hosts={vault().hosts} label={label} active={active() === side}
          onFocus={() => setActive(side)}
          onEnd={(e) => setEnds({ ...ends(), [side]: e })}
          onCopy={(from) => void copy(from, other(side))}
          onDropFiles={(from, dir) => void copy(from, side, dir)}
          onPermissions={(entries, dir) => end !== null && setPerms({ dest: end, dir, entries })}
          onEdit={end === null ? undefined : (path, _entry, how) => void edit(end, path, how)}
          onSwitchPane={() => {
            setActive(other(side));
            boxes[other(side)]?.querySelector<HTMLElement>("[data-files-list]")?.focus();
          }}
          onHostKey={props.onHostKey}
          api={(a) => (apis[side] = a)}
          ref={(el) => (boxes[side] = el)} />
      )}
    </For>
  );

  return (
    <Show when={!tooOld()} fallback={<div class="form-note error" data-files-too-old>{t("Restart Kelpie's background service to use the file manager")}</div>}>
      <div class="files-tab" data-files>
        <Show when={error()}><div class="form-note error" data-files-error>{error()}</div></Show>
        <div class="files-panes">
          {pane("left")}
          <div class="files-mid">
            <button class="icon-btn" data-action="copy-right" title={t("Copy the selection to the right pane (F5)")} onClick={() => copyFrom("left")}><Icon name="forward" size={14} /></button>
            <button class="icon-btn" data-action="copy-left" title={t("Copy the selection to the left pane (F5)")} onClick={() => copyFrom("right")}><Icon name="back" size={14} /></button>
            <button class="icon-btn" data-action="swap-panes" title={t("Swap the panes")} onClick={() => setEnds({ left: ends().right, right: ends().left })}><Icon name="sync" size={14} /></button>
            <Show when={hostToHost()}>
              <select class="select files-route" data-field="route" value={route()} onChange={(e) => setRoute(e.currentTarget.value as XferRoute)}
                title={t("Auto copies directly when the first host can log in to the second by itself (Kelpie never lends it your keys); otherwise the data goes through this computer")}>
                <option value="auto">{t("Copy (Auto)")}</option>
                <option value="relay">{t("Copy through this computer")}</option>
              </select>
            </Show>
          </div>
          {pane("right")}
        </div>
        <EditSessions label={label} />
        <TransferQueue label={label} />
        <Show when={perms()} keyed>
          {(p) => <PermissionsDialog dest={p.dest} dir={p.dir} entries={p.entries} onClose={() => setPerms(null)} onDone={() => (["left", "right"] as Side[]).forEach((s) => ends()[s] === p.dest && void apis[s]?.reload())} />}
        </Show>
      </div>
    </Show>
  );
}
