import { For, Show, createEffect, createMemo, createSignal, on, onCleanup, onMount } from "solid-js";
import { EditorView, GutterMarker, gutter, keymap } from "@codemirror/view";
import { EditorState, StateEffect, StateField } from "@codemirror/state";
import { MergeView, goToNextChunk, goToPreviousChunk, unifiedMergeView } from "@codemirror/merge";
import { gitFileVersions, onEvent } from "../lib/ipc";
import { turnBlob } from "../lib/checkpoints";
import { baseExtensions, languageFor } from "../lib/codemirror";
import { store } from "../lib/store";
import { chooseAgentAndSend, sendSelectionToAgent } from "../lib/send-to-agent";
import { selectionSource } from "../lib/composer-state";
import { openContextMenu } from "./context-menu";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { isMac } from "../lib/platform";
import { Icon } from "./icons";
import type { Project, ReviewNote } from "../lib/types";
import { t } from "../lib/i18n";

/** Lines with a review note, per editor (updated from the store). */
const setNoteLines = StateEffect.define<number[]>();
const noteLines = StateField.define<number[]>({
  create: () => [],
  update: (v, tr) => tr.effects.reduce((acc, e) => (e.is(setNoteLines) ? e.value : acc), v),
});

class NoteMarker extends GutterMarker {
  constructor(readonly has: boolean) {
    super();
  }
  eq(other: NoteMarker) {
    return other.has === this.has;
  }
  toDOM() {
    const el = document.createElement("span");
    el.className = this.has ? "cm-note-mark has" : "cm-note-mark";
    el.textContent = this.has ? "●" : "+";
    return el;
  }
}
const HAS = new NoteMarker(true);
const ADD = new NoteMarker(false);

/** Gutter that shows notes and adds one on click (`onAdd(line, text, event)`). */
function noteGutter(onAdd: (line: number, text: string, e: MouseEvent) => void) {
  return [
    noteLines,
    gutter({
      class: "cm-note-gutter",
      // "+" on every line (shown on hover by CSS), "●" where a note is.
      lineMarker: (view, line) => (view.state.field(noteLines).includes(view.state.doc.lineAt(line.from).number) ? HAS : ADD),
      lineMarkerChange: (update) => update.transactions.some((tr) => tr.effects.some((e) => e.is(setNoteLines))),
      initialSpacer: () => ADD,
      domEventHandlers: {
        mousedown(view, block, e) {
          const line = view.state.doc.lineAt(block.from);
          onAdd(line.number, line.text, e as MouseEvent);
          return true;
        },
      },
    }),
  ];
}

const changeKeys = keymap.of([
  { key: "F7", run: goToNextChunk },
  { key: "Shift-F7", run: goToPreviousChunk },
]);

/** The batch prompt for every note in the project (Annotate AI diff). */
export function notesPrompt(notes: ReviewNote[]): string {
  const byFile = new Map<string, ReviewNote[]>();
  for (const n of notes) byFile.set(n.path, [...(byFile.get(n.path) ?? []), n]);
  const parts = [...byFile].map(([path, list]) =>
    `${path}\n${list.sort((a, b) => a.line - b.line).map((n) => `- ${n.side === "old" ? "removed line" : "line"} ${n.line}${n.text.trim() ? ` (\`${n.text.trim().slice(0, 120)}\`)` : ""}: ${n.note}`).join("\n")}`,
  );
  return `Please address these review notes on the current changes. Each names a file and a line of the diff.\n\n${parts.join("\n\n")}`;
}

/** One file before and after an agent turn: the snapshot ends of the turn and of the one before. */
async function turnVersions(rev: { session: string; n: number; prefix?: string }, path: string): Promise<{ original: string | null; modified: string | null }> {
  const side = async (before: boolean) => {
    const b = await turnBlob(rev.session, rev.n, (rev.prefix ?? "") + path, false, before);
    return b.exists ? (b.binary ? t("(binary file)") : b.too_large ? t("(too large to show)") : b.text ?? "") : null;
  };
  const [original, modified] = await Promise.all([side(true), side(false)]);
  return { original, modified };
}

/** HEAD ↔ working tree (or index when `staged`) diff, side by side or unified, with notes. With
 *  `rev`: the same file before and after that agent turn (read only, never changes). */
