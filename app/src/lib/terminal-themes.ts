// Terminal colour themes (Settings → Terminal → Colour theme). "default" follows the app's
// dark / light theme. The chosen palette is also sent to the daemon, which answers agents'
// colour queries with it (see `set_palette` in crates/kelpied/src/vt.rs).
import { createSignal } from "solid-js";
import type { ITheme } from "@xterm/xterm";

export interface TerminalTheme {
  id: string;
  label: string;
  dark: boolean;
  /** background, foreground, cursor, then ANSI 0–15. */
  colors: [string, string, string, ...string[]];
}

const def = (id: string, label: string, dark: boolean, colors: string): TerminalTheme => ({ id, label, dark, colors: colors.split(" ") as TerminalTheme["colors"] });

export const TERMINAL_THEMES: TerminalTheme[] = [
  def("dracula", "Dracula", true, "#282a36 #f8f8f2 #f8f8f2 #21222c #ff5555 #50fa7b #f1fa8c #bd93f9 #ff79c6 #8be9fd #f8f8f2 #6272a4 #ff6e6e #69ff94 #ffffa5 #d6acff #ff92df #a4ffff #ffffff"),
  def("nord", "Nord", true, "#2e3440 #d8dee9 #d8dee9 #3b4252 #bf616a #a3be8c #ebcb8b #81a1c1 #b48ead #88c0d0 #e5e9f0 #4c566a #bf616a #a3be8c #ebcb8b #81a1c1 #b48ead #8fbcbb #eceff4"),
  def("one-dark", "One Dark", true, "#282c34 #abb2bf #528bff #282c34 #e06c75 #98c379 #e5c07b #61afef #c678dd #56b6c2 #abb2bf #5c6370 #e06c75 #98c379 #e5c07b #61afef #c678dd #56b6c2 #ffffff"),
  def("tokyo-night", "Tokyo Night", true, "#1a1b26 #c0caf5 #c0caf5 #15161e #f7768e #9ece6a #e0af68 #7aa2f7 #bb9af7 #7dcfff #a9b1d6 #414868 #f7768e #9ece6a #e0af68 #7aa2f7 #bb9af7 #7dcfff #c0caf5"),
  def("catppuccin-mocha", "Catppuccin Mocha", true, "#1e1e2e #cdd6f4 #f5e0dc #45475a #f38ba8 #a6e3a1 #f9e2af #89b4fa #f5c2e7 #94e2d5 #bac2de #585b70 #f38ba8 #a6e3a1 #f9e2af #89b4fa #f5c2e7 #94e2d5 #a6adc8"),
  def("gruvbox-dark", "Gruvbox Dark", true, "#282828 #ebdbb2 #ebdbb2 #282828 #cc241d #98971a #d79921 #458588 #b16286 #689d6a #a89984 #928374 #fb4934 #b8bb26 #fabd2f #83a598 #d3869b #8ec07c #ebdbb2"),
  def("solarized-dark", "Solarized Dark", true, "#002b36 #839496 #93a1a1 #073642 #dc322f #859900 #b58900 #268bd2 #d33682 #2aa198 #eee8d5 #002b36 #cb4b16 #586e75 #657b83 #839496 #6c71c4 #93a1a1 #fdf6e3"),
  def("solarized-light", "Solarized Light", false, "#fdf6e3 #657b83 #586e75 #073642 #dc322f #859900 #b58900 #268bd2 #d33682 #2aa198 #eee8d5 #002b36 #cb4b16 #586e75 #657b83 #839496 #6c71c4 #93a1a1 #fdf6e3"),
  def("github-light", "GitHub Light", false, "#ffffff #1f2328 #0969da #24292f #cf222e #116329 #4d2d00 #0969da #8250df #1b7c83 #6e7781 #57606a #a40e26 #1a7f37 #633c01 #218bff #a475f9 #3192aa #8c959f"),
];

const NAMES = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"] as const;

/** Themes added by plugins (lib/plugins.ts). */
const [pluginThemes, setPluginThemes] = createSignal<TerminalTheme[]>([]);
export { setPluginThemes };

export const terminalThemes = () => [...TERMINAL_THEMES, ...pluginThemes()];

export const findTerminalTheme = (id: string | undefined) => terminalThemes().find((x) => x.id === id);

/** xterm.js theme for a library entry. */
export function toXtermTheme(theme: TerminalTheme): ITheme {
  const [background, foreground, cursor, ...ansi] = theme.colors;
  const out: Record<string, string> = { background, foreground, cursor, cursorAccent: background };
  NAMES.forEach((n, i) => {
    out[n] = ansi[i];
    out[`bright${n[0].toUpperCase()}${n.slice(1)}`] = ansi[i + 8];
  });
  const sel = theme.dark ? "rgba(255,255,255,0.22)" : "rgba(0,0,0,0.16)";
  return {
    ...out,
    selectionBackground: sel,
    selectionInactiveBackground: theme.dark ? "rgba(255,255,255,0.12)" : "rgba(0,0,0,0.08)",
    scrollbarSliderBackground: theme.dark ? "rgba(255,255,255,0.18)" : "rgba(0,0,0,0.20)",
    scrollbarSliderHoverBackground: theme.dark ? "rgba(255,255,255,0.28)" : "rgba(0,0,0,0.30)",
    scrollbarSliderActiveBackground: theme.dark ? "rgba(255,255,255,0.36)" : "rgba(0,0,0,0.40)",
  };
}

/** Palette for `daemon.set_theme`. */
export function daemonPalette(theme: TerminalTheme) {
  const [background, foreground, cursor, ...ansi] = theme.colors;
  return { background, foreground, cursor, ansi };
}
