// The person's own terminal themes: checking them, creating / renaming / deleting them, and reading
// and writing colour-scheme files (Windows Terminal JSON, iTerm2 `.itermcolors`, base16 YAML or JSON).
//
// Files are data only: each format is read by a small, strict reader here (no YAML or XML library,
// no entity expansion, nothing evaluated), size-capped, and every colour must be a plain hex value.
// A file that does not match is refused with a message that says what is wrong.
import { t } from "./i18n";
import { CUSTOM_PREFIX, type TerminalTheme } from "./terminal-themes";

/** Larger files are refused before they are read. */
export const MAX_THEME_FILE = 256 * 1024;
/** Themes a person can keep (also what an import or a damaged state file is cut to). */
export const MAX_CUSTOM_THEMES = 100;
const MAX_LABEL = 80;

export class ThemeFileError extends Error {}
const fail = (message: string): never => {
  throw new ThemeFileError(message);
};

// ---------- colours ----------

/** `#rgb` / `#rrggbb` (any case, `#` optional when `bare`) → `#rrggbb`, else null. */
export function normalizeHex(v: unknown, bare = false): string | null {
  if (typeof v !== "string") return null;
  const m = v.trim().match(bare ? /^#?([0-9a-f]{3}|[0-9a-f]{6})$/i : /^#([0-9a-f]{3}|[0-9a-f]{6})$/i);
  if (!m) return null;
  const h = m[1].toLowerCase();
  return `#${h.length === 3 ? [...h].map((c) => c + c).join("") : h}`;
}

const channels = (hex: string) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);

/** `a` moved `amount` (0–1) of the way to `b`, both `#rrggbb`. */
export function mix(a: string, b: string, amount: number): string {
  const [x, y] = [channels(a), channels(b)];
  return `#${x.map((c, i) => Math.round((c + (y[i] - c) * amount) * 255).toString(16).padStart(2, "0")).join("")}`;
}

/** WCAG relative luminance of `#rrggbb`. */
export function luminance(hex: string): number {
  const [r, g, b] = channels(hex).map((c) => (c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4));
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

/** WCAG contrast ratio of two `#rrggbb` colours (1–21). */
export function contrastRatio(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p);
  return (x + 0.05) / (y + 0.05);
}

/** A theme's label: one line, trimmed, at most 80 characters. */
export const cleanLabel = (v: unknown, fallback: string) => {
  const s = typeof v === "string" ? [...v.replace(/[\u0000-\u001f\u007f]/g, " ").trim()].slice(0, MAX_LABEL).join("").trim() : "";
  return s || fallback;
};

// ---------- the stored list ----------

const CUSTOM_ID = /^custom:[a-z0-9]{1,32}$/;

/** One stored theme, or null when its shape is wrong (a damaged entry is dropped, never repaired into something else). */
export function checkTheme(v: unknown): TerminalTheme | null {
  if (!v || typeof v !== "object" || Array.isArray(v)) return null;
  const o = v as Record<string, unknown>;
  if (typeof o.id !== "string" || !CUSTOM_ID.test(o.id) || typeof o.dark !== "boolean" || !Array.isArray(o.colors) || o.colors.length !== 19) return null;
  const colors = o.colors.map((c) => normalizeHex(c));
  if (colors.some((c) => c === null)) return null;
  const selection = o.selection === undefined ? undefined : normalizeHex(o.selection);
  if (selection === null) return null;
  return { id: o.id, label: cleanLabel(o.label, t("Untitled theme")), dark: o.dark, colors: colors as TerminalTheme["colors"], ...(selection ? { selection } : {}) };
}

/** The stored themes that are well formed, ids unique, at most {@link MAX_CUSTOM_THEMES}. */
export function checkThemes(v: unknown): TerminalTheme[] {
  if (!Array.isArray(v)) return [];
  const seen = new Set<string>();
  const out: TerminalTheme[] = [];
  for (const item of v) {
    const th = checkTheme(item);
    if (th && !seen.has(th.id) && out.length < MAX_CUSTOM_THEMES) {
      seen.add(th.id);
      out.push(th);
    }
  }
  return out;
}

/** A fresh id that `taken` does not have. */
export function newThemeId(taken: readonly TerminalTheme[], random: () => string = () => Math.random().toString(36).slice(2, 10)): string {
  for (;;) {
    const id = `${CUSTOM_PREFIX}${random().toLowerCase().replace(/[^a-z0-9]/g, "").slice(0, 32) || "t"}`;
    if (!taken.some((x) => x.id === id)) return id;
  }
}