export function DiffView(props: { project: Project; path: string; staged: boolean; rev?: { session: string; n: number; prefix?: string }; base?: string }) {
  let host!: HTMLDivElement;
  let box!: HTMLDivElement;
  const [unified, setUnified] = createSignal(host?.clientWidth < 900);
  const [info, setInfo] = createSignal<"" | "new" | "deleted">("");
  const [draft, setDraft] = createSignal<{ line: number; side: "new" | "old"; text: string; x: number; y: number } | null>(null);
  const [noteText, setNoteText] = createSignal("");
  let destroy: (() => void) | null = null;
  let editors: { view: EditorView; side: "new" | "old" }[] = [];

  /** Text selected in either side (lines would mean the old or the new file: the path alone). */
  const picked = () => {
    for (const { view } of editors) {
      const r = view.state.selection.main;
      if (!r.empty) return { text: view.state.doc.sliceString(r.from, r.to), path: props.path };
    }
    return null;
  };
  const selection = selectionSource(picked);
  const menu = (e: MouseEvent) => {
    const sel = picked();
    if (!sel) return;
    openContextMenu(e, [
      { label: t("Copy"), hint: isMac ? "⌘C" : "Ctrl+C", run: () => void writeText(sel.text) },
      null,
      { label: t("Send Selection to Agent…"), run: () => sendSelectionToAgent(new MouseEvent("contextmenu", { clientX: e.clientX, clientY: e.clientY }), props.project.id, sel) },
    ]);
  };

  const allNotes = () => store.notesFor(props.project.id);
  const fileNotes = createMemo(() => allNotes().filter((n) => n.path === props.path));

  const pushMarks = () => {
    for (const { view, side } of editors) view.dispatch({ effects: setNoteLines.of(fileNotes().filter((n) => n.side === side).map((n) => n.line)) });
  };

  const openDraft = (side: "new" | "old") => (line: number, text: string, e: MouseEvent) => {
    const r = box.getBoundingClientRect();
    setNoteText("");
    setDraft({ line, side, text, x: Math.min(e.clientX - r.left + 12, r.width - 320), y: e.clientY - r.top + 8 });
    queueMicrotask(() => box.querySelector<HTMLTextAreaElement>(".note-draft textarea")?.focus());
  };

  const saveDraft = () => {
    const d = draft();
    if (!d || !noteText().trim()) return setDraft(null);
    store.addNote(props.project.id, { path: props.path, line: d.line, side: d.side, text: d.text, note: noteText().trim() });
    setDraft(null);
    queueMicrotask(pushMarks);
  };

  async function render() {
    try {
      const v = props.rev ? await turnVersions(props.rev, props.path) : await gitFileVersions(props.project.path, props.path, props.staged, props.base);
      const lang = await languageFor(props.path);
      const exts = (side: "new" | "old") => [...baseExtensions({ readOnly: true }), ...(lang ? [lang] : []), EditorView.editable.of(false), EditorState.readOnly.of(true), changeKeys, noteGutter(openDraft(side)), EditorView.updateListener.of((u) => u.selectionSet && !u.state.selection.main.empty && selection.changed())];
      destroy?.();
      host.replaceChildren();
      const a = v.original ?? "";
      const b = v.modified ?? "";
      setInfo(v.original == null ? "new" : v.modified == null ? "deleted" : "");
      if (unified()) {
        const view = new EditorView({ parent: host, doc: b, extensions: [...exts("new"), unifiedMergeView({ original: a, mergeControls: false, gutter: true })] });
        editors = [{ view, side: "new" }];
        destroy = () => view.destroy();
      } else {
        const mv = new MergeView({ parent: host, a: { doc: a, extensions: exts("old") }, b: { doc: b, extensions: exts("new") }, gutter: true, highlightChanges: true, collapseUnchanged: { margin: 3, minSize: 8 } });
        editors = [{ view: mv.a, side: "old" }, { view: mv.b, side: "new" }];
        destroy = () => mv.destroy();
      }
      pushMarks();
    } catch (e) {
      store.notify(String(e));
    }
  }

  /** Next / previous change in the editor showing the new side. */
  const jump = (forward: boolean) => {
    const target = editors[editors.length - 1]?.view;
    if (!target) return;
    (forward ? goToNextChunk : goToPreviousChunk)(target);
    target.focus();
  };

  // Notes added or resolved anywhere (this view, another diff, an agent hand-off) redraw marks.
  createEffect(on(fileNotes, () => pushMarks(), { defer: true }));

  onMount(() => {
    if (host.clientWidth && host.clientWidth < 900) setUnified(true);
    void render();
    // A turn's snapshots never change: only the working-tree diff follows the folder.
    const un = onEvent<{ root: string; git: boolean }>("fs-changed", (e) => !props.rev && e.root === props.project.path && e.git && void render());
    onCleanup(() => {
      void un.then((f) => f());
      selection.dispose();
      destroy?.();
    });
  });

  const remove = (id: string) => {
    store.removeNotes(props.project.id, [id]);
    queueMicrotask(pushMarks);
  };

  return (
    <div class="viewer" ref={box}>
      <div class="viewer-bar">
        <span class="path">&lrm;{props.path}</span>
        <span style={{ color: "var(--text-faint)" }}>{props.rev ? t("Before ↔ after turn #{n}", { n: props.rev.n }) : props.base ? t("{base} ↔ working tree", { base: props.base }) : props.staged ? t("HEAD ↔ index") : t("HEAD ↔ working tree")}{info() ? ` · ${info() === "new" ? t("new file") : t("deleted")}` : ""}</span>
        <div class="toggles">
          <button class="icon-btn" title={t("Previous change (Shift+F7)")} onClick={() => jump(false)}><Icon name="up" size={14} /></button>
          <button class="icon-btn" title={t("Next change (F7)")} onClick={() => jump(true)}><Icon name="down" size={14} /></button>
          <button class={`toggle${!unified() ? " on" : ""}`} onClick={() => { setUnified(false); void render(); }}>{t("Split")}</button>
          <button class={`toggle${unified() ? " on" : ""}`} onClick={() => { setUnified(true); void render(); }}>{t("Unified")}</button>
          <button class="toggle" onClick={() => store.openFile(props.project.id, props.path)}>{t("Open file")}</button>
        </div>
      </div>
      <Show when={allNotes().length}>
        <div class="notes-bar">
          <Icon name="note" size={14} />
          <span>{allNotes().length > 1 ? t("{n} review notes", { n: allNotes().length }) : t("{n} review note", { n: allNotes().length })}{fileNotes().length !== allNotes().length ? ` (${t("{n} here", { n: fileNotes().length })})` : ""}</span>
          <span style={{ flex: 1 }} />
          <button class="btn primary" onClick={(e) => chooseAgentAndSend(e, props.project.id, notesPrompt(allNotes()))}>{t("Send to agent")} ▾</button>
          <button class="btn" title={t("Remove every note (resolve all)")} onClick={() => { store.removeNotes(props.project.id, allNotes().map((n) => n.id)); queueMicrotask(pushMarks); }}>{t("Resolve all")}</button>
        </div>
        <Show when={fileNotes().length}>
          <div class="notes-list">
            <For each={fileNotes().slice().sort((a, b) => a.line - b.line)}>
              {(n) => (
                <div class="note-item">
                  <span class="note-line">{n.side === "old" ? "−" : ""}{n.line}</span>
                  <span class="note-body">{n.note}<Show when={n.by}><span class="muted-inline" title={t("Written by an agent with kelpied review add")}> · {n.by}</span></Show></span>
                  <button class="icon-btn" title={t("Resolve (remove)")} onClick={() => remove(n.id)}><Icon name="check" size={13} /></button>
                </div>
              )}
            </For>
          </div>
        </Show>
      </Show>
      <div class="cm-host" ref={host} style={{ overflow: "auto" }} onContextMenu={menu} />
      <Show when={draft()}>
        {(d) => (
          <div class="note-draft" style={{ left: `${Math.max(8, d().x)}px`, top: `${d().y}px` }} onPointerDown={(e) => e.stopPropagation()}>
            <div class="muted" style={{ padding: "0 0 6px" }}>{d().side === "old" ? t("Note on removed line {line}", { line: d().line }) : t("Note on line {line}", { line: d().line })}</div>
            <textarea
              class="commit-box"
              value={noteText()}
              placeholder={t("What should change here?")}
              onInput={(e) => setNoteText(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Escape") setDraft(null);
                if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) saveDraft();
              }}
            />
            <div style={{ display: "flex", gap: "6px", "justify-content": "flex-end", "margin-top": "6px" }}>
              <button class="btn" onClick={() => setDraft(null)}>{t("Cancel")}</button>
              <button class="btn primary" onClick={saveDraft}>{t("Add note")}</button>
            </div>
          </div>
        )}
      </Show>
    </div>
  );
}
