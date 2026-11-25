// The computer's screen on the phone: what to see (its screens, Kelpie and IDEs first, then windows), then the picture, which a finger drives
// as a trackpad or directly, a key bar with Ctrl, Shift and Alt (⌘ ⌥ ⌃ ⇧ on a Mac), a field for text (sent as one message, said as sent or not sent), the
// quality line, and Stop. Video is decoded with WebCodecs and drawn with the crisp tiles and the cursor over it; the logic is screen.ts.
// Nothing waits to be shown: a decoded frame is drawn as it comes out of the decoder (no smoothing buffer), on a low-latency canvas where the
// browser offers one, and a frame the decoder is too far behind for is skipped. The cursor is a layer of its own over the picture: moving it
// never draws the picture again, so it follows the finger at the screen's own rate, at once where the finger puts it (local echo).
import { createSignal, For, onCleanup, onMount, Show } from "solid-js";
import { avcChunk, Backlog, canDecode, clampView, decoderWedged, fedSince, detailsText, echoAt, echoPoint, fitView, FrameTimes, followHostCursor, Gestures, keepInView, keyLabel, keyOp, keysFor, LONG_PRESS_MS, modLabel, modsFor, Overlay, parseMedia, pickerHint, qualityText, remoteText, sameSetup, setupFor, shortcutHint, toClient, visibleRect, type CursorEcho, type DecoderSetup, type Feel, type Frame, type GestureOut, type Mod, type ScOp, type TargetRow, type View } from "./screen";
import { holdPageStill } from "./screen-page-guard";
import { screenGesturesSeen, screenMouseBar } from "./prefs";
import { canSharpen, connected, livePathNow, onScreenMedia, remoteScreen, remoteScreens, route } from "./phone-state";
import { pathText } from "./conn-path";
import { needsProof, pathRows, type ViaId } from "./screen-paths";
import { HOSTED } from "./hosted";
import { msg, t } from "./i18n";


/** What a finger does in each mode, as the help sheet lists it: the gesture, then what the computer gets. */
const GESTURE_ROWS: Record<"trackpad" | "direct", [string, string][]> = {
  trackpad: [
    [msg("Slide one finger"), msg("Moves the cursor: slowly for a small target, quickly to cross the screen")],
    [msg("Tap"), msg("Clicks where the cursor is")],
    [msg("Tap twice"), msg("Double-clicks there")],
    [msg("Hold, then lift, or tap with two fingers"), msg("Right-clicks")],
    [msg("Hold until the ring shows, then move"), msg("Drags; lift the finger to drop")],
    [msg("Slide one finger, zoomed in"), msg("The cursor moves inside the picture shown; near an edge the view follows it")],
    [msg("Tap with the cursor out of sight"), msg("Brings the cursor back into view and clicks nothing; tap again to click")],
    [msg("Slide two fingers"), msg("Scrolls; zoomed in, moves this phone's view instead (Fit, then slide, to scroll)")],
    [msg("Pinch"), msg("Zooms this phone's view around the cursor; Fit shows all of it again")],
  ],
  direct: [
    [msg("Tap"), msg("Clicks under the finger")],
    [msg("Tap twice"), msg("Double-clicks there")],
    [msg("Hold, then lift, or tap with two fingers"), msg("Right-clicks")],
    [msg("Hold until the ring shows, then move"), msg("Drags, with a magnifier above the finger; lift it to drop")],
    [msg("Slide one finger"), msg("Scrolls; zoomed in, moves the cursor under the finger, and holding it near an edge moves the view")],
    [msg("Slide two fingers"), msg("Scrolls; zoomed in, moves this phone's view instead (Fit, then slide, to scroll)")],
    [msg("Pinch"), msg("Zooms this phone's view; Fit shows all of it again")],
  ],
};

/** A short buzz to confirm a click or a long press, where the browser can (Android; Safari on the iPhone cannot). */
function buzz(ms: number) {
  try {
    navigator.vibrate?.(ms);
  } catch {
    /* not allowed here: the ring on the picture says it */
  }
}
const BUZZ: Record<Feel, number> = { click: 8, double: 14, right: 20, drag: 25 };
const FEEL_TEXT: Record<Feel, string> = { click: "", double: msg("Double click"), right: msg("Right click"), drag: "" };
/** The magnifier's size (CSS pixels) and how much it enlarges. */
const LOUPE = 110;
const LOUPE_ZOOM = 2;

/** The ways the computer lists for the screen, as the rows of Connect through (none from a computer that offers no choice). */
const connectRows = () => pathRows(remoteScreen()?.paths, remoteScreen()?.relay, { ...remoteScreens.side(remoteScreen()?.p2pFailed ?? false), hosted: HOSTED }, t);
/** The way chosen can be used now (always, from a computer that offers no choice). */
const startable = () => !connectRows().length || !!connectRows().find((r) => r.id === remoteScreen()?.via)?.ok;

