// Virtualized, lazily-loaded file tree. Only rows inside the viewport exist in the DOM, and a
// directory is read from disk the first time it is expanded (then refreshed by the watcher).
import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { setSearchFolder } from "./search-panel";
import { store } from "../lib/store";
import { createEntry, deleteEntry, listDir, onEvent, renameEntry, revealEntry } from "../lib/ipc";
import { gitState } from "../lib/git-state";
import { allPanes } from "../lib/layout";
import { isMac, joinPath } from "../lib/platform";
import { peekView } from "../lib/terminal-host";
import type { DirEntry, Project } from "../lib/types";
import { openContextMenu, type MenuItem } from "./context-menu";
import { t } from "../lib/i18n";
import { confirmDialog } from "../lib/confirm-dialog";

const ROW_H = 24;

const extIcon: Record<string, [string, string]> = {
  rs: ["◆", "#ff8f5a"], ts: ["TS", "#4f9cf9"], tsx: ["⚛", "#5fd7d7"], js: ["JS", "#f2c96d"], jsx: ["⚛", "#5fd7d7"],
  json: ["{}", "#f2c96d"], md: ["M↓", "#8fc8ff"], py: ["Py", "#6cb6ff"], go: ["Go", "#5fd7d7"], toml: ["⚙", "#9aa4b5"],
  yml: ["⚙", "#c592ff"], yaml: ["⚙", "#c592ff"], html: ["<>", "#ff8f5a"], css: ["#", "#6cb6ff"], svg: ["◇", "#f2c96d"],
  png: ["▣", "#c592ff"], jpg: ["▣", "#c592ff"], jpeg: ["▣", "#c592ff"], gif: ["▣", "#c592ff"], webp: ["▣", "#c592ff"],
  lock: ["🔒", "#636c7d"], sh: ["$", "#8bd58b"], ps1: ["$", "#8fc8ff"], txt: ["≡", "#9aa4b5"],
};

function FileIcon(props: { entry: DirEntry; open: boolean }) {
  if (props.entry.is_dir) return <span class="ficon" style={{ color: "#8fa6c8" }}>{props.open ? "▾" : "▸"}</span>;
  const ext = props.entry.name.split(".").pop()?.toLowerCase() ?? "";
  const [g, c] = extIcon[ext] ?? ["·", "#636c7d"];
  return <span class="ficon" style={{ color: c, "font-size": g.length > 1 ? "8.5px" : "11px", "font-weight": 700 }}>{g}</span>;
}

interface Row { entry: DirEntry; depth: number; index: number; ignored: boolean }

export interface FileTreeApi {
  collapseAll(): void;
  refresh(): void;
}

