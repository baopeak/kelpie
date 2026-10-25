import { describe, expect, it, vi } from "vitest";
import type { QuickCommand } from "./types";

const own: QuickCommand[] = [];
vi.mock("./store", () => ({ store: { state: { projects: [{ id: "p1" }, { id: "p2" }], settings: { get quickCommands() { return own; } } } } }));
vi.mock("./ipc", () => ({}));
vi.mock("./terminal-host", () => ({}));
vi.mock("./typed-text", () => ({}));
vi.mock("../chat/mode-switch", () => ({}));
vi.mock("./plugins", () => ({ pluginQuickCommands: () => [] }));

const { DEFAULT_REVIEW_PROMPT, reviewPrompt } = await import("./quick-commands");

const command = (label: string, text: string, projectId: string | null = null): QuickCommand => ({ id: label, label, text, action: "agent", enter: true, projectId });

describe("reviewPrompt", () => {
  it("is the built-in text, which asks for a review and no edits", () => {
    own.length = 0;
    expect(reviewPrompt("p1")).toBe(DEFAULT_REVIEW_PROMPT);
    expect(DEFAULT_REVIEW_PROMPT).toMatch(/Do not change any files/);
  });

  it("is the user's quick command called Review prompt, whatever its case", () => {
    own.splice(0, own.length, command("Fix tests", "run them"), command(" review PROMPT ", "Check the SQL."));
    expect(reviewPrompt("p1")).toBe("Check the SQL.");
  });

  it("is a project's own command for that project only, and an empty one counts for nothing", () => {
    own.splice(0, own.length, command("Review prompt", "for p2", "p2"));
    expect(reviewPrompt("p1")).toBe(DEFAULT_REVIEW_PROMPT);
    expect(reviewPrompt("p2")).toBe("for p2");
    own.splice(0, own.length, command("Review prompt", "   "));
    expect(reviewPrompt("p1")).toBe(DEFAULT_REVIEW_PROMPT);
  });
});