/** `base` when free, else `base 2`, `base 3`… */
export function uniqueLabel(base: string, taken: readonly TerminalTheme[]): string {
  const used = new Set(taken.map((x) => x.label.toLowerCase()));
  if (!used.has(base.toLowerCase())) return base;
  for (let n = 2; ; n++) if (!used.has(`${base} ${n}`.toLowerCase())) return `${base} ${n}`;
}

// ---------- create, duplicate, rename, delete ----------

/** A copy of `from` (any theme: built-in, plugin or own) as a new own theme, added at the end. */
export function duplicateTheme(list: readonly TerminalTheme[], from: TerminalTheme, label?: string): { list: TerminalTheme[]; theme: TerminalTheme } {
  if (list.length >= MAX_CUSTOM_THEMES) fail(t("You can keep up to {n} themes; delete one first.", { n: MAX_CUSTOM_THEMES }));
  // Plugin themes are only checked for their count of colours: each must be a plain hex value here.
  const colors = from.colors.map((c) => normalizeHex(c));
  const selection = from.selection === undefined ? undefined : normalizeHex(from.selection);
  if (colors.length !== 19 || colors.some((c) => !c) || selection === null) fail(t("{name} has colours that are not hex values like #1e1e2e, so it cannot be copied.", { name: from.label }));
  const theme: TerminalTheme = {
    id: newThemeId(list),
    label: uniqueLabel(cleanLabel(label ?? t("{name} copy", { name: from.label }), t("Untitled theme")), list),
    dark: from.dark,
    colors: colors as TerminalTheme["colors"],
    ...(selection ? { selection } : {}),
  };
  return { list: [...list, theme], theme };
}

/** Replaces the own theme with `theme.id` (checked first: a malformed edit is refused). */
export function updateTheme(list: readonly TerminalTheme[], theme: TerminalTheme): TerminalTheme[] {
  const ok = checkTheme(theme);
  if (!ok) fail(t("That theme has a colour that is not a hex value like #1e1e2e."));
  if (!list.some((x) => x.id === ok!.id)) fail(t("That theme no longer exists."));
  return list.map((x) => (x.id === ok!.id ? ok! : x));
}

export const renameTheme = (list: readonly TerminalTheme[], id: string, label: string) => {
  const cur = list.find((x) => x.id === id);
  if (!cur) return fail(t("That theme no longer exists."));
  const name = cleanLabel(label, cur.label);
  return updateTheme(list, { ...cur, label: name.toLowerCase() === cur.label.toLowerCase() ? name : uniqueLabel(name, list.filter((x) => x.id !== id)) });
};

/** `incoming` (a settings import) joined to `local`: a theme with a local id replaces it in place, others
 *  are added after, up to {@link MAX_CUSTOM_THEMES}. Local themes are never dropped. */
export function mergeThemes(local: readonly TerminalTheme[], incoming: readonly TerminalTheme[]): TerminalTheme[] {
  const byId = new Map(incoming.map((x) => [x.id, x]));
  const out = local.map((x) => byId.get(x.id) ?? x);
  for (const th of incoming) if (!local.some((x) => x.id === th.id) && out.length < MAX_CUSTOM_THEMES) out.push(th);
  return out;
}

export const deleteTheme =(list: readonly TerminalTheme[], id: string) => list.filter((x) => x.id !== id);

/** Adds imported themes as new own themes (fresh ids, unique names). */
export function addImported(list: readonly TerminalTheme[], themes: readonly Omit<TerminalTheme, "id">[]): { list: TerminalTheme[]; added: TerminalTheme[] } {
  if (list.length + themes.length > MAX_CUSTOM_THEMES) fail(t("You can keep up to {n} themes; delete one first.", { n: MAX_CUSTOM_THEMES }));
  let next = [...list];
  const added: TerminalTheme[] = [];
  for (const th of themes) {
    const theme = { ...th, id: newThemeId(next), label: uniqueLabel(th.label, next) } as TerminalTheme;
    next = [...next, theme];
    added.push(theme);
  }
  return { list: next, added };
}

// ---------- importing ----------

type Imported = Omit<TerminalTheme, "id">;

