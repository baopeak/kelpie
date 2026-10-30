// The reply box: a native textarea (so the phone's own keyboard, input methods and dictation work
// untouched), Enter that sends but never mid-composition, Shift+Enter for a new line, history, and a
// confirmation before a long paste. A draft is kept per session on the phone, also while offline: nothing
// is queued, and nothing is sent unless the connection is up.
//
// A chat has its own box (`ChatComposer`): a message to an agent is prose, often long and in a language that is
// typed through an input method, so Enter is a new line and the Send button sends (Ctrl/Cmd+Enter from a hardware
// keyboard).
import { createEffect, createSignal, For, on, Show } from "solid-js";
import { checkSend, EnterGuard, isLongPaste, looksLikePasswordPrompt, MAX_SEND_CHARS, SendLatch, unsentRest } from "../../src/lib/composer-core";
import { ctrlArmed, ctrlKey, setCtrlArmed } from "./key-bar";
import { cancelQueued, chat, chatPending, connected, discardChat, resumeChat, retryChat, screen, sendChat, sendKey, sendText, stopChat, stopping, takeChat, setStopping } from "./phone-state";
import { chipView, type PendingSend, type SendResult as ChatSendResult } from "./chat-send";
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

/** What a refused message is told as. The daemon's own words are English; these are the phone's. */
export function refusalText(code: string | undefined, message?: string): string {
  switch (code) {
    case "rate_limited": return t("Sending too fast: wait a moment, then send again.");
    case "queue_full": return t("Too many messages are waiting for this chat. Wait for some to go out.");
    case "too_long": return t("That is too long to send in one go.");
    case "exited": return t("Open this chat on the desktop to resume it.");
    case "switching": return t("This chat is being moved to a terminal.");
    case "command_needs_desktop": return t("This command runs only on the computer. Use Kelpie there.");
    case "command": return t("This command runs only on the computer. Use Kelpie there.");
    case "tui": return t("That command works only in the agent's terminal UI.");
    case "empty": return t("There is nothing to send.");
    case "epoch": return t("Kelpie restarted since this was opened. Your message was not sent.");
    case "read_only": return t("This device can only read.");
    case "cancelled": return t("That message was cancelled before it was sent.");
    case "dropped": return t("That message was dropped before it was sent.");
    default: return message || t("Could not send.");
  }
}

/** The line a message of ours on its way shows under the conversation. */
function stateText(p: PendingSend): string {
  switch (p.state) {
    case "sending": return t("Sending…");
    case "queued": return p.ahead ? t("Queued ({n} ahead)", { n: p.ahead }) : t("Queued");
    case "sent": return t("Sent");
    case "unknown": return t("Not sure it was sent");
    case "lost": return t("Kelpie restarted: this was not sent");
    case "cancelled": return p.cancelledOn === "desktop" ? t("Cancelled on desktop") : t("Cancelled on phone");
  }
}

const chatDraftKey = (id: string) => `kelpie-draft:chat:${id}`;

/** This phone's messages on their way that are not in the queue (yet, or any more): their state, and what can be done with each.
 *  In a running chat (`putBack` given) the text can go back into the box and a message in doubt can be sent again. Outside one
 *  (the chat stopped, the computer restarted) there is no box: the text stays on screen to copy, until the person lets it go. */