/** Connect through: Automatic, Tailscale and the relay, each with whether it can be used and why not. The choice is kept for this pairing. */
function ConnectThrough() {
  const s = () => remoteScreen()!;
  return (
    <Show when={connectRows().length}>
      <h3 class="muted">{t("Connect through")}</h3>
      <div class="connect-through" role="radiogroup" aria-label={t("Connect through")} data-connect-through>
        <For each={connectRows()}>
          {(r) => (
            <label class="row connect-row" classList={{ off: !r.ok }} data-via={r.id} data-ok={String(r.ok)}>
              <input type="radio" name="screen-via" checked={s().via === r.id} disabled={!r.ok || s().phase !== "pick"} onChange={() => remoteScreens.choosePath(r.id)} />
              <span class="takeover-row-main">
                <span>{r.label}<Show when={r.recommended}> <span class="chip" data-via-recommended>{t("Recommended")}</span></Show></span>
                <span class="muted">{r.ok ? t("Available") : r.why}</span>
              </span>
            </label>
          )}
        </For>
      </div>
    </Show>
  );
}

/** The way the picture takes while it is shown, switched without ending the screen: Automatic, peer to peer, or the link's own way. */
function SwitchWay() {
  const s = () => remoteScreen()!;
  const rows = () => connectRows().filter((r) => r.id === "auto" || r.id === "p2p" || r.id === (s().usedVia === "tailscale" || route() === "direct" ? "tailscale" : "relay"));
  return (
    <Show when={rows().some((r) => r.id === "p2p")}>
      <label class="muted screen-way" data-switch-way>
        {t("Connect through")}{" "}
        <select value={s().via} disabled={s().phase !== "live"} onChange={(e) => void remoteScreens.switchVia(e.currentTarget.value as ViaId)}>
          <For each={rows()}>{(r) => <option value={r.id} disabled={!r.ok}>{r.label}</option>}</For>
        </select>
      </label>
    </Show>
  );
}

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
              <div class="muted">{r.kind === "monitor" ? `${r.w}×${r.h}${r.primary ? ` · ${t("main screen")}` : ""}` : r.title === r.app ? "" : r.app}{r.minimized ? ` · ${t("minimized")}` : ""}</div>
            </div>
            <button class="btn primary" disabled={s().phase !== "pick" || !connected() || !startable()} onClick={() => void remoteScreens.pick(r.id)}>{t("See it")}</button>
          </div>
        )}
      </For>
    </Show>
  );
}

/** After a busy refusal: when the phone asks the computer again, counted down, and a way to stop asking. Mounted only while a try waits. */
function RetryLine(props: { at: number }) {
  const [now, setNow] = createSignal(Date.now());
  const tick = setInterval(() => setNow(Date.now()), 1000);
  onCleanup(() => clearInterval(tick));
  const left = () => Math.max(0, Math.ceil((props.at - now()) / 1000));
  return (
    <p class="muted" role="status" data-busy-retry>
      {t("Trying again in {n} s.", { n: left() })}{" "}
      <button class="btn" onClick={() => remoteScreens.cancelRetry()}>{t("Stop trying")}</button>
    </p>
  );
}

function Picker() {
  const s = () => remoteScreen()!;
  const rows = (pred: (r: TargetRow) => boolean) => (s().targets ?? []).filter(pred);
  return (
    <div class="card takeover-pick">
      <h2>{t("See the computer's screen")}</h2>
      <p class="muted">{pickerHint(s().os, t)}</p>
      <Show when={s().message}><p class="banner bad" role="status">{s().message}</p></Show>
      <Show when={s().retry}>{(r) => <RetryLine at={r().at} />}</Show>
      <Show when={s().targets} fallback={<p class="muted">{t("Asking the computer…")}</p>}>
        <ConnectThrough />
        <Show when={connectRows().length && needsProof(s().via, route())}>
          <p class="muted" data-face-id-note>{t("Starting through the relay asks for Face ID once for the screen you pick.")}</p>
        </Show>
        <Group title={t("Screens")} rows={rows((r) => r.kind === "monitor")} />
        <Group title={t("Kelpie and code editors")} rows={rows((r) => r.kind === "window" && !!r.shortcut)} />
        <Group title={t("Windows")} rows={rows((r) => r.kind === "window" && !r.shortcut)} />
      </Show>
    </div>
  );
}

/** A small cursor shown where the computer's is, its point at the centre (24, 24) of its 48×48 box (`.screen-cursor`): an arrow, or an I-beam
 *  over text. Vector, so it is crisp at any pixel ratio. */
function CursorShape(props: { shape: number }) {
  return (
    <Show when={props.shape === 1} fallback={
      <svg width="14" height="22" viewBox="-1 -1 14 22" style={{ left: "23px", top: "23px" }}>
        <path d="M0 0 L0 16 L4.5 12 L8 19 L10.5 18 L7 11 L12 11 Z" fill="#fff" stroke="#000" stroke-width="1.5" />
      </svg>
    }>
      <svg width="6" height="20" viewBox="-3 -10 6 20" style={{ left: "21px", top: "14px" }}>
        <rect x="-1.5" y="-9" width="3" height="18" fill="#fff" stroke="#000" stroke-width="1.5" />
      </svg>
    </Show>
  );
}

