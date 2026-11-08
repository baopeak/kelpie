// One option of Settings → Mobile: its title and control, what it does, what it costs, how safe it is, which phones it is about
// where that differs, and, when it cannot be changed now, why and what to do. Later options add a row here, not a layout.
// The words are in the settings catalog, so the page search finds them; what the row can do now is `optionState`'s.
import { Show, type JSX } from "solid-js";
import { t } from "../../lib/i18n";
import { catalogEntry } from "../../lib/settings-catalog";
import type { OptionId, OptionState } from "../../lib/mobile-options";
import { SettingRow, Toggle } from "./setting-row";

/** `control` replaces the switch, for an option with a select or that this computer only reports. */
export function OptionRow(props: { id: OptionId; state: OptionState; onChange?: (on: boolean) => void; control?: JSX.Element; children?: JSX.Element }) {
  const entry = catalogEntry(props.id)!;
  return (
    <SettingRow
      id={props.id}
      wide
      aside={props.control ?? <Toggle on={props.state.on} disabled={!!props.state.disabled} label={t(entry.label)} onChange={(on) => props.onChange?.(on)} />}
      lines={
        <dl class="option-lines setting-desc">
          <div data-option-what><dt>{t("What it does")}</dt><dd>{t(entry.description)}</dd></div>
          <div data-option-tradeoff><dt>{t("Trade-off")}</dt><dd>{t(entry.tradeoff!)}</dd></div>
          <div data-option-safety><dt>{t("How safe")}</dt><dd>{t(entry.safety!)}</dd></div>
          <Show when={entry.phones}><div data-option-phones><dt>{t("Which phones")}</dt><dd>{t(entry.phones!)}</dd></div></Show>
        </dl>
      }
    >
      <Show when={props.state.disabled} fallback={<Show when={props.state.presence}><div class="muted-inline" data-option-presence>{props.state.presence}</div></Show>}>
        <div class="form-note warn-note" data-option-disabled>{props.state.disabled}</div>
      </Show>
      {props.children}
    </SettingRow>
  );
}
