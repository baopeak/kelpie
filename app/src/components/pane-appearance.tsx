// A pane's quick theme switch (its menu → Appearance…): a theme for this pane only, kept until the
// pane's session closes, and "Save for this host" to make it the host's own. Arrow keys move through
// the themes and the pane follows as you go.
import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { store } from "../lib/store";
import { t } from "../lib/i18n";
import { setCovering } from "../lib/overlays";
import { hostOf, saveHostTheme } from "../lib/ssh-hosts";
import { findTerminalTheme, terminalThemes, type TerminalTheme } from "../lib/terminal-themes";
import { paneTheme, setPaneTheme } from "../lib/terminal-appearance";
import { allViews } from "../lib/terminal-host";
import { openThemeEditor } from "./theme-editor";

const [picker, setPicker] = createSignal<{ sessionId: string; x: number; y: number } | null>(null);

export const openPaneAppearance = (sessionId: string, x: number, y: number) => setPicker({ sessionId, x, y });

export function PaneAppearanceHost() {
  createEffect(() => setCovering("pane-appearance", !!picker()));
  return <Show when={picker()} keyed>{(p) => <PaneAppearance {...p} />}</Show>;
}

function PaneAppearance(props: { sessionId: string; x: number; y: number }) {
  let box!: HTMLDivElement;
  const [error, setError] = createSignal<string | null>(null);
  const [saving, setSaving] = createSignal(false);
  const host = () => hostOf(store.sessions[props.sessionId]?.remote);
  /** "" = no pane pick (the host's theme, else the app's). */
  const current = () => {
    const id = paneTheme(props.sessionId);
    // A pick of a theme deleted since is no pick.
    return id && findTerminalTheme(id) ? id : "";
  };
  const focusPane = () => queueMicrotask(() => [...allViews()].find((v) => v.sessionId === props.sessionId)?.focus());
  const baseLabel = () => {
    const h = host();
    const own = h && findTerminalTheme(h.terminalTheme);
    return own ? t("Host's theme ({name})", { name: own.label }) : t("App's terminal theme");
  };
  const options = () => [{ id: "", label: baseLabel(), theme: undefined as TerminalTheme | undefined }, ...terminalThemes().map((th) => ({ id: th.id, label: th.label, theme: th }))];
  const pick = (id: string) => setPaneTheme(props.sessionId, id || null);
  const close = (refocus = true) => {
    setPicker(null);
    if (refocus) focusPane();
  };
  const save = async () => {
    const h = host();
    const id = current();
    if (!h || !id) return;
    setSaving(true);
    setError(null);
    try {
      await saveHostTheme(h.id, id);
      // The host has it now: the pane no longer needs its own pick.
      setPaneTheme(props.sessionId, null);
      store.notify(t("{host} now uses {theme}", { host: h.label, theme: findTerminalTheme(id)?.label ?? id }));
      close();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };
  const move = (by: number) => {
    const list = options();
    const i = Math.max(0, list.findIndex((o) => o.id === current()));
    const next = list[(i + by + list.length) % list.length];
    pick(next.id);
    queueMicrotask(() => box.querySelector<HTMLElement>(`[data-theme-option="${CSS.escape(next.id)}"]`)?.focus());
  };

  onMount(() => {
    const outside = (e: PointerEvent) => !box.contains(e.target as Node) && close(false);
    // Capture: a focused terminal must not take these keys.
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        close();
      }
    };
    window.addEventListener("pointerdown", outside, true);
    window.addEventListener("keydown", onKey, true);
    onCleanup(() => {
      window.removeEventListener("pointerdown", outside, true);
      window.removeEventListener("keydown", onKey, true);
    });
    queueMicrotask(() => box.querySelector<HTMLElement>(`[data-theme-option="${CSS.escape(current())}"]`)?.focus());
  });

  const left = () => Math.max(8, Math.min(props.x, window.innerWidth - 268));
  const top = () => Math.max(8, Math.min(props.y, window.innerHeight - 420));
  const swatch = (th?: TerminalTheme) => (th ? [th.colors[0], th.colors[4], th.colors[5], th.colors[7], th.colors[1]] : []);

  return (
    <div ref={box} class="pane-appearance" role="dialog" aria-label={t("Appearance for this pane")} data-pane-appearance style={{ left: `${left()}px`, top: `${top()}px` }}>
      <div class="pane-appearance-title">{t("Appearance for this pane")}</div>
      <div class="pane-appearance-list" role="listbox" aria-label={t("Colour theme")}
        onKeyDown={(e) => {
          if (e.key === "ArrowDown" || e.key === "ArrowUp") {
            e.preventDefault();
            move(e.key === "ArrowDown" ? 1 : -1);
          } else if (e.key === "Enter") {
            e.preventDefault();
            close();
          }
        }}>
        <For each={options()}>
          {(o) => (
            <button class={`pane-appearance-option${o.id === current() ? " on" : ""}`} role="option" aria-selected={o.id === current()} tabIndex={o.id === current() ? 0 : -1}
              data-theme-option={o.id} onClick={() => pick(o.id)}>
              <span class="theme-strip" aria-hidden="true"><For each={swatch(o.theme)}>{(c) => <span style={{ background: c }} />}</For></span>
              <span class="theme-list-label">{o.label}</span>
            </button>
          )}
        </For>
      </div>
      <Show when={error()}><div class="form-note error" role="alert">{error()}</div></Show>
      <div class="pane-appearance-foot">
        <Show when={host()}>
          <button class="btn primary" data-action="save-host-theme" disabled={!current() || saving()} onClick={() => void save()}
            title={t("Panes on {host} use this theme from now on", { host: host()!.label })}>{t("Save for this host")}</button>
        </Show>
        <button class="btn" onClick={() => { close(false); openThemeEditor(undefined, focusPane); }}>{t("Edit themes…")}</button>
      </div>
    </div>
  );
}
