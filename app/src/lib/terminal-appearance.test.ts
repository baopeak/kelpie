import { describe, expect, it } from "vitest";
import { hostAppearance, paneTheme, paneThemes, prunePaneThemes, resolveAppearance, setPaneTheme, themeIdFor, type AppearanceOverrides } from "./terminal-appearance";
import { defaultSettings } from "./default-settings";
import { findTerminalTheme, setCustomThemes, toXtermTheme, type TerminalTheme } from "./terminal-themes";
import { terminalTheme as appDark, terminalThemeLight as appLight } from "./theme";

/** A saved host's appearance fields as `newHost` (lib/ssh-hosts.ts) starts them, with `over`. */
const newHost = (over: AppearanceOverrides = {}) => ({ label: "box", terminalTheme: "", fontFamily: "", fontSize: null, lineHeight: null, cursorStyle: "", cursorBlink: null, ...over });
import type { Settings } from "./types";

const app: Settings = { ...defaultSettings, fontFamily: "App Mono", fontSize: 13, lineHeight: 1.2, cursorStyle: "bar", cursorBlink: true, terminalTheme: "nord" };

describe("a pane's appearance", () => {
  it("is the app's when its host has no overrides", () => {
    for (const host of [undefined, hostAppearance(newHost())]) {
      expect(resolveAppearance(app, host)).toEqual({
        themeId: "nord", theme: toXtermTheme(findTerminalTheme("nord")!), fontFamily: "App Mono", fontSize: 13, lineHeight: 1.2, cursorStyle: "bar", cursorBlink: true,
      });
    }
  });

  it("takes each override the host sets", () => {
    const host = hostAppearance(newHost({ terminalTheme: "kanagawa", fontFamily: " Fira Code ", fontSize: 16, lineHeight: 1.5, cursorStyle: "block", cursorBlink: false }));
    expect(resolveAppearance(app, host)).toMatchObject({ themeId: "kanagawa", fontFamily: "Fira Code", fontSize: 16, lineHeight: 1.5, cursorStyle: "block", cursorBlink: false });
    expect(resolveAppearance(app, host).theme.background).toBe("#1f1f28");
  });

  it("ignores out-of-range or unknown values (the app's apply)", () => {
    const host = { fontFamily: "   ", fontSize: 99, lineHeight: 0.5, cursorStyle: "beam", cursorBlink: null, terminalTheme: "removed-plugin.theme" };
    expect(resolveAppearance(app, host)).toMatchObject({ themeId: "nord", fontFamily: "App Mono", fontSize: 13, lineHeight: 1.2, cursorStyle: "bar", cursorBlink: true });
    expect(resolveAppearance(app, { fontSize: Number.NaN }).fontSize).toBe(13);
  });

  it("puts the pane's own pick over its host's", () => {
    const host = { terminalTheme: "kanagawa" };
    expect(themeIdFor(app, host, "night-owl")).toBe("night-owl");
    expect(themeIdFor(app, host, "gone")).toBe("kanagawa");
    expect(themeIdFor(app, undefined, "night-owl")).toBe("night-owl");
    expect(themeIdFor({ terminalTheme: "" }, undefined, undefined)).toBe("default");
  });

  it("follows the app's dark or light palette by default", () => {
    expect(resolveAppearance({ ...app, terminalTheme: "default", theme: "light" }).theme).toBe(appLight);
    expect(resolveAppearance({ ...app, terminalTheme: "default", theme: "dark" }).theme).toBe(appDark);
  });

  it("settles on the app's settings from a state file older than cursor styles", () => {
    const { cursorStyle: _c, ...old } = app;
    expect(resolveAppearance(old as Settings).cursorStyle).toBe("bar");
  });

  it("uses one of the person's themes and falls back when it is deleted", () => {
    const mine: TerminalTheme = { id: "custom:mine", label: "Mine", dark: true, colors: Array(19).fill("#101010") as TerminalTheme["colors"], selection: "#202020" };
    setCustomThemes([mine]);
    const look = resolveAppearance(app, { terminalTheme: mine.id });
    expect(look.themeId).toBe(mine.id);
    expect(look.theme.selectionBackground).toBe("#202020");
    setCustomThemes([]);
    expect(resolveAppearance(app, { terminalTheme: mine.id }).themeId).toBe("nord");
  });
});

describe("a pane's own theme", () => {
  it("is kept per session until cleared or the session goes", () => {
    setPaneTheme("a", "dracula");
    setPaneTheme("b", "nord");
    expect(paneTheme("a")).toBe("dracula");
    setPaneTheme("a", null);
    expect(paneTheme("a")).toBeUndefined();
    const before = paneThemes();
    prunePaneThemes((id) => id === "b");
    expect(paneThemes()).toBe(before);
    prunePaneThemes(() => false);
    expect(paneThemes()).toEqual({});
  });
});

describe("saved hosts from older files", () => {
  it("have no overrides when the fields are missing", () => {
    // What the backend returns for a host saved before these fields existed (serde fills defaults; a
    // window-side copy may lack them altogether).
    const old = { ...newHost({ terminalTheme: "nord" }) } as Record<string, unknown>;
    for (const k of ["fontFamily", "fontSize", "lineHeight", "cursorStyle", "cursorBlink"]) delete old[k];
    expect(resolveAppearance(app, hostAppearance(old as AppearanceOverrides))).toMatchObject({ themeId: "nord", fontFamily: "App Mono", fontSize: 13, cursorStyle: "bar", cursorBlink: true });
  });
});
