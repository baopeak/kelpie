// Workspace board: every project / worktree as a card in Todo · In progress · Review · Done,
// with the agents running in it. Drag cards between columns (pointer events: HTML5 drag and
// drop does not work while Tauri handles file drops); click a card to open it.
import { For, Show, createMemo, createSignal, onCleanup } from "solid-js";
import { store } from "../lib/store";
import { Icon, KindGlyph } from "./icons";
import { openContextMenu, type MenuItem } from "./context-menu";
import { StatusDot } from "./sidebar";
import { ReviewBody } from "./review-card";
import { moveToStatus } from "../lib/review-queue";
import type { Project, WorkStatus } from "../lib/types";
import { t, msg } from "../lib/i18n";

const COLUMNS: { id: WorkStatus; label: string }[] = [
  { id: "todo", label: msg("Todo") },
  { id: "progress", label: msg("In progress") },
  { id: "review", label: msg("Review") },
  { id: "done", label: msg("Done") },
];

interface Drag {
  id: string;
  x: number;
  y: number;
  dx: number;
  dy: number;
  width: number;
  over: WorkStatus | null;
}

function cardMenu(p: Project, open: () => void): MenuItem[] {
  return [
    { label: t("Open"), run: open },
    { label: p.pinned ? t("Unpin") : t("Pin to top"), run: () => store.togglePin(p.id) },
    null,
    ...COLUMNS.filter((c) => c.id !== (p.status ?? "todo")).map((c) => ({ label: t("Move to {column}", { column: t(c.label) }), run: () => void moveToStatus(p, c.id) })),
  ];
}

function Card(props: { project: Project; ghost?: boolean; onPointerDown?: (e: PointerEvent) => void; onOpen?: () => void }) {
  const sessions = createMemo(() => Object.values(store.sessions).filter((s) => s.project_id === props.project.id && s.alive));
  const parent = () => store.state.projects.find((p) => p.id === props.project.parentId);
  const info = () => store.projectInfo[props.project.id];
  const attention = () => sessions().filter((s) => s.status === "waiting" || s.unread).length;
  return (
    <div
      class={`board-card${props.ghost ? " ghost" : ""}${store.state.activeProject === props.project.id ? " current" : ""}`}
      data-card={props.project.id}
      tabIndex={props.ghost ? -1 : 0}
      onPointerDown={props.onPointerDown}
      // Keyboard: Enter opens, the context-menu key (or Shift+F10) moves between columns.
      onKeyDown={(e) => {
        if (e.key === "Enter") props.onOpen?.();
        if (e.key === "ContextMenu" || (e.shiftKey && e.key === "F10")) {
          e.preventDefault();
          const r = e.currentTarget.getBoundingClientRect();
          openContextMenu(new MouseEvent("contextmenu", { clientX: r.left + 20, clientY: r.bottom }), cardMenu(props.project, () => props.onOpen?.()));
        }
      }}
      onContextMenu={(e) => !props.ghost && openContextMenu(e, cardMenu(props.project, () => props.onOpen?.()))}
    >
      <div class="board-card-head">
        <Show when={parent()} fallback={<span class="project-swatch" style={{ background: props.project.color }}>{props.project.name.slice(0, 1).toUpperCase()}</span>}>
          <span class="wt-glyph" style={{ color: props.project.color }}><Icon name="branch" size={13} /></span>
        </Show>
        <span class="board-card-name">{props.project.name}</span>
        <button class={`icon-btn mini pin${props.project.pinned ? " on" : ""}`} title={props.project.pinned ? t("Unpin") : t("Pin to top")}
          onPointerDown={(e) => e.stopPropagation()} onClick={(e) => { e.stopPropagation(); store.togglePin(props.project.id); }}>
          <Icon name="pin" size={12} />
        </button>
      </div>
      <div class="board-card-sub">
        <Show when={parent()}><span>{parent()!.name}</span></Show>
        <Show when={info()?.is_repo && (info()!.branch ?? "") !== props.project.name}>
          <span class="board-branch"><Icon name="branch" size={10} /> {info()!.branch ?? props.project.branch ?? t("detached")}{info()!.dirty ? "*" : ""}</span>
        </Show>
      </div>
      <Show when={store.metaFor(props.project)?.comment}>
        {(c) => <div class="board-card-comment" title={c()}>{c()}</div>}
      </Show>
      <Show when={sessions().length}>
        <div class="board-card-agents">
          <For each={sessions()}>{(s) => <span class="board-agent" title={s.title ?? s.kind}><StatusDot s={s} /><KindGlyph kind={s.kind} size={12} /></span>}</For>
          <Show when={attention()}><span class="badge waiting">{attention()}</span></Show>
        </div>
      </Show>
      <Show when={!props.ghost && props.project.parentId && (props.project.status ?? "todo") === "review"}>
        <ReviewBody project={props.project} />
      </Show>
    </div>
  );
}

