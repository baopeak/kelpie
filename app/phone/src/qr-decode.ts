// Reads a QR code from a camera frame, for a browser with no BarcodeDetector (iOS above all). Loaded only when needed, and small
// on purpose: no Kanji mode, no mirrored codes. Dark modules on a light background, as Kelpie's desktop shows them.
// Tables and layout rules are the QR standard's (ISO/IEC 18004).

type Point = { x: number; y: number; size: number; seen: number };

/** Dark pixels (1), each against the mean of the window around it, so that a shadow across the code does not matter. */
export function binarize(rgba: Uint8ClampedArray, w: number, h: number): Uint8Array {
  const sum = new Int32Array((w + 1) * (h + 1));
  const gray = new Uint8Array(w * h);
  for (let y = 0, i = 0; y < h; y++) {
    let row = 0;
    for (let x = 0; x < w; x++, i++) {
      const v = (rgba[i * 4] * 77 + rgba[i * 4 + 1] * 150 + rgba[i * 4 + 2] * 29) >> 8;
      gray[i] = v;
      row += v;
      sum[(y + 1) * (w + 1) + x + 1] = sum[y * (w + 1) + x + 1] + row;
    }
  }
  const r = Math.max(8, Math.round(Math.max(w, h) / 16));
  const out = new Uint8Array(w * h);
  for (let y = 0, i = 0; y < h; y++) {
    const y0 = Math.max(0, y - r);
    const y1 = Math.min(h, y + r + 1);
    for (let x = 0; x < w; x++, i++) {
      const x0 = Math.max(0, x - r);
      const x1 = Math.min(w, x + r + 1);
      const area = (x1 - x0) * (y1 - y0);
      const total = sum[y1 * (w + 1) + x1] - sum[y0 * (w + 1) + x1] - sum[y1 * (w + 1) + x0] + sum[y0 * (w + 1) + x0];
      // Well under the mean, in proportion to it: speckle on white stays white, and a dim frame still has its dark modules.
      out[i] = gray[i] * area * 100 < total * 85 - 200 * area ? 1 : 0;
    }
  }
  return out;
}

const finderRatio = (c: number[]) => {
  const total = c[0] + c[1] + c[2] + c[3] + c[4];
  if (total < 7 || c.some((n) => n === 0)) return false;
  const m = total / 7;
  const ok = (n: number, want: number) => Math.abs(n - want * m) < want * m * 0.6;
  return ok(c[0], 1) && ok(c[1], 1) && ok(c[2], 3) && ok(c[3], 1) && ok(c[4], 1);
};

/** The centre and module size of a finder pattern along one line through (x, y), walking in (dx, dy); null if none is there. */
function along(img: Uint8Array, w: number, h: number, x: number, y: number, dx: number, dy: number): { at: number; size: number } | null {
  const dark = (px: number, py: number) => px >= 0 && py >= 0 && px < w && py < h && img[py * w + px] === 1;
  const c = [0, 0, 0, 0, 0];
  let px = x;
  let py = y;
  while (dark(px, py)) (c[2]++, (px -= dx), (py -= dy));
  if (!c[2]) return null;
  for (const [slot, want] of [[1, false], [0, true]] as const) {
    while (px >= 0 && py >= 0 && px < w && py < h && dark(px, py) === want) (c[slot]++, (px -= dx), (py -= dy));
  }
  px = x + dx;
  py = y + dy;
  while (dark(px, py)) (c[2]++, (px += dx), (py += dy));
  for (const [slot, want] of [[3, false], [4, true]] as const) {
    while (px >= 0 && py >= 0 && px < w && py < h && dark(px, py) === want) (c[slot]++, (px += dx), (py += dy));
  }
  if (!finderRatio(c)) return null;
  // `px, py` now stands just past the fifth run.
  const end = dx ? px : py;
  return { at: end - c[4] - c[3] - c[2] / 2, size: c.reduce((a, b) => a + b) / 7 };
}

