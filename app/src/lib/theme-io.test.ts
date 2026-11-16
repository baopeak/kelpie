import { describe, expect, it } from "vitest";
import {
  MAX_CUSTOM_THEMES, MAX_THEME_FILE, ThemeFileError, addImported, checkTheme, checkThemes, contrastRatio, deleteTheme, duplicateTheme, importThemeFile, mergeThemes,
  normalizeHex, readPlist, readSimpleYaml, renameTheme, toWindowsTerminal, updateTheme,
} from "./theme-io";
import { TERMINAL_THEMES, findTerminalTheme, setCustomThemes, terminalThemes, type TerminalTheme } from "./terminal-themes";
import { importableSettings } from "./config-transfer";
import { defaultSettings } from "./default-settings";

const WT_KEYS = ["black", "red", "green", "yellow", "blue", "purple", "cyan", "white", "brightBlack", "brightRed", "brightGreen", "brightYellow", "brightBlue", "brightPurple", "brightCyan", "brightWhite"];
const ansi = Array.from({ length: 16 }, (_, i) => `#${(i * 16).toString(16).padStart(2, "0")}0a0b`);

const wtScheme = (over: Record<string, unknown> = {}): Record<string, unknown> => ({
  name: "Campbell",
  background: "#0C0C0C",
  foreground: "#CCCCCC",
  cursorColor: "#FFFFFF",
  selectionBackground: "#FFFFFF",
  ...Object.fromEntries(WT_KEYS.map((k, i) => [k, ansi[i]])),
  ...over,
});

const itermColor = (r: number, g: number, b: number) =>
  `<dict><key>Alpha Component</key><real>1</real><key>Blue Component</key><real>${b}</real><key>Color Space</key><string>sRGB</string><key>Green Component</key><real>${g}</real><key>Red Component</key><real>${r}</real></dict>`;
const iterm = (extra = "", skip?: string) => `<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
${Array.from({ length: 16 }, (_, i) => (skip === `Ansi ${i} Color` ? "" : `  <key>Ansi ${i} Color</key>\n  ${itermColor(i / 15, 0, 1 - i / 15)}`)).join("\n")}
  <key>Background Color</key>
  ${itermColor(0, 0, 0)}
  <key>Foreground Color</key>
  ${itermColor(1, 1, 1)}
  <key>Cursor Color</key>
  ${itermColor(1, 0.5, 0)}
  <key>Selection Color</key>
  ${itermColor(0.2, 0.2, 0.4)}
  <!-- a comment -->
  ${extra}
</dict>
</plist>`;

const base16Yaml = `# a comment
system: "base16"
name: "Monokai"
author: "Wimer Hazenberg (http://www.monokai.nl)"
variant: "dark"
palette:
  base00: "#272822"
  base01: "#383830"
  base02: "#49483e"
  base03: "#75715e"
  base04: "#a59f85"
  base05: "#f8f8f2"
  base06: "#f5f4f1"
  base07: "#f9f8f5"
  base08: "#f92672"
  base09: "#fd971f"
  base0A: "#f4bf75"
  base0B: "#a6e22e"
  base0C: "#a1efe4"
  base0D: "#66d9ef"
  base0E: "#ae81ff"
  base0F: "#cc6633"
`;

/** The older base16 layout: flat keys, bare hex without `#`. */
const base16Legacy = `scheme: "Ocean"
author: "Chris Kempson"
base00: "2b303b"
base01: "343d46"
base02: "4f5b66"
base03: "65737e"
base04: "a7adba"
base05: "c0c5ce"
base06: "dfe1e8"
base07: "eff1f5"
base08: "bf616a"
base09: "d08770"
base0A: "ebcb8b"
base0B: "a3be8c"
base0C: "96b5b4"
base0D: "8fa1b3"
base0E: "b48ead"
base0F: "ab7967"
`;

const fails = (fn: () => unknown, message: RegExp) => {
  expect(fn).toThrow(ThemeFileError);
  expect(fn).toThrow(message);
};

