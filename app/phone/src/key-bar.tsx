// The keys a phone keyboard lacks. Ctrl is sticky for one key: arm it, then type a letter in the reply box
// and Ctrl+that letter is sent instead of the letter. Which keys show, and in what order, is the person's choice
// (Settings); Ctrl always comes first. Hidden for chat sessions.
import { createSignal, For, onCleanup, Show } from "solid-js";
import { keyResultText } from "./notice";
import { connected, sendKey } from "./phone-state";
import { confirmCtrlC, keyBar, shownKeys, type KeyId } from "./prefs";
import { t } from "./i18n";

const [ctrlArmed, setCtrlArmed] = createSignal(false);
export { ctrlArmed, setCtrlArmed };

/** The key name the daemon takes for `letter` typed while Ctrl is armed; null if it is not a letter. */
export function ctrlKey(letter: string): string | null {
  return /^[a-z]$/i.test(letter) ? `ctrl-${letter.toLowerCase()}` : null;
}

export const KEYS: Record<KeyId, { label: string; title?: string }> = {
  esc: { label: "Esc" },
  tab: { label: "Tab" },
  "shift-tab": { label: "⇧Tab", title: "Shift+Tab" },
  left: { label: "←" },
  up: { label: "↑" },
  down: { label: "↓" },
  right: { label: "→" },
  pgup: { label: "PgUp" },
  pgdn: { label: "PgDn" },
  "ctrl-c": { label: "Ctrl+C" },
  enter: { label: "Enter" },
};

/** A Ctrl+C waiting for the person to say Send: what sends it. */
const [askingCtrlC, setAskingCtrlC] = createSignal<(() => void) | null>(null);

/** Sends `key` to session `id` and says how it went. Ctrl+C stops what the agent is doing, so it asks first when the person wants that:
 *  the key bar's key and Ctrl with "c" in the reply box both come through here. */
export function pressKey(id: string, key: string, onResult: (message: string | null) => void) {
  const go = () => void sendKey(id, key).then((r) => onResult(keyResultText(r)));
  if (key === "ctrl-c" && confirmCtrlC.get()) setAskingCtrlC(() => go);
  else go();
}

export function KeyBar(props: { id: string; onResult: (message: string | null) => void }) {
  // A question about another session, or one left when this bar goes, is not asked any more.
  onCleanup(() => setAskingCtrlC(null));
  const press = (key: string) => {
    setCtrlArmed(false);
    pressKey(props.id, key, props.onResult);
  };
  const send = () => {
    const go = askingCtrlC();
    setAskingCtrlC(null);
    go?.();
  };
  return (
    <>
      <Show when={askingCtrlC()}>
        <div class="confirm" role="alert" data-ctrl-c-confirm>
          <span>{t("Send Ctrl+C? It stops what the agent is doing.")}</span>
          <button class="btn" data-ctrl-c-cancel onClick={() => setAskingCtrlC(null)}>{t("Cancel")}</button>
          <button class="btn primary" data-ctrl-c-send disabled={!connected()} onClick={send}>{t("Send")}</button>
        </div>
      </Show>
      <div class="keys" role="toolbar" aria-label={t("Keys")}>
        <button
          class="key" classList={{ armed: ctrlArmed() }} aria-pressed={ctrlArmed()} disabled={!connected()}
          onMouseDown={(e) => e.preventDefault()} onClick={() => setCtrlArmed(!ctrlArmed())}
        >
          Ctrl
        </button>
        <For each={shownKeys(keyBar.get())}>
          {(k) => (
            <button class="key" data-key={k} title={KEYS[k].title} disabled={!connected()} onMouseDown={(e) => e.preventDefault()} onClick={() => press(k)}>
              {KEYS[k].label}
            </button>
          )}
        </For>
      </div>
    </>
  );
}
