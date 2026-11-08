// The status bar's sign that a phone sees this computer's screen: who, what, and Stop. It is there for as long as one does; the border and
// badge the helper draws around what is shared say the same on the screen itself.
import { For, Show } from "solid-js";
import { stopScreen, viewers } from "../lib/screen";
import { store } from "../lib/store";
import { t } from "../lib/i18n";
import { Icon } from "./icons";

export function ScreenIndicator() {
  return (
    <Show when={viewers().length}>
      <For each={viewers()}>
        {(v) => (
          <span class="item on screen-shared" data-screen-shared={v.sc} title={t("{device} sees {title}. {hotkey} stops it too.", { device: v.device, title: v.title, hotkey: store.state.settings.screenStopHotkey })}>
            <Icon name="screen" size={13} /> {t("Shared with {device}", { device: v.device })}
            <button class="btn danger mini" data-screen-stop onClick={() => void stopScreen(v.sc)}>{t("Stop")}</button>
          </span>
        )}
      </For>
    </Show>
  );
}