describe("colours", () => {
  it("normalizes hex and refuses anything else", () => {
    expect(normalizeHex("#ABC")).toBe("#aabbcc");
    expect(normalizeHex(" #1E1e2E ")).toBe("#1e1e2e");
    expect(normalizeHex("1e1e2e")).toBeNull();
    expect(normalizeHex("1e1e2e", true)).toBe("#1e1e2e");
    for (const bad of ["#12345", "#gggggg", "rgb(1,2,3)", "red", "#1e1e2e80", "", null, 3, "url(javascript:x)"]) expect(normalizeHex(bad)).toBeNull();
  });

  it("measures WCAG contrast", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 5);
    expect(contrastRatio("#777777", "#777777")).toBe(1);
  });
});

describe("Windows Terminal schemes", () => {
  it("reads a scheme, keeping cursor and selection", () => {
    const [th] = importThemeFile(JSON.stringify(wtScheme()));
    expect(th.label).toBe("Campbell");
    expect(th.dark).toBe(true);
    expect(th.colors).toEqual(["#0c0c0c", "#cccccc", "#ffffff", ...ansi]);
    expect(th.selection).toBe("#ffffff");
  });

  it("reads the schemes of a settings file and a list", () => {
    expect(importThemeFile(JSON.stringify({ schemes: [wtScheme(), wtScheme({ name: "Two" })] })).map((x) => x.label)).toEqual(["Campbell", "Two"]);
    expect(importThemeFile(JSON.stringify([wtScheme()]))).toHaveLength(1);
  });

  it("takes the foreground as the cursor when there is none", () => {
    const { cursorColor: _c, selectionBackground: _s, ...rest } = wtScheme();
    const [th] = importThemeFile(JSON.stringify(rest));
    expect(th.colors[2]).toBe("#cccccc");
    expect(th.selection).toBeUndefined();
  });

  it("refuses missing and malformed colours, naming them", () => {
    const { brightPurple: _p, ...missing } = wtScheme();
    fails(() => importThemeFile(JSON.stringify(missing)), /brightPurple/);
    fails(() => importThemeFile(JSON.stringify(wtScheme({ red: "crimson" }))), /red is not a hex colour/);
    fails(() => importThemeFile(JSON.stringify(wtScheme({ cursorColor: 12 }))), /cursorColor/);
  });

  it("refuses files that are not JSON or not a scheme", () => {
    fails(() => importThemeFile("{ name: 'x', }"), /not valid JSON/);
    fails(() => importThemeFile(JSON.stringify({ profiles: {} })), /not a Windows Terminal, iTerm2 or base16/);
    fails(() => importThemeFile(JSON.stringify({ schemes: [] })), /no colour schemes/);
    fails(() => importThemeFile(JSON.stringify([1])), /not a colour scheme/);
    fails(() => importThemeFile(""), /empty/);
    fails(() => importThemeFile(" ".repeat(MAX_THEME_FILE + 1)), /too large/);
  });

  it("keeps a name to one short line", () => {
    const [th] = importThemeFile(JSON.stringify(wtScheme({ name: `a\nb${"x".repeat(200)}` })));
    expect(th.label).not.toContain("\n");
    expect(th.label.length).toBeLessThanOrEqual(80);
  });

  it("exports what it imports", () => {
    const theme: TerminalTheme = { id: "custom:a", label: "Mine", dark: true, colors: ["#0c0c0c", "#cccccc", "#ffffff", ...ansi] as TerminalTheme["colors"], selection: "#334455" };
    const out = toWindowsTerminal(theme);
    expect(out).toMatchObject({ name: "Mine", background: "#0c0c0c", foreground: "#cccccc", cursorColor: "#ffffff", selectionBackground: "#334455", purple: ansi[5], brightWhite: ansi[15] });
    const [back] = importThemeFile(JSON.stringify(out));
    expect(back.colors).toEqual(theme.colors);
    expect(back.selection).toBe("#334455");
  });

  it("exports a built-in theme without a selection colour with a solid one", () => {
    const sel = toWindowsTerminal(findTerminalTheme("dracula")!).selectionBackground;
    expect(normalizeHex(sel)).toBe(sel);
  });
});

