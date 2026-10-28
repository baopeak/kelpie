import { For, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import { allPanes } from "../lib/layout";
import { chordLabel } from "../lib/platform";
import { chordOf, commands } from "../lib/commands";
import { basename } from "../lib/platform";
import { Icon, KindGlyph } from "./icons";
import { ContextRing } from "./context-ring";
import { SplitView, paneDrag, startTabDrag } from "./split-view";
import { StatusDot } from "./sidebar";
import { Launcher } from "./welcome";
import { openLayoutMenu } from "./layout-menu";
import { browserState, openBrowser } from "../lib/browser";
import { HostChip } from "./host-target";
import { projectTarget, sessionTarget } from "../lib/remote";
import { kindInfo } from "../lib/agents";
import type { Pane, Project, Tab } from "../lib/types";
import { t } from "../lib/i18n";
import { confirmDialog } from "../lib/confirm-dialog";

export function paneTitle(pane: Pane): string {
  if (pane.sessionId) {
    const s = store.sessions[pane.sessionId];
    return s ? sessionLabel(s) : t("Session");
  }
  if (pane.path) return (pane.kind === "diff" ? "Δ " : "") + basename(pane.path) + (pane.rev ? ` #${pane.rev.n}` : "");
  if (pane.kind === "browser") {
    const title = browserState[pane.id]?.title;
    if (title) return title;
    try {
      return pane.url ? new URL(pane.url).host || t("Browser") : t("Browser");
    } catch {
      return t("Browser");
    }
  }
  return t("Empty pane");
}

/** A pane's agent colour; panes without a session (files, browser, empty) stay neutral. */
const paneColor = (pane: Pane) => {
  const s = pane.sessionId ? store.sessions[pane.sessionId] : undefined;
  return s ? kindInfo(s.kind).color : "var(--text-faint)";
};

function TabTitle(props: { tab: Tab; project: Project }) {
  const panes = createMemo(() => allPanes(props.tab.root));
  const focus = () => panes().find((p) => p.id === props.tab.focus) ?? panes()[0];
  const session = () => (focus()?.sessionId ? store.sessions[focus()!.sessionId!] : undefined);
  /** The focused pane's host when it is not the project's own machine. */
  const host = () => {
    const tg = session() && sessionTarget(session()!);
    return tg && tg.dest !== projectTarget(props.project)?.dest ? tg : undefined;
  };
  return (
    <>
      <Show when={session()} fallback={<Icon name={focus()?.kind === "diff" ? "git" : "files"} size={14} />}>
        {(s) => <StatusDot s={s()} />}
      </Show>
      <Show when={session()}>{(s) => <KindGlyph kind={s().kind} size={14} />}</Show>
      <Show when={host()}>{(tg) => <HostChip target={tg()} />}</Show>
      <Show when={props.tab.broadcast}><span class="badge broadcast-badge" title={t("Broadcast input is on")}>⇶</span></Show>
      <span class="title">{props.tab.name ?? paneTitle(focus())}</span>
      <Show when={session()}>{(s) => <ContextRing s={s()} size={12} />}</Show>
      <Show when={panes().length > 1}>
        <span class="tab-panes">
          <For each={panes()}>{(p) => <i classList={{ on: p.id === focus()?.id }} style={{ background: paneColor(p) }} />}</For>
        </span>
      </Show>
    </>
  );
}

export function WorkspaceView(props: { project: Project }) {
  const ws = () => store.workspace(props.project.id);
  const active = () => store.activeTab(props.project.id);
  const [renaming, setRenaming] = createSignal<string | null>(null);
  const hint = (id: string) => {
    const c = commands.find((x) => x.id === id);
    const k = c && chordOf(c);
    return k ? ` (${chordLabel(k)})` : "";
  };

  const closeTab = async (tab: Tab) => {
    const live = allPanes(tab.root).filter((p) => p.sessionId && store.sessions[p.sessionId]?.alive && store.sessions[p.sessionId]?.kind !== "shell");
    if (live.length && !(await confirmDialog(live.length === 1 ? t("Stop {n} running agent session in this tab?", { n: live.length }) : t("Stop {n} running agent sessions in this tab?", { n: live.length })))) return;
    for (const pane of allPanes(tab.root)) await store.closePane(props.project.id, tab.id, pane.id);
  };

  // The active tab stays in view when tabs overflow: on selection and when the bar resizes.
  let tabsRef!: HTMLDivElement;
  const showActive = () => tabsRef?.querySelector(".tab.active")?.scrollIntoView({ block: "nearest", inline: "nearest" });
  createEffect(() => {
    active()?.id;
    queueMicrotask(showActive);
  });
  onMount(() => {
    const ro = new ResizeObserver(showActive);
    ro.observe(tabsRef);
    onCleanup(() => ro.disconnect());
  });

  return (
    <>
      <div class={`tabbar${ws().rightPanel ? " panel-open" : ""}${paneDrag()?.toTab === "new" ? " drop-new" : ""}`}>
        <div class="tabs" ref={tabsRef}>
          <For each={ws().tabs}>
            {(tab) => (
              <div
                class={`tab${active()?.id === tab.id ? " active" : ""}${paneDrag()?.toTab === tab.id ? " drop-target" : ""}${paneDrag()?.tabId === tab.id && !paneDrag()?.paneId ? " dragging" : ""}`}
                data-tab-id={tab.id}
                title={t("Drag to reorder, or onto a pane edge to merge")}
                onPointerDown={(e) => renaming() !== tab.id && startTabDrag(e, props.project, tab)}
                onClick={() => store.selectTab(props.project.id, tab.id)}
                onDblClick={() => setRenaming(tab.id)}
                onAuxClick={(e) => e.button === 1 && void closeTab(tab)}
              >
                <Show when={renaming() === tab.id} fallback={<TabTitle tab={tab} project={props.project} />}>
                  <input
                    class="tab-rename"
                    value={tab.name ?? ""}
                    placeholder={t("Tab name")}
                    ref={(el) => queueMicrotask(() => { el.focus(); el.select(); })}
                    onClick={(e) => e.stopPropagation()}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") e.currentTarget.blur();
                      if (e.key === "Escape") { e.currentTarget.value = tab.name ?? ""; e.currentTarget.blur(); }
                    }}
                    onBlur={(e) => { store.renameTab(props.project.id, tab.id, e.currentTarget.value); setRenaming(null); }}
                  />
                </Show>
                <button class="icon-btn close" onClick={(e) => { e.stopPropagation(); void closeTab(tab); }}><Icon name="close" size={12} /></button>
              </div>
            )}
          </For>
        </div>
        <Show when={paneDrag()?.barX}>
          {(x) => <div class="tab-insert" style={{ left: `${x()}px`, top: `${tabsRef.getBoundingClientRect().top + 5}px` }} />}
        </Show>
        <div class="tabbar-actions">
          <button class="icon-btn" title={t("New terminal") + hint("new.shell")} onClick={() => void store.newSession(props.project.id, "shell")}><Icon name="terminal" /></button>
          <button class="icon-btn" title={t("New session…") + hint("new.menu")} onClick={() => store.setOverlay("newsession")}><Icon name="plus" /></button>
          <button class="icon-btn" title={t("Split right") + hint("split.right")} onClick={() => store.splitPicker(props.project.id, "right")}><Icon name="splitRight" /></button>
          <button class="icon-btn" title={t("Split down") + hint("split.down")} onClick={() => store.splitPicker(props.project.id, "down")}><Icon name="splitDown" /></button>
          <button class="icon-btn" title={t("Browser") + hint("browser.open")} onClick={() => openBrowser()}><Icon name="globe" /></button>
          <button class="icon-btn layout-btn" title={t("Layout: grid, equal sizes, broadcast, saved layouts") + hint("layout.menu")} disabled={!active()} onClick={(e) => openLayoutMenu(e, props.project, active())}><Icon name="grid" /></button>
          <span class="panel-toggle" style={{ width: "8px" }} />
          <button class={`icon-btn panel-toggle${ws().rightPanel === "files" ? " active" : ""}`} title={t("Explorer") + hint("panel.files")} onClick={() => store.setRightPanel("files")}><Icon name="files" /></button>
          <button class={`icon-btn panel-toggle${ws().rightPanel === "git" ? " active" : ""}`} title={t("Source control") + hint("panel.git")} onClick={() => store.setRightPanel("git")}><Icon name="git" /></button>
          <button class={`icon-btn panel-toggle${ws().rightPanel === "search" ? " active" : ""}`} title={t("Search") + hint("panel.search")} onClick={() => store.setRightPanel("search")}><Icon name="search" /></button>
        </div>
      </div>
      <div class="tab-content">
        <Show when={active()} keyed fallback={<Launcher project={props.project} />}>
          {(tab) => <SplitView project={props.project} tab={tab} />}
        </Show>
      </div>
    </>
  );
}
