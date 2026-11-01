import { describe, expect, it } from "vitest";
import fixtures from "./qr-fixtures.json";
import { decodeQr, readGrid } from "./qr-decode";

// Real codes from the encoder the daemon uses (kept in step by a test in crates/kelpied/src/mobile.rs), drawn the way a camera would
// see them: seeded noise, so that a failure repeats.
function seeded(seed: number) {
  let x = seed;
  return () => {
    x = (x * 1664525 + 1013904223) >>> 0;
    return x / 2 ** 32;
  };
}

const grid = (rows: string[]) => Uint8Array.from(rows.join("").split("").map(Number));

interface Scene {
  size?: number;
  /** Pixels per module. */
  scale: number;
  degrees?: number;
  /** How much further away the top edge is. */
  tilt?: number;
  noise?: number;
  /** Brightness falling across the frame, 0 to 1. */
  shadow?: number;
  /** A square of glare over part of the code, in pixels from the centre, as [x, y, half]. */
  glare?: [number, number, number];
}

function draw(rows: string[], scene: Scene) {
  const n = rows.length;
  const size = scene.size ?? Math.ceil((n + 8) * scene.scale * 1.6);
  const out = new Uint8ClampedArray(size * size * 4);
  const random = seeded(n * 31 + scene.scale);
  const a = ((scene.degrees ?? 0) * Math.PI) / 180;
  const [c, s] = [Math.cos(a), Math.sin(a)];
  for (let py = 0; py < size; py++) {
    for (let px = 0; px < size; px++) {
      const dx = px + 0.5 - size / 2;
      const dy = py + 0.5 - size / 2;
      // From the frame back to the code: perspective, then rotation.
      const w = 1 - (scene.tilt ?? 0) * (dy / size);
      const rx = (dx * c + dy * s) / w;
      const ry = (-dx * s + dy * c) / w;
      const mx = Math.floor(rx / scene.scale + n / 2);
      const my = Math.floor(ry / scene.scale + n / 2);
      const dark = mx >= 0 && my >= 0 && mx < n && my < n && rows[my][mx] === "1";
      let v = dark ? 30 : 225;
      v *= 1 - (scene.shadow ?? 0) * (px / size);
      if (scene.glare && Math.abs(dx - scene.glare[0]) < scene.glare[2] && Math.abs(dy - scene.glare[1]) < scene.glare[2]) v = 250;
      v += (random() - 0.5) * (scene.noise ?? 0);
      const i = (py * size + px) * 4;
      out[i] = out[i + 1] = out[i + 2] = v;
      out[i + 3] = 255;
    }
  }
  return { data: out, size };
}

describe("what a grid says", () => {
  it("reads every kind of code the encoder makes", () => {
    for (const f of fixtures.codes) expect(readGrid(grid(f.rows), f.rows.length)).toBe(f.text);
  });

  it("corrects damaged modules, and refuses what is beyond correcting", () => {
    const f = fixtures.codes[fixtures.codes.length - 1];
    const n = f.rows.length;
    const damaged = grid(f.rows);
    const random = seeded(7);
    // Flips in the data area, a handful: well within what level M corrects.
    for (let i = 0; i < 12; i++) damaged[(10 + Math.floor(random() * (n - 20))) * n + 10 + Math.floor(random() * (n - 20))] ^= 1;
    expect(readGrid(damaged, n)).toBe(f.text);
    const ruined = grid(f.rows);
    for (let i = 10 * n; i < (n - 10) * n; i++) if (random() < 0.5) ruined[i] ^= 1;
    expect(readGrid(ruined, n)).toBeNull();
  });

  it("is no code at all when the size is not one", () => {
    expect(readGrid(new Uint8Array(100), 10)).toBeNull();
    expect(readGrid(new Uint8Array(21 * 21), 21)).toBeNull();
  });
});

describe("a code in a frame", () => {
  it.each([
    ["straight", { scale: 5 }],
    ["small in the frame", { scale: 4, size: 640 }],
    ["turned", { scale: 5, degrees: 17 }],
    ["upside down", { scale: 5, degrees: 180 }],
    ["on its side", { scale: 5, degrees: 90 }],
    ["seen at an angle", { scale: 5, tilt: 0.12, degrees: -8 }],
    ["with noise", { scale: 5, noise: 50 }],
    ["in a shadow", { scale: 5, shadow: 0.55 }],
    ["all of it at once", { scale: 5, degrees: 11, tilt: 0.12, noise: 30, shadow: 0.35 }],
  ] as [string, Scene][])("%s", (_name, scene) => {
    for (const f of fixtures.codes) {
      // At an angle: what Kelpie's links really are. The smallest code has no alignment pattern to correct one with, and modules as fine as
      // the largest's are beyond a phone's camera at the size a code has on a screen.
      if (scene.tilt && (f.rows.length === 21 || f.rows.length > 100)) continue;
      const { data, size } = draw(f.rows, scene);
      expect(decodeQr(data, size, size)).toBe(f.text);
    }
  });

  it("through a glare on a corner of the data", () => {
    const f = fixtures.codes[fixtures.codes.length - 1];
    const { data, size } = draw(f.rows, { scale: 5, glare: [40, 40, 10] });
    expect(decodeQr(data, size, size)).toBe(f.text);
  });

  it("nothing in the frame is nothing", () => {
    const size = 320;
    const random = seeded(3);
    const blank = new Uint8ClampedArray(size * size * 4).fill(200);
    const static_ = new Uint8ClampedArray(size * size * 4).map(() => random() * 255);
    expect(decodeQr(blank, size, size)).toBeNull();
    expect(decodeQr(static_, size, size)).toBeNull();
  });
});
