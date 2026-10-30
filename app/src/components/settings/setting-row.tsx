// Building blocks of the settings page: a catalog-backed row (label, one-line description,
// hidden when it does not match the page search) and small controls.
import { For, Show, children, createSignal, onMount, type JSX } from "solid-js";
import { store } from "../../lib/store";
import { t } from "../../lib/i18n";
import { allViews } from "../../lib/terminal-host";
import { catalogEntry, entryMatches } from "../../lib/settings-catalog";
import type { Settings } from "../../lib/types";

/** Text of the settings page search box. */
export const [settingsQuery, setSettingsQuery] = createSignal("");

/** Saves settings and re-applies them to open terminals. */
export function applySettings(patch: Partial<Settings>) {
  store.setSettings(patch);
  for (const v of allViews()) v.applySettings(store.state.settings);
}

/** `match` replaces the catalog match for rows whose content is searchable too. `aside` (usually the
 *  row's on/off switch) sits right of the label, as in a narrow row, with a wide row's body below. */
export function SettingRow(props: { id: string; wide?: boolean; match?: (query: string) => boolean; aside?: JSX.Element; children?: JSX.Element }) {
  const entry = catalogEntry(props.id)!;
  const aside = children(() => props.aside);
  const visible = () => !settingsQuery().trim() || (props.match ?? ((q: string) => entryMatches(entry, q)))(settingsQuery());
  const target = () => store.settingsItem() === props.id;
  let el: HTMLDivElement | undefined;
  onMount(() => target() && queueMicrotask(() => el?.scrollIntoView({ block: "center" })));
  return (
    <Show when={visible()}>
      <div ref={el} class={`setting-row${props.wide ? " wide" : ""}${aside() ? " has-aside" : ""}${target() ? " target" : ""}`} data-setting={props.id}>
        <div class="setting-text">
          <div class="setting-label">{t(entry.label)}</div>
          <div class="setting-desc">{t(entry.description)}</div>
        </div>
        <Show when={aside()}><div class="setting-aside">{aside()}</div></Show>
        <div class="setting-control">{props.children}</div>
      </div>
    </Show>
  );
}

export function Toggle(props: { on: boolean; onChange: (on: boolean) => void; label?: string }) {
  return (
    <button class={`switch${props.on ? " on" : ""}`} role="switch" aria-checked={props.on} aria-label={props.label} onClick={() => props.onChange(!props.on)}>
      <span />
    </button>
  );
}

export function Choice<T extends string | number>(props: { value: T; options: [T, string][]; onChange: (v: T) => void }) {
  return (
    <div class="seg">
      <For each={props.options}>
        {([v, label]) => <button class={props.value === v ? "on" : ""} onClick={() => props.onChange(v)}>{label}</button>}
      </For>
    </div>
  );
}
