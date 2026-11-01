// The question a session waits on, as buttons: one tap answers it. The keys come from the daemon's own
// prompt detection; what a key sends is the desktop's mapping (`keyAnswer`). Answers go out only while
// connected; one that may not have arrived is retried once by the link, and after that the person decides.
import { createSignal, For, onCleanup, Show } from "solid-js";
import { answeredBeforeText } from "../../src/chat/chat-model";
import { CommandBlock } from "./command-block";
import type { WaitInfo } from "./command-text";
import { lastingText, noticeFor, refusalText } from "./notice";
import { answerPrompt, connected, showNotice } from "./phone-state";
import type { Prompt } from "./screen-sync";
import { t } from "./i18n";

type Phase = "ready" | "sending" | "sent" | "stale" | "unknown" | "error";

export function PromptCard(props: { id: string; prompt: Prompt; waitingSince: number | null; canType: boolean; wait?: WaitInfo | null }) {
  const [phase, setPhase] = createSignal<Phase>("ready");
  // A long command is read to its end before anything can be chosen.
  const [seen, setSeen] = createSignal(true);
  const [text, setText] = createSignal("");
  const [lastKey, setLastKey] = createSignal("");
  // The next screen frame can remove this card before the reply to a late tap arrives. Then, and only then, the
  // outcome is told at the level of the session (while the card is here it says it itself).
  let gone = false;
  onCleanup(() => (gone = true));
  const pick = async (key: string) => {
    if (!props.canType || phase() === "sending") return;
    setLastKey(key);
    setPhase("sending");
    const o = await answerPrompt(props.id, props.waitingSince, props.prompt, key);
    const n = noticeFor(o, !gone);
    if (n?.kind === "stale") showNotice(props.id, answeredBeforeText(n.by, n.at));
    else if (n?.kind === "dup") showNotice(props.id, t("Already answered from this device."));
    if (o.t === "ack" || (o.t === "error" && o.result === "dup")) return setPhase("sent");
    if (o.t === "error" && o.result === "stale") {
      setText(answeredBeforeText(o.answered_by, o.at));
      return setPhase("stale");
    }
    if (o.t === "error") {
      setText(refusalText(o));
      return setPhase("error");
    }
    if (o.t === "offline") {
      setText(t("You are offline. Answer when the connection is back."));
      return setPhase("error");
    }
    setPhase("unknown");
  };
  return (
    <div class="prompt" data-prompt>
      <CommandBlock command={props.wait?.command} risk={props.wait?.risk} onSeen={setSeen} />
      <Show when={props.prompt.question}><div class="q">{props.prompt.question}</div></Show>
      <div class="actions">
        <For each={props.prompt.keys.filter((k) => !props.prompt.lasting?.includes(k))}>
          {(k) => (
            <button class="btn primary key-answer" disabled={!props.canType || !connected() || !seen() || phase() === "sending" || phase() === "sent"} onClick={() => pick(k)}>
              {k}
            </button>
          )}
        </For>
      </div>
      <Show when={props.prompt.lasting?.length}><div class="muted" data-lasting-hidden>{lastingText()}</div></Show>
      <Show when={phase() === "sending"}><div class="muted" role="status">{t("Sending…")}</div></Show>
      <Show when={phase() === "sent"}><div class="muted" role="status">{t("Sent")}</div></Show>
      <Show when={phase() === "stale"}><div class="muted stale" role="status">{text()}</div></Show>
      <Show when={phase() === "error"}><div class="muted err" role="status">{text()}</div></Show>
      <Show when={phase() === "unknown"}>
        <div class="muted err" role="status">
          {t("Not sure this went through. Check the screen, or try again: the computer ignores a repeat.")}
          <button class="btn" onClick={() => pick(lastKey())}>{t("Try again")}</button>
        </div>
      </Show>
      <Show when={!props.canType}><div class="muted">{t("This device can only read.")}</div></Show>
    </div>
  );
}