/** Builds a theme from named colours; `need` names the file's own keys in the message. */
function fromParts(label: string, parts: { background: string | null; foreground: string | null; cursor?: string | null; selection?: string | null; ansi: (string | null)[] }, keyOf: (slot: string) => string): Imported {
  const missing: string[] = [];
  if (!parts.background) missing.push(keyOf("background"));
  if (!parts.foreground) missing.push(keyOf("foreground"));
  parts.ansi.forEach((c, i) => !c && missing.push(keyOf(`ansi${i}`)));
  if (missing.length) fail(t("The file has no valid colour for {keys}.", { keys: missing.slice(0, 6).join(", ") + (missing.length > 6 ? "…" : "") }));
  const bg = parts.background!;
  return {
    label: cleanLabel(label, t("Imported theme")),
    dark: luminance(bg) < 0.4,
    colors: [bg, parts.foreground!, parts.cursor || parts.foreground!, ...(parts.ansi as string[])] as TerminalTheme["colors"],
    ...(parts.selection ? { selection: parts.selection } : {}),
  };
}

/** Windows Terminal's names for ANSI 0–15. */
const WT_ANSI = ["black", "red", "green", "yellow", "blue", "purple", "cyan", "white", "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue", "brightPurple", "brightCyan", "brightWhite"];

function readWindowsTerminal(o: Record<string, unknown>): Imported {
  // A colour present but malformed is an error, never quietly replaced.
  const color = (k: string, required: boolean) => {
    if (o[k] === undefined && !required) return undefined;
    const c = normalizeHex(o[k]);
    if (o[k] !== undefined && !c) fail(t("{key} is not a hex colour like #1e1e2e.", { key: k }));
    return c;
  };
  const keyOf = (slot: string) => (slot.startsWith("ansi") ? WT_ANSI[Number(slot.slice(4))] : slot);
  return fromParts(typeof o.name === "string" ? o.name : "", {
    background: color("background", true) ?? null,
    foreground: color("foreground", true) ?? null,
    cursor: color("cursorColor", false),
    selection: color("selectionBackground", false),
    ansi: WT_ANSI.map((k) => color(k, true) ?? null),
  }, keyOf);
}

const BASE16 = ["00", "01", "02", "03", "04", "05", "06", "07", "08", "09", "0A", "0B", "0C", "0D", "0E", "0F"].map((n) => `base${n}`);
/** base16-shell's mapping: ANSI slot → base16 slot. */
const BASE16_ANSI = ["base00", "base08", "base0B", "base0A", "base0D", "base0E", "base0C", "base05", "base03", "base08", "base0B", "base0A", "base0D", "base0E", "base0C", "base07"];

function readBase16(o: Record<string, unknown>): Imported {
  const palette = o.palette && typeof o.palette === "object" && !Array.isArray(o.palette) ? (o.palette as Record<string, unknown>) : o;
  const get = (k: string) => {
    const key = Object.keys(palette).find((x) => x.toLowerCase() === k.toLowerCase());
    const v = key === undefined ? undefined : palette[key];
    if (v === undefined) return null;
    const c = normalizeHex(v, true);
    if (!c) fail(t("{key} is not a hex colour like #1e1e2e.", { key: k }));
    return c;
  };
  const missing = BASE16.filter((k) => !get(k));
  if (missing.length) fail(t("The file has no valid colour for {keys}.", { keys: missing.join(", ") }));
  const label = typeof o.name === "string" ? o.name : typeof o.scheme === "string" ? o.scheme : "";
  return fromParts(label, { background: get("base00"), foreground: get("base05"), cursor: get("base05"), selection: get("base02"), ansi: BASE16_ANSI.map(get) }, (s) => s);
}

/** A small YAML subset: `key: value` lines, comments, quoted strings and one level of nested maps
 *  (base16's `palette:`). Anything else (lists, anchors, tags, multi-line values) is refused. */
