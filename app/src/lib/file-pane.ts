// What a Files pane does with a listing, without the DOM: sort, filter, select, permissions as text and bits,
// paths on this computer and on a host, sizes and times. Tested in file-pane.test.ts.
import type { RemoteEntry } from "./types";

/** `null`: this computer; else a saved host's alias. */
export type PaneEnd = string | null;

export type SortKey = "name" | "size" | "mtime" | "mode" | "owner";

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: "base" });

const isFolder = (e: RemoteEntry) => e.kind === "dir";

/** Folders first, then by `key` (names in natural order: `file2` before `file10`); `asc` false reverses within each group. */
export function sortEntries(list: RemoteEntry[], key: SortKey, asc = true): RemoteEntry[] {
  const by = (a: RemoteEntry, b: RemoteEntry): number => {
    switch (key) {
      case "size":
        return a.size - b.size;
      case "mtime":
        return a.mtime - b.mtime;
      case "mode":
        return (a.mode ?? -1) - (b.mode ?? -1);
      case "owner":
        return collator.compare(a.owner ?? "", b.owner ?? "");
      default:
        return 0;
    }
  };
  return [...list].sort((a, b) => {
    if (isFolder(a) !== isFolder(b)) return isFolder(a) ? -1 : 1;
    const d = by(a, b) || collator.compare(a.name, b.name);
    return asc ? d : -d;
  });
}

/** Names containing `text`, ignoring case. */
export function filterEntries(list: RemoteEntry[], text: string): RemoteEntry[] {
  const q = text.trim().toLowerCase();
  return q ? list.filter((e) => e.name.toLowerCase().includes(q)) : list;
}

export interface Selection {
  picked: string[];
  /** Where a Shift+click range starts. */
  anchor: string | null;
}

/** A click on `name` in the shown `order`: plain picks one, Ctrl/Cmd toggles, Shift picks the range from the anchor. */
export function clickSelect(sel: Selection, order: string[], name: string, mods: { toggle?: boolean; range?: boolean }): Selection {
  if (mods.range && sel.anchor && order.includes(sel.anchor)) {
    const [a, b] = [order.indexOf(sel.anchor), order.indexOf(name)].sort((x, y) => x - y);
    const range = order.slice(a, b + 1);
    return { picked: mods.toggle ? [...new Set([...sel.picked, ...range])] : range, anchor: sel.anchor };
  }
  if (mods.toggle) {
    const on = sel.picked.includes(name);
    return { picked: on ? sel.picked.filter((x) => x !== name) : [...sel.picked, name], anchor: name };
  }
  return { picked: [name], anchor: name };
}

export const selectAll = (order: string[]): Selection => ({ picked: [...order], anchor: order[0] ?? null });

// ---- permissions ----

const KIND_CHAR: Record<RemoteEntry["kind"], string> = { dir: "d", link: "l", file: "-", other: "?" };

/** `drwxr-xr-x`, with s/S and t/T for setuid, setgid and sticky. */
export function modeString(mode: number | null, kind: RemoteEntry["kind"] = "file"): string {
  if (mode == null) return "";
  const rwx = (bits: number, special: boolean, ch: string) => `${bits & 4 ? "r" : "-"}${bits & 2 ? "w" : "-"}${special ? (bits & 1 ? ch : ch.toUpperCase()) : bits & 1 ? "x" : "-"}`;
  return KIND_CHAR[kind] + rwx((mode >> 6) & 7, !!(mode & 0o4000), "s") + rwx((mode >> 3) & 7, !!(mode & 0o2000), "s") + rwx(mode & 7, !!(mode & 0o1000), "t");
}

/** The 9 rwx bits, owner first: [r, w, x, r, w, x, r, w, x]. */
export const modeBits = (mode: number): boolean[] => Array.from({ length: 9 }, (_, i) => !!(mode & (1 << (8 - i))));

/** The bits back into a mode, keeping the setuid / setgid / sticky bits of `keep`. */
export const bitsMode = (bits: boolean[], keep = 0): number => bits.reduce((m, on, i) => (on ? m | (1 << (8 - i)) : m), keep & 0o7000);

export const octal = (mode: number): string => (mode & 0o7777).toString(8).padStart(3, "0");

/** `755` or `0755` (3 or 4 octal digits), else null. */
export function parseOctal(text: string): number | null {
  const s = text.trim();
  return /^[0-7]{3,4}$/.test(s) ? parseInt(s, 8) : null;
}

/** The rwx bits every one of `modes` has (special bits never: they are not offered for several items at once). */
export const sharedBits = (modes: (number | null)[]): number => modes.reduce<number>((acc, m) => acc & (m ?? 0), 0o777) & 0o777;

/** The rwx bits the person changed from `start` to `target`. */
export const changedBits = (start: number, target: number): number => (start ^ target) & 0o777;

/** `mode` with only the `changed` bits taken from `target` (its other bits, special ones included, kept). */
export const withChange = (mode: number, target: number, changed: number): number => ((mode & 0o7777) & ~changed) | (target & changed);

// ---- delete ----

/** The selection kept to what is shown (a filter hides items: an action never reaches what the person cannot see). */
export function pruneSelection(sel: Selection, shown: string[]): Selection {
  const picked = sel.picked.filter((n) => shown.includes(n));
  return { picked, anchor: sel.anchor && shown.includes(sel.anchor) ? sel.anchor : picked[0] ?? null };
}

/** The names a delete lists: the first `max`, and how many more. */
export function listNames(names: string[], max = 5): { shown: string[]; more: number } {
  return { shown: names.slice(0, max), more: Math.max(0, names.length - max) };
}

// ---- paths ----

const windowsPath = (p: string) => /^[A-Za-z]:/.test(p) || p.startsWith("\\\\");

/** `name` inside `dir`: `/` on a host; on this computer the folder's own separator, and a drive (`""`) is its letter. */
export function joinPath(end: PaneEnd, dir: string, name: string): string {
  if (end !== null) return `${dir.replace(/\/+$/, "")}/${name}`;
  if (dir === "") return name; // the list of drives: `C:\`
  if (windowsPath(dir)) return `${dir.replace(/[\\/]+$/, "")}\\${name}`;
  return `${dir.replace(/\/+$/, "")}/${name}`;
}

/** The folder above `path`; on Windows the drive's root goes up to the list of drives (`""`). */
export function parentPath(end: PaneEnd, path: string): string {
  if (end !== null) return path.replace(/\/[^/]*\/?$/, "") || "/";
  if (windowsPath(path)) {
    const t = path.replace(/[\\/]+$/, "");
    if (/^[A-Za-z]:$/.test(t)) return "";
    const up = t.replace(/[\\/][^\\/]*$/, "");
    return /^[A-Za-z]:$/.test(up) ? `${up}\\` : up;
  }
  return path.replace(/\/[^/]*\/?$/, "") || "/";
}

/** The last part of a path on either end. */
export const baseName = (path: string): string => path.replace(/[\\/]+$/, "").split(/[\\/]/).pop() ?? path;

// ---- text ----

export function formatSize(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${units[i]}`;
}

/** `2026-11-06 14:03`, local time; nothing for an unknown time. */
export function formatTime(secs: number): string {
  if (!secs) return "";
  const d = new Date(secs * 1000);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}
