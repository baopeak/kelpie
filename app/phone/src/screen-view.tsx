// The computer's screen on the phone: what to see (its screens, Kelpie and IDEs first, then windows), then the picture, which a finger drives
// as a trackpad or directly, a key bar with Ctrl, Shift and Alt, a field for text (sent as one message, said as sent or not sent), the
// quality line, and Stop. Video is decoded with WebCodecs and drawn with the crisp tiles and the cursor over it; the logic is screen.ts.
import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { avcChunk, canDecode, clampView, fitView, Gestures, KEYS, MODS, Overlay, parseMedia, qualityText, remoteText, sameSetup, setupFor, toClient, type DecoderSetup, type Frame, type Mod, type ScOp, type TargetRow, type View } from "./screen";
import { connected, onScreenMedia, remoteScreen, remoteScreens } from "./phone-state";
import { t } from "./i18n";

const KEY_LABELS: Record<(typeof KEYS)[number], string> = { Escape: "Esc", Tab: "⇥", Enter: "⏎", Backspace: "⌫", Delete: "Del", ArrowLeft: "←", ArrowUp: "↑", ArrowDown: "↓", ArrowRight: "→", Home: "Home", End: "End", PageUp: "PgUp", PageDown: "PgDn", F5: "F5", ContextMenu: "☰" };
const MOD_LABELS: Record<Mod, string> = { ctrl: "Ctrl", shift: "Shift", alt: "Alt" };

function Group(props: { title: string; rows: TargetRow[] }) {
  const s = () => remoteScreen()!;
  return (
    <Show when={props.rows.length}>
      <h3 class="muted">{props.title}</h3>
      <For each={props.rows}>
        {(r) => (
          <div class="row takeover-row" data-target={r.id}>
            <div class="takeover-row-main">
              <div>{r.title}</div>
              <div class="muted">{r.kind === "monitor" ? `${r.w}×${r.h}${r.primary ? ` · ${t("main screen")}` : ""}` : r.app}{r.minimized ? ` · ${t("minimized")}` : ""}</div>
            </div>
            <button class="btn primary" disabled={s().phase !== "pick" || !connected()} onClick={() => void remoteScreens.pick(r.id)}>{t("See it")}</button>
          </div>
        )}
      </For>
    </Show>
  );
}

function Picker() {
  const s = () => remoteScreen()!;
  const rows = (pred: (r: TargetRow) => boolean) => (s().targets ?? []).filter(pred);
  return (
    <div class="card takeover-pick">
      <h2>{t("See the computer's screen")}</h2>
      <p class="muted">{t("Pick a screen or one window. The computer shows a border around what you see and can stop it at any time; Windows security prompts and the lock screen cannot be seen or answered from here.")}</p>
      <Show when={s().message}><p class="banner bad" role="status">{s().message}</p></Show>
      <Show when={s().targets} fallback={<p class="muted">{t("Asking the computer…")}</p>}>
        <Group title={t("Screens")} rows={rows((r) => r.kind === "monitor")} />
        <Group title={t("Kelpie and code editors")} rows={rows((r) => r.kind === "window" && !!r.shortcut)} />
        <Group title={t("Windows")} rows={rows((r) => r.kind === "window" && !r.shortcut)} />
      </Show>
    </div>
  );
}

/** A small cursor drawn where the computer's is: an arrow, or an I-beam over text. */
function drawCursor(ctx: CanvasRenderingContext2D, x: number, y: number, shape: number, dpr: number) {
  ctx.save();
  ctx.translate(x, y);
  ctx.scale(dpr, dpr);
  ctx.lineWidth = 1.5;
  ctx.strokeStyle = "#000";
  ctx.fillStyle = "#fff";
  ctx.beginPath();
  if (shape === 1) {
    ctx.rect(-1.5, -9, 3, 18);
  } else {
    ctx.moveTo(0, 0);
    ctx.lineTo(0, 16);
    ctx.lineTo(4.5, 12);
    ctx.lineTo(8, 19);
    ctx.lineTo(10.5, 18);
    ctx.lineTo(7, 11);
    ctx.lineTo(12, 11);
    ctx.closePath();
  }
  ctx.fill();
  ctx.stroke();
  ctx.restore();
}

