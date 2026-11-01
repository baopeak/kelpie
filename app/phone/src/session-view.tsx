// One open session: a terminal's text with the prompt card, reply box and key bar; or a chat. What the
// person may do depends on the device's permission and on whether the connection is up.
import { createMemo, createSignal, Show } from "solid-js";
import { ChatView } from "./chat-view";
import { ago, clock } from "./conn-text";
import { Composer } from "./composer";
import { KeyBar } from "./key-bar";
import { PromptCard } from "./prompt-card";
import { TerminalText } from "./terminal-text";
import { canType, connected, dataState, dismissNotice, lastLiveAt, notice as sessionNotice, now, paintedFrom, screen, sessions } from "./phone-state";
import { promptKey } from "./screen-sync";
import { t } from "./i18n";

/** Says how old what is shown is: amber when the computer's updates arrive slowly, red when they stopped. */
function AgeChip() {
  return (
    <Show when={connected() && dataState() !== "fresh"}>
      <div class={`age ${dataState()}`} role="status">{dataState() === "slow" ? t("Updating slowly") : t("The computer is not responding")}</div>
    </Show>
  );
}

export function SessionView(props: { id: string }) {
  const row = createMemo(() => sessions().find((s) => s.id === props.id));
  const [notice, setNotice] = createSignal<string | null>(null);
  const waiting = () => !!row()?.alive && row()?.status === "waiting";
  const stale = () => !connected();

  return (
    <Show when={row()}>
      {(s) => (
        <Show
          when={!s().chat}
          fallback={<ChatView row={s()} dim={stale()} />}
        >
          <div class="session" data-id={props.id} classList={{ dim: stale() }}>
            <AgeChip />
            <Show when={stale() && (paintedFrom() ?? lastLiveAt())}>
              <div class="asof">{t("As of {time} ({ago})", { time: clock((paintedFrom() ?? lastLiveAt())!, true), ago: ago((paintedFrom() ?? lastLiveAt())!, now()) })}</div>
            </Show>
            <Show when={sessionNotice()?.session === props.id ? sessionNotice() : null}>
              {(n) => (
                <div class="notice-bar" role="status">
                  <span>{n().text}</span>
                  <button class="icon" aria-label={t("Dismiss")} onClick={dismissNotice}>×</button>
                </div>
              )}
            </Show>
            <Show when={waiting() && screen().prompt} fallback={<Show when={waiting()}><div class="needs">{t("This agent is waiting for you.")}</div></Show>}>
              {/* Keyed by the question: the next prompt gets a fresh card, not the last one's "Sent". */}
              <Show when={promptKey(screen().prompt!, screen().waitingSince ?? s().waiting_since)} keyed>
                {(_key) => <PromptCard id={props.id} prompt={screen().prompt!} waitingSince={screen().waitingSince ?? s().waiting_since} canType={canType()} wait={screen().wait} />}
              </Show>
            </Show>
            <TerminalText rows={screen().rows.length ? screen().rows : ["…"]} dim={stale()} />
            <Show
              when={canType() && s().alive}
              fallback={
                <div class="readonly">
                  {!s().alive ? t("This session has stopped.") : t("This device can only read. To type, allow it in Kelpie → Settings → Mobile, or use the Kelpie app or Tailscale/HTTPS.")}
                </div>
              }
            >
              <div class="composer">
                <KeyBar id={props.id} onResult={setNotice} />
                <Composer id={props.id} notice={notice()} setNotice={setNotice} />
              </div>
            </Show>
          </div>
        </Show>
      )}
    </Show>
  );
}
