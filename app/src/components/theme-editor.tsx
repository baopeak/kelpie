// Terminal themes of one's own (Settings → Terminal → Colour theme → Edit themes, or a host's
// Appearance): create, duplicate, edit each colour, rename, delete, import and export. Edits are kept
// in the app settings as they are made and repaint the terminals that use the theme at once.
import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { save } from "@tauri-apps/plugin-dialog";
import { store } from "../lib/store";
import { t } from "../lib/i18n";
import { writeFile } from "../lib/ipc";
import { setCovering } from "../lib/overlays";
import { confirmDialog } from "../lib/confirm-dialog";
import { terminalTheme as appDark, terminalThemeLight as appLight } from "../lib/theme";
import { customThemes, findTerminalTheme, terminalThemes, toXtermTheme, type TerminalTheme } from "../lib/terminal-themes";
import {
  MAX_THEME_FILE, ThemeFileError, addImported, checkThemes, contrastRatio, deleteTheme, duplicateTheme, exportFileName, importThemeFile, mix, normalizeHex,
  renameTheme, toWindowsTerminal, updateTheme,
} from "../lib/theme-io";
import { resolveAppearance } from "../lib/terminal-appearance";
import { TerminalPreview } from "./terminal-preview";
import { Icon } from "./icons";

const [editing, setEditing] = createSignal<{ selected: string | null; refocus?: () => void } | null>(null);

/** Opens the theme editor, on `id` when it is one of the person's themes. `refocus` says where the focus goes
 *  back to when it closes, for an opener that is gone by then (the pane's theme switch). */
export const openThemeEditor = (id?: string, refocus?: () => void) => setEditing({ selected: id ?? null, refocus });

const errText = (e: unknown) => (e instanceof ThemeFileError || e instanceof Error ? e.message : String(e));

/** The app's own palette (dark or light) as a starting point for a new theme. */
function appPalette(light: boolean): TerminalTheme {
  const x = light ? appLight : appDark;
  const ansi = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white", "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue", "brightMagenta", "brightCyan", "brightWhite"] as const;
  const bg = x.background!;
  return { id: "", label: t("My theme"), dark: !light, colors: [bg, x.foreground!, x.cursor!, ...ansi.map((k) => x[k]!)], selection: mix(bg, x.cursor!, 0.3) };
}

const SLOT_LABELS = () => [
  t("Background"), t("Foreground"), t("Cursor"),
  t("Black"), t("Red"), t("Green"), t("Yellow"), t("Blue"), t("Magenta"), t("Cyan"), t("White"),
  t("Bright black"), t("Bright red"), t("Bright green"), t("Bright yellow"), t("Bright blue"), t("Bright magenta"), t("Bright cyan"), t("Bright white"),
];

export function ThemeEditorHost() {
  createEffect(() => setCovering("theme-editor", !!editing()));
  return <Show when={editing()}>{(e) => <ThemeEditor initial={e().selected} refocus={e().refocus} />}</Show>;
}

