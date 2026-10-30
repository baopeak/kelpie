// The reply box: a native textarea (so the phone's own keyboard, input methods and dictation work
// untouched), Enter that sends but never mid-composition, Shift+Enter for a new line, history, and a
// confirmation before a long paste. A draft is kept per session on the phone, also while offline: nothing
// is queued, and nothing is sent unless the connection is up.
import { createEffect, createSignal, on, Show } from "solid-js";
import { checkSend, EnterGuard, isLongPaste, looksLikePasswordPrompt, MAX_SEND_CHARS, SendLatch, unsentRest } from "../../src/lib/composer-core";
import { ctrlArmed, ctrlKey, setCtrlArmed } from "./key-bar";
import { connected, screen, sendKey, sendText } from "./phone-state";
import { history } from "./history";
import { store } from "./storage";
import { t } from "./i18n";

const draftKey = (id: string) => `kelpie-draft:${id}`;

export function Composer(props: { id: string; notice: string | null; setNotice: (message: string | null) => void }) {
  let box!: HTMLTextAreaElement;
  const guard = new EnterGuard();
  const latch = new SendLatch();
  const [sending, setSending] = createSignal(false);
  const [paste, setPaste] = createSignal<string | null>(null);
  const [force, setForce] = createSignal(false);

  // The draft of this session comes back where it was left.
  createEffect(on(() => props.id, (id) => {
    box.value = store.get(draftKey(id)) ?? "";
    grow();
  }));

  const grow = () => {
    box.style.height = "auto";
    box.style.height = `${Math.min(box.scrollHeight, 140)}px`;
  };
  const saveDraft = () => {
    if (box.value) store.set(draftKey(props.id), box.value);
    else store.del(draftKey(props.id));
  };

  const submit = async (anyway = false) => {
    if (latch.sending) return;
    const check = checkSend(box.value);
    if (!check.ok) {
      props.setNotice(check.reason === "too_long" ? t("That is too long to send in one go ({n} characters at most).", { n: MAX_SEND_CHARS }) : null);
      return;
    }
    if (!connected()) return props.setNotice(t("Offline: your draft is kept. Send it when the connection is back."));
    // On a password prompt the text is not remembered for ↑ either.
    const secret = looksLikePasswordPrompt(screen().rows);
    // Enter again, or the button, while this one is on its way: nothing.
    setSending(true);
    props.setNotice(null);
    const r = await latch.run(() => sendText(props.id, check.text, anyway));
    setSending(false);
    if (!r) return;
    if (r.ok) {
      if (!secret) history.sent(check.text);
      // What was typed while it was on its way stays.
      box.value = unsentRest(box.value, check.text);
      saveDraft();
      grow();
      setForce(false);
      // Someone else typed meanwhile, or a prompt came up: the text is on the prompt line, not sent.
      if (!r.entered) props.setNotice(t("Not sent: something else happened on the session first. The text is on the prompt line: press Enter to send it."));
    } else if (r.reason === "prompt") {
      setForce(true);
      props.setNotice(t("A prompt is waiting on this session: answer it, or send the text anyway."));
    } else if (r.reason === "offline") {
      props.setNotice(t("Offline: your draft is kept. Send it when the connection is back."));
    } else if (r.reason === "unknown") {
      props.setNotice(t("Not sure this was sent. Look at the screen before sending it again; your draft is kept."));
    } else {
      props.setNotice(r.message ?? t("Could not send."));
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if (guard.sends(e) && !e.shiftKey) {
      e.preventDefault();
      void submit(force());
      return;
    }
    if ((e.key === "ArrowUp" || e.key === "ArrowDown") && !e.isComposing) {
      const atEdge = e.key === "ArrowUp" ? box.selectionStart === 0 : box.selectionEnd === box.value.length;
      if (!atEdge && box.value) return;
      const next = history.step(e.key === "ArrowUp" ? 1 : -1, box.value);
      if (next !== null) {
        e.preventDefault();
        box.value = next;
        saveDraft();
        grow();
      }
    }
  };

  // Ctrl armed: the next letter typed goes as Ctrl+letter, not as text.
  const onBeforeInput = (e: InputEvent) => {
    // Armed, but this input is not a plain letter (an input method's composition): Ctrl is not left armed for later.
    if (ctrlArmed() && (e.isComposing || e.inputType !== "insertText")) setCtrlArmed(false);
    if (!ctrlArmed() || e.isComposing || e.inputType !== "insertText" || !e.data) return;
    const key = ctrlKey(e.data);
    setCtrlArmed(false);
    if (!key) return;
    e.preventDefault();
    void sendKey(props.id, key).then((r) => props.setNotice(r.ok ? null : t("Offline: the key was not sent.")));
  };

  const onPaste = (e: ClipboardEvent) => {
    const text = e.clipboardData?.getData("text") ?? "";
    if (isLongPaste(text)) {
      e.preventDefault();
      setPaste(text);
    }
  };
  const acceptPaste = () => {
    const text = paste();
    setPaste(null);
    if (text === null) return;
    box.setRangeText(text, box.selectionStart, box.selectionEnd, "end");
    saveDraft();
    grow();
    box.focus();
  };

  return (
    <div class="send">
      <Show when={paste() !== null}>
        <div class="confirm" role="alert">
          <span>{t("Paste {n} characters?", { n: paste()!.length })}</span>
          <button class="btn" onClick={() => setPaste(null)}>{t("Cancel")}</button>
          <button class="btn primary" onClick={acceptPaste}>{t("Paste")}</button>
        </div>
      </Show>
      <div class="send-row">
        <textarea
          ref={box} class="field" rows="1" enterkeyhint="send" autocapitalize="off" autocomplete="off" spellcheck={false}
          placeholder={connected() ? t("Type a reply…") : t("Offline: your draft is kept")}
          onKeyDown={onKeyDown} onBeforeInput={onBeforeInput} onPaste={onPaste}
          onCompositionStart={() => guard.compositionStart()} onCompositionEnd={() => guard.compositionEnd()}
          onInput={() => { history.edited(); props.setNotice(null); saveDraft(); grow(); }}
        />
        <button class="btn primary" disabled={sending() || !connected()} onClick={() => void submit(force())}>{force() ? t("Send anyway") : t("Send")}</button>
      </div>
      <Show when={props.notice}><div class="notice" role="status">{props.notice}</div></Show>
    </div>
  );
}
