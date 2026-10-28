import { describe, expect, it, vi } from "vitest";

vi.mock("./store", () => ({ store: { state: { approvedSetups: {} }, sessions: {}, notify: vi.fn() } }));
vi.mock("./ipc", () => ({}));
vi.mock("./terminal-host", () => ({ disposeView: vi.fn() }));

const { branchFromPrompt, parseKelpieConfig, uniqueName } = await import("./worktrees");

describe("check: in .kelpie.yaml", () => {
  it("is a list of commands, or a single one", () => {
    expect(parseKelpieConfig("check:\n  - npm test\n  - cargo clippy -- -D warnings\n").check).toEqual(["npm test", "cargo clippy -- -D warnings"]);
    expect(parseKelpieConfig("check: npm test\n").check).toEqual(["npm test"]);
  });
  it("is empty when absent or blank", () => {
    expect(parseKelpieConfig("setup: [pnpm i]\n").check).toEqual([]);
    expect(parseKelpieConfig("check:\n  - '  '\n  - make\n").check).toEqual(["make"]);
  });
  it("keeps the approval of the start-up commands apart from the checks", () => {
    const base = parseKelpieConfig("setup: [pnpm i]\ncheck: a\n").setupKey;
    expect(parseKelpieConfig("setup: [pnpm i]\ncheck: b\n").setupKey).toBe(base);
    expect(parseKelpieConfig("setup: [pnpm i, rm -rf x]\ncheck: a\n").setupKey).not.toBe(base);
    expect(parseKelpieConfig("setup: [pnpm i]\ntabs: [claude]\ncheck: a\n").setupKey).not.toBe(base);
    expect(parseKelpieConfig("setup: [pnpm i]\nlink: [node_modules]\n").setupKey).not.toBe(base);
  });
});

describe("branchFromPrompt", () => {
  it("makes a short slug from the first words", () => {
    expect(branchFromPrompt("Fix the login redirect, please!", "x")).toBe("fix-the-login-redirect-please");
    expect(branchFromPrompt("Add   dark mode to the settings page and the board", "x")).toBe("add-dark-mode-to-the-settings");
  });
  it("drops accents and keeps it within 40 characters", () => {
    expect(branchFromPrompt("Sửa lỗi đăng nhập", "x")).toBe("sua-loi-dang-nhap");
    expect(branchFromPrompt("a".repeat(80) + " b", "x").length).toBeLessThanOrEqual(40);
  });
  it("falls back when nothing is usable", () => {
    expect(branchFromPrompt("", "claude-1024")).toBe("claude-1024");
    expect(branchFromPrompt("!!! ???", "claude-1024")).toBe("claude-1024");
  });
});

describe("uniqueName", () => {
  it("keeps a free name and numbers a taken one", () => {
    expect(uniqueName("fix-login", ["main"])).toBe("fix-login");
    expect(uniqueName("fix-login", ["fix-login"])).toBe("fix-login-2");
    expect(uniqueName("fix-login", ["fix-login", "fix-login-2"])).toBe("fix-login-3");
    expect(uniqueName("fix-login", ["Fix-Login"])).toBe("fix-login-2");
    expect(uniqueName("fix-login", [], (n) => n === "fix-login")).toBe("fix-login-2");
  });
});
