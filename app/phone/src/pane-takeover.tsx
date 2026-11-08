// Taking over a browser pane of the computer: the list of panes, then the pane's picture, which a finger drives (tap, scroll, a long press to
// drag), a field for text (a password goes as one message, said as sent or not sent), the keys a form needs, and Stop. The logic is takeover.ts.
import { createSignal, For, onCleanup, Show } from "solid-js";
import { drawnBox, Gesture, KEYS, toPage, type TkOp } from "./takeover";
import { connected, takeover, takeovers } from "./phone-state";
import { t } from "./i18n";

const KEY_LABELS: Record<(typeof KEYS)[number], string> = { Enter: "⏎", Tab: "⇥", Backspace: "⌫", Escape: "Esc", ArrowLeft: "←", ArrowRight: "→", ArrowUp: "↑", ArrowDown: "↓" };

function Picker() {
  const s = () => takeover()!;
  return (
    <div class="card takeover-pick">
      <h2>{t("Take over a browser pane")}</h2>
      <p class="muted">{t("Use it to sign in, pass a code or a check on the computer's page. The computer shows that this phone controls it, and either side can stop.")}</p>
      <Show when={s().message}><p class="banner bad" role="status">{s().message}</p></Show>
      <Show when={s().panes} fallback={<p class="muted">{t("Asking the computer…")}</p>}>
        <For each={s().panes!} fallback={<p class="muted">{t("No browser pane with a page is open in Kelpie on the computer.")}</p>}>
          {(p) => (
            <div class="row takeover-row" data-pane={p.pane}>
              <div class="takeover-row-main">
                <div>{p.title}</div>
                <div class="muted">{p.host} · {p.project_name}</div>
              </div>
              <button class="btn primary" disabled={s().phase !== "pick" || p.held || !connected()} onClick={() => void takeovers.pick(p.pane)}>{p.held ? t("Taken") : t("Take over")}</button>
            </div>
          )}
        </For>
      </Show>
    </div>
  );
}

function Live() {
  const s = () => takeover()!;
  let img!: HTMLImageElement;
  const gesture = new Gesture();
  const [text, setText] = createSignal("");
  const live = () => s().phase === "live" && connected();

  // A frame is acknowledged once the image is decoded and on screen: the computer sends the next one then. One that cannot be decoded is acknowledged
  // all the same, so the stream goes on, and the screen says the picture is behind.
  let shownSeq = 0;
  const settled = (ok: boolean) => {
    const seq = s().frame?.seq;
    if (!seq || seq === shownSeq) return;
    shownSeq = seq;
    if (ok) takeovers.shown(seq);
    else takeovers.failed(seq);
  };

  const at = (e: PointerEvent) => {
    const meta = s().frame?.meta;
    // No picture on the screen (the pane is hidden on the computer): no point on the page either.
    if (!meta || !img?.isConnected) return null;
    const r = img.getBoundingClientRect();
    return toPage(e.clientX, e.clientY, drawnBox({ left: r.left, top: r.top, width: r.width, height: r.height }, img.naturalWidth, img.naturalHeight), meta);
  };
  const send = (ops: TkOp[]) => live() && takeovers.input(ops);
  // Each finger by its pointer id: only the first one drives the page, and a second one (a pinch) gives the gesture up.
  const down = (e: PointerEvent) => {
    const p = at(e);
    if (!p || !live()) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    send(gesture.down(e.pointerId, p, e.timeStamp));
  };
  const move = (e: PointerEvent) => {
    const p = at(e);
    if (p) send(gesture.move(e.pointerId, p, e.timeStamp));
  };
  const up = (e: PointerEvent) => send(gesture.up(e.pointerId, at(e)));
  const cancel = (e: PointerEvent) => send(gesture.cancel(e.pointerId));

  const sendText = async () => {
    if (await takeovers.type(text())) setText("");
  };
  onCleanup(() => send(gesture.cancel(-1)));

  return (
    <div class="takeover" classList={{ dim: !live() }} data-phase={s().phase}>
      <div class="takeover-head">
        <span class="takeover-title">{s().title || s().host}<Show when={s().frame?.meta.window === "popup"}> · {t("sign-in window")}</Show></span>
        <button class="btn danger" data-takeover-stop onClick={() => void takeovers.stop()}>{t("Stop")}</button>
      </div>
      <div class="takeover-screen" onPointerDown={down} onPointerMove={move} onPointerUp={up} onPointerCancel={cancel}>
        {/* Hidden on the computer, the last picture is not shown: it would look live, and nothing sent would reach the page. */}
        <Show when={s().phase !== "hidden"} fallback={
          <div class="takeover-hidden" role="status" data-takeover-hidden>
            <p>{t("The pane is hidden on the computer.")}</p>
            <p class="muted">{t("Nothing is sent to it until it is shown there again.")}</p>
          </div>
        }>
          <Show when={s().frame} fallback={<p class="muted">{t("Waiting for the page…")}</p>}>
            {(f) => <img ref={img} src={`data:image/jpeg;base64,${f().data}`} alt={t("The computer's browser pane")} draggable={false} onLoad={() => settled(true)} onError={() => settled(false)} />}
          </Show>
        </Show>
        <Show when={s().phase === "frozen" || !connected()} fallback={<Show when={s().broken}><div class="takeover-lost" role="status" data-takeover-broken>{t("The last picture could not be shown: what you see may be behind the page.")}</div></Show>}>
          <div class="takeover-lost" role="status">{t("Connection lost: nothing is sent until the picture is back.")}</div>
        </Show>
      </div>
      <div class="composer">
        <div class="keybar">
          <For each={KEYS}>{(k) => <button class="btn" disabled={!live()} aria-label={k} onClick={() => send([{ k: "key", key: k }])}>{KEY_LABELS[k]}</button>}</For>
        </div>
        <form class="takeover-text" onSubmit={(e) => { e.preventDefault(); void sendText(); }}>
          <input class="field" type="password" autocomplete="off" autocapitalize="off" spellcheck={false} placeholder={t("Text to type into the page")} value={text()} onInput={(e) => setText(e.currentTarget.value)} disabled={!live()} />
          <button class="btn primary" disabled={!live() || !text() || s().text === "sending"}>{t("Type it")}</button>
        </form>
        <Show when={s().text}>
          {(r) => <p class="muted" role="status" data-text-result={r()}>{r() === "sending" ? t("Sending…") : r() === "sent" ? t("Sent to the page.") : t("Not sent: check the page, then type it again.")}</p>}
        </Show>
      </div>
    </div>
  );
}

/** The takeover screen, from the list of panes to its end. */
export function PaneTakeover() {
  return (
    <Show when={takeover()}>
      {(s) => (
        <Show when={s().phase !== "pick"} fallback={<Picker />}>
          <Show when={s().phase !== "ended"} fallback={
            <div class="card" role="status" data-takeover-ended>
              <h2>{t("Takeover ended")}</h2>
              <p class="muted">{s().message}</p>
              <div class="actions"><button class="btn" onClick={() => takeovers.close()}>{t("Close")}</button></div>
            </div>
          }>
            <Live />
          </Show>
        </Show>
      )}
    </Show>
  );
}