describe("iTerm2 presets", () => {
  it("reads the colours, named after the file", () => {
    const [th] = importThemeFile(iterm(), "C:\\themes\\Solar Flare.itermcolors");
    expect(th.label).toBe("Solar Flare");
    expect(th.colors[0]).toBe("#000000");
    expect(th.colors[1]).toBe("#ffffff");
    expect(th.colors[2]).toBe("#ff8000");
    expect(th.colors[3]).toBe("#0000ff");
    expect(th.colors[18]).toBe("#ff0000");
    expect(th.selection).toBe("#333366");
    expect(th.dark).toBe(true);
  });

  it("refuses a missing colour, entities and other elements", () => {
    fails(() => importThemeFile(iterm("", "Ansi 7 Color"), "x.itermcolors"), /Ansi 7 Color/);
    const entity = iterm().replace('"http://www.apple.com/DTDs/PropertyList-1.0.dtd">', '"x" [<!ENTITY boom "boom">]>');
    fails(() => importThemeFile(entity), /entities/);
    fails(() => importThemeFile(iterm("<key>X</key><array><string>a</string></array>")), /iTerm2 colour preset/);
    fails(() => importThemeFile(iterm("<key>Bad</key>")), /iTerm2 colour preset/);
    fails(() => importThemeFile("<plist><dict><key>a</key><real>1</real></dict></plist><plist/>"), /iTerm2 colour preset/);
    fails(() => importThemeFile(iterm().replace(itermColor(1, 1, 1), itermColor(2, 1, 1))), /Foreground Color is not a colour/);
    fails(() => importThemeFile(iterm().replace(`<real>${1 - 7 / 15}</real>`, "<real></real>")), /iTerm2 colour preset/);
    fails(() => importThemeFile(iterm().replace(`<real>${1 - 7 / 15}</real>`, "<real>0x1</real>")), /iTerm2 colour preset/);
    fails(() => importThemeFile(`<plist><real>${"1".repeat(MAX_THEME_FILE - 40)}x</real></plist>`), /iTerm2 colour preset/);
  });

  it("reads nested dicts and decodes plain XML escapes in strings", () => {
    expect(readPlist("<plist><dict><key>a &amp; b</key><dict><key>n</key><integer>3</integer></dict></dict></plist>")).toEqual({ "a & b": { n: 3 } });
  });
});

describe("base16 schemes", () => {
  it("reads the current YAML layout with the base16-shell mapping", () => {
    const [th] = importThemeFile(base16Yaml);
    expect(th.label).toBe("Monokai");
    // The built-in Monokai is this scheme read the same way.
    expect(th.colors).toEqual(findTerminalTheme("monokai")!.colors);
    expect(th.selection).toBe("#49483e");
  });

  it("reads the older flat layout and JSON", () => {
    const [th] = importThemeFile(base16Legacy);
    expect(th.label).toBe("Ocean");
    expect(th.colors.slice(0, 4)).toEqual(["#2b303b", "#c0c5ce", "#c0c5ce", "#2b303b"]);
    const json = JSON.stringify({ scheme: "Ocean", ...readSimpleYaml(base16Legacy) });
    expect(importThemeFile(json)[0].colors).toEqual(th.colors);
  });

  it("refuses missing or malformed slots and YAML beyond key: value", () => {
    fails(() => importThemeFile(base16Yaml.replace(/ {2}base0E: .*\n/, "")), /base0E/);
    fails(() => importThemeFile(base16Yaml.replace('"#a6e22e"', '"green"')), /base0B is not a hex colour/);
    fails(() => importThemeFile(`${base16Yaml}evil: !!python/object x\n`), /Line \d+/);
    fails(() => importThemeFile(`${base16Yaml}list:\n  - a\n`), /Line \d+/);
    fails(() => importThemeFile("anchor: &a 1\n"), /Line 1/);
    fails(() => importThemeFile("name: x\nother: y\n"), /not a Windows Terminal, iTerm2 or base16/);
  });
});

