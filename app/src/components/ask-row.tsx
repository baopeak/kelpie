// A one-line question shown in place (no blocking browser prompt): the Hosts page and the Files panes.
import { createSignal, onMount } from "solid-js";
import { t } from "../lib/i18n";

export type Ask = { label: string; value: string; secret?: boolean; ok: (v: string) => void };

export function AskRow(props: { ask: Ask; onClose: () => void }) {
  const [v, setV] = createSignal(props.ask.value);
  let input!: HTMLInputElement;
  onMount(() => input.focus());
  const ok = () => {
    props.onClose();
    props.ask.ok(v());
  };
  return (
    <div class="inline-fields ask-row">
      <span class="muted-inline">{props.ask.label}</span>
      <input ref={input} class="text-input" data-field="ask" style={{ flex: 1, width: "auto" }} type={props.ask.secret ? "password" : "text"} autocomplete="off" value={v()}
        onInput={(e) => setV(e.currentTarget.value)} onKeyDown={(e) => { if (e.key === "Enter") ok(); if (e.key === "Escape") { e.preventDefault(); props.onClose(); } }} />
      <button class="btn primary" data-action="ask-ok" onClick={ok}>{t("OK")}</button>
      <button class="btn" onClick={props.onClose}>{t("Cancel")}</button>
    </div>
  );
}
