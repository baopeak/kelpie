// Scanning the pairing QR code inside the installed app: a camera scan would open the browser instead, whose storage is not the app's.
// The browser's own BarcodeDetector where there is one, otherwise a small decoder loaded only then (qr-decode.ts). No camera, or no
// permission, leaves the person with pasting the link, which is always offered.

/** How often a frame is looked at. */
const SCAN_GAP_MS = 200;
/** Frames are read at this width at most: enough for a code on a screen, and little work for the phone. */
const FRAME_WIDTH = 960;

export interface FrameSource {
  videoWidth: number;
  videoHeight: number;
}

interface BarcodeDetectorLike {
  detect(source: unknown): Promise<{ rawValue?: string }[]>;
}

export interface ScanEnv {
  /** The camera's stream, or a rejection. Undefined when this browser or origin has none. */
  camera?: () => Promise<MediaStream>;
  BarcodeDetector?: { new (o: { formats: string[] }): BarcodeDetectorLike; getSupportedFormats?: () => Promise<string[]> };
  /** The decoder for when there is no detector. */
  decoder: () => Promise<(rgba: Uint8ClampedArray, w: number, h: number) => string | null>;
  /** The pixels of the frame now. */
  pixels: (source: FrameSource) => { data: Uint8ClampedArray; width: number; height: number } | null;
}

export type Detect = (source: FrameSource) => Promise<string | null>;

function pixelsOf(canvas: HTMLCanvasElement) {
  return (source: FrameSource) => {
    if (!source.videoWidth || !source.videoHeight) return null;
    const scale = Math.min(1, FRAME_WIDTH / source.videoWidth);
    canvas.width = Math.round(source.videoWidth * scale);
    canvas.height = Math.round(source.videoHeight * scale);
    const c = canvas.getContext("2d", { willReadFrequently: true });
    if (!c) return null;
    c.drawImage(source as unknown as HTMLVideoElement, 0, 0, canvas.width, canvas.height);
    return c.getImageData(0, 0, canvas.width, canvas.height);
  };
}

export function browserScanEnv(): ScanEnv {
  const media = navigator.mediaDevices;
  return {
    camera: media?.getUserMedia ? () => media.getUserMedia({ audio: false, video: { facingMode: { ideal: "environment" }, width: { ideal: 1280 } } }) : undefined,
    BarcodeDetector: (window as unknown as { BarcodeDetector?: ScanEnv["BarcodeDetector"] }).BarcodeDetector,
    decoder: () => import("./qr-decode").then((m) => m.decodeQr),
    pixels: pixelsOf(document.createElement("canvas")),
  };
}

/** The decoder, loaded the first time a frame needs it. Where it is the only way to read, it is loaded first (`chooseDetect`): if it cannot be, there is no scanner to offer. */
function viaDecoder(env: ScanEnv): Detect {
  let decode: Awaited<ReturnType<ScanEnv["decoder"]>> | undefined;
  return async (source) => {
    decode ??= await env.decoder();
    const frame = env.pixels(source);
    return frame ? decode(frame.data, frame.width, frame.height) : null;
  };
}

/** How frames are read here: the browser's detector when it does QR codes, or the decoder. A detector that claims to and then refuses a frame is not asked again. */
export async function chooseDetect(env: ScanEnv): Promise<Detect> {
  const fallback = viaDecoder(env);
  const Native = env.BarcodeDetector;
  if (Native) {
    try {
      const formats = await Native.getSupportedFormats?.();
      if (!formats || formats.includes("qr_code")) {
        const detector = new Native({ formats: ["qr_code"] });
        let works = true;
        return async (source) => {
          if (works) {
            try {
              return (await detector.detect(source))[0]?.rawValue ?? null;
            } catch {
              works = false;
            }
          }
          return fallback(source);
        };
      }
    } catch {
      /* the decoder, then */
    }
  }
  const decode = await env.decoder();
  return async (source) => {
    const frame = env.pixels(source);
    return frame ? decode(frame.data, frame.width, frame.height) : null;
  };
}

export interface Scanner {
  stop(): void;
}

/** Starts the camera in `video` and calls `found` with each code it reads until stopped. Null when there is no camera to use. The camera
 *  is let go on stop, whatever happened. */
export async function startScan(video: HTMLVideoElement, found: (text: string) => void, env: ScanEnv = browserScanEnv()): Promise<Scanner | null> {
  if (!env.camera) return null;
  let stream: MediaStream;
  try {
    stream = await env.camera();
  } catch {
    return null;
  }
  let stopped = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const stop = () => {
    stopped = true;
    clearTimeout(timer);
    for (const track of stream.getTracks()) track.stop();
    video.srcObject = null;
  };
  try {
    video.srcObject = stream;
    video.setAttribute("playsinline", "");
    video.muted = true;
    await video.play();
    const detect = await chooseDetect(env);
    const look = async () => {
      if (stopped) return;
      let text: string | null = null;
      try {
        text = await detect(video);
      } catch {
        /* the next frame */
      }
      if (stopped) return;
      if (text) found(text);
      timer = setTimeout(() => void look(), SCAN_GAP_MS);
    };
    void look();
  } catch {
    stop();
    return null;
  }
  return { stop };
}
