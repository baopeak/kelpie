import { describe, expect, it, vi } from "vitest";
import { chooseDetect, startScan, type FrameSource, type ScanEnv } from "./qr-scan";

const frame: FrameSource = { videoWidth: 4, videoHeight: 4 };
const env = (over: Partial<ScanEnv> = {}): ScanEnv => ({
  decoder: async () => () => "from the decoder",
  pixels: () => ({ data: new Uint8ClampedArray(64), width: 4, height: 4 }),
  ...over,
});
const stream = () => {
  const stop = vi.fn();
  return { stop, value: { getTracks: () => [{ stop }] } as unknown as MediaStream };
};
const fakeVideo = (play: () => Promise<void> = async () => {}) => ({ srcObject: null as unknown, muted: false, setAttribute() {}, play, videoWidth: 4, videoHeight: 4 }) as unknown as HTMLVideoElement;

function nativeDetector(formats: string[] | null) {
  return class {
    static getSupportedFormats = formats ? async () => formats : undefined;
    async detect() {
      return [{ rawValue: "from the browser" }];
    }
  } as unknown as ScanEnv["BarcodeDetector"];
}

describe("how frames are read", () => {
  it("the browser's detector when it does QR codes", async () => {
    expect(await (await chooseDetect(env({ BarcodeDetector: nativeDetector(["qr_code", "ean_13"]) })))(frame)).toBe("from the browser");
    expect(await (await chooseDetect(env({ BarcodeDetector: nativeDetector(null) })))(frame)).toBe("from the browser");
  });

  it("the decoder when there is no detector, when it does not do QR codes, or when it cannot be made", async () => {
    expect(await (await chooseDetect(env()))(frame)).toBe("from the decoder");
    expect(await (await chooseDetect(env({ BarcodeDetector: nativeDetector(["ean_13"]) })))(frame)).toBe("from the decoder");
    const broken = class {
      constructor() {
        throw new Error("no");
      }
    } as unknown as ScanEnv["BarcodeDetector"];
    expect(await (await chooseDetect(env({ BarcodeDetector: broken })))(frame)).toBe("from the decoder");
  });

  it("a detector that claims QR codes and then refuses a frame gives way to the decoder for good", async () => {
    let asked = 0;
    const refusing = class {
      static getSupportedFormats = async () => ["qr_code"];
      async detect() {
        asked++;
        throw new Error("this frame cannot be read");
      }
    } as unknown as ScanEnv["BarcodeDetector"];
    const detect = await chooseDetect(env({ BarcodeDetector: refusing }));
    expect(await detect(frame)).toBe("from the decoder");
    expect(await detect(frame)).toBe("from the decoder");
    expect(asked).toBe(1);
  });

  it("a frame with nothing in it yet is nothing", async () => {
    expect(await (await chooseDetect(env({ pixels: () => null })))(frame)).toBeNull();
  });
});

describe("the camera", () => {
  it("no camera, or no permission, leaves pasting: no scanner, nothing held", async () => {
    expect(await startScan(fakeVideo(), () => {}, env())).toBeNull();
    expect(await startScan(fakeVideo(), () => {}, env({ camera: () => Promise.reject(new Error("denied")) }))).toBeNull();
  });

  it("a video that will not play lets the camera go", async () => {
    const s = stream();
    const scanner = await startScan(fakeVideo(() => Promise.reject(new Error("no"))), () => {}, env({ camera: async () => s.value }));
    expect(scanner).toBeNull();
    expect(s.stop).toHaveBeenCalled();
  });

  it("a code is handed over, and stopping lets the camera go and looks at no more frames", async () => {
    const s = stream();
    const video = fakeVideo();
    const seen: string[] = [];
    let got!: () => void;
    const first = new Promise<void>((resolve) => (got = resolve));
    const scanner = await startScan(video, (t) => {
      seen.push(t);
      got();
    }, env({ camera: async () => s.value }));
    await first;
    expect(seen[0]).toBe("from the decoder");
    expect(video.srcObject).toBe(s.value);
    scanner!.stop();
    expect(s.stop).toHaveBeenCalled();
    expect(video.srcObject).toBeNull();
    const after = seen.length;
    await new Promise((r) => setTimeout(r, 300));
    expect(seen.length).toBe(after);
  });

  it("a frame that fails is not the end", async () => {
    const s = stream();
    let tries = 0;
    let got!: () => void;
    const seen = new Promise<void>((resolve) => (got = resolve));
    const scanner = await startScan(fakeVideo(), () => got(), env({
      camera: async () => s.value,
      decoder: async () => () => {
        if (++tries < 2) throw new Error("bad frame");
        return "at last";
      },
    }));
    await seen;
    expect(tries).toBeGreaterThanOrEqual(2);
    scanner!.stop();
  });
});