export function PendingChips(props: { putBack?: (text: string) => void; setNotice: (message: string | null) => void; busy?: boolean }) {
  const onTheWay = () => chatPending().filter((p) => p.state !== "queued");
  const retry = async (p: PendingSend) => {
    const r = await retryChat(p.cid);
    // A refusal took the message off the list: its text goes back into the box, with the reason.
    if (r && !r.accepted && r.reason === "refused") {
      props.putBack?.(r.text);
      props.setNotice(refusalText(r.code, r.message));
    }
  };
  const putBack = async (p: PendingSend) => {
    const text = await takeChat(p.cid);
    if (text === null) props.setNotice(t("That message already went out."));
    else props.putBack?.(text);
  };
  const copy = async (p: PendingSend) => {
    try {
      await navigator.clipboard.writeText(p.text);
      props.setNotice(t("Copied"));
    } catch {
      props.setNotice(t("Could not copy: select the text and copy it."));
    }
  };
  return (
    <For each={onTheWay()}>
      {(p) => {
        // A message is a new object whenever it changes (chat-send.ts): this row is drawn again for each state.
        const v = chipView(p);
        return (
          <div class="chip" classList={{ warn: v.warn }} role="status">
            <span class="chip-text" title={p.text}>{p.text}</span>
            <span class="chip-meta">{stateText(p)}</span>
            <Show when={v.retry && props.putBack}>
              <button class="link-btn" disabled={!connected() || props.busy} onClick={() => void retry(p)}>{t("Retry")}</button>
            </Show>
            <Show when={v.putBack && props.putBack}>
              <button class="link-btn" onClick={() => void putBack(p)}>{t("Put back")}</button>
            </Show>
            <Show when={v.warn && (!props.putBack || v.unsaved)}>
              <button class="link-btn" onClick={() => void copy(p)}>{t("Copy")}</button>
            </Show>
            <Show when={v.discard}>
              <button class="link-btn" onClick={() => discardChat(p.cid)}>{t("Discard")}</button>
            </Show>
            {/* The browser would not keep a copy (storage full, or too many waiting for a decision): only this page has it. */}
            <Show when={v.unsaved}>
              <div class="chip-note" role="alert" data-unsaved>{t("This phone could not save a copy: copy the text now, or it is lost if the page reloads.")}</div>
            </Show>
          </div>
        );
      }}
    </For>
  );
}

