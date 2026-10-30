import { formatRelative, t } from "./i18n";

export const isMac = /Mac|iPhone|iPad/.test(navigator.platform) || navigator.userAgent.includes("Mac OS");
export const isWindows = /Win/.test(navigator.platform);

/** Human label for a chord like "mod+shift+p". */
export function chordLabel(chord: string): string {
  return chord
    .split("+")
    .filter(Boolean)
    .map((k) => {
      switch (k) {
        case "mod": return isMac ? "⌘" : "Ctrl";
        case "ctrl": return isMac ? "⌃" : "Ctrl";
        case "shift": return isMac ? "⇧" : "Shift";
        case "alt": return isMac ? "⌥" : "Alt";
        case "arrowleft": return "←";
        case "arrowright": return "→";
        case "arrowup": return "↑";
        case "arrowdown": return "↓";
        case "enter": return isMac ? "↩" : "Enter";
        case "numpadadd": return "Num+";
        case "numpadsubtract": return "Num-";
        case "numpadmultiply": return "Num*";
        case "numpaddivide": return "Num/";
        default: return k.length === 1 ? k.toUpperCase() : k[0].toUpperCase() + k.slice(1);
      }
    })
    .join(isMac ? "" : "+");
}

/** Comparable form of a folder path: forward slashes, no trailing slash, case-folded. */
export const normPath = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();

export function basename(p: string): string {
  const parts = p.replace(/\\/g, "/").replace(/\/+$/, "").split("/");
  return parts[parts.length - 1] || p;
}

export function joinPath(root: string, rel: string): string {
  const sep = root.includes("\\") ? "\\" : "/";
  return rel ? root.replace(/[\\/]+$/, "") + sep + rel.replace(/\//g, sep) : root;
}

export const relTime = (ms: number) => formatRelative(ms);

/** Compact age: "now", "5m", "3h", "2d", "4w" (Vietnamese: "vừa xong", "5 ph", …). */
export function relTimeShort(ms: number, now = Date.now()): string {
  const d = Math.max(0, now - ms);
  if (d < 60_000) return t("now");
  if (d < 3_600_000) return t("{n}m", { n: Math.floor(d / 60_000) });
  if (d < 86_400_000) return t("{n}h", { n: Math.floor(d / 3_600_000) });
  if (d < 7 * 86_400_000) return t("{n}d", { n: Math.floor(d / 86_400_000) });
  return t("{n}w", { n: Math.floor(d / (7 * 86_400_000)) });
}

export const uid = () => Math.random().toString(36).slice(2, 10) + Date.now().toString(36).slice(-4);