export function Board() {
  const [drag, setDrag] = createSignal<Drag | null>(null);
  let detach: (() => void) | null = null;
  onCleanup(() => detach?.());
  const byColumn = createMemo(() => {
    const out: Record<WorkStatus, Project[]> = { todo: [], progress: [], review: [], done: [] };
    for (const p of store.state.projects) out[p.status ?? "todo"].push(p);
    for (const list of Object.values(out)) list.sort((a, b) => Number(!!b.pinned) - Number(!!a.pinned));
    return out;
  });

  const open = (id: string) => {
    store.setOverlay(null);
    store.switchProject(id);
  };

  // Press → move ≥ 4 px starts a drag; release over a column moves the card there, a release
  // without moving opens the project. The pointer is captured, so a release outside the window
  // (or a cancelled pointer) still ends the drag.
  function startDrag(e: PointerEvent, id: string) {
    if (e.button !== 0) return;
    const card = e.currentTarget as HTMLElement;
    card.setPointerCapture(e.pointerId);
    const rect = card.getBoundingClientRect();
    const start = { x: e.clientX, y: e.clientY };
    let moved = false;
    const columnAt = (x: number, y: number) => (document.elementFromPoint(x, y)?.closest("[data-col]") as HTMLElement | null)?.dataset.col as WorkStatus | undefined;
    const cancel = () => {
      detach?.();
      setDrag(null);
    };
    const move = (ev: PointerEvent) => {
      if (ev.buttons === 0) return cancel();
      if (!moved && Math.hypot(ev.clientX - start.x, ev.clientY - start.y) < 4) return;
      moved = true;
      setDrag({ id, x: ev.clientX, y: ev.clientY, dx: start.x - rect.left, dy: start.y - rect.top, width: rect.width, over: columnAt(ev.clientX, ev.clientY) ?? null });
    };
    const up = (ev: PointerEvent) => {
      detach?.();
      const target = moved ? columnAt(ev.clientX, ev.clientY) : undefined;
      setDrag(null);
      if (!moved) open(id);
      else if (target) {
        const p = store.state.projects.find((x) => x.id === id);
        if (p) void moveToStatus(p, target);
      }
    };
    detach?.();
    detach = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      card.removeEventListener("pointercancel", cancel);
      card.removeEventListener("lostpointercapture", cancel);
      detach = null;
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    card.addEventListener("pointercancel", cancel);
    card.addEventListener("lostpointercapture", cancel);
  }

  const dragged = () => (drag() ? store.state.projects.find((p) => p.id === drag()!.id) : undefined);

  return (
    <div class="board" tabIndex={-1} ref={(el) => queueMicrotask(() => el.focus())} onKeyDown={(e) => e.key === "Escape" && store.setOverlay(null)}>
      <div class="board-head">
        <h2>{t("Workspace board")}</h2>
        <span class="muted-inline">{t("Drag cards between columns · click to open")}</span>
        <span style={{ flex: 1 }} />
        <button class="icon-btn" title={t("Close (Esc)")} onClick={() => store.setOverlay(null)}><Icon name="close" /></button>
      </div>
      <div class="board-cols">
        <For each={COLUMNS}>
          {(col) => (
            <div class={`board-col${drag()?.over === col.id ? " over" : ""}`} data-col={col.id}>
              <div class="board-col-head">{t(col.label)}<span class="board-count">{byColumn()[col.id].length}</span></div>
              <div class="board-col-body">
                <For each={byColumn()[col.id]}>
                  {(p) => (
                    <div style={{ opacity: drag()?.id === p.id ? 0.35 : 1 }}>
                      <Card project={p} onPointerDown={(e) => startDrag(e, p.id)} onOpen={() => open(p.id)} />
                    </div>
                  )}
                </For>
              </div>
            </div>
          )}
        </For>
      </div>
      <Show when={dragged()}>
        {(p) => (
          <div class="board-ghost" style={{ left: `${drag()!.x - drag()!.dx}px`, top: `${drag()!.y - drag()!.dy}px`, width: `${drag()!.width}px` }}>
            <Card project={p()} ghost />
          </div>
        )}
      </Show>
    </div>
  );
}