function ThemeEditor(props: { initial: string | null; refocus?: () => void }) {
  const list = () => customThemes();
  const [selectedId, setSelectedId] = createSignal<string | null>(props.initial ?? list()[0]?.id ?? null);
  const selected = () => list().find((x) => x.id === selectedId());
  const [error, setError] = createSignal<string | null>(null);
  const [note, setNote] = createSignal<string | null>(null);
  let fileInput!: HTMLInputElement;
  let dialog!: HTMLDivElement;
  const before = document.activeElement as HTMLElement | null;

  const close = () => setEditing(null);
  // Only well-formed themes are stored (one the state file could not read back is never kept).
  const commit = (next: TerminalTheme[]) => store.setSettings({ customThemes: checkThemes(next) });
  const attempt = (fn: () => void) => {
    setError(null);
    setNote(null);
    try {
      fn();
    } catch (e) {
      setError(errText(e));
    }
  };

  const create = () => attempt(() => {
    const { list: next, theme } = duplicateTheme(list(), appPalette(store.state.settings.theme === "light"), t("My theme"));
    commit(next);
    setSelectedId(theme.id);
  });
  const duplicate = (id: string) => attempt(() => {
    const from = findTerminalTheme(id);
    if (!from) return;
    const { list: next, theme } = duplicateTheme(list(), from);
    commit(next);
    setSelectedId(theme.id);
  });
  const edit = (patch: Partial<TerminalTheme>) => {
    const cur = selected();
    if (cur) attempt(() => commit(updateTheme(list(), { ...cur, ...patch })));
  };
  const setColor = (slot: number | "selection", value: string) => {
    const cur = selected();
    const hex = normalizeHex(value);
    if (!cur || !hex) return false;
    if (slot === "selection") edit({ selection: hex });
    else {
      const colors = [...cur.colors] as TerminalTheme["colors"];
      colors[slot] = hex;
      edit({ colors });
    }
    return true;
  };
  const remove = async () => {
    const cur = selected();
    if (!cur || !(await confirmDialog(t("Delete the theme {name}? Panes that use it go back to the app's terminal theme.", { name: cur.label })))) return;
    const index = list().findIndex((x) => x.id === cur.id);
    attempt(() => {
      const next = deleteTheme(list(), cur.id);
      commit(next);
      if (store.state.settings.terminalTheme === cur.id) store.setSettings({ terminalTheme: "default" });
      setSelectedId(next[Math.min(index, next.length - 1)]?.id ?? null);
    });
  };
  const importFile = async (file: File) => {
    setError(null);
    setNote(null);
    if (file.size > MAX_THEME_FILE) return setError(t("The file is too large for a colour scheme."));
    try {
      const themes = importThemeFile(await file.text(), file.name);
      const { list: next, added } = addImported(list(), themes);
      commit(next);
      setSelectedId(added[0].id);
      setNote(t("Imported {n} theme(s).", { n: added.length }));
    } catch (e) {
      setError(t("Could not import {file}: {error}", { file: file.name, error: errText(e) }));
    }
  };
  const exportTheme = async () => {
    const cur = selected();
    if (!cur) return;
    setError(null);
    setNote(null);
    try {
      const path = await save({ title: t("Export theme as Windows Terminal JSON"), defaultPath: exportFileName(cur), filters: [{ name: "JSON", extensions: ["json"] }] });
      if (!path) return;
      const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
      await writeFile(path.slice(0, cut + 1), path.slice(cut + 1), `${JSON.stringify(toWindowsTerminal(cur), null, 2)}\n`);
      setNote(t("Saved {file}", { file: path }));
    } catch (e) {
      setError(errText(e));
    }
  };

  onMount(() => {
    // Capture: the settings page and a focused terminal must not take this Escape.
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      e.preventDefault();
      e.stopImmediatePropagation();
      close();
    };
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => {
      window.removeEventListener("keydown", onKey, true);
      if (before?.isConnected) before.focus();
      else props.refocus?.();
    });
    queueMicrotask(() => dialog.querySelector<HTMLElement>("[data-autofocus]")?.focus());
  });

  /** Tab stays inside the dialog: a terminal under it must never take typing it cannot show. */
  const trapTab = (e: KeyboardEvent) => {
    if (e.key !== "Tab") return;
    const all = [...dialog.querySelectorAll<HTMLElement>("button, input, select, textarea, [tabindex]")].filter((x) => !x.hasAttribute("disabled") && x.tabIndex >= 0 && x.offsetParent !== null);
    if (!all.length) return;
    const [first, last] = [all[0], all[all.length - 1]];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  };
  /** Up / Down in the list select the theme before or after. */
  const listKeys = (e: KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    const ids = list().map((x) => x.id);
    if (!ids.length) return;
    const i = ids.indexOf(selectedId() ?? "");
    const next = ids[Math.max(0, Math.min(ids.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)))];
    setSelectedId(next);
    queueMicrotask(() => dialog.querySelector<HTMLElement>(`[data-theme-id="${CSS.escape(next)}"]`)?.focus());
  };

  const look = () => {
    const cur = selected();
    const base = resolveAppearance(store.state.settings);
    return cur ? { ...base, theme: toXtermTheme(cur) } : base;
  };
  const contrast = () => {
    const cur = selected();
    return cur ? contrastRatio(cur.colors[0], cur.colors[1]) : 21;
  };
  const strip = (th: TerminalTheme) => [th.colors[0], th.colors[1], th.colors[4], th.colors[5], th.colors[7]];

  return (
    <div class="overlay theme-editor-overlay" onPointerDown={(e) => e.target === e.currentTarget && close()}>
      <div ref={dialog} class="modal theme-editor" role="dialog" aria-modal="true" aria-labelledby="theme-editor-title" data-theme-editor onKeyDown={trapTab}>
        <div class="theme-editor-head">
          <h2 id="theme-editor-title">{t("Terminal themes")}</h2>
          <button class="icon-btn" title={t("Close (Esc)")} aria-label={t("Close (Esc)")} onClick={close}><Icon name="close" size={13} /></button>
        </div>
        <div class="theme-editor-body">
          <div class="theme-list-col">
            <div class="theme-list" role="listbox" aria-label={t("Your themes")} onKeyDown={listKeys}>
              <For each={list()} fallback={<div class="muted-inline theme-list-empty">{t("No themes of your own yet. Start one, duplicate a theme or import a file.")}</div>}>
                {(th) => (
                  <button
                    class={`theme-list-item${th.id === selectedId() ? " on" : ""}`}
                    role="option"
                    aria-selected={th.id === selectedId()}
                    tabIndex={th.id === selectedId() || (!selected() && th === list()[0]) ? 0 : -1}
                    data-theme-id={th.id}
                    onClick={() => setSelectedId(th.id)}
                  >
                    <span class="theme-strip" aria-hidden="true"><For each={strip(th)}>{(c) => <span style={{ background: c }} />}</For></span>
                    <span class="theme-list-label">{th.label}</span>
                  </button>
                )}
              </For>
            </div>
            <div class="theme-list-actions">
              <button class="btn" data-action="theme-new" data-autofocus onClick={create}>{t("New theme")}</button>
              <select class="text-input" data-action="theme-duplicate" aria-label={t("Duplicate a theme")} value="" onChange={(e) => { const v = e.currentTarget.value; e.currentTarget.value = ""; if (v) duplicate(v); }}>
                <option value="">{t("Duplicate a theme…")}</option>
                <For each={terminalThemes()}>{(th) => <option value={th.id}>{th.label}</option>}</For>
              </select>
              <button class="btn" data-action="theme-import" onClick={() => fileInput.click()}>{t("Import…")}</button>
              <input ref={fileInput} type="file" hidden accept=".json,.itermcolors,.yaml,.yml,.xml" data-field="theme-file"
                onChange={(e) => { const f = e.currentTarget.files?.[0]; e.currentTarget.value = ""; if (f) void importFile(f); }} />
              <div class="form-note">{t("Windows Terminal (.json), iTerm2 (.itermcolors) or base16 (.yaml, .json).")}</div>
            </div>
          </div>
          <div class="theme-edit-col">
            <Show when={selected()} fallback={<TerminalPreview look={look()} />}>
              {(cur) => (
                <>
                  <div class="inline-fields">
                    <input class="text-input" data-field="theme-name" aria-label={t("Theme name")} value={cur().label} maxLength={80}
                      onChange={(e) => {
                        const field = e.currentTarget;
                        attempt(() => commit(renameTheme(list(), cur().id, field.value)));
                        // An empty or taken name: the field shows the name the theme has.
                        field.value = cur().label;
                      }} />
                    <div class="seg" role="group" aria-label={t("Background kind")}>
                      <button class={cur().dark ? "on" : ""} aria-pressed={cur().dark} onClick={() => edit({ dark: true })}>{t("Dark")}</button>
                      <button class={!cur().dark ? "on" : ""} aria-pressed={!cur().dark} onClick={() => edit({ dark: false })}>{t("Light")}</button>
                    </div>
                  </div>
                  <TerminalPreview look={look()} />
                  <Show when={contrast() < 4.5}>
                    <div class="form-note warn" role="status">{t("Text on the background has a contrast of {ratio}:1; 4.5:1 or more is easier to read.", { ratio: contrast().toFixed(1) })}</div>
                  </Show>
                  <div class="theme-colors">
                    <For each={[0, 1, 2]}>{(i) => <ColorField label={SLOT_LABELS()[i]} slot={String(i)} value={cur().colors[i]} onPick={(v) => setColor(i, v)} />}</For>
                    <ColorField label={cur().selection ? t("Selection") : t("Selection (automatic)")} slot="selection" value={cur().selection ?? mix(cur().colors[0], cur().colors[1], 0.25)} onPick={(v) => setColor("selection", v)} />
                    <Show when={cur().selection}>
                      <button class="btn" data-action="selection-auto" onClick={() => edit({ selection: undefined })} title={t("A translucent wash over the text, as before")}>{t("Automatic selection")}</button>
                    </Show>
                  </div>
                  <div class="theme-colors ansi">
                    <For each={Array.from({ length: 16 }, (_, k) => k + 3)}>{(i) => <ColorField label={SLOT_LABELS()[i]} slot={String(i)} value={cur().colors[i]} onPick={(v) => setColor(i, v)} />}</For>
                  </div>
                  <div class="inline-fields theme-edit-actions">
                    <button class="btn" data-action="theme-export" onClick={() => void exportTheme()}>{t("Export…")}</button>
                    <button class="btn danger" data-action="theme-delete" onClick={() => void remove()}>{t("Delete")}</button>
                  </div>
                </>
              )}
            </Show>
          </div>
        </div>
        <Show when={error()}><div class="form-note error" role="alert" data-theme-error>{error()}</div></Show>
        <Show when={note()}><div class="form-note" role="status">{note()}</div></Show>
        <div class="footer">
          <button class="btn primary" onClick={close}>{t("Done")}</button>
        </div>
      </div>
    </div>
  );
}

