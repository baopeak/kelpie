import { For, Show, createMemo, createSignal } from "solid-js";
import { store } from "../lib/store";
import { allPanes, findPane } from "../lib/layout";
import type { LayoutNode, LeafNode, Project, SplitNode, Tab } from "../lib/types";
import { PaneView } from "./pane-view";
import { t } from "../lib/i18n";

export type DropSide = "center" | "left" | "right" | "top" | "bottom";

/** A drag in progress: a pane by its header, or a whole tab from the tab bar.
 *  - `view`/`target`/`side`/`rect`: dropping onto a pane of tab `view` (swap or move beside it);
 *  - `toTab`: a pane dropped onto another tab in the bar ("new" = empty bar space, a new tab);
 *  - `barIndex`/`barX`: a tab dropped back into the bar at that position. */
type Drag = {
  tabId: string;
  paneId: string | null;
  view: string | null;
  target: string | null;
  side: DropSide;
  rect: DOMRect | null;
  toTab: string | null;
  barIndex: number | null;
  barX: number | null;
};
const [drag, setDrag] = createSignal<Drag | null>(null);
export const paneDrag = drag;

const idle = (tabId: string, paneId: string | null): Drag => ({ tabId, paneId, view: null, target: null, side: "center", rect: null, toTab: null, barIndex: null, barX: null });

/** Where a drop at (x, y) over `rect` lands: the middle swaps, the outer bands move. Without
 *  `center` (a whole tab cannot swap with a pane) the nearest edge wins. */
function sideAt(rect: DOMRect, x: number, y: number, center = true): DropSide {
  const fx = (x - rect.left) / rect.width;
  const fy = (y - rect.top) / rect.height;
  if (center && fx > 0.3 && fx < 0.7 && fy > 0.3 && fy < 0.7) return "center";
  const d = { left: fx, right: 1 - fx, top: fy, bottom: 1 - fy };
  return (Object.keys(d) as (keyof typeof d)[]).reduce((a, b) => (d[a] <= d[b] ? a : b));
}

function zoneRect(rect: DOMRect, side: DropSide): DOMRect {
  const { left, top, width, height } = rect;
  switch (side) {
    case "left": return new DOMRect(left, top, width / 2, height);
    case "right": return new DOMRect(left + width / 2, top, width / 2, height);
    case "top": return new DOMRect(left, top, width, height / 2);
    case "bottom": return new DOMRect(left, top + height / 2, width, height / 2);
    default: return rect;
  }
}

/** Follows the pointer once it has moved a few pixels (so clicks stay clicks). Window
 *  listeners: the pointer leaves the handle long before the drop target. */
function trackDrag(e: PointerEvent, onMove: (ev: PointerEvent) => void, onDrop: () => void) {
  const x0 = e.clientX;
  const y0 = e.clientY;
  let active = false;
  const move = (ev: PointerEvent) => {
    if (!active) {
      if (Math.hypot(ev.clientX - x0, ev.clientY - y0) < 6) return;
      active = true;
    }
    onMove(ev);
  };
  const up = () => {
    window.removeEventListener("pointermove", move, true);
    window.removeEventListener("pointerup", up, true);
    window.removeEventListener("pointercancel", up, true);
    if (active) onDrop();
    setDrag(null);
  };
  window.addEventListener("pointermove", move, true);
  window.addEventListener("pointerup", up, true);
  window.addEventListener("pointercancel", up, true);
}

const under = (ev: PointerEvent) => document.elementFromPoint(ev.clientX, ev.clientY) as HTMLElement | null;

/** Starts dragging a pane from its header: onto a pane of the same tab (swap / move), onto
 *  another tab in the tab bar, or onto empty tab bar space (a new tab). */
export function startPaneDrag(e: PointerEvent, project: Project, tab: Tab, paneId: string) {
  if (e.button !== 0 || (e.target as HTMLElement).closest("button, input")) return;
  trackDrag(
    e,
    (ev) => {
      const el = under(ev);
      const tabEl = el?.closest<HTMLElement>("[data-tab-id]");
      if (tabEl) return void setDrag({ ...idle(tab.id, paneId), toTab: tabEl.dataset.tabId === tab.id ? null : tabEl.dataset.tabId! });
      if (el?.closest(".tabbar")) return void setDrag({ ...idle(tab.id, paneId), toTab: "new" });
      const over = el?.closest<HTMLElement>("[data-pane]");
      const target = over?.dataset.pane && over.dataset.tab === tab.id ? over.dataset.pane : null;
      if (!target || target === paneId) return void setDrag(idle(tab.id, paneId));
      const rect = over!.getBoundingClientRect();
      const side = sideAt(rect, ev.clientX, ev.clientY);
      setDrag({ ...idle(tab.id, paneId), view: tab.id, target, side, rect: zoneRect(rect, side) });
    },
    () => {
      const d = drag();
      if (d?.toTab) store.movePaneToTab(project.id, tab.id, paneId, d.toTab === "new" ? null : d.toTab);
      else if (d?.target) store.dropPane(project.id, tab.id, paneId, d.target, d.side);
    },
  );
}

/** Starts dragging a tab: along the tab bar to reorder, or onto an edge of a pane of the
 *  active tab to merge the dragged tab's panes into it. */
