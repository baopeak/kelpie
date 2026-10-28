// Answer a session's prompt without switching to it: the bottom of its screen, a button per key
// the prompt offers, Enter / Esc, and a line of free text. The answer names the prompt this
// popover shows (`waiting_since` read with the screen); the daemon types it only while the session
// still waits on that one, and the popover locks as soon as the prompt changes.
import { For, Show, createSignal, onCleanup, onMount } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import { t } from "../lib/i18n";
import { daemon } from "../lib/ipc";
import { answering, keyAnswer, setAnswering, waitedFor, type ScreenPrompt } from "../lib/attention";

const SHOWN_LINES = 12;
/** How often the shown screen is refreshed (and checked against the prompt it answers). */
const REFRESH_MS = 1000;

interface Shown {
  text: string;
  prompt?: ScreenPrompt | null;
  waiting_since?: number | null;
}

export default function QuickAnswer() {
  const session = () => {
    const id = answering();
    return id ? store.sessions[id] : undefined;
  };
  const [shown, setShown] = createSignal<Shown | null>(null);
  /** The prompt this popover answers: the wait it was opened on. */
  const [since, setSince] = createSignal<number | null>(null);
  const [changed, setChanged] = createSignal(false);
  const [text, setText] = createSignal("");
  const [error, setError] = createSignal<string | null>(null);

  const read = async (first: boolean) => {
    const s = session();
    if (!s) return;
    const r = await daemon<Shown>("session.read", { handle: s.handle }).catch(() => null);
    if (!r) return;
    if (first) {
      setShown(r);
      setSince(r.waiting_since ?? null);
    } else if ((r.waiting_since ?? null) !== since()) setChanged(true);
    else setShown(r);
  };
  onMount(() => {
    void read(true);
    const timer = setInterval(() => void read(false), REFRESH_MS);
    onCleanup(() => clearInterval(timer));
  });

  const close = () => {
    setAnswering(null);
    store.setOverlay(null);
  };
  const showNew = () => {
    setChanged(false);
    setError(null);
    void read(true);
  };
  const lines = () => (shown()?.text ?? "").split("\n").map((l) => l.trimEnd()).filter((l) => l).slice(-SHOWN_LINES).join("\n");
  const send = async (params: { data?: string; enter?: boolean; key?: "esc" | "enter" }) => {
    const s = session();
    if (!s || changed()) return;
    if (since() == null) {
      setError(t("This session no longer waits for you."));
      setChanged(true);
      return;
    }
    try {
      await daemon("session.answer", { handle: s.handle, waiting_since: since(), ...params });
      close();
    } catch (e) {
      setError(String(e));
      setChanged(true);
    }
  };
  return (
    <div class="overlay" onPointerDown={(e) => e.target === e.currentTarget && close()}>
      <div class="modal quick-answer" onKeyDown={(e) => e.key === "Escape" && close()}>
        <Show when={session()} fallback={<p>{t("This session no longer waits for you.")}</p>}>
          {(s) => (
            <>
              <h2>{sessionLabel(s())}</h2>
              <Show when={since()}>{(at) => <span class="muted-inline">{t("Waiting for {time}", { time: waitedFor(at()) })}</span>}</Show>
              <pre class="quick-answer-screen">{lines()}</pre>
              <Show when={changed()}>
                <div class="quick-answer-changed">
                  <span>{t("The prompt changed since this was shown.")}</span>
                  <button class="btn" onClick={showNew}>{t("Show the new prompt")}</button>
                </div>
              </Show>
              <Show when={shown()?.prompt}>
                {(p) => (
                  <div class="quick-answer-keys">
                    <For each={p().keys}>{(k) => <button class="btn primary" disabled={changed()} onClick={() => void send(keyAnswer(p(), k))}>{k}</button>}</For>
                  </div>
                )}
              </Show>
              <form class="quick-answer-text" onSubmit={(e) => { e.preventDefault(); if (text().trim()) void send({ data: text(), enter: true }); }}>
                <input class="text-input" disabled={changed()} placeholder={t("Type an answer and press Enter")} value={text()} onInput={(e) => setText(e.currentTarget.value)} ref={(el) => queueMicrotask(() => el.focus())} />
              </form>
              <Show when={error()}><p class="update-error">{error()}</p></Show>
              <div class="footer">
                <button class="btn" disabled={changed()} onClick={() => void send({ key: "esc" })}>Esc</button> {/* i18n-ignore: key name */}
                <button class="btn" disabled={changed()} onClick={() => void send({ key: "enter" })}>Enter</button> {/* i18n-ignore: key name */}
                <span style={{ flex: 1 }} />
                <button class="btn" onClick={() => { const id = s().id; close(); store.revealSession(id); }}>{t("Open session")}</button>
              </div>
            </>
          )}
        </Show>
      </div>
    </div>
  );
}
