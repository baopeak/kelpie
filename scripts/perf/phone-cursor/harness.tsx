// The screen view with a fake computer: 1920×1080 H.264 at 30 frames a second (WebCodecs' own encoder, a picture that changes every frame),
// and the computer's cursor, which follows this page's moves one way late and is told back one way late again, polled every 33 ms like the
// screen helper's (?rtt=ms, the round trip; 400 by default, about the relay's). window.__harness has the stats the driver reads.
import { rec, shownCursor } from "./probes";
import { render } from "solid-js/web";
import { feed, onInput } from "./phone-state-stub";
import { ScreenView } from "../../../app/phone/src/screen-view";
import "../../../app/phone/src/phone.css";

const params = new URLSearchParams(location.search);
const RTT = Number(params.get("rtt") ?? 400);
const W = 1920;
const H = 1080;
const FPS = 30;
const t0 = performance.now();

render(() => <ScreenView />, document.getElementById("root")!);

// ---------- the computer's cursor ----------
const host = { x: W / 2, y: H / 2 };
let told: string | null = null;
onInput((ops) => {
  for (const op of ops) if (op.k === "move") setTimeout(() => Object.assign(host, { x: op.x, y: op.y }), RTT / 2);
});
const cursorMessage = (x: number, y: number) => {
  const b = new Uint8Array(12);
  const v = new DataView(b.buffer);
  b.set([1, 3, 0, 1]);
  v.setInt32(4, x, true);
  v.setInt32(8, y, true);
  return b;
};
setInterval(() => {
  const now = `${host.x},${host.y}`;
  if (now === told) return;
  told = now;
  const msg = cursorMessage(host.x, host.y);
  setTimeout(() => feed(msg), RTT / 2);
}, 33);

// ---------- the picture ----------
const art = new OffscreenCanvas(W, H);
const g = art.getContext("2d")!;
let seq = 0;
const videoMessage = (au: Uint8Array, key: boolean, s: number) => {
  const b = new Uint8Array(20 + au.length);
  const v = new DataView(b.buffer);
  b.set([1, 1, key ? 1 : 0, 0]);
  v.setUint32(4, s, true);
  v.setUint32(8, Math.round(performance.now() - t0), true);
  v.setUint16(12, W, true);
  v.setUint16(14, H, true);
  v.setUint16(16, W, true);
  v.setUint16(18, H, true);
  b.set(au, 20);
  return b;
};
const encoder = new VideoEncoder({
  output: (chunk) => {
    const au = new Uint8Array(chunk.byteLength);
    chunk.copyTo(au);
    feed(videoMessage(au, chunk.type === "key", chunk.timestamp));
  },
  error: (e) => console.error("encoder", e),
});
encoder.configure({ codec: "avc1.42E028", width: W, height: H, bitrate: 4_000_000, framerate: FPS, latencyMode: "realtime", avc: { format: "annexb" } });
setInterval(() => {
  seq++;
  // A desktop-like picture that changes every frame: bands, a moving block and a line of text.
  g.fillStyle = "#1e1e2e";
  g.fillRect(0, 0, W, H);
  for (let i = 0; i < 24; i++) {
    g.fillStyle = i % 2 ? "#313244" : "#45475a";
    g.fillRect(0, i * 45, W, 22);
  }
  g.fillStyle = "#f38ba8";
  g.fillRect((seq * 17) % W, (seq * 7) % H, 240, 160);
  g.fillStyle = "#cdd6f4";
  g.font = "28px monospace";
  g.fillText(`frame ${seq} · ${new Date().toISOString()}`, 40, 60 + (seq % 900));
  const frame = new VideoFrame(art, { timestamp: seq });
  if (encoder.encodeQueueSize < 4) encoder.encode(frame, { keyFrame: seq % 60 === 1 });
  frame.close();
}, 1000 / FPS);

// ---------- the stats ----------
const q = (v: number[], p: number) => {
  if (!v.length) return null;
  const s = [...v].sort((a, b) => a - b);
  return Math.round(s[Math.min(s.length - 1, Math.round((s.length - 1) * p))] * 10) / 10;
};
const spread = (v: number[]) => ({ n: v.length, p50: q(v, 0.5), p95: q(v, 0.95), max: q(v, 1) });

(window as unknown as { __harness: unknown }).__harness = {
  ready: () => rec.videoDraws.length > 10 && shownCursor() !== null,
  now: () => performance.now(),
  /** The stroke from `from` to `to` (performance.now() ms), and the settle window after it until `settle`. */
  stats(from: number, to: number, settle: number) {
    const moves = rec.moves.filter((m) => m.start >= from && m.start <= to);
    const frames = rec.frames.filter((f) => f.at >= from && f.at <= to);
    const intervals = frames.slice(1).map((f, i) => f.at - frames[i].at);
    let changed = 0;
    for (let i = 1; i < frames.length; i++) if (frames[i].x !== frames[i - 1].x || frames[i].y !== frames[i - 1].y) changed++;
    const draws = rec.videoDraws.filter((d) => d.at >= from && d.at <= to);
    const secs = (to - from) / 1000;
    // After the finger stops: how far the shown cursor goes from where the finger left it, and for how many frames.
    const after = rec.frames.filter((f) => f.at > to && f.at <= settle && f.x !== null);
    const last = rec.frames.filter((f) => f.at <= to && f.x !== null).pop();
    const off = after.map((f) => (last ? Math.hypot(f.x! - last.x!, f.y! - last.y!) : 0));
    return {
      moves: moves.length,
      inputDelayMs: spread(moves.map((m) => m.start - m.ts)),
      handlerMs: spread(moves.map((m) => m.end - m.start)),
      eventToFrameMs: spread(moves.filter((m) => m.frame !== null).map((m) => m.frame! - m.ts)),
      frameIntervalMs: spread(intervals),
      longFrames: intervals.filter((x) => x > 25).length,
      fps: Math.round((frames.length / secs) * 10) / 10,
      cursorUpdatesPerSec: Math.round((changed / secs) * 10) / 10,
      videoDrawsPerSec: Math.round((draws.length / secs) * 10) / 10,
      videoDrawMs: spread(draws.map((d) => d.ms)),
      settle: { maxOffPx: Math.round(Math.max(0, ...off) * 10) / 10, framesOff: off.filter((d) => d > 2).length, frames: off.length },
    };
  },
};