function findFinders(img: Uint8Array, w: number, h: number): Point[] {
  const found: Point[] = [];
  for (let y = 0; y < h; y += 2) {
    const run = [0, 0, 0, 0, 0];
    let slot = 0;
    for (let x = 0; x <= w; x++) {
      const dark = x < w && img[y * w + x] === 1;
      if (dark === (slot % 2 === 0)) {
        run[slot]++;
        continue;
      }
      if (slot < 4) {
        if (dark || run[0] > 0) slot++;
        run[slot] = dark === (slot % 2 === 0) ? 1 : 0;
        continue;
      }
      if (finderRatio(run)) {
        const cx = Math.round(x - run[4] - run[3] - run[2] / 2);
        const v = along(img, w, h, cx, y, 0, 1);
        const h1 = v && along(img, w, h, cx, Math.round(v.at), 1, 0);
        const v1 = h1 && along(img, w, h, Math.round(h1.at), Math.round(v.at), 0, 1);
        if (h1 && v1) {
          const size = (h1.size + v1.size) / 2;
          const same = found.find((f) => Math.abs(f.x - h1.at) <= size * 2 && Math.abs(f.y - v1.at) <= size * 2 && Math.abs(f.size - size) <= Math.max(f.size, size) * 0.6);
          if (same) {
            same.x = (same.x * same.seen + h1.at) / (same.seen + 1);
            same.y = (same.y * same.seen + v1.at) / (same.seen + 1);
            same.size = (same.size * same.seen + size) / (same.seen + 1);
            same.seen++;
          } else {
            found.push({ x: h1.at, y: v1.at, size, seen: 1 });
          }
        }
      }
      // The last three runs may be the start of another pattern.
      run[0] = run[2];
      run[1] = run[3];
      run[2] = run[4];
      run[3] = 1;
      run[4] = 0;
      slot = 3;
    }
  }
  return found;
}

const dist = (a: Point, b: Point) => Math.hypot(a.x - b.x, a.y - b.y);

/** The three finders most like the corners of a square, as top-left, top-right, bottom-left. */
function corners(all: Point[]): [Point, Point, Point] | null {
  const pts = [...all].sort((a, b) => b.seen - a.seen).slice(0, 10);
  let best: [Point, Point, Point] | null = null;
  let bestScore = Infinity;
  for (let i = 0; i < pts.length; i++) {
    for (let j = i + 1; j < pts.length; j++) {
      for (let k = j + 1; k < pts.length; k++) {
        const [a, b, c] = [pts[i], pts[j], pts[k]];
        if (Math.max(a.size, b.size, c.size) > 1.8 * Math.min(a.size, b.size, c.size)) continue;
        // The two furthest apart are the ones across from the top-left.
        const options: [Point, Point, Point][] = [[a, b, c], [b, a, c], [c, a, b]];
        const [tl, p, q] = options.sort((x, y) => dist(y[1], y[2]) - dist(x[1], x[2]))[0];
        const s1 = dist(tl, p);
        const s2 = dist(tl, q);
        const hyp = dist(p, q);
        // At least the 14 modules of the smallest code, give or take how the module size was measured.
        if (Math.min(s1, s2) < 10 * Math.max(a.size, b.size, c.size)) continue;
        const score = Math.abs(s1 - s2) / Math.max(s1, s2) + Math.abs(hyp - Math.SQRT2 * (s1 + s2) / 2) / hyp;
        if (score > 0.5 || score >= bestScore) continue;
        bestScore = score;
        const clockwise = (p.x - tl.x) * (q.y - tl.y) - (p.y - tl.y) * (q.x - tl.x) > 0;
        best = clockwise ? [tl, p, q] : [tl, q, p];
      }
    }
  }
  return best;
}

/** The map from four points to four points, as the eight unknowns of a homography. */
function homography(from: number[][], to: number[][]): number[] | null {
  const m: number[][] = [];
  for (let i = 0; i < 4; i++) {
    const [x, y] = from[i];
    const [u, v] = to[i];
    m.push([x, y, 1, 0, 0, 0, -u * x, -u * y, u]);
    m.push([0, 0, 0, x, y, 1, -v * x, -v * y, v]);
  }
  for (let c = 0; c < 8; c++) {
    let top = c;
    for (let r = c + 1; r < 8; r++) if (Math.abs(m[r][c]) > Math.abs(m[top][c])) top = r;
    if (Math.abs(m[top][c]) < 1e-9) return null;
    [m[c], m[top]] = [m[top], m[c]];
    for (let r = 0; r < 8; r++) {
      if (r === c) continue;
      const f = m[r][c] / m[c][c];
      for (let k = c; k < 9; k++) m[r][k] -= f * m[c][k];
    }
  }
  return m.map((row, i) => row[8] / row[i]);
}

type Spot = { x: number; y: number };

