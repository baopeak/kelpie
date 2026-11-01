// Files panel: header actions, a name filter over the project index, and Names / Contents modes
// (Contents is search-in-files). Mirrors the explorer ergonomics users know from Orca/VS Code.
import { For, Match, Show, Switch, createEffect, createMemo, createSignal, on } from "solid-js";
import { store } from "../lib/store";
import { quickOpen, type QuickOpenHit } from "../lib/ipc";
import { gitState } from "../lib/git-state";
import { FileTree, type FileTreeApi } from "./file-tree";
import { SearchPanel } from "./search-panel";
import { Icon } from "./icons";
import type { Project } from "../lib/types";
import { t } from "../lib/i18n";
import { LoadBar } from "./load-bar";

export function Explorer(props: { project: Project }) {
  const mode = () => (store.workspace(props.project.id).rightPanel === "search" ? "contents" : "names");
  const [filter, setFilter] = createSignal("");
  const [hits, setHits] = createSignal<QuickOpenHit[]>([]);
  const [finding, setFinding] = createSignal(false);
  let api: FileTreeApi | undefined;
  let seq = 0;

  createEffect(on(filter, (q) => {
    const mine = ++seq;
    setFinding(!!q.trim());
    if (!q.trim()) return setHits([]);
    void quickOpen(props.project.path, q, 200)
      .then((h) => mine === seq && setHits(h))
      .catch(() => mine === seq && setHits([]))
      .finally(() => mine === seq && setFinding(false));
  }));

  const deco = createMemo(() => gitState.decorations());
  const filtering = () => !!filter().trim();

  return (
    <div class="explorer">
      <LoadBar when={finding()} />
      <div class="explorer-head">
        <span class="explorer-title" title={props.project.path}>{props.project.name}</span>
        <button class="icon-btn" title={t("Collapse all")} onClick={() => api?.collapseAll()}><Icon name="minus" size={14} /></button>
        <button class="icon-btn" title={t("Refresh")} onClick={() => { api?.refresh(); void gitState.refresh(); }}><Icon name="refresh" size={14} /></button>
      </div>
      <div class="explorer-tools">
        <Show when={mode() === "names"}>
          <input class="text-input" placeholder={t("Filter files by name")} value={filter()} onInput={(e) => setFilter(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") setFilter("");
              if (e.key === "Enter" && hits()[0]) store.openFile(props.project.id, hits()[0].path);
            }} />
        </Show>
        <div class="seg">
          <button class={mode() === "names" ? "on" : ""} onClick={() => store.showRightPanel("files")}>{t("Names")}</button>
          <button class={mode() === "contents" ? "on" : ""} onClick={() => store.showRightPanel("search")}>{t("Contents")}</button>
        </div>
      </div>
      <Show when={mode() === "contents"}><SearchPanel project={props.project} /></Show>
      {/* The tree stays mounted (hidden) so expansion and scroll survive filtering. */}
      <div style={{ display: mode() === "names" && !filtering() ? "contents" : "none" }}>
        <FileTree project={props.project} api={(a) => (api = a)} />
      </div>
      <Switch>
        <Match when={mode() === "names" && filtering()}>
          <div class="list">
            <For each={hits()} fallback={<div class="muted">{t("No matching files")}</div>}>
              {(h) => {
                const name = h.path.split("/").pop()!;
                const dir = h.path.slice(0, h.path.length - name.length - 1);
                const st = () => deco().get(h.path);
                return (
                  <div class={`list-row${st() ? ` g-${st()}` : ""}`} onClick={() => store.openFile(props.project.id, h.path)} title={h.path}>
                    <span class="name">{name}</span>
                    <span class="dir">{dir}</span>
                  </div>
                );
              }}
            </For>
          </div>
        </Match>
      </Switch>
    </div>
  );
}