describe("hostile files", () => {
  const fast = (fn: () => unknown) => {
    const start = performance.now();
    expect(fn).toThrow(ThemeFileError);
    expect(performance.now() - start).toBeLessThan(1000);
  };

  it("never reach an object's prototype", () => {
    fails(() => importThemeFile(`__proto__:\n  polluted: "yes"\n${base16Yaml}`), /Line 1/);
    fails(() => importThemeFile(iterm("<key>__proto__</key><dict><key>polluted</key><string>yes</string></dict>")), /iTerm2 colour preset/);
    // JSON.parse keeps `__proto__` as a plain key.
    importThemeFile(JSON.stringify({ ...wtScheme(), ["__proto__"]: { polluted: "yes" } }));
    expect(({} as Record<string, unknown>).polluted).toBeUndefined();
  });

  it("are refused in linear time near the size cap", () => {
    const n = MAX_THEME_FILE - 100;
    fast(() => importThemeFile(`name: x${" ".repeat(n)}y`));
    fast(() => importThemeFile(`name: "x"${" ".repeat(n)}y`));
    fast(() => importThemeFile(`<plist>${"<!--".repeat(n / 4)}`));
    fast(() => importThemeFile(`<plist>${"<a ".repeat(n / 3)}`));
    fast(() => importThemeFile(`<plist>${"<?xml ".repeat(n / 6)}`));
    fast(() => importThemeFile(`<!DOCTYPE${" x".repeat(n / 2)}`));
  });
});

describe("own themes", () => {
  const base = TERMINAL_THEMES[0];
  const ids = (() => {
    let n = 0;
    return () => `t${++n}`;
  })();

  it("duplicates any theme under a unique name and a custom id", () => {
    const one = duplicateTheme([], base);
    expect(one.theme.id).toMatch(/^custom:[a-z0-9]+$/);
    expect(one.theme.label).toBe("Dracula copy");
    expect(one.theme.colors).toEqual(base.colors);
    expect(one.theme.colors).not.toBe(base.colors);
    const two = duplicateTheme(one.list, base);
    expect(two.theme.label).toBe("Dracula copy 2");
    expect(new Set(two.list.map((x) => x.id)).size).toBe(2);
  });

  it("copies a plugin theme only when its colours are hex", () => {
    const plugin = { ...base, id: "acme.mono", colors: ["rgb(0,0,0)", ...base.colors.slice(1)] as TerminalTheme["colors"] };
    expect(() => duplicateTheme([], plugin)).toThrow(/cannot be copied/);
    const upper = { ...base, id: "acme.up", colors: base.colors.map((c) => c.toUpperCase()) as TerminalTheme["colors"] };
    expect(duplicateTheme([], upper).theme.colors).toEqual(base.colors);
  });

  it("edits, renames and deletes", () => {
    let { list, theme } = duplicateTheme([], base, "Mine");
    list = updateTheme(list, { ...theme, colors: ["#000000", ...theme.colors.slice(1)] as TerminalTheme["colors"], selection: "#123456" });
    expect(list[0].colors[0]).toBe("#000000");
    expect(list[0].selection).toBe("#123456");
    expect(() => updateTheme(list, { ...list[0], colors: ["blue", ...theme.colors.slice(1)] as TerminalTheme["colors"] })).toThrow(/hex value/);
    const other = duplicateTheme(list, base, "Other");
    list = renameTheme(other.list, theme.id, "  Other \n");
    expect(list[0].label).toBe("Other 2");
    list = renameTheme(list, theme.id, "");
    expect(list[0].label).toBe("Other 2");
    list = deleteTheme(list, theme.id);
    expect(list.map((x) => x.label)).toEqual(["Other"]);
    expect(() => renameTheme(list, theme.id, "x")).toThrow(/no longer exists/);
  });

  it("adds imported themes with fresh ids and stops at the limit", () => {
    const { list, added } = addImported([], importThemeFile(JSON.stringify([wtScheme(), wtScheme()])));
    expect(added.map((x) => x.label)).toEqual(["Campbell", "Campbell 2"]);
    expect(checkThemes(list)).toEqual(list);
    const full = Array.from({ length: MAX_CUSTOM_THEMES }, () => ({ ...base, id: `custom:${ids()}` }));
    expect(() => addImported(full, [base])).toThrow(/up to 100/);
    expect(() => duplicateTheme(full, base)).toThrow(/up to 100/);
  });

  it("joins every theme list and is found by id", () => {
    const { theme } = duplicateTheme([], base, "Mine");
    setCustomThemes([theme]);
    expect(terminalThemes().at(-1)).toBe(theme);
    expect(findTerminalTheme(theme.id)).toBe(theme);
    setCustomThemes([]);
    expect(findTerminalTheme(theme.id)).toBeUndefined();
  });
});