/** Near where the bottom-right alignment pattern should be, the places that look most like one (dark ring, light ring, dark centre), best
 *  first. On a big code a patch of data can look as good, so only reading the code can tell which it was. */
function findAlignment(img: Uint8Array, w: number, h: number, tl: Point, tr: Point, bl: Point, modules: number): Spot[] {
  const ux = (tr.x - tl.x) / modules;
  const uy = (tr.y - tl.y) / modules;
  const vx = (bl.x - tl.x) / modules;
  const vy = (bl.y - tl.y) / modules;
  const ex = tl.x + (tr.x - tl.x) + (bl.x - tl.x) - 3 * (ux + vx);
  const ey = tl.y + (tr.y - tl.y) + (bl.y - tl.y) - 3 * (uy + vy);
  const unit = Math.max(Math.hypot(ux, uy), Math.hypot(vx, vy));
  const reach = Math.ceil(unit * Math.min(16, 3 + modules / 6));
  const step = Math.max(1, Math.floor(unit / 3));
  const found: (Spot & { hits: number; off: number })[] = [];
  for (let dy = -reach; dy <= reach; dy += step) {
    for (let dx = -reach; dx <= reach; dx += step) {
      const cx = ex + dx;
      const cy = ey + dy;
      let hits = 0;
      for (let j = -2; j <= 2; j++) {
        for (let i = -2; i <= 2; i++) {
          const px = Math.floor(cx + i * ux + j * vx);
          const py = Math.floor(cy + i * uy + j * vy);
          if (px < 0 || py < 0 || px >= w || py >= h) continue;
          const ring = Math.max(Math.abs(i), Math.abs(j));
          if (img[py * w + px] === (ring === 1 ? 0 : 1)) hits++;
        }
      }
      if (hits >= 21) found.push({ x: cx, y: cy, hits, off: Math.hypot(dx, dy) });
    }
  }
  found.sort((a, b) => b.hits - a.hits || a.off - b.off);
  const out: Spot[] = [];
  for (const f of found) {
    if (out.length >= 24) break;
    if (out.every((o) => Math.hypot(o.x - f.x, o.y - f.y) > unit * 1.5)) out.push(f);
  }
  return out;
}

/** Reads the grid of modules with `size` to a side from the image, with the alignment pattern at `align` (or none, on the smallest code or when none was found). */
function sample(img: Uint8Array, w: number, h: number, tl: Point, tr: Point, bl: Point, size: number, align: Spot | null): Uint8Array | null {
  const far = size - 3.5;
  const to = [[tl.x, tl.y], [tr.x, tr.y], [bl.x, bl.y], align ? [align.x, align.y] : [tr.x + bl.x - tl.x, tr.y + bl.y - tl.y]];
  const from = [[3.5, 3.5], [far, 3.5], [3.5, far], align ? [size - 6.5, size - 6.5] : [far, far]];
  const t = homography(from, to);
  if (!t) return null;
  const out = new Uint8Array(size * size);
  for (let y = 0; y < size; y++) {
    for (let x = 0; x < size; x++) {
      const mx = x + 0.5;
      const my = y + 0.5;
      const d = t[6] * mx + t[7] * my + 1;
      const px = Math.floor((t[0] * mx + t[1] * my + t[2]) / d);
      const py = Math.floor((t[3] * mx + t[4] * my + t[5]) / d);
      if (px < 0 || py < 0 || px >= w || py >= h) return null;
      out[y * size + x] = img[py * w + px];
    }
  }
  return out;
}

// ---------- the grid's meaning ----------

