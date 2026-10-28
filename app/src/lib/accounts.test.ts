import { describe, expect, it, vi } from "vitest";

const settings: { accounts: unknown[]; activeAccounts: Record<string, string> } = { accounts: [], activeAccounts: {} };
vi.mock("./store", () => ({ store: { state: { settings } }, sessionLabel: () => "label" }));
vi.mock("./ipc", () => ({ daemon: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("./platform", () => ({ uid: () => "id" }));
vi.mock("./confirm-dialog", () => ({ confirmDialog: vi.fn() }));

const { switchRefusal } = await import("./accounts");
const work = { id: "work", agent: "claude", label: "Work", dir: "C:\\accounts\\claude\\work" } as const;
settings.accounts = [work];

const session = (extra = {}) => ({ kind: "claude", chat: false, remote: null, agent_session_id: "conv-1", env: {} as Record<string, string>, ...extra }) as Parameters<typeof switchRefusal>[0];

describe("switchRefusal", () => {
  it("allows a terminal Claude session with a conversation to move to another account", () => {
    expect(switchRefusal(session(), work)).toBeNull();
    expect(switchRefusal(session({ env: { CLAUDE_CONFIG_DIR: work.dir } }), null)).toBeNull();
  });

  it("refuses with the reason when the conversation cannot be carried over", () => {
    expect(switchRefusal(session({ kind: "shell" }), work)).toContain("Only Claude Code and Codex");
    expect(switchRefusal(session({ kind: "omp" }), work)).toContain("Only Claude Code and Codex");
    expect(switchRefusal(session({ chat: true }), work)).toContain("chat view");
    expect(switchRefusal(session({ remote: { dest: "box", tmux: false } }), work)).toContain("another machine");
    expect(switchRefusal(session({ agent_session_id: null }), work)).toContain("no conversation");
  });

  it("refuses the account the session is already on", () => {
    expect(switchRefusal(session({ env: { CLAUDE_CONFIG_DIR: work.dir } }), work)).toContain("already");
    expect(switchRefusal(session(), null)).toContain("already");
  });
});
