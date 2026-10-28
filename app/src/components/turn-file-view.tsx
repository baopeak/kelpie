// A file as it was at the end of an agent turn: rendered when it reads better that way
// (markdown, images, Mermaid, CSV), source otherwise. A comment on a rendered markdown block
// becomes a review note on that line, to send to an agent like any other note.
import { For, Match, Show, Switch, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { EditorView } from "@codemirror/view";
import { EditorState } from "@codemirror/state";
import { marked } from "marked";
import DOMPurify from "dompurify";
import { store, sessionLabel } from "../lib/store";
import { baseExtensions, languageFor } from "../lib/codemirror";
import { artifactKind, imageMime, markdownBlocks, turnBlob, type BlobReply } from "../lib/checkpoints";
import { errorText, t } from "../lib/i18n";
import { CsvView, MermaidView, renderMermaidBlocks } from "./rich-viewers";
import { Icon } from "./icons";
import type { Project } from "../lib/types";
import "./turn-timeline.css";

export interface TurnRev {
  session: string;
  n: number;
  /** Folder of the project below the repository top: the daemon names files from the top. */
  prefix?: string;
}

export default function TurnFileView(props: { project: Project; path: string; rev: TurnRev }) {
  const kind = createMemo(() => artifactKind(props.path));
  const [source, setSource] = createSignal(false);
  const [blob] = createResource(
    () => ({ rev: props.rev, path: props.path }),
    (k) => turnBlob(k.rev.session, k.rev.n, (k.rev.prefix ?? "") + k.path, artifactKind(k.path) === "image").catch((e) => ({ exists: false, error: errorText(e) }) as BlobReply & { error?: string }),
  );
  const who = () => {
    const s = store.sessions[props.rev.session];
    return s ? sessionLabel(s) : t("a closed session");
  };
  const rendered = () => !!kind() && kind() !== "image" && !source();

  return (
    <div class="viewer">
      <div class="viewer-bar">
        <span class="path" title={props.path}>&lrm;{props.path}</span>
        <span style={{ color: "var(--text-faint)" }}>{t("End of turn #{n} · {name}", { n: props.rev.n, name: who() })}</span>
        <div class="toggles">
          <Show when={kind() && kind() !== "image"}>
            <button class={`toggle${!source() ? " on" : ""}`} onClick={() => setSource(false)}>{t("Preview")}</button>
            <button class={`toggle${source() ? " on" : ""}`} onClick={() => setSource(true)}>{t("Source")}</button>
          </Show>
          <button class="toggle" onClick={() => store.openFile(props.project.id, props.path, { diff: true, rev: props.rev })}>{t("Diff")}</button>
        </div>
      </div>
      <div class="turn-file-view">
        <Show when={!blob.loading} fallback={<div class="muted">{t("Loading…")}</div>}>
          <Show when={blob()?.exists} fallback={<div class="muted">{(blob() as { error?: string } | undefined)?.error ?? t("This file did not exist at the end of that turn")}</div>}>
            <Show when={!blob()!.too_large} fallback={<div class="muted">{t("Too large to show here")}</div>}>
              <Switch>
                <Match when={kind() === "image" && blob()!.base64}>
                  <div class="image-view"><img src={`data:${imageMime(props.path)};base64,${blob()!.base64}`} alt={props.path} /></div>
                </Match>
                <Match when={blob()!.binary}><div class="muted">{t("Binary file")}</div></Match>
                <Match when={rendered() && kind() === "markdown"}>
                  <MarkdownComments project={props.project} path={props.path} text={blob()!.text ?? ""} />
                </Match>
                <Match when={rendered() && kind() === "mermaid"}><MermaidView project={props.project} path={props.path} text={blob()!.text ?? ""} /></Match>
                <Match when={rendered() && kind() === "csv"}><CsvView project={props.project} path={props.path} text={blob()!.text ?? ""} /></Match>
                <Match when={true}><ReadOnlyText path={props.path} text={blob()!.text ?? ""} /></Match>
              </Switch>
            </Show>
          </Show>
        </Show>
      </div>
    </div>
  );
}

function ReadOnlyText(props: { path: string; text: string }) {
  let host!: HTMLDivElement;
  onMount(async () => {
    const lang = await languageFor(props.path);
    const view = new EditorView({ parent: host, doc: props.text, extensions: [...baseExtensions({ readOnly: true }), ...(lang ? [lang] : []), EditorView.editable.of(false), EditorState.readOnly.of(true)] });
    onCleanup(() => view.destroy());
  });
  return <div class="cm-host turn-file-body" ref={host} />;
}

/** Rendered markdown, one block at a time, each with a comment button. */
function MarkdownComments(props: { project: Project; path: string; text: string }) {
  const blocks = createMemo(() =>
    markdownBlocks(props.text).map((b) => ({
      ...b,
      html: DOMPurify.sanitize(marked.parse(b.raw, { gfm: true, async: false }) as string),
    })),
  );
  const notes = () => store.notesFor(props.project.id).filter((n) => n.path === props.path);
  const [drafting, setDrafting] = createSignal<number | null>(null);
  const [text, setText] = createSignal("");
  let root!: HTMLDivElement;

  onMount(() => {
    if (root.querySelector("code.language-mermaid")) void renderMermaidBlocks(root);
  });

  const save = (line: number, raw: string) => {
    const note = text().trim();
    if (note) {
      store.addNote(props.project.id, { path: props.path, line, side: "new", text: (raw.split("\n").find((l) => l.trim()) ?? "").trim().slice(0, 200), note });
      store.notify(t("Added a review note. Send it to an agent from any diff."));
    }
    setDrafting(null);
    setText("");
  };

  return (
    <div class="markdown turn-file-body" ref={root}>
      <For each={blocks()}>
        {(b) => (
          <>
            <div class="md-block">
              <div innerHTML={b.html} />
              <button class="icon-btn md-comment" title={t("Comment: adds a review note on this part")} onClick={() => { setText(""); setDrafting(b.line); }}><Icon name="note" size={13} /></button>
            </div>
            <For each={notes().filter((n) => n.line === b.line)}>
              {(n) => (
                <div class="md-note">
                  {n.note}
                  <button class="icon-btn" title={t("Resolve (remove)")} onClick={() => store.removeNotes(props.project.id, [n.id])}><Icon name="check" size={12} /></button>
                </div>
              )}
            </For>
            <Show when={drafting() === b.line}>
              <div class="md-note-input">
                <textarea
                  class="commit-box"
                  placeholder={t("What should change here?")}
                  value={text()}
                  ref={(el) => queueMicrotask(() => el.focus())}
                  onInput={(e) => setText(e.currentTarget.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Escape") setDrafting(null);
                    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) save(b.line, b.raw);
                  }}
                />
                <button class="btn primary" onClick={() => save(b.line, b.raw)}>{t("Add note")}</button>
              </div>
            </Show>
          </>
        )}
      </For>
    </div>
  );
}
