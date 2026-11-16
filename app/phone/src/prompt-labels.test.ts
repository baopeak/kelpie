import { describe, expect, it } from "vitest";
import { promptLabels } from "./prompt-labels";

const CLAUDE = [
  "Bash(rm -rf node_modules/.vite && pnpm build)",
  " Do you want to proceed?",
  " ❯ 1. Yes",
  "   2. Yes, and don't ask again for pnpm build",
  "   3. No, and tell Claude what to do differently (esc)",
  "",
];

describe("promptLabels", () => {
  it("names each key from the menu on screen, marker or not", () => {
    expect(promptLabels(CLAUDE, ["1", "2", "3"])).toEqual({ "1": "Yes", "2": "Yes, and don't ask again for pnpm build", "3": "No, and tell Claude what to do differently (esc)" });
  });

  it("reads the menu that ends the screen, not a numbered list above it", () => {
    const rows = ["Plan:", "1. Read the file", "2. Edit it", "", ...CLAUDE];
    expect(promptLabels(rows, ["1", "2", "3"])["1"]).toBe("Yes");
  });

  it("reads a boxed menu and one with ◯ markers, as the daemon does, and never an older list above them", () => {
    const old = ["1. Drop the users table", "2. Recreate it empty"];
    expect(promptLabels([...old, "│ Do you want to proceed? │", "│ ❯ 1. Yes │", "│   2. No │", "╰──────────╯"], ["1", "2"])).toEqual({ "1": "Yes", "2": "No" });
    expect(promptLabels([...old, "Continue?", "◯ 1. Yes", "◯ 2. No"], ["1", "2"])).toEqual({ "1": "Yes", "2": "No" });
  });

  it("gives nothing when the menu is not at the bottom of the screen", () => {
    expect(promptLabels(["1. Drop the users table", "2. Recreate it empty", "Some output", "more output", "and more", "and the end"], ["1", "2"])).toEqual({});
  });

  it("keeps a hint and a wrapped line under the last option", () => {
    expect(promptLabels([...CLAUDE, "   Esc to cancel"], ["1", "2", "3"])["3"]).toBe("No, and tell Claude what to do differently (esc)");
  });

  it("gives nothing when the menu's numbers are not exactly the prompt's keys", () => {
    expect(promptLabels(CLAUDE, ["1", "3"])).toEqual({});
    expect(promptLabels(CLAUDE, ["1", "2", "3", "4"])).toEqual({});
    expect(promptLabels(CLAUDE, ["3", "2", "1"])).toEqual({});
  });

  it("gives nothing for a prompt with no numbered options (y/n, letters)", () => {
    expect(promptLabels(["Overwrite the file? [y/N]"], ["y", "n"])).toEqual({});
    expect(promptLabels(["Yes (y)", "No, provide feedback (n)"], ["y", "n"])).toEqual({});
    expect(promptLabels([], ["1"])).toEqual({});
  });
});