/** One colour: a picker and its hex value. A typed value applies once it is a whole `#rrggbb` (or a
 *  `#rgb` when the field is left), so typing never jumps under the cursor. */
function ColorField(props: { label: string; slot: string; value: string; onPick: (hex: string) => boolean }) {
  const [draft, setDraft] = createSignal<string | null>(null);
  const bad = () => draft() !== null && !normalizeHex(draft());
  // Dragging in the picker sends many values: one is applied per frame.
  let pending: string | null = null;
  const pickSoon = (v: string) => {
    if (pending === null) {
      requestAnimationFrame(() => {
        const last = pending;
        pending = null;
        if (last) props.onPick(last);
      });
    }
    pending = v;
  };
  const settle = () => {
    const d = draft();
    if (d !== null && normalizeHex(d)) props.onPick(d);
    setDraft(null);
  };
  return (
    <div class="color-field" data-color-slot={props.slot}>
      <span class="color-field-label">{props.label}</span>
      <input type="color" value={props.value} aria-label={props.label} onInput={(e) => pickSoon(e.currentTarget.value)} onChange={(e) => props.onPick(e.currentTarget.value)} />
      <input class="text-input mono" value={draft() ?? props.value} aria-label={t("{name} hex value", { name: props.label })} aria-invalid={bad()} spellcheck={false} maxLength={7}
        onInput={(e) => {
          const v = e.currentTarget.value.trim();
          if (/^#[0-9a-f]{6}$/i.test(v) && props.onPick(v)) setDraft(null);
          else setDraft(v);
        }}
        onKeyDown={(e) => e.key === "Enter" && settle()}
        onBlur={settle} />
    </div>
  );
}