describe("stored themes", () => {
  const good = { id: "custom:abc", label: "Mine", dark: false, colors: Array(19).fill("#FFFFFF"), selection: "#000" };

  it("keeps well-formed entries, normalized", () => {
    expect(checkTheme(good)).toEqual({ ...good, colors: Array(19).fill("#ffffff"), selection: "#000000" });
  });

  it("drops damaged entries and duplicates instead of failing the whole list", () => {
    const list = [good, { ...good }, { ...good, id: "dracula" }, { ...good, id: "custom:x", colors: Array(18).fill("#fff") }, { ...good, id: "custom:y", colors: [...Array(18).fill("#fff"), "red"] },
      { ...good, id: "custom:z", dark: "yes" }, { ...good, id: "custom:w", selection: "x" }, null, "x", { ...good, id: "custom:ok" }];
    expect(checkThemes(list).map((x) => x.id)).toEqual(["custom:abc", "custom:ok"]);
    expect(checkThemes(undefined)).toEqual([]);
    expect(checkThemes({ a: 1 })).toEqual([]);
  });

  it("join this computer's on import, never replacing them all", () => {
    const a = checkTheme({ ...good, id: "custom:a", label: "A" })!;
    const b = checkTheme({ ...good, id: "custom:b", label: "B" })!;
    const b2 = { ...b, label: "B from the file" };
    const c = checkTheme({ ...good, id: "custom:c", label: "C" })!;
    expect(mergeThemes([a, b], [])).toEqual([a, b]);
    expect(mergeThemes([a, b], [b2, c])).toEqual([a, b2, c]);
    const full = Array.from({ length: MAX_CUSTOM_THEMES }, (_, i) => ({ ...a, id: `custom:f${i}` }));
    expect(mergeThemes(full, [c])).toEqual(full);
  });

  it("is a portable setting, taken from a file only when every theme is well formed", () => {
    expect(defaultSettings.customThemes).toEqual([]);
    expect(importableSettings({ customThemes: [good], cursorStyle: "block" }, defaultSettings)).toMatchObject({ values: { customThemes: [checkTheme(good)], cursorStyle: "block" }, invalid: [] });
    expect(importableSettings({ customThemes: [good, { ...good, id: "bad id" }], cursorStyle: "beam" }, defaultSettings).invalid.sort()).toEqual(["cursorStyle", "customThemes"]);
  });
});

describe("built-in themes", () => {
  it("are complete hex palettes with unique ids and readable text", () => {
    expect(new Set(TERMINAL_THEMES.map((x) => x.id)).size).toBe(TERMINAL_THEMES.length);
    for (const th of TERMINAL_THEMES) {
      expect(th.colors, th.id).toHaveLength(19);
      for (const c of [...th.colors, ...(th.selection ? [th.selection] : [])]) expect(normalizeHex(c), `${th.id} ${c}`).toBe(c);
      // Solarized's body text is 4.1:1 by design; every theme added since reads at 4.5:1 or more.
      expect(contrastRatio(th.colors[0], th.colors[1]), th.id).toBeGreaterThanOrEqual(th.id.startsWith("solarized") ? 4 : 4.5);
    }
  });

  it("include the new well-known themes", () => {
    for (const id of ["monokai", "material-dark", "ayu-dark", "ayu-light", "rose-pine", "rose-pine-dawn", "everforest-dark", "kanagawa", "night-owl"]) expect(findTerminalTheme(id), id).toBeTruthy();
    expect(findTerminalTheme("rose-pine-dawn")!.dark).toBe(false);
    expect(findTerminalTheme("ayu-light")!.dark).toBe(false);
  });
});