// Error correction words per block and blocks, by level (index = the format bits: M, L, H, Q) and version (index 1 on).
const ECC: Record<number, number[]> = {
  1: [7, 10, 15, 20, 26, 18, 20, 24, 30, 18, 20, 24, 26, 30, 22, 24, 28, 30, 28, 28, 28, 28, 30, 30, 26, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
  0: [10, 16, 26, 18, 24, 16, 18, 22, 22, 26, 30, 22, 22, 24, 24, 28, 28, 26, 26, 26, 26, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28],
  3: [13, 22, 18, 26, 18, 24, 18, 22, 20, 24, 28, 26, 24, 20, 30, 24, 28, 28, 26, 30, 28, 30, 30, 30, 30, 28, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
  2: [17, 28, 22, 16, 22, 28, 26, 26, 24, 28, 24, 28, 22, 24, 24, 30, 28, 28, 26, 28, 30, 24, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30, 30],
};
const BLOCKS: Record<number, number[]> = {
  1: [1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 4, 4, 6, 6, 6, 6, 7, 8, 8, 9, 9, 10, 12, 12, 12, 13, 14, 15, 16, 17, 18, 19, 19, 20, 21, 22, 24, 25],
  0: [1, 1, 1, 2, 2, 4, 4, 4, 5, 5, 5, 8, 9, 9, 10, 10, 11, 13, 14, 16, 17, 17, 18, 20, 21, 23, 25, 26, 28, 29, 31, 33, 35, 37, 38, 40, 43, 45, 47, 49],
  3: [1, 1, 2, 2, 4, 4, 6, 6, 8, 8, 8, 10, 12, 16, 12, 17, 16, 18, 21, 20, 23, 23, 25, 27, 29, 34, 34, 35, 38, 40, 43, 45, 48, 51, 53, 56, 59, 62, 65, 68],
  2: [1, 1, 2, 4, 4, 4, 5, 6, 8, 8, 11, 11, 16, 16, 18, 16, 19, 21, 25, 25, 25, 34, 30, 32, 35, 37, 40, 42, 45, 48, 51, 54, 57, 60, 63, 66, 70, 74, 77, 81],
};

function alignmentCenters(version: number): number[] {
  if (version === 1) return [];
  const count = Math.floor(version / 7) + 2;
  const step = version === 32 ? 26 : Math.ceil((version * 4 + 4) / (count * 2 - 2)) * 2;
  const out = [6];
  for (let i = 0, at = version * 4 + 10; i < count - 1; i++, at -= step) out.splice(1, 0, at);
  return out;
}

function functionModules(version: number): Uint8Array {
  const size = version * 4 + 17;
  const f = new Uint8Array(size * size);
  const fill = (x0: number, y0: number, wide: number, tall: number) => {
    for (let y = y0; y < y0 + tall; y++) for (let x = x0; x < x0 + wide; x++) f[y * size + x] = 1;
  };
  fill(0, 0, 9, 9);
  fill(size - 8, 0, 8, 9);
  fill(0, size - 8, 9, 8);
  fill(6, 0, 1, size);
  fill(0, 6, size, 1);
  const centers = alignmentCenters(version);
  for (const cy of centers) {
    for (const cx of centers) {
      const onFinder = (cx === 6 && cy === 6) || (cx === 6 && cy === size - 7) || (cx === size - 7 && cy === 6);
      if (!onFinder) fill(cx - 2, cy - 2, 5, 5);
    }
  }
  if (version >= 7) {
    fill(size - 11, 0, 3, 6);
    fill(0, size - 11, 6, 3);
  }
  return f;
}

const MASKS: ((x: number, y: number) => boolean)[] = [
  (x, y) => (x + y) % 2 === 0,
  (_, y) => y % 2 === 0,
  (x) => x % 3 === 0,
  (x, y) => (x + y) % 3 === 0,
  (x, y) => (Math.floor(x / 3) + Math.floor(y / 2)) % 2 === 0,
  (x, y) => ((x * y) % 2) + ((x * y) % 3) === 0,
  (x, y) => (((x * y) % 2) + ((x * y) % 3)) % 2 === 0,
  (x, y) => (((x + y) % 2) + ((x * y) % 3)) % 2 === 0,
];

/** The closest of the 32 format codes, at most three bits off. */
function closestFormat(bits: number): number | null {
  let best: number | null = null;
  let bestOff = 4;
  for (let data = 0; data < 32; data++) {
    let rem = data;
    for (let i = 0; i < 10; i++) rem = (rem << 1) ^ ((rem >> 9) * 0x537);
    const code = ((data << 10) | rem) ^ 0x5412;
    let off = 0;
    for (let d = code ^ bits; d; d &= d - 1) off++;
    if (off < bestOff) {
      bestOff = off;
      best = data;
    }
  }
  return best;
}

function readFormat(grid: Uint8Array, size: number): number | null {
  const at = (x: number, y: number) => grid[y * size + x];
  let one = 0;
  for (let i = 0; i < 6; i++) one |= at(8, i) << i;
  one |= (at(8, 7) << 6) | (at(8, 8) << 7) | (at(7, 8) << 8);
  for (let i = 9; i < 15; i++) one |= at(14 - i, 8) << i;
  let two = 0;
  for (let i = 0; i < 8; i++) two |= at(size - 1 - i, 8) << i;
  for (let i = 8; i < 15; i++) two |= at(8, size - 15 + i) << i;
  return closestFormat(one) ?? closestFormat(two);
}

// GF(256) with the QR polynomial.
const EXP = new Uint8Array(512);
const LOG = new Uint8Array(256);
for (let i = 0, x = 1; i < 255; i++) {
  EXP[i] = x;
  EXP[i + 255] = x;
  LOG[x] = i;
  x = (x << 1) ^ (x & 0x80 ? 0x11d : 0);
}
const mul = (a: number, b: number) => (a && b ? EXP[LOG[a] + LOG[b]] : 0);

/** Corrects one block in place (`ecc` words at its end); false when it cannot be. */
function correct(block: Uint8Array, ecc: number): boolean {
  const n = block.length;
  const syndromes = new Uint8Array(ecc);
  let clean = true;
  for (let i = 0; i < ecc; i++) {
    let s = 0;
    for (let j = 0; j < n; j++) s = mul(s, EXP[i]) ^ block[j];
    syndromes[i] = s;
    if (s) clean = false;
  }
  if (clean) return true;
  // Berlekamp-Massey: the error locator.
  let sigma = [1];
  let prev = [1];
  let errors = 0;
  let shift = 1;
  let lastDelta = 1;
  for (let i = 0; i < ecc; i++) {
    let delta = syndromes[i];
    for (let j = 1; j <= errors; j++) delta ^= mul(sigma[j] ?? 0, syndromes[i - j] ?? 0);
    if (delta === 0) {
      shift++;
      continue;
    }
    const scale = EXP[(LOG[delta] + 255 - LOG[lastDelta]) % 255];
    const next = [...sigma];
    for (let j = 0; j < prev.length; j++) next[j + shift] = (next[j + shift] ?? 0) ^ mul(scale, prev[j]);
    if (2 * errors <= i) {
      errors = i + 1 - errors;
      prev = sigma;
      lastDelta = delta;
      shift = 1;
    } else {
      shift++;
    }
    sigma = next;
  }
  if (errors * 2 > ecc) return false;
  // Chien search: where they are.
  const places: number[] = [];
  for (let at = 0; at < n; at++) {
    const x = at === 0 ? 1 : EXP[255 - at];
    let v = 0;
    for (let j = sigma.length - 1; j >= 0; j--) v = mul(v, x) ^ (sigma[j] ?? 0);
    if (v === 0) places.push(at);
  }
  if (places.length !== errors) return false;
  // Forney: what they are.
  const omega = new Uint8Array(ecc);
  for (let i = 0; i < ecc; i++) {
    let v = 0;
    for (let j = 0; j <= i; j++) v ^= mul(sigma[j] ?? 0, syndromes[i - j]);
    omega[i] = v;
  }
  for (const at of places) {
    const x = at === 0 ? 1 : EXP[255 - at];
    let top = 0;
    for (let j = ecc - 1; j >= 0; j--) top = mul(top, x) ^ omega[j];
    let bottom = 0;
    for (let j = 1; j < sigma.length; j += 2) bottom ^= mul(sigma[j] ?? 0, j === 1 ? 1 : EXP[(LOG[x] * (j - 1)) % 255]);
    if (!bottom) return false;
    const size = EXP[(LOG[top] + 255 - LOG[bottom]) % 255];
    const magnitude = mul(size, EXP[at % 255]);
    block[n - 1 - at] ^= magnitude;
  }
  for (let i = 0; i < ecc; i++) {
    let s = 0;
    for (let j = 0; j < n; j++) s = mul(s, EXP[i]) ^ block[j];
    if (s) return false;
  }
  return true;
}

const ALNUM = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

/** The data words as text: numeric, alphanumeric and byte segments, ECI assignments skipped. */
function readSegments(data: Uint8Array, version: number): string | null {
  let bit = 0;
  const take = (n: number) => {
    let v = 0;
    for (let i = 0; i < n; i++, bit++) v = (v << 1) | (((data[bit >> 3] ?? 0) >> (7 - (bit & 7))) & 1);
    return v;
  };
  const left = () => data.length * 8 - bit;
  const big = version < 10 ? 0 : version < 27 ? 1 : 2;
  const out: number[] = [];
  while (left() >= 4) {
    const mode = take(4);
    if (mode === 0) break;
    if (mode === 1) {
      let n = take([10, 12, 14][big]);
      for (; n >= 3; n -= 3) out.push(...String(take(10)).padStart(3, "0").split("").map((c) => c.charCodeAt(0)));
      if (n === 2) out.push(...String(take(7)).padStart(2, "0").split("").map((c) => c.charCodeAt(0)));
      else if (n === 1) out.push(48 + take(4));
    } else if (mode === 2) {
      let n = take([9, 11, 13][big]);
      for (; n >= 2; n -= 2) {
        const v = take(11);
        out.push(ALNUM.charCodeAt(Math.floor(v / 45)), ALNUM.charCodeAt(v % 45));
      }
      if (n === 1) out.push(ALNUM.charCodeAt(take(6)));
    } else if (mode === 4) {
      const n = take([8, 16, 16][big]);
      if (n * 8 > left()) return null;
      for (let i = 0; i < n; i++) out.push(take(8));
    } else if (mode === 7) {
      if (take(1)) take(take(1) ? 22 : 14);
      else take(7);
    } else {
      return null;
    }
  }
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(Uint8Array.from(out));
  } catch {
    return null;
  }
}

/** What a grid of modules says, or null when it is not a good code. */
export function readGrid(grid: Uint8Array, size: number): string | null {
  const version = (size - 17) / 4;
  if (!Number.isInteger(version) || version < 1 || version > 40) return null;
  const format = readFormat(grid, size);
  if (format === null) return null;
  const level = format >> 3;
  const mask = MASKS[format & 7];
  const isFunction = functionModules(version);
  let raw = (16 * version + 128) * version + 64;
  if (version >= 2) {
    const count = Math.floor(version / 7) + 2;
    raw -= (25 * count - 10) * count - 55;
    if (version >= 7) raw -= 36;
  }
  const words = new Uint8Array(raw >> 3);
  let i = 0;
  for (let right = size - 1; right >= 1; right -= 2) {
    if (right === 6) right = 5;
    for (let v = 0; v < size; v++) {
      for (let j = 0; j < 2; j++) {
        const x = right - j;
        const y = ((right + 1) & 2) === 0 ? size - 1 - v : v;
        if (isFunction[y * size + x]) continue;
        const dark = grid[y * size + x] === 1 !== mask(x, y);
        if (dark && i >> 3 < words.length) words[i >> 3] |= 0x80 >> (i & 7);
        i++;
      }
    }
  }
  const ecc = ECC[level][version - 1];
  const blocks = BLOCKS[level][version - 1];
  const short = blocks - (words.length % blocks);
  const shortLen = Math.floor(words.length / blocks);
  const all: Uint8Array[] = [];
  for (let b = 0; b < blocks; b++) all.push(new Uint8Array(shortLen + (b < short ? 0 : 1)));
  let w = 0;
  // Data words go round the blocks; a short block has no last data word. Then the error correction words.
  for (let k = 0; k < shortLen - ecc + 1; k++) {
    for (let b = 0; b < blocks; b++) {
      if (k === shortLen - ecc && b < short) continue;
      all[b][k] = words[w++];
    }
  }
  for (let k = 0; k < ecc; k++) {
    for (let b = 0; b < blocks; b++) all[b][all[b].length - ecc + k] = words[w++];
  }
  const data: number[] = [];
  for (const block of all) {
    if (!correct(block, ecc)) return null;
    data.push(...block.subarray(0, block.length - ecc));
  }
  return readSegments(Uint8Array.from(data), version);
}

/** The text of the QR code in an RGBA image, or null. */
export function decodeQr(rgba: Uint8ClampedArray, w: number, h: number): string | null {
  const img = binarize(rgba, w, h);
  const c = corners(findFinders(img, w, h));
  if (!c) return null;
  const [tl, tr, bl] = c;
  const unit = (tl.size + tr.size + bl.size) / 3;
  const across = ((dist(tl, tr) + dist(tl, bl)) / 2) / unit + 7;
  const near = Math.round((across - 17) / 4) * 4 + 17;
  // A module size measured from a few pixels can be off by a version on a big code.
  for (const size of [near, near + 4, near - 4, near + 8, near - 8]) {
    if (size < 21 || size > 177) continue;
    const spots = size > 21 ? findAlignment(img, w, h, tl, tr, bl, size - 7) : [];
    for (const spot of [...spots, null]) {
      const grid = sample(img, w, h, tl, tr, bl, size, spot);
      const text = grid && readGrid(grid, size);
      if (text) return text;
    }
  }
  return null;
}
