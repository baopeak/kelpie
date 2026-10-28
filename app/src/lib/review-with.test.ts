import { describe, expect, it, vi } from "vitest";

vi.mock("./store", () => ({ store: { state: { settings: { hiddenAgents: [] } }, sessions: {}, daemonApi: () => 13 } }));
vi.mock("./ipc", () => ({}));
vi.mock("./review-queue", () => ({ baseOf: () => "main" }));
vi.mock("./send-to-agent", () => ({}));
vi.mock("./review-facts", () => ({ sessionsOf: () => [] }));
vi.mock("./agents", () => ({ launchableAgents: () => ["claude", "codex"] }));

const { reviewMessage } = await import("./review-with");

const input = { prompt: "Review it.", name: "fix-login", branch: "kelpie/fix-login", base: "main", committed: "Commits:\n- fix\n\ndiff --git a/x b/x\n+new", uncommitted: "", exe: "C:\\Kelpie\\kelpied.exe" };

describe("reviewMessage", () => {
  it("is the prompt, how to leave a comment on a line, and the diff", () => {
    const m = reviewMessage(input);
    expect(m.startsWith("Review it.")).toBe(true);
    expect(m).toContain('"C:/Kelpie/kelpied.exe" review add --base main <file>:<line> "<comment>"');
    expect(m).toContain("Changes in kelpie/fix-login against main:\n```diff\nCommits:");
    expect(m).not.toContain("Uncommitted changes");
  });

  it("adds the uncommitted changes when there are some", () => {
    expect(reviewMessage({ ...input, uncommitted: "diff --git a/y b/y\n+x" })).toContain("Uncommitted changes in the worktree:\n```diff\ndiff --git a/y b/y");
  });

  it("does not offer the command to a daemon that cannot take notes", () => {
    expect(reviewMessage({ ...input, exe: null })).not.toContain("review add");
  });

  it("leaves a base that could be mistaken for an option out of the command", () => {
    expect(reviewMessage({ ...input, base: "--output=x" })).not.toContain("--base");
    expect(reviewMessage({ ...input, base: "feature/a b" })).not.toContain("--base");
  });

  it("fences a diff that holds backticks with a longer fence", () => {
    const m = reviewMessage({ ...input, committed: "+```js\n+x\n+```" });
    expect(m).toContain("````diff\n+```js");
    expect(m).toContain("+```\n````");
  });

  it("falls back to a note when nothing was committed", () => {
    expect(reviewMessage({ ...input, committed: "  " })).toContain("(no committed changes)");
  });

  it("falls back to the name when the worktree has no branch", () => {
    expect(reviewMessage({ ...input, branch: null })).toContain("Changes in fix-login against main");
  });
});