function Live() {
  const s = () => remoteScreen()!;
  let box!: HTMLDivElement;
  let canvas!: HTMLCanvasElement;
  const gestures = new Gestures();
  const [mode, setMode] = createSignal<"trackpad" | "direct">("trackpad");
  const [mods, setMods] = createSignal<Mod[]>([]);
  const [text, setText] = createSignal("");
  const [debug, setDebug] = createSignal(false);
  const [view, setView] = createSignal<View>({ zoom: 1, cx: 0, cy: 0 });
  const [queue, setQueue] = createSignal(0);
  const live = () => s().phase === "live" && connected();

  let decoder: VideoDecoder | null = null;
  let setup: DecoderSetup | null = null;
  let latest: VideoFrame | null = null;
  let skipToKey = true;
  let src = { w: 0, h: 0 };
  let cursor = { shape: 0, visible: false, x: 0, y: 0 };
  const pending = new Map<number, { unpaint: { x: number; y: number; w: number; h: number }[]; unpaintAll: boolean; srcW: number; srcH: number }>();
  const overlay = new Overlay<ImageBitmap>((b) => b.close());
  let shownSeq = 0;
  let drawQueued = false;

  const frame = (): Frame => {
    const r = box.getBoundingClientRect();
    return { el: { left: r.left, top: r.top, width: r.width, height: r.height }, srcW: src.w, srcH: src.h, view: view() };
  };

  const draw = () => {
    drawQueued = false;
    const ctx = canvas.getContext("2d");
    if (!ctx || !src.w) return;
    const dpr = window.devicePixelRatio || 1;
    const r = box.getBoundingClientRect();
    if (canvas.width !== Math.round(r.width * dpr) || canvas.height !== Math.round(r.height * dpr)) {
      canvas.width = Math.round(r.width * dpr);
      canvas.height = Math.round(r.height * dpr);
    }
    const el = { left: 0, top: 0, width: canvas.width, height: canvas.height };
    const v = view();
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, canvas.width, canvas.height);
    const at = toClient(0, 0, el, src.w, src.h, v);
    const far = toClient(src.w, src.h, el, src.w, src.h, v);
    ctx.imageSmoothingQuality = "high";
    if (latest) ctx.drawImage(latest, at.x, at.y, far.x - at.x, far.y - at.y);
    for (const tile of overlay.drawable(shownSeq)) {
      const a = toClient(tile.rect.x, tile.rect.y, el, src.w, src.h, v);
      const b = toClient(tile.rect.x + tile.rect.w, tile.rect.y + tile.rect.h, el, src.w, src.h, v);
      ctx.drawImage(tile.image, a.x, a.y, b.x - a.x, b.y - a.y);
    }
    if (cursor.visible) {
      const c = toClient(cursor.x, cursor.y, el, src.w, src.h, v);
      drawCursor(ctx, c.x, c.y, cursor.shape, dpr);
    }
  };
  const redraw = () => {
    if (!drawQueued) {
      drawQueued = true;
      requestAnimationFrame(draw);
    }
  };

  const configure = (next: DecoderSetup) => {
    decoder?.close();
    latest?.close();
    latest = null;
    setup = next;
    decoder = new VideoDecoder({
      output: (f) => {
        const seq = f.timestamp;
        latest?.close();
        latest = f;
        const p = pending.get(seq);
        pending.delete(seq);
        if (p) {
          overlay.onFrame(p);
          if (p.srcW !== src.w || p.srcH !== src.h) {
            src = { w: p.srcW, h: p.srcH };
            setView(fitView(src.w, src.h));
          }
        }
        shownSeq = seq;
        setQueue(decoder?.decodeQueueSize ?? 0);
        remoteScreens.decoded(seq);
        redraw();
      },
      error: () => {
        // A decoder that failed is made again from the next keyframe.
        setup = null;
        skipToKey = true;
        remoteScreens.needKeyframe();
      },
    });
    decoder.configure({ codec: next.codec, description: next.description, codedWidth: next.codedWidth, codedHeight: next.codedHeight, optimizeForLatency: true, hardwareAcceleration: "no-preference" });
  };

  const onMedia = (bytes: Uint8Array) => {
    const m = parseMedia(bytes);
    if (!m) return;
    if (m.kind === "cursor") {
      cursor = { shape: m.shape, visible: m.visible, x: m.x, y: m.y };
      if (mode() === "direct" || !gestures.cursor.x) gestures.cursor = { x: m.x, y: m.y };
      return redraw();
    }
    if (m.kind === "tile") {
      void createImageBitmap(new Blob([m.image.slice()], { type: m.format === "png" ? "image/png" : "image/jpeg" })).then((image) => {
        overlay.add({ id: m.id, seq: m.seq, rect: m.rect, image });
        redraw();
      }, () => {});
      return;
    }
    if (m.key) {
      const next = setupFor(m.au, m.codedW, m.codedH);
      if (next && (!sameSetup(next, setup) || !decoder || decoder.state !== "configured")) configure(next);
      skipToKey = false;
    }
    if (!decoder || decoder.state !== "configured" || skipToKey) {
      remoteScreens.needKeyframe();
      return;
    }
    // Far behind: frames after a dropped one cannot be decoded, so everything waits for a keyframe.
    if (decoder.decodeQueueSize > 3 && !m.key) {
      skipToKey = true;
      remoteScreens.needKeyframe();
      return;
    }
    pending.set(m.seq, { unpaint: m.unpaint, unpaintAll: m.unpaintAll, srcW: m.srcW, srcH: m.srcH });
    if (pending.size > 30) pending.delete(pending.keys().next().value!);
    decoder.decode(new EncodedVideoChunk({ type: m.key ? "key" : "delta", timestamp: m.seq, data: avcChunk(m.au) }));
  };

  onMount(() => {
    onScreenMedia(onMedia);
    const resize = () => redraw();
    window.addEventListener("resize", resize);
    onCleanup(() => window.removeEventListener("resize", resize));
  });
  onCleanup(() => {
    onScreenMedia(null);
    decoder?.close();
    latest?.close();
    overlay.clear();
    gestures.cancel();
  });

  const send = (ops: ScOp[]) => {
    if (ops.length && live()) remoteScreens.input(ops);
  };
  const out = (o: { ops: ScOp[]; view?: View }) => {
    if (o.view) {
      setView(o.view);
      redraw();
    }
    send(o.ops);
    if (o.ops.some((op) => op.k === "move")) {
      cursor = { ...cursor, x: gestures.cursor.x, y: gestures.cursor.y };
      redraw();
    }
  };
  const touch = (e: PointerEvent) => ({ id: e.pointerId, x: e.clientX, y: e.clientY });
  const down = (e: PointerEvent) => {
    if (!live() || !src.w) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    gestures.mode = mode();
    out(gestures.down(touch(e), e.timeStamp, frame()));
  };
  const move = (e: PointerEvent) => out(gestures.move(touch(e), e.timeStamp, frame()));
  const up = (e: PointerEvent) => out(gestures.up(touch(e), e.timeStamp, frame()));
  const cancel = () => out(gestures.cancel());

  const key = (k: string) => {
    send([{ k: "key", key: k, mods: mods() }]);
    setMods([]);
  };
  const toggleMod = (m: Mod) => setMods((all) => (all.includes(m) ? all.filter((x) => x !== m) : [...all, m]));
  const sendText = async () => {
    const value = text();
    // With a modifier held, one letter or digit is a shortcut (Ctrl+S), not text.
    if (mods().length && /^[A-Za-z0-9]$/.test(value)) {
      key(value.toUpperCase());
      setText("");
      return;
    }
    if (await remoteScreens.type(value)) setText("");
  };
  const resetView = () => {
    setView(clampView(frame().el, src.w, src.h, fitView(src.w, src.h)));
    redraw();
  };

  return (
    <div class="takeover screen-live" classList={{ dim: !live() }} data-phase={s().phase}>
      <div class="takeover-head">
        <span class="takeover-title">{s().title}</span>
        <button class="btn" data-screen-mode onClick={() => setMode(mode() === "trackpad" ? "direct" : "trackpad")}>{mode() === "trackpad" ? t("Trackpad") : t("Direct touch")}</button>
        <Show when={view().zoom > 1}><button class="btn" onClick={resetView}>{t("Fit")}</button></Show>
        <button class="btn danger" data-screen-stop onClick={() => void remoteScreens.stop()}>{t("Stop")}</button>
      </div>
      <div class="takeover-screen screen-canvas" ref={box} style={{ "touch-action": "none" }} onPointerDown={down} onPointerMove={move} onPointerUp={up} onPointerCancel={cancel}>
        <canvas ref={canvas} aria-label={t("The computer's screen")} />
        <Show when={remoteText(s().remote, t)}>{(line) => <div class="takeover-lost" role="status" data-remote-state={s().remote}>{line()}</div>}</Show>
        <Show when={s().phase === "frozen" || !connected()}><div class="takeover-lost" role="status">{t("Connection lost: nothing is sent until the picture is back.")}</div></Show>
        <Show when={debug()}>
          <pre class="screen-debug">{JSON.stringify({ ...s().stats, queue: queue(), shown: shownSeq, tiles: overlay.count, encoder: s().encoder })}</pre>
        </Show>
      </div>
      <p class="muted screen-quality" onClick={() => setDebug(!debug())}>{qualityText(s().stats, t)}<Show when={s().encoder === "sw"}> · {t("software encoder")}</Show></p>
      <div class="composer">
        <div class="keybar">
          <For each={MODS}>{(m) => <button class="btn" classList={{ primary: mods().includes(m) }} aria-pressed={mods().includes(m)} disabled={!live()} onClick={() => toggleMod(m)}>{MOD_LABELS[m]}</button>}</For>
          <For each={KEYS}>{(k) => <button class="btn" disabled={!live()} aria-label={k} onClick={() => key(k)}>{KEY_LABELS[k]}</button>}</For>
        </div>
        <form class="takeover-text" onSubmit={(e) => { e.preventDefault(); void sendText(); }}>
          <input class="field" type="password" autocomplete="off" autocapitalize="off" spellcheck={false} placeholder={t("Text to type on the computer")} value={text()} onInput={(e) => setText(e.currentTarget.value)} disabled={!live()} />
          <button class="btn primary" disabled={!live() || !text() || s().text === "sending"}>{t("Type it")}</button>
        </form>
        <Show when={s().text}>
          {(r) => <p class="muted" role="status" data-text-result={r()}>{r() === "sending" ? t("Sending…") : r() === "sent" ? t("Typed on the computer.") : t("Not sent: check the screen, then type it again.")}</p>}
        </Show>
      </div>
    </div>
  );
}