export function readSimpleYaml(text: string): Record<string, unknown> {
  const out = Object.create(null) as Record<string, unknown>;
  let nested: Record<string, unknown> | null = null;
  let nestedIndent = -1;
  const lines = text.replace(/^﻿/, "").split(/\r?\n/);
  lines.forEach((raw, i) => {
    if (/^\s*(#.*)?$/.test(raw) || raw.trim() === "---") return;
    // Greedy and anchored: no pattern here backtracks over a long line.
    const m = raw.match(/^([ \t]*)([A-Za-z0-9_-]+)[ \t]*:(.*)$/);
    if (!m || (m[3] && !/^[ \t]/.test(m[3])) || UNSAFE_KEYS.has(m[2])) fail(t("Line {n} is not a simple key: value line.", { n: i + 1 }));
    const [, indent, key, after] = m!;
    const value = parseScalar(after.trim(), i + 1);
    if (indent.length === 0) {
      nested = null;
      if (value === null) {
        nested = Object.create(null) as Record<string, unknown>;
        out[key] = nested;
        nestedIndent = -1;
      } else out[key] = value;
      return;
    }
    if (!nested) fail(t("Line {n} is not a simple key: value line.", { n: i + 1 }));
    if (nestedIndent === -1) nestedIndent = indent.length;
    if (indent.length !== nestedIndent || value === null) fail(t("Line {n} is not a simple key: value line.", { n: i + 1 }));
    nested![key] = value;
  });
  return out;
}

function parseScalar(rest: string, line: number): string | null {
  if (rest === "" || rest.startsWith("#")) return null;
  const q = rest[0];
  if (q === '"' || q === "'") {
    const end = rest.indexOf(q, 1);
    if (end < 0 || !/^\s*(#.*)?$/.test(rest.slice(end + 1))) fail(t("Line {n} is not a simple key: value line.", { n: line }));
    return rest.slice(1, end);
  }
  if (/^[&*!|>[{-]/.test(rest)) fail(t("Line {n} is not a simple key: value line.", { n: line }));
  // A comment starts at a `#` after a space; found with a one-character search (no backtracking).
  const comment = rest.search(/\s#/);
  return (comment < 0 ? rest : rest.slice(0, comment)).trim();
}

/** Keys that would reach an object's prototype; never valid in a colour scheme. */
const UNSAFE_KEYS = new Set(["__proto__", "prototype", "constructor"]);

/** `text` without `<!-- … -->` comments (an unclosed one is refused). Done with indexOf: a lazy regex
 *  over many unclosed comments would take quadratic time. */
function stripComments(text: string): string {
  let out = "";
  let i = 0;
  for (;;) {
    const a = text.indexOf("<!--", i);
    if (a < 0) return out + text.slice(i);
    const b = text.indexOf("-->", a + 4);
    if (b < 0) fail(t("The file is not an iTerm2 colour preset (.itermcolors)."));
    out += text.slice(i, a);
    i = b + 3;
  }
}

/** A strict reader for the plist subset `.itermcolors` files use: `dict`, `key`, `real`, `integer`,
 *  `string`. A DOCTYPE with an internal subset (entities) or any other element is refused. */
export function readPlist(text: string): Record<string, unknown> {
  let s = text.replace(/^﻿/, "");
  // `[^<>]*`: a tag never runs past the next `<`, so each scan stays linear.
  s = stripComments(s).replace(/<\?xml[^<>]*\?>/g, "");
  const doctype = s.match(/<!DOCTYPE[^<>]*>/i);
  if (doctype) {
    if (doctype[0].includes("[")) fail(t("The file declares XML entities, which are not allowed."));
    s = s.replace(doctype[0], "");
  }
  if (/<!/.test(s)) fail(t("The file declares XML entities, which are not allowed."));
  const tokens = [...s.matchAll(/<(\/?)([A-Za-z]+)(\s[^<>]*?)?(\/?)>|([^<]+)/g)];
  let pos = 0;
  const bad = (): never => fail(t("The file is not an iTerm2 colour preset (.itermcolors)."));
  const skipSpace = () => {
    while (pos < tokens.length && tokens[pos][5] !== undefined && !tokens[pos][5].trim()) pos++;
  };
  const open = (name?: string) => {
    skipSpace();
    const tk = tokens[pos];
    if (!tk || tk[5] !== undefined || tk[1] || (name && tk[2] !== name)) bad();
    pos++;
    return { name: tk[2], empty: tk[4] === "/" };
  };
  const close = (name: string) => {
    skipSpace();
    const tk = tokens[pos];
    if (!tk || tk[1] !== "/" || tk[2] !== name) bad();
    pos++;
  };
  const textUntil = (name: string) => {
    const tk = tokens[pos];
    let v = "";
    if (tk && tk[5] !== undefined) {
      v = tk[5];
      pos++;
    }
    close(name);
    return v.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&quot;/g, '"').replace(/&apos;/g, "'").replace(/&amp;/g, "&").trim();
  };
  const value = (depth: number): unknown => {
    const el = open();
    if (el.name === "dict") {
      if (depth > 3) bad();
      const out = Object.create(null) as Record<string, unknown>;
      if (el.empty) return out;
      for (;;) {
        skipSpace();
        if (tokens[pos]?.[1] === "/" && tokens[pos][2] === "dict") {
          pos++;
          return out;
        }
        open("key");
        const k = textUntil("key");
        if (UNSAFE_KEYS.has(k)) bad();
        out[k] = value(depth + 1);
      }
    }
    if (el.empty) bad();
    if (el.name === "real" || el.name === "integer") {
      // A plain decimal only (Number() would read "" as 0 and accept hex or Infinity).
      const raw = textUntil(el.name);
      if (!/^[+-]?(\d+(\.\d*)?|\.\d+)([eE][+-]?\d+)?$/.test(raw)) bad();
      return Number(raw);
    }
    if (el.name === "string") return textUntil("string");
    return bad();
  };
  open("plist");
  const root = value(0);
  close("plist");
  skipSpace();
  if (pos !== tokens.length || !root || typeof root !== "object") bad();
  return root as Record<string, unknown>;
}

function readIterm(o: Record<string, unknown>, name: string): Imported {
  const color = (k: string) => {
    const d = o[k];
    if (d === undefined) return null;
    const c = d as Record<string, unknown>;
    const parts = ["Red Component", "Green Component", "Blue Component"].map((p) => c?.[p]);
    if (!d || typeof d !== "object" || parts.some((p) => typeof p !== "number" || p < 0 || p > 1)) fail(t("{key} is not a colour.", { key: k }));
    return `#${(parts as number[]).map((p) => Math.round(p * 255).toString(16).padStart(2, "0")).join("")}`;
  };
  return fromParts(name, {
    background: color("Background Color"),
    foreground: color("Foreground Color"),
    cursor: color("Cursor Color"),
    selection: color("Selection Color"),
    ansi: Array.from({ length: 16 }, (_, i) => color(`Ansi ${i} Color`)),
  }, (s) => (s.startsWith("ansi") ? `Ansi ${s.slice(4)} Color` : `${s[0].toUpperCase()}${s.slice(1)} Color`));
}

const baseName = (file: string) => file.replace(/^.*[\\/]/, "").replace(/\.[^.]*$/, "");

/** Reads a colour-scheme file into one or more themes (not yet stored). The format is told by the
 *  content, so a renamed file still reads; `fileName` names an iTerm2 preset (it has no name inside). */
export function importThemeFile(text: string, fileName = ""): Imported[] {
  if (text.length > MAX_THEME_FILE) fail(t("The file is too large for a colour scheme."));
  const body = text.replace(/^﻿/, "").trim();
  if (!body) fail(t("The file is empty."));
  if (body.startsWith("<")) return [readIterm(readPlist(body), baseName(fileName))];
  if (body.startsWith("{") || body.startsWith("[")) {
    let data: unknown;
    try {
      data = JSON.parse(body);
    } catch {
      return fail(t("The file is not valid JSON."));
    }
    const isObj = (v: unknown): v is Record<string, unknown> => !!v && typeof v === "object" && !Array.isArray(v);
    const one = (v: unknown): Imported => {
      if (!isObj(v)) return fail(t("The file is not a colour scheme."));
      if ("base00" in v || "palette" in v) return readBase16(v);
      if ("background" in v || "foreground" in v || "black" in v) return readWindowsTerminal(v);
      return fail(t("The file is not a Windows Terminal, iTerm2 or base16 colour scheme."));
    };
    // Windows Terminal settings hold `schemes: [...]`; an exported list is an array.
    const list = Array.isArray(data) ? data : isObj(data) && Array.isArray(data.schemes) ? data.schemes : [data];
    if (!list.length) fail(t("The file has no colour schemes."));
    if (list.length > MAX_CUSTOM_THEMES) fail(t("You can keep up to {n} themes; delete one first.", { n: MAX_CUSTOM_THEMES }));
    return list.map(one);
  }
  const yaml = readSimpleYaml(body);
  if (!("palette" in yaml) && !Object.keys(yaml).some((k) => k.toLowerCase() === "base00")) fail(t("The file is not a Windows Terminal, iTerm2 or base16 colour scheme."));
  return [readBase16(yaml)];
}

// ---------- exporting ----------

/** A Windows Terminal colour scheme (`schemes` entry) for a theme. */
export function toWindowsTerminal(theme: TerminalTheme): Record<string, string> {
  const [background, foreground, cursor, ...ansi] = theme.colors;
  const out: Record<string, string> = { name: theme.label, background, foreground, cursorColor: cursor };
  // Windows Terminal draws a solid selection: without one of its own, a quarter of the way to the text colour.
  out.selectionBackground = theme.selection ?? mix(background, foreground, 0.25);
  WT_ANSI.forEach((k, i) => (out[k] = ansi[i]));
  return out;
}

/** A file name for an exported theme. */
export const exportFileName = (theme: TerminalTheme) => `${theme.label.replace(/[^\p{L}\p{N} ._-]+/gu, "").trim() || "theme"}.json`;
