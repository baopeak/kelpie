// The keys a phone keyboard lacks. Ctrl is sticky for one key: arm it, then type a letter in the reply box
// and Ctrl+that letter is sent instead of the letter. Hidden for chat sessions.
import { createSignal, For } from "solid-js";
import { connected, sendKey } from "./phone-state";
import { t } from "./i18n";

const [ctrlArmed, setCtrlArmed] = createSignal(false);
export { ctrlArmed, setCtrlArmed };

/** The key name the daemon takes for `letter` typed while Ctrl is armed; null if it is not a letter. */
export function ctrlKey(letter: string): string | null {
  return /^[a-z]$/i.test(letter) ? `ctrl-${letter.toLowerCase()}` : null;
}

const KEYS: { label: string; key: string; title?: string }[] = [
  { label: "Esc", key: "esc" },
  { label: "Tab", key: "tab" },
  { label: "⇧Tab", key: "shift-tab", title: "Shift+Tab" },
  { label: "←", key: "left" },
  { label: "↑", key: "up" },
  { label: "↓", key: "down" },
  { label: "→", key: "right" },
  { label: "PgUp", key: "pgup" },
  { label: "PgDn", key: "pgdn" },
  { label: "Ctrl+C", key: "ctrl-c" },
  { label: "Enter", key: "enter" },
];

export function KeyBar(props: { id: string; onResult: (message: string | null) => void }) {
  const press = async (key: string) => {
    setCtrlArmed(false);
    const r = await sendKey(props.id, key);
    props.onResult(r.ok ? null : r.reason === "offline" ? t("Offline: the key was not sent.") : r.reason === "unknown" ? t("Not sure the key arrived. Check the screen.") : r.message ?? null);
  };
  return (
    <div class="keys" role="toolbar" aria-label={t("Keys")}>
      <button
        class="key" classList={{ armed: ctrlArmed() }} aria-pressed={ctrlArmed()} disabled={!connected()}
        onMouseDown={(e) => e.preventDefault()} onClick={() => setCtrlArmed(!ctrlArmed())}
      >
        Ctrl
      </button>
      <For each={KEYS}>
        {(k) => (
          <button class="key" title={k.title} disabled={!connected()} onMouseDown={(e) => e.preventDefault()} onClick={() => press(k.key)}>
            {k.label}
          </button>
        )}
      </For>
    </div>
  );
}
