import { For, Show, createEffect, createMemo, createSignal, on, onMount } from "solid-js";
import { store } from "../lib/store";
import { searchText, type TextHit } from "../lib/ipc";
import { Icon } from "./icons";
import type { Project } from "../lib/types";
import { t } from "../lib/i18n";
import { LoadBar } from "./load-bar";

/** Folder the next search is limited to ("Find in Folder" in the explorer); "" = whole project. */
export const [searchFolder, setSearchFolder] = createSignal("");

export function SearchPanel(props: { project: Project }) {
  let input!: HTMLInputElement;
  const [query, setQuery] = createSignal("");
  const [regex, setRegex] = createSignal(false);
  const [caseSensitive, setCase] = createSignal(false);
  const [result, setResult] = createSignal<{ hits: TextHit[]; truncated: boolean; files_searched: number } | null>(null);
  const [busy, setBusy] = createSignal(false);
  onMount(() => input.focus());
  // Find in Folder while the panel is already open: search again in the new scope.
  createEffect(on(searchFolder, () => { input.focus(); if (query().trim()) void run(); }, { defer: true }));

  let seq = 0;
  const run = async () => {
    const q = query();
    if (!q.trim()) return setResult(null);
    const mine = ++seq;
    setBusy(true);
    try {
      const r = await searchText(props.project.path, q, regex(), caseSensitive(), searchFolder() || undefined);
      if (mine === seq) setResult(r);
    } catch (e) {
      if (mine === seq) store.notify(String(e));
    } finally {
      if (mine === seq) setBusy(false);
    }
  };

  const grouped = createMemo(() => {
    const map = new Map<string, TextHit[]>();
    for (const h of result()?.hits ?? []) {
      if (!map.has(h.path)) map.set(h.path, []);
      map.get(h.path)!.push(h);
    }
    return [...map.entries()];
  });

  return (
    <>
      <LoadBar when={busy()} />
      <div class="search-box">
        <input
          ref={input}
          class="text-input"
          placeholder={t("Search in files (Enter)")}
          value={query()}
          onInput={(e) => setQuery(e.currentTarget.value)}
          onKeyDown={(e) => e.key === "Enter" && void run()}
        />
        <div class="toggles">
          <button class={`toggle${caseSensitive() ? " on" : ""}`} title={t("Match case")} onClick={() => { setCase(!caseSensitive()); void run(); }}>Aa</button>
          <button class={`toggle${regex() ? " on" : ""}`} title={t("Regular expression")} onClick={() => { setRegex(!regex()); void run(); }}>.*</button>
          <Show when={searchFolder()}>
            <span class="chip" title={t("Searching only this folder")}>{t("in {folder}/", { folder: searchFolder() })} <button class="icon-btn mini" title={t("Search the whole project")} onClick={() => { setSearchFolder(""); void run(); }}><Icon name="close" size={9} /></button></span>
          </Show>
          <span style={{ flex: 1 }} />
          <Show when={busy()}><span class="muted" style={{ padding: 0 }}>{t("searching…")}</span></Show>
          <Show when={result() && !busy()}>
            <span class="muted" style={{ padding: 0 }}>{t("{hits} hits · {files} files", { hits: `${result()!.hits.length}${result()!.truncated ? "+" : ""}`, files: result()!.files_searched })}</span>
          </Show>
        </div>
      </div>
      <div class="list">
        <For each={grouped()}>
          {([path, hits]) => (
            <>
              <div class="list-row" style={{ "font-weight": 500 }} onClick={() => store.openFile(props.project.id, path)}>
                <span class="name">{path.split("/").pop()}</span>
                <span class="dir">{path}</span>
                <span class="kbd">{hits.length}</span>
              </div>
              <For each={hits}>
                {(h) => (
                  <div class="hit" onClick={() => store.openFile(props.project.id, h.path, { line: h.line })} title={h.text}>
                    <span class="ln">{h.line}</span>{h.text.trim()}
                  </div>
                )}
              </For>
            </>
          )}
        </For>
      </div>
    </>
  );
}