/** The screen, from what to see to its end; or a plain word when this browser cannot show it. */
export function ScreenView() {
  const [supported, setSupported] = createSignal<boolean | null>(null);
  onMount(() => void canDecode(window as unknown as Parameters<typeof canDecode>[0]).then(setSupported));
  return (
    <Show when={remoteScreen()}>
      {(s) => (
        <Show when={supported() !== false} fallback={
          <div class="card" role="status" data-screen-unsupported>
            <h2>{t("This browser cannot show the screen")}</h2>
            <p class="muted">{t("Showing the computer's screen needs a browser that decodes H.264 video with WebCodecs: Safari on iOS 16.4 or later (iOS 26 works best), or Chrome on Android. Update the phone, then open Kelpie again.")}</p>
            <div class="actions"><button class="btn" onClick={() => remoteScreens.close()}>{t("Close")}</button></div>
          </div>
        }>
          <Show when={s().phase !== "pick"} fallback={<Picker />}>
            <Show when={s().phase !== "ended"} fallback={
              <div class="card" role="status" data-screen-ended>
                <h2>{t("Screen ended")}</h2>
                <p class="muted">{s().message}</p>
                <div class="actions"><button class="btn" onClick={() => remoteScreens.close()}>{t("Close")}</button></div>
              </div>
            }>
              <Live />
            </Show>
          </Show>
        </Show>
      )}
    </Show>
  );
}
