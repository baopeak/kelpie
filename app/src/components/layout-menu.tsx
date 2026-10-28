// Layout popover in the tab bar: arrange the tab's panes in a grid (presets, a rows × columns
// picker or per-row column counts), equalize sizes, broadcast input, and saved layouts.
import { allLayouts } from "../lib/plugins";
import { For, Index, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { store } from "../lib/store";
import { GRID_PRESETS, MAX_GRID, allPanes, autoGrid, cellCount, gridLabel, templateKinds, validGrid, type GridSpec } from "../lib/layout";
import { kindInfo } from "../lib/agents";
import type { LayoutTemplate, Project, Tab } from "../lib/types";
import { Icon, KindGlyph } from "./icons";
import { setCovering } from "../lib/overlays";
import { t } from "../lib/i18n";

const [openFor, setOpenFor] = createSignal<{ projectId: string; tabId: string; x: number; y: number } | null>(null);

export function openLayoutMenu(e: MouseEvent, project: Project, tab: Tab | undefined) {
  e.stopPropagation();
  const r = (e.currentTarget as HTMLElement).getBoundingClientRect();
  if (!tab) return;
  setOpenFor((cur) => (cur?.tabId === tab.id ? null : { projectId: project.id, tabId: tab.id, x: r.right, y: r.bottom + 4 }));
}

export const closeLayoutMenu = () => setOpenFor(null);

/** Panes that hold something (empty launcher cells do not count). */
const realPanes = (tab: Tab) => allPanes(tab.root).filter((p) => p.kind !== "empty").length;

/** A small picture of a grid. */
export function GridThumb(props: { spec: GridSpec; size?: number }) {
  const size = () => props.size ?? 34;
  return (
    <span class={`grid-thumb ${props.spec.mode}`} style={{ width: `${size()}px`, height: `${Math.round(size() * 0.72)}px` }}>
      <Index each={props.spec.counts}>
        {(n) => (
          <span class="grid-thumb-line">
            <Index each={Array.from({ length: n() })}>{() => <span class="grid-thumb-cell" />}</Index>
          </span>
        )}
      </Index>
    </span>
  );
}

/** A picture of a saved layout's tree with each pane's agent colour. */
export function TemplateThumb(props: { template: LayoutTemplate; size?: number }) {
  const size = () => props.size ?? 34;
  const Node = (p: { t: LayoutTemplate }) =>
    p.t.type === "leaf" ? (
      <span class="grid-thumb-cell" style={{ background: p.t.kind ? kindInfo(p.t.kind).color : undefined, opacity: p.t.kind ? 0.75 : undefined }} />
    ) : (
      <span class="grid-thumb-line" style={{ "flex-direction": p.t.dir === "row" ? "row" : "column" }}>
        <For each={(p.t as Extract<LayoutTemplate, { type: "split" }>).children}>{(c) => <Node t={c} />}</For>
      </span>
    );
  return (
    <span class="grid-thumb" style={{ width: `${size()}px`, height: `${Math.round(size() * 0.72)}px` }}>
      <Node t={props.template} />
    </span>
  );
}

/** Parses "2,3" / "2 3" into per-line counts. */
export function parseCounts(text: string): number[] | null {
  const parts = text.split(/[\s,;+x×]+/).filter(Boolean).map(Number);
  return parts.length && parts.every((n) => Number.isInteger(n) && n >= 1 && n <= MAX_GRID) && parts.length <= MAX_GRID ? parts : null;
}

/** Hover a rows × columns picker (like a table-size picker); click applies. */
export function GridPicker(props: { onPick: (spec: GridSpec) => void }) {
  const [hover, setHover] = createSignal<[number, number]>([0, 0]);
  return (
    <div class="grid-picker" onPointerLeave={() => setHover([0, 0])}>
      <div class="grid-picker-cells" style={{ "grid-template-columns": `repeat(${MAX_GRID}, 16px)` }}>
        <For each={Array.from({ length: MAX_GRID * 4 })}>
          {(_, i) => {
            const r = Math.floor(i() / MAX_GRID) + 1;
            const c = (i() % MAX_GRID) + 1;
            return (
              <span
                class={`grid-picker-cell${r <= hover()[0] && c <= hover()[1] ? " on" : ""}`}
                onPointerEnter={() => setHover([r, c])}
                onClick={() => props.onPick({ mode: "rows", counts: Array.from({ length: r }, () => c) })}
              />
            );
          }}
        </For>
      </div>
      <div class="grid-picker-label">
        {hover()[0]
          ? `${hover()[0] === 1 ? t("{n} row", { n: 1 }) : t("{n} rows", { n: hover()[0] })} × ${hover()[1] === 1 ? t("{n} column", { n: 1 }) : t("{n} columns", { n: hover()[1] })}`
          : t("Rows × columns")}
      </div>
    </div>
  );
}

export function LayoutMenuHost() {
  createEffect(() => setCovering("layout-menu", !!openFor()));
  let el!: HTMLDivElement;
  const [custom, setCustom] = createSignal("");
  const [byColumns, setByColumns] = createSignal(false);
  const [saving, setSaving] = createSignal<string | null>(null);
  onMount(() => {
    const onDown = (e: PointerEvent) => {
      if (openFor() && el && !el.contains(e.target as Node)) setOpenFor(null);
    };
    // Capture phase: a focused terminal would otherwise take Escape.
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape" || !openFor()) return;
      e.preventDefault();
      e.stopPropagation();
      setOpenFor(null);
    };
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("keydown", onKey, true);
    });
  });
  const ctx = () => {
    const o = openFor();
    const tab = o && store.workspace(o.projectId).tabs.find((t) => t.id === o.tabId);
    return o && tab ? { ...o, tab } : null;
  };
  const arrange = (spec: GridSpec) => {
    const c = ctx();
    if (!c || !validGrid(spec)) return;
    store.arrangeTab(c.projectId, c.tabId, spec);
    setOpenFor(null);
  };
  const customSpec = (): GridSpec | null => {
    const counts = parseCounts(custom());
    return counts ? { mode: byColumns() ? "cols" : "rows", counts } : null;
  };
  return (
    <Show when={ctx()}>
      {(c) => (
        <div ref={el} class="layout-menu" style={{ left: `${Math.max(8, Math.min(c().x - 300, window.innerWidth - 316))}px`, top: `${c().y}px` }}>
          <div class="layout-menu-head">
            {t("Arrange this tab")}{" "}
            <span class="muted-inline">({((n) => (n === 1 ? t("{n} pane", { n }) : t("{n} panes", { n })))(realPanes(c().tab))})</span>
          </div>
          <div class="layout-flows">
            <button class="layout-menu-item" data-action="tile" disabled={!store.canTile(c().projectId, c().tabId)}
              title={t("This tab's panes and every one-CLI tab, in an even grid")}
              onClick={() => { store.tileTab(c().projectId, c().tabId); setOpenFor(null); }}>
              <GridThumb spec={autoGrid(Math.max(2, realPanes(c().tab) + store.tileCandidates(c().projectId, c().tabId).length))} size={22} />
              <span style={{ flex: 1, "text-align": "left" }}>{t("Tile all")}</span>
              <Show when={store.tileCandidates(c().projectId, c().tabId).length}>
                {(n) => <span class="muted-inline">{n() === 1 ? t("+{n} tab", { n: n() }) : t("+{n} tabs", { n: n() })}</span>}
              </Show>
            </button>
            <button class="layout-menu-item" data-action="spread" disabled={!store.canSpread(c().projectId, c().tabId)}
              title={t("Every pane of this tab gets its own tab")}
              onClick={() => { store.spreadTab(c().projectId, c().tabId); setOpenFor(null); }}>
              <Icon name="files" size={14} /> <span style={{ flex: 1, "text-align": "left" }}>{t("Spread into tabs")}</span>
            </button>
          </div>
          <div class="layout-menu-sep" />
          <div class="layout-presets">
            <For each={GRID_PRESETS}>
              {(spec) => (
                <button class="layout-preset" title={t("{grid} · {n} panes", { grid: gridLabel(spec), n: cellCount(spec) })} onClick={() => arrange(spec)}>
                  <GridThumb spec={spec} />
                </button>
              )}
            </For>
          </div>
          <GridPicker onPick={arrange} />
          <div class="layout-custom">
            <select class="text-input" value={byColumns() ? "cols" : "rows"} onChange={(e) => setByColumns(e.currentTarget.value === "cols")}>
              <option value="rows">{t("Panes per row")}</option>
              <option value="cols">{t("Panes per column")}</option>
            </select>
            <input class="text-input" placeholder={t("e.g. 2, 3")} value={custom()} onInput={(e) => setCustom(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && customSpec() && arrange(customSpec()!)} />
            <button class="btn" disabled={!customSpec()} onClick={() => arrange(customSpec()!)}>{t("Apply")}</button>
          </div>
          <Show when={customSpec()}>{(spec) => <div class="layout-custom-preview"><GridThumb spec={spec()} size={44} /> <span class="muted-inline">{gridLabel(spec())}</span></div>}</Show>
          <div class="layout-menu-sep" />
          <button class="layout-menu-item" onClick={() => { store.equalizeTab(c().projectId, c().tabId); setOpenFor(null); }}>
            <Icon name="dashboard" size={14} /> {t("Make all panes equal")}
          </button>
          <button class="layout-menu-item" onClick={() => store.setBroadcast(c().projectId, c().tabId, !c().tab.broadcast)}>
            <Icon name={c().tab.broadcast ? "check" : "terminal"} size={14} /> {t("Broadcast input to all terminals")} {c().tab.broadcast ? t("(on)") : ""}
          </button>
          <Show
            when={saving() !== null}
            fallback={<button class="layout-menu-item" onClick={() => setSaving(c().tab.name ?? "")}><Icon name="pin" size={14} /> {t("Save this layout…")}</button>}
          >
            <div class="layout-custom">
              <input class="text-input" autofocus placeholder={t("Layout name")} value={saving()!} onInput={(e) => setSaving(e.currentTarget.value)}
                onKeyDown={(e) => { if (e.key === "Enter" && saving()!.trim()) { store.saveLayout(c().projectId, c().tabId, saving()!); setSaving(null); } }} />
              <button class="btn primary" disabled={!saving()!.trim()} onClick={() => { store.saveLayout(c().projectId, c().tabId, saving()!); setSaving(null); }}>{t("Save")}</button>
            </div>
          </Show>
          <Show when={allLayouts().length}>
            <div class="layout-menu-sep" />
            <div class="layout-menu-head">{t("Open a saved layout in a new tab")}</div>
            <For each={allLayouts()}>
              {(l) => (
                <button class="layout-menu-item" data-layout={l.id} onClick={() => { void store.applyLayout(c().projectId, l.template); setOpenFor(null); }}>
                  <TemplateThumb template={l.template} size={26} />
                  <span style={{ flex: 1, "text-align": "left" }}>{l.name}</span>
                  <span class="layout-kinds">
                    <For each={[...new Set(templateKinds(l.template).filter(Boolean))] as string[]}>{(k) => <KindGlyph kind={k} size={14} />}</For>
                  </span>
                </button>
              )}
            </For>
          </Show>
        </div>
      )}
    </Show>
  );
}
