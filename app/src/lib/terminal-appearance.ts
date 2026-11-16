// How a terminal pane looks: the app's terminal settings, then its saved host's overrides (Hosts →
// edit → Appearance), then a theme picked for that pane alone (its menu → Appearance), which lasts
// until the pane's session closes.
import { createSignal } from "solid-js";
import type { ITheme } from "@xterm/xterm";
import { terminalThemeFor } from "./theme";
import { findTerminalTheme } from "./terminal-themes";
import type { Settings } from "./types";

export type CursorStyle = "block" | "underline" | "bar";
export const CURSOR_STYLES: readonly CursorStyle[] = ["block", "underline", "bar"];

/** A host's overrides; empty / null fields take the app's setting. Field names match the host
 *  record (lib/ssh-hosts.ts, app/src-tauri/src/ssh_hosts.rs). */
export interface AppearanceOverrides {
  terminalTheme?: string;
  fontFamily?: string;
  fontSize?: number | null;
  lineHeight?: number | null;
  cursorStyle?: string;
  cursorBlink?: boolean | null;
}

/** A saved host's overrides (undefined: not a saved host). */
export function hostAppearance(h: AppearanceOverrides | undefined): AppearanceOverrides | undefined {
  if (!h) return undefined;
  const { terminalTheme, fontFamily, fontSize, lineHeight, cursorStyle, cursorBlink } = h;
  return { terminalTheme, fontFamily, fontSize, lineHeight, cursorStyle, cursorBlink };
}

/** What a pane is drawn with. */
export interface Appearance {
  /** The theme id in use ("default": the app's own palette). */
  themeId: string;
  theme: ITheme;
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  cursorStyle: CursorStyle;
  cursorBlink: boolean;
}

export const FONT_SIZE = { min: 8, max: 28 } as const;
export const LINE_HEIGHT = { min: 1, max: 2 } as const;

const inRange = (v: unknown, r: { min: number; max: number }): v is number => typeof v === "number" && Number.isFinite(v) && v >= r.min && v <= r.max;
export const isCursorStyle = (v: unknown): v is CursorStyle => CURSOR_STYLES.includes(v as CursorStyle);

/** The theme id a pane uses: its own pick, else its host's, else the app's. An id no longer known
 *  (a deleted theme, a removed plugin) is skipped. */
export function themeIdFor(s: Pick<Settings, "terminalTheme">, host?: AppearanceOverrides, pane?: string): string {
  for (const id of [pane, host?.terminalTheme]) if (id && findTerminalTheme(id)) return id;
  return s.terminalTheme || "default";
}

/** A pane's appearance from the app settings, its host's overrides and its own theme pick. */
export function resolveAppearance(s: Settings, host?: AppearanceOverrides, pane?: string): Appearance {
  const themeId = themeIdFor(s, host, pane);
  const appStyle = isCursorStyle(s.cursorStyle) ? s.cursorStyle : "bar";
  return {
    themeId,
    theme: terminalThemeFor(s.theme, themeId),
    fontFamily: host?.fontFamily?.trim() || s.fontFamily,
    fontSize: inRange(host?.fontSize, FONT_SIZE) ? host!.fontSize! : s.fontSize,
    lineHeight: inRange(host?.lineHeight, LINE_HEIGHT) ? host!.lineHeight! : s.lineHeight,
    cursorStyle: isCursorStyle(host?.cursorStyle) ? host!.cursorStyle as CursorStyle : appStyle,
    cursorBlink: typeof host?.cursorBlink === "boolean" ? host.cursorBlink : s.cursorBlink,
  };
}

// ---------- a theme for one pane ----------

/** Theme picked for one pane (by session id) from its menu, kept in memory only. */
const [paneThemes, setPaneThemes] = createSignal<Record<string, string>>({});
export { paneThemes };

export const paneTheme = (sessionId: string): string | undefined => paneThemes()[sessionId];

/** Sets (or with null clears) the pane's own theme. */
export function setPaneTheme(sessionId: string, themeId: string | null) {
  const { [sessionId]: _, ...rest } = paneThemes();
  setPaneThemes(themeId ? { ...rest, [sessionId]: themeId } : rest);
}

/** Forgets picks of sessions that are gone (`alive`: the ids still open) and of themes deleted since. */
export function prunePaneThemes(alive: (id: string) => boolean) {
  const cur = paneThemes();
  const kept = Object.entries(cur).filter(([id, theme]) => alive(id) && !!findTerminalTheme(theme));
  if (kept.length !== Object.keys(cur).length) setPaneThemes(Object.fromEntries(kept));
}
