// Copying from the transcript: the shared clipboard call and the small copy button of its rows.
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { store } from "../lib/store";
import { t } from "../lib/i18n";
import { Icon } from "./icons";

/** Copies `text`, ticking the button that asked for a moment. */
export function copyText(text: string, button: HTMLElement) {
  void writeText(text).then(
    () => {
      button.classList.add("copied");
      setTimeout(() => button.classList.remove("copied"), 1200);
    },
    () => store.notify(t("Could not copy")),
  );
}

export function CopyButton(props: { text: string }) {
  return (
    <button class="msg-action" title={t("Copy")} aria-label={t("Copy")} onClick={(e) => copyText(props.text, e.currentTarget)}>
      <span class="i-copy"><Icon name="copy" size={12} /></span><span class="i-done"><Icon name="check" size={12} /></span>
    </button>
  );
}