export function FileTree(props: { project: Project; api?: (api: FileTreeApi) => void }) {
  const root = () => props.project.path;
  const [children, setChildren] = createStore<Record<string, DirEntry[]>>({});
  const [expanded, setExpanded] = createStore<Record<string, boolean>>({ "": true });
  const [selected, setSelected] = createSignal<string | null>(null);
  const [scrollTop, setScrollTop] = createSignal(0);
  const [height, setHeight] = createSignal(600);
  const [editing, setEditing] = createSignal<{ dir: string; kind: "file" | "dir" | "rename"; entry?: DirEntry } | null>(null);
  let scroller!: HTMLDivElement;

  async function load(dir: string) {
    try {
      const list = await listDir(root(), dir);
      setChildren(dir, list);
    } catch (e) {
      // A folder deleted behind our back simply disappears; only the root is worth a toast.
      setChildren(produce((c) => delete c[dir]));
      if (!dir) store.notify(String(e));
    }
  }

  onMount(() => {
    void load("");
    props.api?.({
      collapseAll: () => setExpanded(reconcile({ "": true })),
      refresh: () => {
        // Only the root and folders currently expanded (with all their parents expanded).
        const open = (d: string): boolean => !d || (!!expanded[d] && open(d.includes("/") ? d.slice(0, d.lastIndexOf("/")) : ""));
        for (const d of Object.keys(children)) if (open(d)) void load(d);
      },
    });
    const ro = new ResizeObserver(() => setHeight(scroller.clientHeight));
    ro.observe(scroller);
    const un = onEvent<{ root: string; dirs: string[] }>("fs-changed", (e) => {
      if (e.root !== root()) return;
      for (const d of e.dirs) if (children[d]) void load(d);
    });
    onCleanup(() => {
      ro.disconnect();
      void un.then((f) => f());
    });
  });

  const rows = createMemo<Row[]>(() => {
    const out: Row[] = [];
    const walk = (dir: string, depth: number, parentIgnored: boolean) => {
      for (const entry of children[dir] ?? []) {
        const ignored = parentIgnored || entry.ignored;
        out.push({ entry, depth, index: out.length, ignored });
        if (entry.is_dir && expanded[entry.path]) walk(entry.path, depth + 1, ignored);
      }
    };
    walk("", 0, false);
    return out;
  });

  const visible = createMemo(() => {
    const start = Math.max(0, Math.floor(scrollTop() / ROW_H) - 8);
    const end = Math.min(rows().length, Math.ceil((scrollTop() + height()) / ROW_H) + 8);
    // Same Row objects while scrolling, so <For> keeps existing DOM rows.
    return rows().slice(start, end);
  });

  const deco = createMemo(() => gitState.decorations());

  const toggle = (entry: DirEntry) => {
    const open = !expanded[entry.path];
    setExpanded(entry.path, open);
    if (open && !children[entry.path]) void load(entry.path);
  };

  const activate = (entry: DirEntry) => {
    setSelected(entry.path);
    if (entry.is_dir) toggle(entry);
    else store.openFile(props.project.id, entry.path);
  };

  const parentOf = (p: string) => (p.includes("/") ? p.slice(0, p.lastIndexOf("/")) : "");

  const focusedTerminal = () => {
    const tab = store.activeTab(props.project.id);
    const pane = tab && allPanes(tab.root).find((p) => p.id === tab.focus);
    return pane?.sessionId ? peekView(pane.sessionId) : undefined;
  };

  const commit = async (value: string) => {
    const ed = editing();
    setEditing(null);
    if (!ed || !value.trim()) return;
    try {
      if (ed.kind === "rename" && ed.entry) {
        await renameEntry(root(), ed.entry.path, (parentOf(ed.entry.path) ? parentOf(ed.entry.path) + "/" : "") + value.trim());
        await load(parentOf(ed.entry.path));
      } else {
        const path = (ed.dir ? ed.dir + "/" : "") + value.trim();
        await createEntry(root(), path, ed.kind === "dir");
        setExpanded(ed.dir, true);
        await load(ed.dir);
        if (ed.kind === "file") store.openFile(props.project.id, path);
      }
    } catch (e) {
      store.notify(String(e));
    }
  };

  const menuItems = (entry: DirEntry | null) => {
    const dir = entry ? (entry.is_dir ? entry.path : parentOf(entry.path)) : "";
    const items: MenuItem[] = [
      { label: t("New File…"), run: () => { setExpanded(dir, true); setEditing({ dir, kind: "file" }); } },
      { label: t("New Folder…"), run: () => { setExpanded(dir, true); setEditing({ dir, kind: "dir" }); } },
    ];
    if (entry) {
      items.push(
        null,
        { label: t("Insert Path into Terminal"), run: () => { const v = focusedTerminal(); if (v) { v.paste(entry.path.includes(" ") ? `"${entry.path}" ` : entry.path + " "); v.focus(); } } },
        ...(entry.is_dir ? [{ label: t("Find in Folder…"), run: () => { setSearchFolder(entry.path); store.showRightPanel("search"); } }] : []),
        { label: t("Copy Relative Path"), run: () => void writeText(entry.path) },
        { label: t("Copy Path"), run: () => void writeText(joinPath(root(), entry.path)) },
        { label: isMac ? t("Reveal in Finder") : t("Reveal in Explorer"), run: () => void revealEntry(root(), entry.path) },
        null,
        { label: t("Rename…"), run: () => setEditing({ dir: parentOf(entry.path), kind: "rename", entry }) },
        {
          label: t("Move to Trash"), danger: true,
          run: async () => {
            if (!(await confirmDialog(t("Move \"{name}\" to the trash?", { name: entry.name })))) return;
            try { await deleteEntry(root(), entry.path); await load(parentOf(entry.path)); } catch (e) { store.notify(String(e)); }
          },
        },
      );
    }
    items.push(null, { label: t("Refresh"), run: () => { for (const d of Object.keys(children)) void load(d); void gitState.refresh(); } });
    return items;
  };

  return (
    <>
      <div
        class="tree"
        ref={scroller}
        tabIndex={0}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        onContextMenu={(e) => openContextMenu(e, menuItems(null))}
      >
        <div style={{ height: `${rows().length * ROW_H + ROW_H}px`, position: "relative" }}>
          <For each={visible()}>
            {(row) => {
              const status = () => deco().get(row.entry.path);
              const renaming = () => editing()?.kind === "rename" && editing()?.entry?.path === row.entry.path;
              return (
                <div
                  class={`tree-row${selected() === row.entry.path ? " selected" : ""}${row.ignored ? " ignored" : ""}${status() ? ` g-${status() === "•" ? "M" : status()}` : ""}`}
                  style={{ top: `${row.index * ROW_H}px`, "padding-left": `${8 + row.depth * 14}px` }}
                  onClick={() => activate(row.entry)}
                  onContextMenu={(e) => { setSelected(row.entry.path); openContextMenu(e, menuItems(row.entry)); }}
                  title={row.entry.path}
                >
                  <FileIcon entry={row.entry} open={!!expanded[row.entry.path]} />
                  <Show when={renaming()} fallback={<span class="fname">{row.entry.name}</span>}>
                    <InlineInput initial={row.entry.name} onDone={(v) => void commit(v)} />
                  </Show>
                  <Show when={status() && status() !== "•"}><span class="gstat">{status()}</span></Show>
                </div>
              );
            }}
          </For>
          <Show when={editing() && editing()!.kind !== "rename"}>
            <div class="tree-row" style={{ top: `${rows().length * ROW_H}px`, "padding-left": "8px" }}>
              <span class="ficon">{editing()!.kind === "dir" ? "▸" : "·"}</span>
              <InlineInput initial="" placeholder={editing()!.kind === "dir" ? t("folder name in /{dir}", { dir: editing()!.dir }) : t("file name in /{dir}", { dir: editing()!.dir })} onDone={(v) => void commit(v)} />
            </div>
          </Show>
        </div>
      </div>
    </>
  );
}

function InlineInput(props: { initial: string; placeholder?: string; onDone: (v: string) => void }) {
  let el!: HTMLInputElement;
  let done = false;
  const finish = (v: string) => {
    if (done) return;
    done = true;
    props.onDone(v);
  };
  onMount(() => {
    el.focus();
    const dot = props.initial.lastIndexOf(".");
    el.setSelectionRange(0, dot > 0 ? dot : props.initial.length);
  });
  return (
    <input
      ref={el}
      class="inline-input"
      value={props.initial}
      placeholder={props.placeholder}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(e.currentTarget.value);
        if (e.key === "Escape") finish("");
      }}
      onBlur={(e) => finish(e.currentTarget.value === props.initial ? "" : e.currentTarget.value)}
    />
  );
}

