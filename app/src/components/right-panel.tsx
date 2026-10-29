import { Match, Show, Switch, createEffect, lazy, onCleanup } from "solid-js";
import { store } from "../lib/store";
import { unwatchRoot, watchRoot } from "../lib/ipc";
import { gitState } from "../lib/git-state";
import { Explorer } from "./explorer";
import { GitPanel } from "./git-panel";
import { HistoryPanel } from "./history-panel";
import { IssuesPanel } from "./issues-panel";
import { CHECKPOINT_API } from "../lib/checkpoint-api";
import { PROCESS_API } from "../lib/process-api";
import { inReview } from "../lib/review-queue";
import type { Project } from "../lib/types";
import { t } from "../lib/i18n";

const TurnTimeline = lazy(() => import("./turn-timeline"));
const ReviewPanel = lazy(() => import("./review-panel"));
const PlanPanel = lazy(() => import("./plan-panel"));
const ProcessesPanel = lazy(() => import("./processes-panel"));

export function RightPanel(props: { project: Project }) {
  const panel = () => store.workspace(props.project.id).rightPanel;
  let tabs!: HTMLDivElement;
  // The tabs are wider than the default panel: the shown one is scrolled into view, and the wheel scrolls the bar.
  createEffect(() => {
    panel();
    queueMicrotask(() => tabs.querySelector(".ptab.active")?.scrollIntoView({ inline: "nearest", block: "nearest" }));
  });

  // Watch only the project being looked at; background projects cost nothing.
  createEffect(() => {
    const root = props.project.path;
    void watchRoot(root).catch(() => {});
    gitState.setRoot(root);
    onCleanup(() => void unwatchRoot(root).catch(() => {}));
  });

  /** Worktrees of this repository waiting for review. */
  const reviewCount = () => {
    const repo = props.project.parentId ?? props.project.id;
    return store.state.projects.filter((p) => p.parentId === repo && inReview(p)).length;
  };
  const reviewable = () => !props.project.remote && !!store.projectInfo[props.project.parentId ?? props.project.id]?.is_repo;

  const startResize = (e: PointerEvent) => {
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    const startX = e.clientX;
    const startW = store.state.settings.rightPanelWidth;
    // The panel is zoomed by the interface scale; its width is in unzoomed pixels.
    const scale = store.state.settings.uiScale || 1;
    const move = (ev: PointerEvent) => store.setSettings({ rightPanelWidth: Math.min(700, Math.max(200, startW - (ev.clientX - startX) / scale)) });
    const up = () => {
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  };

  const tab = (id: "files" | "git" | "history" | "issues" | "turns" | "review" | "plan" | "processes", label: string) => (
    <button class={`ptab${panel() === id || (id === "files" && panel() === "search") ? " active" : ""}`} onClick={() => store.showRightPanel(id)}>
      {label}
      {id === "git" && gitState.status()?.files.length ? ` ${gitState.status()!.files.length}` : ""}
      {id === "review" && reviewCount() ? ` ${reviewCount()}` : ""}
    </button>
  );

  return (
    <aside class="rightpanel">
      <div class="panel-resizer" onPointerDown={startResize} />
      <div class="panel-tabs" ref={tabs} onWheel={(e) => { if (e.deltaY) tabs.scrollLeft += e.deltaY; }}>
        {tab("files", t("Files"))}
        {tab("git", t("Changes"))}
        {tab("history", t("Sessions"))}
        {tab("issues", t("Issues"))}
        <Show when={store.daemonApi() >= CHECKPOINT_API}>{tab("turns", t("Turns"))}</Show>
        <Show when={reviewable()}>{tab("review", t("Review"))}</Show>
        {tab("plan", t("Plan"))}
        <Show when={store.daemonApi() >= PROCESS_API}>{tab("processes", t("Processes"))}</Show>
      </div>
      <div class="panel-body">
        <Switch>
          <Match when={panel() === "files" || panel() === "search"}><Explorer project={props.project} /></Match>
          <Match when={panel() === "git"}><GitPanel project={props.project} /></Match>
          <Match when={panel() === "history"}><HistoryPanel project={props.project} /></Match>
          <Match when={panel() === "issues"}><IssuesPanel project={props.project} /></Match>
          <Match when={panel() === "turns" && store.daemonApi() >= CHECKPOINT_API}><TurnTimeline project={props.project} /></Match>
          <Match when={panel() === "review"}><ReviewPanel project={props.project} /></Match>
          <Match when={panel() === "plan"}><PlanPanel project={props.project} /></Match>
          <Match when={panel() === "processes" && store.daemonApi() >= PROCESS_API}><ProcessesPanel project={props.project} /></Match>
        </Switch>
      </div>
    </aside>
  );
}