export function startTabDrag(e: PointerEvent, project: Project, tab: Tab) {
  if (e.button !== 0 || (e.target as HTMLElement).closest("button, input")) return;
  trackDrag(
    e,
    (ev) => {
      const el = under(ev);
      if (el?.closest(".tabbar")) {
        const tabs = [...document.querySelectorAll<HTMLElement>("[data-tab-id]")];
        const rects = tabs.map((x) => x.getBoundingClientRect());
        const from = tabs.findIndex((x) => x.dataset.tabId === tab.id);
        if (from < 0) return void setDrag(idle(tab.id, null));
        // Slots count the dragged tab too: the two on either side of it leave it in place.
        const slot = rects.filter((r) => ev.clientX > r.left + r.width / 2).length;
        const to = slot > from ? slot - 1 : slot;
        const barX = slot < rects.length ? rects[slot].left - 1 : rects[rects.length - 1].right + 1;
        return void setDrag({ ...idle(tab.id, null), barIndex: to, barX: to === from ? null : barX });
      }
      const active = store.activeTab(project.id);
      const over = el?.closest<HTMLElement>("[data-pane]");
      if (!active || active.id === tab.id || !over?.dataset.pane || over.dataset.tab !== active.id) return void setDrag(idle(tab.id, null));
      const rect = over.getBoundingClientRect();
      const side = sideAt(rect, ev.clientX, ev.clientY, false);
      setDrag({ ...idle(tab.id, null), view: active.id, target: over.dataset.pane, side, rect: zoneRect(rect, side) });
    },
    () => {
      const d = drag();
      if (d?.barIndex != null) store.moveTab(project.id, tab.id, d.barIndex);
      else if (d?.view && d.target && d.side !== "center") store.mergeTab(project.id, tab.id, d.view, d.target, d.side);
    },
  );
}

/** Renders a tab's split tree. Nodes are keyed by id so splitting one pane keeps the others. */
export function SplitView(props: { project: Project; tab: Tab }) {
  const multi = createMemo(() => allPanes(props.tab.root).length > 1);
  // A maximized pane renders alone; the others are parked like any hidden pane.
  const zoomedPane = createMemo(() => {
    const id = store.zoomed()[props.tab.id];
    return id ? findPane(props.tab.root, id) : undefined;
  });
  return (
    <div class={multi() ? "multi" : ""} style={{ position: "absolute", inset: 0 }}>
      <Show when={zoomedPane()} keyed fallback={<NodeView node={props.tab.root} project={props.project} tab={props.tab} />}>
        {(pane) => <PaneView pane={pane} project={props.project} tab={props.tab} />}
      </Show>
      <Show when={drag()?.view === props.tab.id && drag()?.rect}>
        {(r) => (
          <div class="pane-drop" style={{ left: `${r().left}px`, top: `${r().top}px`, width: `${r().width}px`, height: `${r().height}px` }}>
            {drag()!.side === "center" ? t("Swap") : drag()!.paneId ? t("Move here") : t("Merge tab here")}
          </div>
        )}
      </Show>
    </div>
  );
}

// Keyed by node id (not object identity): resizing produces new node objects but must not
// remount the terminals inside. A given id never changes type.
function NodeView(props: { node: LayoutNode; project: Project; tab: Tab }) {
  return (
    <Show when={props.node.id} keyed>
      {(_id) =>
        props.node.type === "leaf" ? (
          <PaneView pane={(props.node as LeafNode).pane} project={props.project} tab={props.tab} />
        ) : (
          <SplitNodeView node={props.node as SplitNode} project={props.project} tab={props.tab} />
        )
      }
    </Show>
  );
}

function SplitNodeView(props: { node: SplitNode; project: Project; tab: Tab }) {
  let container!: HTMLDivElement;
  const [dragging, setDragging] = createSignal<number | null>(null);
  const startDrag = (e: PointerEvent, index: number) => {
    e.preventDefault();
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture(e.pointerId);
    setDragging(index);
    const rect = container.getBoundingClientRect();
    const move = (ev: PointerEvent) => {
      const pos = props.node.dir === "row" ? (ev.clientX - rect.left) / rect.width : (ev.clientY - rect.top) / rect.height;
      store.resizeSplit(props.project.id, props.tab.id, props.node.id, index, pos);
    };
    const up = () => {
      setDragging(null);
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
  };
  // Children keep their element across resizes: <For> is keyed by node identity, so map ids.
  const ids = createMemo(() => props.node.children.map((c) => c.id), undefined, { equals: (a, b) => a.length === b.length && a.every((x, i) => x === b[i]) });
  const child = (id: string) => props.node.children.find((c) => c.id === id)!;
  const size = (id: string) => props.node.sizes[props.node.children.findIndex((c) => c.id === id)] ?? 0;
  const gutters = () => props.node.children.length - 1;
  return (
    <div ref={container} class={`split ${props.node.dir}`}>
      <For each={ids()}>
        {(id, i) => (
          <>
            <div class="child" style={{ "flex-basis": `calc(${size(id) * 100}% - var(--gutter) * ${gutters() / props.node.children.length})`, "flex-grow": 0, "flex-shrink": 0 }}>
              <NodeView node={child(id)} project={props.project} tab={props.tab} />
            </div>
            <Show when={i() < gutters()}>
              <div
                class={`gutter${dragging() === i() ? " dragging" : ""}`}
                title={t("Drag to resize · double-click to make all panes equal")}
                onPointerDown={(e) => startDrag(e, i())}
                onDblClick={() => store.equalizeTab(props.project.id, props.tab.id)}
              />
            </Show>
          </>
        )}
      </For>
    </div>
  );
}