function Live() {
  const s = () => remoteScreen()!;
  let box!: HTMLDivElement;
  let canvas!: HTMLCanvasElement;
  let cursorEl!: HTMLDivElement;
  let loupeEl: HTMLCanvasElement | undefined;
  let textField!: HTMLInputElement;
  const gestures = new Gestures();
  const [mode, setMode] = createSignal<"trackpad" | "direct">("trackpad");
  const [mods, setMods] = createSignal<Mod[]>([]);
  const [text, setText] = createSignal("");
  const [debug, setDebug] = createSignal(false);
  const [view, setView] = createSignal<View>({ zoom: 1, cx: 0, cy: 0 });
  const [queue, setQueue] = createSignal(0);
  // What the fingers did, shown on the picture: rings where clicks went (gone after a moment), the long press held, a drag going on, the
  // direct mode's magnifier (CSS pixels from the picture box's corner).
  const [marks, setMarks] = createSignal<{ id: number; x: number; y: number; feel: Feel }[]>([]);
  const [hold, setHold] = createSignal<{ x: number; y: number } | null>(null);
  const [dragging, setDragging] = createSignal(false);
  const [loupe, setLoupe] = createSignal<{ x: number; y: number } | null>(null);
  const [help, setHelp] = createSignal(!screenGesturesSeen.get());
  // The on-screen mouse (under the picture, never over it), and whether its Hold keeps the left button down.
  const [mouseBar, setMouseBar] = createSignal(screenMouseBar.get());
  const [holding, setHolding] = createSignal(false);
  // The text field shows what is typed; hidden (a password) on request.
  const [hideText, setHideText] = createSignal(false);
  // The on-screen keyboard is up: the screen fits in what is left above it (the visual viewport), not under it.
  const [keyboard, setKeyboard] = createSignal<{ top: number; height: number } | null>(null);
  const live = () => s().phase === "live" && connected();

  let decoder: VideoDecoder | null = null;
  let setup: DecoderSetup | null = null;
  let latest: VideoFrame | null = null;
  let skipToKey = true;
  // For a wedged decoder (`decoderWedged`): the first frame given to it since its last picture, and when its last picture came out.
  let fedAt: number | null = null;
  // Wedged decoders made again in a row with no picture between (`unwedge`): from the second one on, a software decoder is tried (a
  // hardware one still waking keeps wedging), unless one failed here already.
  let wedges = 0;
  let softFailed = false;
  let soft = false;
  let outputAt: number | null = null;
  let src = { w: 0, h: 0 };
  let cursor = { shape: 0, visible: false, x: 0, y: 0 };
  const [cursorShape, setCursorShape] = createSignal(0);
  const pending = new Map<number, { unpaint: { x: number; y: number; w: number; h: number }[]; unpaintAll: boolean; srcW: number; srcH: number; captureMs: number }>();
  const overlay = new Overlay<ImageBitmap>((b) => b.close());
  let shownSeq = 0;
  let drawFrame = 0;
  /** The edge pan's next frame (direct touch, zoomed in), 0 when none is asked for. */
  let panFrame = 0;
  let ctx2d: CanvasRenderingContext2D | null = null;
  // Where and when the phone last put the cursor, and the places it sent before: the computer's cursor messages (a round trip old, or
  // later) never pull it back to one of those (`reconcileCursor`).
  let echo: CursorEcho | null = null;
  const times = new FrameTimes();
  const backlog = new Backlog();
  // For the measurement plan: what the phone saw, read from the page (a debugging console or a test driver).
  (globalThis as { __kelpieScreenTimes?: () => unknown }).__kelpieScreenTimes = () => ({ ...times.snapshot(), desynchronized: lowLatency() });
  // And how the picture is shown now (zoom, and the source point at the box's centre): whether a gesture moved the view or only the cursor.
  (globalThis as { __kelpieScreenView?: () => View }).__kelpieScreenView = () => ({ ...view() });
  // Asked for once, before any other drawing: a canvas the browser may show without waiting for the next composited frame.
  const context = () => (ctx2d ??= canvas.getContext("2d", { desynchronized: true, alpha: false }));
  const lowLatency = () => !!(context()?.getContextAttributes?.() as { desynchronized?: boolean } | undefined)?.desynchronized;

  const frame = (): Frame => {
    const r = box.getBoundingClientRect();
    return { el: { left: r.left, top: r.top, width: r.width, height: r.height }, srcW: src.w, srcH: src.h, view: view() };
  };

  /** Moves the cursor's layer to where the cursor is in the picture as shown now (zoom and pan included); nothing else is drawn. */
  const placeCursor = () => {
    if (!cursor.visible || !src.w) {
      cursorEl.style.display = "none";
      return;
    }
    const r = box.getBoundingClientRect();
    const c = toClient(cursor.x, cursor.y, { left: 0, top: 0, width: r.width, height: r.height }, src.w, src.h, view());
    cursorEl.style.transform = `translate3d(${c.x}px, ${c.y}px, 0)`;
    cursorEl.style.display = "";
    setCursorShape(cursor.shape);
  };

  /** The picture and its crisp tiles, drawn again whole: for a new picture or tile, a new view or size (never for the cursor). */
  const draw = () => {
    if (drawFrame) cancelAnimationFrame(drawFrame);
    drawFrame = 0;
    const ctx = context();
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
    // The view or the picture's size may have changed: the cursor goes with the picture.
    placeCursor();
    drawLoupe();
  };
  /** One whole drawing at the next animation frame, however many things asked for it (a picture drawn meanwhile takes it). */
  const redraw = () => {
    drawFrame ||= requestAnimationFrame(draw);
  };

  const configure = (next: DecoderSetup) => {
    decoder?.close();
    latest?.close();
    latest = null;
    setup = next;
    fedAt = outputAt = null;
    soft = wedges >= 2 && !softFailed;
    decoder = new VideoDecoder({
      output: (f) => {
        const seq = f.timestamp;
        const out = performance.now();
        outputAt = out;
        wedges = 0;
        latest?.close();
        latest = f;
        const p = pending.get(seq);
        pending.delete(seq);
        if (p) {
          remoteScreens.presented(p.captureMs);
          overlay.onFrame({ seq, ...p });
          if (p.srcW !== src.w || p.srcH !== src.h) {
            src = { w: p.srcW, h: p.srcH };
            setView(fitView(src.w, src.h));
          }
        }
        shownSeq = seq;
        setQueue(decoder?.decodeQueueSize ?? 0);
        remoteScreens.decoded(seq);
        // Drawn now, not at the next animation frame: nothing holds a decoded picture back.
        draw();
        times.presented(seq, out, performance.now());
      },
      error: () => {
        if (soft) softFailed = true;
        remake();
      },
    });
    decoder.configure({ codec: next.codec, description: next.description, codedWidth: next.codedWidth, codedHeight: next.codedHeight, optimizeForLatency: true, hardwareAcceleration: soft ? "prefer-software" : "no-preference" });
  };

  /** A decoder that failed, or wedged, is made again from the next keyframe. The frames it held are acknowledged, so the computer never
   *  waits for them (it stops sending while they are unanswered), and their unpaints still apply. */
  const remake = () => {
    setup = null;
    skipToKey = true;
    fedAt = outputAt = null;
    let last = 0;
    for (const [seq, p] of pending) {
      overlay.onFrame({ seq, ...p });
      last = Math.max(last, seq);
    }
    pending.clear();
    if (last) remoteScreens.skipped(last);
    else remoteScreens.needKeyframe();
    redraw();
  };
  /** A wedged decoder (`decoderWedged`) never fails on its own: it is closed and made again as a failed one is. */
  const unwedge = () => {
    if (!decoder || decoder.state !== "configured" || !decoderWedged(fedAt, outputAt, performance.now())) return;
    decoder.close();
    decoder = null;
    wedges++;
    remake();
  };

  const onMedia = (bytes: Uint8Array) => {
    const m = parseMedia(bytes);
    if (!m) return;
    if (m.kind === "cursor") {
      // With no finger down, a tap clicks where the cursor is drawn (the computer's place once the echo is over), never elsewhere.
      const followed = followHostCursor(cursor, gestures.cursor, { shape: m.shape, visible: m.visible, x: m.x, y: m.y }, performance.now(), echo, gestures.idle());
      cursor = followed.drawn;
      echo = followed.echo;
      gestures.cursor = followed.tapAt;
      return placeCursor();
    }
    if (m.kind === "tile") {
      void createImageBitmap(new Blob([m.image.slice()], { type: m.format === "png" ? "image/png" : "image/jpeg" })).then((image) => {
        overlay.add({ id: m.id, seq: m.seq, rect: m.rect, image });
        redraw();
      }, () => {});
      return;
    }
    times.arrival(m.seq, performance.now());
    if (m.key) {
      const next = setupFor(m.au, m.codedW, m.codedH);
      if (next && (!sameSetup(next, setup) || !decoder || decoder.state !== "configured")) configure(next);
      skipToKey = false;
    }
    // A frame that is not decoded is still acknowledged (the computer must not wait for it), and its unpaint still applies, so that no crisp tile
    // stays over what it changed.
    const skip = () => {
      times.skipped(m.seq);
      overlay.onFrame({ seq: m.seq, unpaint: m.unpaint, unpaintAll: m.unpaintAll, srcW: m.srcW, srcH: m.srcH });
      remoteScreens.skipped(m.seq);
      redraw();
    };
    if (!decoder || decoder.state !== "configured" || skipToKey) return skip();
    // Far behind for this decoder (`Backlog`): a newer picture is better than a late one, and frames after a dropped one cannot be decoded,
    // so everything waits for a keyframe.
    if (m.key) backlog.reset();
    else if (backlog.skip(decoder.decodeQueueSize, performance.now())) {
      skipToKey = true;
      times.backlogSkips++;
      return skip();
    }
    pending.set(m.seq, { unpaint: m.unpaint, unpaintAll: m.unpaintAll, srcW: m.srcW, srcH: m.srcH, captureMs: m.captureMs });
    if (pending.size > 30) pending.delete(pending.keys().next().value!);
    times.decoding(m.seq, performance.now());
    fedAt = fedSince(fedAt, outputAt, performance.now());
    decoder.decode(new EncodedVideoChunk({ type: m.key ? "key" : "delta", timestamp: m.seq, data: avcChunk(m.au) }));
  };

  onMount(() => {
    onScreenMedia(onMedia);
    // The box changes size with the window, the keyboard and the lines around it (a status, the way's choice): the picture and the cursor
    // follow, and a zoomed view keeps a cursor that was in it in it (where the person is typing, near enough). A view moved away from the
    // cursor on purpose stays where it is: the lines under the picture change height as their figures change.
    let before: Frame["el"] | null = null;
    const sized = new ResizeObserver(() => {
      const now = frame();
      if (src.w && cursor.visible && before) {
        const c = toClient(cursor.x, cursor.y, before, src.w, src.h, view());
        const wasShown = c.x >= before.left && c.x <= before.left + before.width && c.y >= before.top && c.y <= before.top + before.height;
        if (wasShown) setView((v) => keepInView(now.el, src.w, src.h, v, cursor));
      }
      before = now.el;
      redraw();
    });
    sized.observe(box);
    onCleanup(() => sized.disconnect());
    // A wedged decoder is looked for on a timer: one that never answers leaves frames unacknowledged, and the computer stops sending.
    const wedgeCheck = setInterval(unwedge, 500);
    onCleanup(() => clearInterval(wedgeCheck));
    // Only the app's view zooms and pans: Safari's own pinch and the page's scroll are cancelled while the screen is open.
    onCleanup(holdPageStill(box, document, () => window.visualViewport?.scale ?? 1));
    // The keyboard: Safari shrinks the visual viewport and leaves the page's height alone, so the screen is fitted to the visual viewport
    // while the text field has the focus, and the picture keeps the part above the keyboard.
    const vv = window.visualViewport;
    if (vv) {
      const fit = () => setKeyboard(document.activeElement === textField && vv.height < document.documentElement.clientHeight - 80 ? { top: vv.offsetTop, height: vv.height } : null);
      vv.addEventListener("resize", fit);
      vv.addEventListener("scroll", fit);
      textField.addEventListener("focus", fit);
      textField.addEventListener("blur", fit);
      onCleanup(() => {
        vv.removeEventListener("resize", fit);
        vv.removeEventListener("scroll", fit);
      });
    }
  });
  onCleanup(() => {
    delete (globalThis as { __kelpieScreenTimes?: unknown }).__kelpieScreenTimes;
    delete (globalThis as { __kelpieScreenView?: unknown }).__kelpieScreenView;
    if (drawFrame) cancelAnimationFrame(drawFrame);
    drawFrame = 0;
    if (panFrame) cancelAnimationFrame(panFrame);
    panFrame = 0;
    clearTimeout(holdTimer);
    letGo();
    onScreenMedia(null);
    decoder?.close();
    latest?.close();
    latest = null;
    overlay.clear();
    gestures.cancel();
  });

  const send = (ops: ScOp[]) => {
    if (ops.length && live()) remoteScreens.input(ops);
  };
  /** A source point → CSS pixels from the picture box's corner. */
  const inBox = (p: { x: number; y: number }) => {
    const r = box.getBoundingClientRect();
    return toClient(p.x, p.y, { left: 0, top: 0, width: r.width, height: r.height }, src.w, src.h, view());
  };
  let markId = 0;
  /** A ring where a click, double click, right click or drag went (a drag's where it was picked up), and a short buzz. */
  const feel = (o: GestureOut) => {
    const at = o.feel === "drag" ? echoPoint(o.ops.slice(0, 1)) : o.feel ? echoPoint(o.ops) : null;
    if (!o.feel || !at) return;
    buzz(BUZZ[o.feel]);
    const m = { id: ++markId, ...inBox(at), feel: o.feel };
    setMarks((all) => [...all.slice(-4), m]);
    setTimeout(() => setMarks((all) => all.filter((x) => x.id !== m.id)), 600);
  };
  /** The magnifier above the finger (direct mode): what the picture shows under it, enlarged, with the point marked. */
  const drawLoupe = () => {
    const at = loupe();
    const ctx = at && loupeEl?.getContext("2d");
    if (!at || !ctx || !loupeEl) return;
    const dpr = window.devicePixelRatio || 1;
    const size = Math.round(LOUPE * dpr);
    if (loupeEl.width !== size) loupeEl.width = loupeEl.height = size;
    const half = (LOUPE / LOUPE_ZOOM / 2) * dpr;
    ctx.fillStyle = "#000";
    ctx.fillRect(0, 0, size, size);
    ctx.drawImage(canvas, at.x * dpr - half, at.y * dpr - half, half * 2, half * 2, 0, 0, size, size);
    ctx.strokeStyle = "rgba(232, 116, 59, .9)";
    ctx.lineWidth = dpr;
    ctx.beginPath();
    ctx.moveTo(size / 2 - 6 * dpr, size / 2);
    ctx.lineTo(size / 2 + 6 * dpr, size / 2);
    ctx.moveTo(size / 2, size / 2 - 6 * dpr);
    ctx.lineTo(size / 2, size / 2 + 6 * dpr);
    ctx.stroke();
  };
  /** The long press, the drag and the magnifier as the gesture is now. */
  const showHeld = () => {
    const r = box.getBoundingClientRect();
    const h = gestures.held(performance.now(), frame());
    setHold(h ? inBox(h) : null);
    setDragging(gestures.dragging());
    const finger = gestures.finger();
    setLoupe(mode() === "direct" && finger && (h || gestures.dragging()) ? { x: finger.x - r.left, y: finger.y - r.top } : null);
    drawLoupe();
  };
  let holdTimer: ReturnType<typeof setTimeout> | undefined;

  const out = (o: GestureOut) => {
    if (o.view) {
      setView(o.view);
      redraw();
    }
    const sent = o.ops.length > 0 && live();
    // Anything else that lets the left button go (a tap, a drag's end, the Left button) ends Hold too: the button says what the computer has.
    if (sent && holding() && o.ops.some((op) => (op.k === "up" || op.k === "click") && op.b === "left")) setHolding(false);
    send(o.ops);
    // What was sent puts the computer's cursor somewhere (a move, or a click, press or wheel at the finger in direct mode): shown there at
    // once (local echo), the computer's next positions reconciled to it. Nothing sent, nothing echoed.
    const at = sent ? echoPoint(o.ops) : null;
    if (at) {
      cursor = { ...cursor, x: at.x, y: at.y };
      echo = echoAt(echo, at, performance.now());
      placeCursor();
    }
    // Shown and buzzed only when it went to the computer.
    if (sent) feel(o);
  };
  const touch = (e: PointerEvent) => ({ id: e.pointerId, x: e.clientX, y: e.clientY });
  const down = (e: PointerEvent) => {
    if (!live() || !src.w) return;
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    gestures.mode = mode();
    out(gestures.down(touch(e), e.timeStamp, frame()));
    // Held still long enough, the press shows what it will do (a ring and a buzz; the magnifier in direct mode).
    clearTimeout(holdTimer);
    holdTimer = setTimeout(() => {
      showHeld();
      if (hold()) buzz(BUZZ.right);
    }, LONG_PRESS_MS + 10);
  };
  // Direct touch, zoomed in: while a finger moving the cursor or dragging is held near an edge, the view pans each frame (`Gestures.tick`),
  // even with the finger still.
  const edgePan = () => {
    panFrame = 0;
    if (!gestures.edgePanning()) return;
    out(gestures.tick(performance.now(), frame()));
    if (loupe()) showHeld();
    panFrame = requestAnimationFrame(edgePan);
  };
  const move = (e: PointerEvent) => {
    out(gestures.move(touch(e), e.timeStamp, frame()));
    if (hold() || dragging() || loupe()) showHeld();
    if (!panFrame && view().zoom > 1 && gestures.edgePanning()) panFrame = requestAnimationFrame(edgePan);
  };
  const up = (e: PointerEvent) => {
    out(gestures.up(touch(e), e.timeStamp, frame()));
    if (gestures.idle()) clearTimeout(holdTimer);
    showHeld();
  };
  const cancel = () => {
    out(gestures.cancel());
    clearTimeout(holdTimer);
    showHeld();
  };

  /** An on-screen mouse button at the cursor: a click, a scroll notch, or Hold (the left button down until tapped again; a slide meanwhile drags).
   *  A cursor out of sight (zoomed away from it) is brought back first, and nothing else is done. */
  const mouse = (what: "left" | "middle" | "right" | "up" | "down" | "hold") => {
    if (!live() || !src.w) return;
    gestures.mode = mode();
    // Release lets go where the cursor is, seen or not: bringing it back first would drag what is held to the view's edge.
    if (what === "hold" && holding()) {
      setHolding(false);
      return send([{ k: "up", x: gestures.cursor.x, y: gestures.cursor.y, b: "left" }]);
    }
    const back = gestures.bringBack(frame());
    if (back) return out(back);
    const at = gestures.cursor;
    if (what === "up" || what === "down") return out({ ops: [{ k: "wheel", x: at.x, y: at.y, dx: 0, dy: what === "down" ? 120 : -120 }] });
    if (what === "hold") {
      setHolding(true);
      return out({ ops: [{ k: "down", x: at.x, y: at.y, b: "left" }], feel: "drag" });
    }
    out({ ops: [{ k: "click", x: at.x, y: at.y, b: what }], feel: what === "right" ? "right" : "click" });
  };
  /** Hold let go wherever the screen goes (stopped, ended, lost): the computer's button is never left down. */
  const letGo = () => {
    if (!holding()) return;
    setHolding(false);
    send([{ k: "up", x: gestures.cursor.x, y: gestures.cursor.y, b: "left" }]);
  };
  const toggleMouseBar = () => {
    const on = !mouseBar();
    if (!on) letGo();
    setMouseBar(on);
    screenMouseBar.set(on);
  };

  const key = (k: string) => {
    send([keyOp(k, mods())]);
    setMods([]);
  };
  const toggleMod = (m: Mod) => setMods((all) => (all.includes(m) ? all.filter((x) => x !== m) : [...all, m]));
  // For a field run (plans: the remote desktop's measurements): what this phone measured of the picture and its way, sizes and timings only,
  // never what the picture shows, copied to paste into a note.
  const [copied, setCopied] = createSignal<"yes" | "no" | null>(null);
  const copyMeasurements = async () => {
    const st = s();
    // Sizes, timings and the kind of connection (and the browser, which decodes): never what the picture shows.
    const summary = { at: new Date().toISOString(), path: st.path, way: st.pathStats, via: st.usedVia, stats: st.stats, age_ms_p50: st.age, phone: { ...times.snapshot(), desynchronized: lowLatency() }, agent: navigator.userAgent };
    let done: "yes" | "no" = "no";
    try {
      await navigator.clipboard.writeText(JSON.stringify(summary));
      done = "yes";
    } catch {
      /* no clipboard here (an insecure page, or refused): said as not copied */
    }
    setCopied(done);
    setTimeout(() => setCopied(null), 2000);
  };
  const sendText = async () => {
    const value = text();
    // With a modifier held, one letter or digit is a shortcut (Ctrl+S), not text.
    if (mods().length && /^[A-Za-z0-9]$/.test(value)) {
      key(value.toUpperCase());
      setText("");
      return;
    }
    // Only what was sent leaves the field: words typed while it went stay there.
    if (await remoteScreens.type(value)) setText((now) => (now.startsWith(value) ? now.slice(value.length) : now));
  };
  /** The field keeps the focus (and the keyboard stays up) after a button next to it: asked again within the tap itself. */
  const keepTyping = () => textField.focus({ preventScroll: true });
  // The part shown now, sent crisp once (while crisp text is off for a slow connection).
  const sharpen = () => remoteScreens.sharpen(visibleRect(frame().el, src.w, src.h, view()));
  const resetView = () => {
    setView(clampView(frame().el, src.w, src.h, fitView(src.w, src.h)));
    redraw();
  };
  const closeHelp = () => {
    screenGesturesSeen.set(true);
    setHelp(false);
  };

  return (
    <div class="takeover screen-live" classList={{ dim: !live(), keyboard: !!keyboard() }} data-phase={s().phase} data-keyboard={keyboard() ? "up" : undefined}
      style={keyboard() ? { position: "fixed", left: "0", right: "0", top: `${keyboard()!.top}px`, height: `${keyboard()!.height}px` } : undefined}>
      <div class="takeover-head">
        <span class="takeover-title">{s().title}</span>
        <button class="btn" data-screen-mode onClick={() => setMode(mode() === "trackpad" ? "direct" : "trackpad")}>{mode() === "trackpad" ? t("Trackpad") : t("Direct touch")}</button>
        <button class="btn" data-screen-help aria-label={t("How the fingers work")} aria-expanded={help()} onClick={() => (help() ? closeHelp() : setHelp(true))}>?</button>
        <button class="btn" classList={{ primary: mouseBar() }} aria-pressed={mouseBar()} data-screen-mouse-toggle onClick={toggleMouseBar}>{t("Mouse")}</button>
        <Show when={view().zoom > 1}><button class="btn" data-screen-fit onClick={resetView}>{t("Fit")}</button></Show>
        <button class="btn danger" data-screen-stop onClick={() => void remoteScreens.stop()}>{t("Stop")}</button>
      </div>
      <div class="takeover-screen screen-canvas" ref={box} style={{ "touch-action": "none" }} onPointerDown={down} onPointerMove={move} onPointerUp={up} onPointerCancel={cancel}>
        <canvas ref={canvas} aria-label={t("The computer's screen")} />
        <div class="screen-cursor" ref={cursorEl} style={{ display: "none" }} aria-hidden="true" data-screen-cursor><CursorShape shape={cursorShape()} /></div>
        <For each={marks()}>
          {(m) => (
            <div class={`screen-mark ${m.feel}`} style={{ transform: `translate3d(${m.x}px, ${m.y}px, 0)` }} aria-hidden="true" data-screen-mark={m.feel}>
              <Show when={FEEL_TEXT[m.feel]}><span>{t(FEEL_TEXT[m.feel])}</span></Show>
            </div>
          )}
        </For>
        <Show when={hold()}>
          {(h) => (
            <div class="screen-mark held" style={{ transform: `translate3d(${h().x}px, ${h().y}px, 0)` }} data-screen-held>
              <span>{t("Lift for a right click, or move to drag")}</span>
            </div>
          )}
        </Show>
        <Show when={loupe()}>
          {(l) => <canvas class="screen-loupe" ref={(el) => { loupeEl = el; queueMicrotask(drawLoupe); }} style={{ transform: `translate3d(${l().x - LOUPE / 2}px, ${Math.max(0, l().y - LOUPE - 40)}px, 0)` }} aria-hidden="true" data-screen-loupe />}
        </Show>
        <Show when={dragging()}><div class="screen-chip" role="status" data-screen-dragging>{t("Dragging: lift your finger to drop")}</div></Show>
        <Show when={remoteText(s().remote, t, s().os)}>{(line) => <div class="takeover-lost" role="status" data-remote-state={s().remote}>{line()}</div>}</Show>
        <Show when={s().phase === "frozen" || !connected()}>
          <div class="takeover-lost" role="status" data-screen-reconnecting>
            {s().graceLeft !== null ? t("Reconnecting… the computer keeps this screen for {s} s more. Nothing is sent until the picture is back.", { s: s().graceLeft ?? 0 }) : t("Connection lost: nothing is sent until the picture is back.")}
          </div>
        </Show>
        <Show when={s().phase === "live" && s().stuck}><div class="takeover-lost" role="status" data-screen-stuck>{t("The picture is stuck: asking the computer for a fresh one.")}</div></Show>
        <Show when={debug()}>
          <pre class="screen-debug">{JSON.stringify({ ...s().stats, age_ms_p50: s().age, queue: queue(), shown: shownSeq, tiles: overlay.count, encoder: s().encoder, phone: { ...times.snapshot(), desynchronized: lowLatency() } }, null, 1)}</pre>
        </Show>
      </div>
      <Show when={mouseBar()}>
        {/* Under the picture, never over it: buttons that act where the cursor is, for a person who would rather press than gesture. */}
        <div class="screen-mouse" role="toolbar" aria-label={t("Mouse buttons")} data-screen-mouse>
          <button class="btn" disabled={!live()} onClick={() => mouse("left")} data-mouse="left">{t("Left")}</button>
          <button class="btn" disabled={!live()} onClick={() => mouse("middle")} data-mouse="middle">{t("Middle")}</button>
          <button class="btn" disabled={!live()} onClick={() => mouse("right")} data-mouse="right">{t("Right")}</button>
          <button class="btn" disabled={!live()} aria-label={t("Scroll up")} onClick={() => mouse("up")} data-mouse="up">▲</button>
          <button class="btn" disabled={!live()} aria-label={t("Scroll down")} onClick={() => mouse("down")} data-mouse="down">▼</button>
          <button class="btn" classList={{ primary: holding() }} aria-pressed={holding()} disabled={!live()} onClick={() => mouse("hold")} data-mouse="hold">{holding() ? t("Release") : t("Hold")}</button>
        </div>
      </Show>
      <p class="muted screen-quality" onClick={() => setDebug(!debug())}>
        {qualityText(s().stats, t)}
        <Show when={s().encoder === "sw"}> · {t("software encoder")}</Show>
        <Show when={s().stats?.crisp === false && live() && canSharpen()}>
          {" "}
          <button class="btn" data-screen-sharpen onClick={(e) => { e.stopPropagation(); sharpen(); }}>{t("Sharpen")}</button>
        </Show>
      </p>
      <Show when={debug()}>
        {/* Outside the picture: a tap here never reaches the computer. */}
        <button class="btn" data-screen-copy onClick={() => void copyMeasurements()}>{copied() === "yes" ? t("Copied") : copied() === "no" ? t("Not copied") : t("Copy measurements")}</button>
      </Show>
      <p class="muted screen-details" data-screen-details data-screen-path={s().path ?? ""} data-screen-p2p={s().path === "p2p" ? (s().pathStats?.path ?? "p2p") : ""}>
        {detailsText(s().stats, connected() ? pathText(livePathNow(), t) : null, s().age, t, s().path === "link" ? (s().usedVia === "p2p" ? null : s().usedVia) : s().usedVia, s().path === "p2p" ? (s().pathStats ?? { path: "p2p", local: "", remote: "", relayProtocol: null, rttMs: null, bytesReceived: 0 }) : null)}
      </p>
      <SwitchWay />
      <Show when={help()}>
        <div class="screen-help card" role="dialog" aria-modal="false" aria-label={t("How the fingers work")} data-screen-help-sheet={mode()}>
          <h3>{mode() === "trackpad" ? t("Trackpad: the finger moves the cursor") : t("Direct touch: the finger is the cursor")}</h3>
          <table>
            <tbody>
              <For each={GESTURE_ROWS[mode()]}>{([g, a]) => <tr><th>{t(g)}</th><td>{t(a)}</td></tr>}</For>
            </tbody>
          </table>
          <p class="muted">{t("The button at the top switches between Trackpad and Direct touch. Text you type below is shown as you type it; Hide covers a password.")}</p>
          <p class="muted" data-screen-shortcut-hint>{shortcutHint(s().os, t)}</p>
          <div class="actions"><button class="btn primary" data-screen-help-close onClick={closeHelp}>{t("Got it")}</button></div>
        </div>
      </Show>
      <div class="composer">
        <div class="keybar">
          <For each={modsFor(s().os)}>{(m) => <button class="btn" classList={{ primary: mods().includes(m) }} aria-pressed={mods().includes(m)} disabled={!live()} onClick={() => toggleMod(m)} data-mod={m}>{modLabel(m, s().os)}</button>}</For>
          <For each={keysFor(s().os)}>{(k) => <button class="btn" disabled={!live()} aria-label={k} onClick={() => key(k)} data-key={k}>{keyLabel(k)}</button>}</For>
        </div>
        <form class="takeover-text" onSubmit={(e) => { e.preventDefault(); keepTyping(); void sendText(); }}>
          {/* Shown as typed, so the person sees what goes to the computer (a password field would also force an English keyboard on the
              iPhone, with no Vietnamese); Hide masks it for a password. Enter while a word is still being composed does not send. */}
          <input class="field" ref={textField} type={hideText() ? "password" : "text"} autocomplete="off" autocapitalize="off" autocorrect="off" spellcheck={false} enterkeyhint="send" placeholder={t("Text to type on the computer")} value={text()} onInput={(e) => setText(e.currentTarget.value)} onKeyDown={(e) => { if (e.key === "Enter" && (e.isComposing || e.keyCode === 229)) e.preventDefault(); }} disabled={!live()} data-screen-text />
          <button type="button" class="btn" classList={{ primary: hideText() }} aria-pressed={hideText()} onPointerDown={(e) => e.preventDefault()} onClick={() => { setHideText(!hideText()); keepTyping(); }} data-screen-text-hide>{t("Hide")}</button>
          {/* Pressed without taking the focus from the field: the keyboard stays up for the next words. */}
          <button class="btn primary" onPointerDown={(e) => e.preventDefault()} disabled={!live() || !text() || s().text === "sending"} data-screen-type>{t("Type it")}</button>
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
