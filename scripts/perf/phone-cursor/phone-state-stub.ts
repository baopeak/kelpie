// What screen-view.tsx reads of the phone's state, for the harness: a screen that is live on a connected phone, the media sink, and the
// input it sends (handed to the fake computer in harness.tsx).
import { createSignal } from "solid-js";
import type { ScOp, ScreenState } from "../../../app/phone/src/screen";

let sink: ((bytes: Uint8Array) => void) | null = null;
let inputSink: ((ops: ScOp[]) => void) | null = null;

export function onScreenMedia(s: ((bytes: Uint8Array) => void) | null) {
  sink = s;
}
/** The fake computer's media, as the link would hand it over. */
export const feed = (bytes: Uint8Array) => sink?.(bytes);
export const onInput = (s: (ops: ScOp[]) => void) => (inputSink = s);

const live: ScreenState = { phase: "live", targets: null, sc: "harness", title: "Harness", kind: "monitor", remote: "live", stats: null, encoder: "hw", message: null, text: null, stuck: false, t0: null, age: null, graceLeft: null, paths: null, relay: null, via: "auto", usedVia: null, path: null, pathStats: null, p2pFailed: false };
export const [remoteScreen] = createSignal<ScreenState | null>(live);
export const connected = () => true;
export const canSharpen = () => false;
export const livePathNow = () => "relay" as const;
export const route = () => "relay" as const;
const none = () => {};
export const remoteScreens = {
  input: (ops: ScOp[]) => inputSink?.(ops),
  presented: none,
  decoded: none,
  skipped: none,
  needKeyframe: none,
  stop: none,
  close: none,
  sharpen: none,
  choosePath: none,
  switchVia: async () => {},
  pick: async () => {},
  type: async () => true,
  side: () => ({ hosted: false, webrtc: true, p2pFailed: false }),
};
