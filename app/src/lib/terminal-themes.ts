// Terminal colour themes (Settings → Terminal → Colour theme). "default" follows the app's
// dark / light theme. The chosen palette is also sent to the daemon, which answers agents'
// colour queries with it (see `set_palette` in crates/kelpied/src/vt.rs).
//
// Three sources, one list: the built-in themes below, themes added by plugins (lib/plugins.ts) and
// the person's own themes (Settings → Terminal → Edit themes, kept in the app settings as
// `customThemes`, see lib/theme-io.ts).
import { createSignal } from "solid-js";
import type { ITheme } from "@xterm/xterm";

export interface TerminalTheme {
  id: string;
  label: string;
  dark: boolean;
  /** background, foreground, cursor, then ANSI 0–15. */
  colors: [string, string, string, ...string[]];
  /** Selection background; absent: a translucent white (dark) or black (light) wash. */
  selection?: string;
}

/** `colors`: background, foreground, cursor, selection, then ANSI 0–15 ("-" for no selection colour). */
const def = (id: string, label: string, dark: boolean, colors: string): TerminalTheme => {
  const [bg, fg, cursor, selection, ...ansi] = colors.split(" ");
  return { id, label, dark, colors: [bg, fg, cursor, ...ansi], ...(selection !== "-" ? { selection } : {}) };
};

export const TERMINAL_THEMES: TerminalTheme[] = [
  def("dracula", "Dracula", true, "#282a36 #f8f8f2 #f8f8f2 - #21222c #ff5555 #50fa7b #f1fa8c #bd93f9 #ff79c6 #8be9fd #f8f8f2 #6272a4 #ff6e6e #69ff94 #ffffa5 #d6acff #ff92df #a4ffff #ffffff"),
  def("nord", "Nord", true, "#2e3440 #d8dee9 #d8dee9 - #3b4252 #bf616a #a3be8c #ebcb8b #81a1c1 #b48ead #88c0d0 #e5e9f0 #4c566a #bf616a #a3be8c #ebcb8b #81a1c1 #b48ead #8fbcbb #eceff4"),
  def("one-dark", "One Dark", true, "#282c34 #abb2bf #528bff - #282c34 #e06c75 #98c379 #e5c07b #61afef #c678dd #56b6c2 #abb2bf #5c6370 #e06c75 #98c379 #e5c07b #61afef #c678dd #56b6c2 #ffffff"),
  def("tokyo-night", "Tokyo Night", true, "#1a1b26 #c0caf5 #c0caf5 - #15161e #f7768e #9ece6a #e0af68 #7aa2f7 #bb9af7 #7dcfff #a9b1d6 #414868 #f7768e #9ece6a #e0af68 #7aa2f7 #bb9af7 #7dcfff #c0caf5"),
  def("catppuccin-mocha", "Catppuccin Mocha", true, "#1e1e2e #cdd6f4 #f5e0dc - #45475a #f38ba8 #a6e3a1 #f9e2af #89b4fa #f5c2e7 #94e2d5 #bac2de #585b70 #f38ba8 #a6e3a1 #f9e2af #89b4fa #f5c2e7 #94e2d5 #a6adc8"),
  def("gruvbox-dark", "Gruvbox Dark", true, "#282828 #ebdbb2 #ebdbb2 - #282828 #cc241d #98971a #d79921 #458588 #b16286 #689d6a #a89984 #928374 #fb4934 #b8bb26 #fabd2f #83a598 #d3869b #8ec07c #ebdbb2"),
  def("solarized-dark", "Solarized Dark", true, "#002b36 #839496 #93a1a1 - #073642 #dc322f #859900 #b58900 #268bd2 #d33682 #2aa198 #eee8d5 #002b36 #cb4b16 #586e75 #657b83 #839496 #6c71c4 #93a1a1 #fdf6e3"),
  def("solarized-light", "Solarized Light", false, "#fdf6e3 #657b83 #586e75 - #073642 #dc322f #859900 #b58900 #268bd2 #d33682 #2aa198 #eee8d5 #002b36 #cb4b16 #586e75 #657b83 #839496 #6c71c4 #93a1a1 #fdf6e3"),
  def("github-light", "GitHub Light", false, "#ffffff #1f2328 #0969da - #24292f #cf222e #116329 #4d2d00 #0969da #8250df #1b7c83 #6e7781 #57606a #a40e26 #1a7f37 #633c01 #218bff #a475f9 #3192aa #8c959f"),
  // Base16 Monokai (Wimer Hazenberg), mapped the base16-shell way.
  def("monokai", "Monokai", true, "#272822 #f8f8f2 #f8f8f2 #49483e #272822 #f92672 #a6e22e #f4bf75 #66d9ef #ae81ff #a1efe4 #f8f8f2 #75715e #f92672 #a6e22e #f4bf75 #66d9ef #ae81ff #a1efe4 #f9f8f5"),
  // Base16 Material Darker (Nate Peterson), mapped the base16-shell way.
  def("material-dark", "Material Dark", true, "#212121 #eeffff #eeffff #353535 #212121 #f07178 #c3e88d #ffcb6b #82aaff #c792ea #89ddff #eeffff #4a4a4a #f07178 #c3e88d #ffcb6b #82aaff #c792ea #89ddff #ffffff"),
  // Ayu (ayu-theme/ayu-colors, built values of the `ayu` 9.1 package); translucent selections are blended over the
  // background. Ayu Dark's bright black is #0a0000 upstream, invisible as dim text: the earlier Ayu grey is kept for it.
  def("ayu-dark", "Ayu Dark", true, "#10141c #bfbdb6 #e6b450 #193155 #0a0000 #e6495a #97c142 #e89d37 #17acf2 #c385fe #84ceb5 #ffffff #686868 #f07178 #aad94c #ffb454 #59c2ff #d2a6ff #95e6cb #ffffff"),
  def("ayu-light", "Ayu Light", false, "#fcfcfc #5c6166 #f29718 #d7e4f6 #86878c #f07171 #86b300 #eba400 #22a4e6 #a37acc #4cbf99 #adaeb1 #939498 #f07171 #86b300 #eba400 #22a4e6 #a37acc #4cbf99 #c5c5c8"),
  // Rosé Pine: rose-pine/alacritty.
  def("rose-pine", "Rosé Pine", true, "#191724 #e0def4 #524f67 #403d52 #26233a #eb6f92 #31748f #f6c177 #9ccfd8 #c4a7e7 #ebbcba #e0def4 #6e6a86 #eb6f92 #31748f #f6c177 #9ccfd8 #c4a7e7 #ebbcba #e0def4"),
  def("rose-pine-dawn", "Rosé Pine Dawn", false, "#faf4ed #575279 #cecacd #dfdad9 #f2e9e1 #b4637a #286983 #ea9d34 #56949f #907aa9 #d7827e #575279 #9893a5 #b4637a #286983 #ea9d34 #56949f #907aa9 #d7827e #575279"),
  // Everforest dark, medium contrast (sainnhe's terminal colours; selection: the theme's bg_visual).
  def("everforest-dark", "Everforest Dark", true, "#2d353b #d3c6aa #d3c6aa #543a48 #475258 #e67e80 #a7c080 #dbbc7f #7fbbb3 #d699b6 #83c092 #d3c6aa #475258 #e67e80 #a7c080 #dbbc7f #7fbbb3 #d699b6 #83c092 #d3c6aa"),
  // Kanagawa Wave: rebelot/kanagawa.nvim extras.
  def("kanagawa", "Kanagawa", true, "#1f1f28 #dcd7ba #c8c093 #2d4f67 #090618 #c34043 #76946a #c0a36e #7e9cd8 #957fb8 #6a9589 #c8c093 #727169 #e82424 #98bb6c #e6c384 #7fb4ca #938aa9 #7aa89f #dcd7ba"),
  // Night Owl: sdras/night-owl-vscode-theme (terminal selection #1b90dd at 30%, blended).
  def("night-owl", "Night Owl", true, "#011627 #d6deeb #80a4c2 #093b5e #011627 #ef5350 #22da6e #c5e478 #82aaff #c792ea #21c7a8 #ffffff #575656 #ef5350 #22da6e #ffeb95 #82aaff #c792ea #7fdbca #ffffff"),
];

