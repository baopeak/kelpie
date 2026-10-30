// A chat whose queue is held up by a message only this window can send (one with a picture), while that chat's pane is not on
// screen (another tab or project unmounts it): the chat's own chips say so, and this says so wherever the user is. The daemon
// keeps strict order, so nothing behind that message goes out until the chat is open here again or someone skips the message.
import { For, Show } from "solid-js";
import { sessionLabel, store } from "../lib/store";
import { t } from "../lib/i18n";
import type { SessionInfo } from "../lib/types";

/** `waits_for_window` is set by the daemon on a chat session's info (API 21). */
type Info = SessionInfo & { waits_for_window?: boolean };

export function ChatWaitNotice() {
  const waiting = () => (Object.values(store.sessions) as Info[]).filter((s) => s?.waits_for_window && !store.isSessionVisible(s.id));
  return (
    <Show when={waiting().length}>
      <div style={{ position: "fixed", bottom: "76px", left: "50%", transform: "translateX(-50%)", "z-index": 119, display: "flex", "flex-direction": "column", gap: "6px", "max-width": "70vw" }}>
        <For each={waiting()}>
          {(s) => (
            <div class="toast chat-wait-notice" data-session={s.id} role="status" style={{ position: "static", transform: "none", "max-width": "none" }}>
              <span>{t("A message in “{chat}” can only be sent with its chat open here. Messages sent after it wait too.", { chat: sessionLabel(s) })}</span>
              <button class="btn" style={{ height: "24px", "margin-left": "10px" }} onClick={() => store.revealSession(s.id)}>{t("Open chat")}</button>
            </div>
          )}
        </For>
      </div>
    </Show>
  );
}