/** The reply box of a chat: messages to the agent, the queue they wait in (everyone's, with who sent each), Stop and Resume. */
export function ChatComposer(props: { id: string; alive: boolean }) {
  let box!: HTMLTextAreaElement;
  const guard = new EnterGuard();
  const [sending, setSending] = createSignal(false);
  const [notice, setNotice] = createSignal<string | null>(null);

  createEffect(on(() => props.id, (id) => {
    box.value = store.get(chatDraftKey(id)) ?? "";
    grow();
  }));
  // The agent stopped: no more "Stopping…".
  createEffect(() => {
    if (!chat()?.state.busy) setStopping(false);
  });

  const grow = () => {
    box.style.height = "auto";
    // About six lines.
    box.style.height = `${Math.min(box.scrollHeight, 150)}px`;
  };
  const saveDraft = () => {
    if (box.value) store.set(chatDraftKey(props.id), box.value);
    else store.del(chatDraftKey(props.id));
  };
  /** Puts a message's text back in the box, ahead of what is there. */
  const putBack = (text: string) => {
    box.value = box.value.trim() ? `${text}\n${box.value}` : text;
    saveDraft();
    grow();
    box.focus();
  };

  const submit = async () => {
    // Tapping again, or Enter from a keyboard, while this one is on its way does nothing.
    if (sending()) return;
    const text = box.value;
    setNotice(null);
    setSending(true);
    let r: ChatSendResult;
    try {
      r = await sendChat(text);
    } finally {
      setSending(false);
    }
    if (r.accepted) {
      // What was typed while it was on its way stays; the text sent is not cleared if the person changed it.
      box.value = unsentRest(box.value, text);
      saveDraft();
      grow();
    } else if (r.reason === "refused") {
      setNotice(refusalText(r.code, r.message));
    } else if (r.reason === "offline") {
      setNotice(t("Offline: your draft is kept. Send it when the connection is back."));
    } else if (r.reason !== "empty" && r.reason !== "busy") {
      setNotice(refusalText(r.reason));
    }
  };

  const onKeyDown = (e: KeyboardEvent) => {
    // Enter is a new line. Ctrl/Cmd+Enter sends, but never as a composition is being confirmed.
    if ((e.ctrlKey || e.metaKey) && guard.sends(e)) {
      e.preventDefault();
      void submit();
    }
  };

  const edit = async (p: { cid: string }) => {
    const text = await takeChat(p.cid);
    if (text === null) setNotice(t("That message already went out."));
    else putBack(text);
  };
  const remove = async (qid: string) => {
    await cancelQueued(qid);
  };
  /** Our messages that are on their way and not in the queue yet (or no longer sure to be). */
  const onTheWay = () => chatPending().filter((p) => p.state !== "queued");
  const pendingOf = (cid: string) => chatPending().find((p) => p.cid === cid);
  const queue = () => chat()?.state.queue ?? [];
  const busy = () => !!chat()?.state.busy;
  /** A desktop message that only the desktop window can send (a picture) holds the head: everything behind it waits for it. */
  const heldHead = () => (queue()[0]?.needsWindow ? queue()[0] : undefined);

  return (
    <div class="composer chat-composer">
      <Show when={queue().length || onTheWay().length}>
        <div class="chips">
          <For each={queue()}>
            {(m, i) => (
              <div class="chip" classList={{ held: i() > 0 && !!heldHead() && !!m.from }}>
                <span class="chip-text" title={m.text}>{m.text}</span>
                <span class="chip-meta">{m.from && !m.mine ? t("from {device}", { device: m.from }) : null} {m.needsWindow ? t("needs the desktop window") : m.mine ? (i() ? t("Queued ({n} ahead)", { n: i() }) : t("Queued")) : i() ? t("{n} ahead", { n: i() }) : null}</span>
                <Show when={m.mine && pendingOf(m.cid ?? "")}>
                  <button class="link-btn" disabled={!connected()} onClick={() => void edit({ cid: m.cid! })}>{t("Edit")}</button>
                </Show>
                <button class="icon" aria-label={t("Remove from the queue")} disabled={!connected()} onClick={() => void remove(m.id)}>✕</button>
                {/* A phone message waiting behind it says why, and Skip takes the held one out (strict order otherwise). */}
                <Show when={i() > 0 && m.from ? heldHead() : undefined}>
                  {(head) => (
                    <div class="chip-note" role="status">
                      <span>{t("A message only the computer can send is waiting for its chat to be open there")}</span>
                      <button class="link-btn" data-skip disabled={!connected()} onClick={() => void remove(head().id)}>{t("Skip")}</button>
                    </div>
                  )}
                </Show>
              </div>
            )}
          </For>
          <PendingChips putBack={putBack} setNotice={setNotice} busy={sending()} />
        </div>
      </Show>
      <Show when={chat()?.state.queuePaused && queue().length}>
        <div class="confirm" role="status">
          <span>{t("The queue is paused.")}</span>
          <button class="btn primary" disabled={!connected()} onClick={resumeChat}>{t("Resume")}</button>
        </div>
      </Show>
      <Show when={busy()}>
        <button class="btn danger stop" disabled={!connected() || stopping()} onClick={stopChat}>{stopping() ? t("Stopping…") : t("Stop")}</button>
      </Show>
      <div class="send">
        <div class="send-row">
          <textarea
            ref={box} class="field" rows="1" enterkeyhint="enter" autocapitalize="sentences" autocomplete="off" spellcheck={true} disabled={!props.alive}
            placeholder={connected() ? t("Message the agent…") : t("Offline: your draft is kept")}
            onKeyDown={onKeyDown} onCompositionStart={() => guard.compositionStart()} onCompositionEnd={() => guard.compositionEnd()}
            onInput={() => { setNotice(null); saveDraft(); grow(); }}
          />
          <button class="btn primary" disabled={sending() || !connected() || !props.alive} onClick={() => void submit()}>{t("Send")}</button>
        </div>
        <Show when={notice()}><div class="notice" role="status">{notice()}</div></Show>
      </div>
    </div>
  );
}