export const ANSI_NAMES = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"] as const;

/** Themes added by plugins (lib/plugins.ts). */
const [pluginThemes, setPluginThemes] = createSignal<TerminalTheme[]>([]);
export { setPluginThemes };

/** The person's own themes (`settings.customThemes`, already checked by lib/theme-io.ts). */
const [customThemes, setCustomThemes] = createSignal<TerminalTheme[]>([]);
export { customThemes, setCustomThemes };

export const terminalThemes = () => [...TERMINAL_THEMES, ...pluginThemes(), ...customThemes()];

export const findTerminalTheme = (id: string | undefined) => (id ? terminalThemes().find((x) => x.id === id) : undefined);

/** Ids of the person's own themes; never a built-in or plugin id (plugin ids have no colon). */
export const CUSTOM_PREFIX = "custom:";
export const isCustomTheme = (id: string | undefined) => !!id && id.startsWith(CUSTOM_PREFIX);

/** xterm.js theme for a library entry. */
export function toXtermTheme(theme: TerminalTheme): ITheme {
  const [background, foreground, cursor, ...ansi] = theme.colors;
  const out: Record<string, string> = { background, foreground, cursor, cursorAccent: background };
  ANSI_NAMES.forEach((n, i) => {
    out[n] = ansi[i];
    out[`bright${n[0].toUpperCase()}${n.slice(1)}`] = ansi[i + 8];
  });
  const wash = theme.dark ? "rgba(255,255,255,0.22)" : "rgba(0,0,0,0.16)";
  return {
    ...out,
    selectionBackground: theme.selection ?? wash,
    selectionInactiveBackground: theme.selection ? `${theme.selection}b3` : theme.dark ? "rgba(255,255,255,0.12)" : "rgba(0,0,0,0.08)",
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
