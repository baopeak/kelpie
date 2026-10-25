import { describe, expect, it, vi } from "vitest";

vi.mock("./store", () => ({ store: { state: { projects: [] }, daemonApi: () => 13 } }));
vi.mock("./ipc", () => ({ worktreeMaterializeEstimate: vi.fn(async () => ({ copy_bytes: 6, copy_entries: 1, copied: [".env"], linked: [], skipped: [] })) }));
vi.mock("./runs", () => ({}));
vi.mock("./worktrees", async () => ({ branchFromPrompt: (p: string) => p.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, ""), configApproved: vi.fn(), configAtRef: vi.fn() }));
vi.mock("./agents", () => ({ kindInfo: (k: string) => ({ label: k[0].toUpperCase() + k.slice(1) }) }));

const { MAX_PROMPT, bringingText, candidateTasks, planBringing, problem, runTitle, taskIds } = await import("./try-agents");
const worktrees = await import("./worktrees");
const ipc = await import("./ipc");

describe("taskIds", () => {
  it("are the agents' names, numbered when one repeats", () => {
    expect(taskIds(["claude", "codex", "claude", "claude"])).toEqual(["claude", "codex", "claude-2", "claude-3"]);
  });
  it("only use characters a run accepts", () => {
    expect(taskIds(["my.agent!"])[0]).toMatch(/^[A-Za-z0-9_-]+$/);
  });
});

describe("candidateTasks", () => {
  it("are independent tasks that each get a worktree and the same prompt", () => {
    const tasks = candidateTasks(["claude", "codex", "claude"], "Fix the login");
    expect(tasks.map((x) => [x.id, x.title, x.kind])).toEqual([["claude", "Claude #1", "claude"], ["codex", "Codex", "codex"], ["claude-2", "Claude #2", "claude"]]);
    for (const x of tasks) expect(x).toMatchObject({ prompt: "Fix the login", deps: [], worktree: true, approve: false });
  });
});

describe("problem", () => {
  it("wants 2 to 5 agents and a prompt of a size an agent can take", () => {
    expect(problem(["claude"], "x")).toMatch(/between 2 and 5/);
    expect(problem(Array(6).fill("claude"), "x")).toMatch(/between 2 and 5/);
    expect(problem(["claude", "codex"], "  ")).toMatch(/prompt/);
    expect(problem(["claude", "codex"], "x".repeat(MAX_PROMPT + 1))).toMatch(/too long/);
    expect(problem(["claude", "codex"], "x")).toBeNull();
    expect(problem(Array(5).fill("claude"), "x")).toBeNull();
  });
});

describe("runTitle", () => {
  it("names the run from the prompt", () => {
    expect(runTitle("Fix the login redirect")).toBe("Try: fix the login redirect");
    expect(runTitle("x".repeat(200)).length).toBeLessThanOrEqual(60);
  });
});

describe("bringingText", () => {
  const est = { copy_bytes: 2048, copy_entries: 3, copied: [".env"], linked: ["node_modules"], skipped: [] };
  it("says what each worktree gets and what the copies add up to", () => {
    const text = bringingText({ approved: true, copy: [".env"], link: ["node_modules"], unapproved: false, estimate: est }, 3);
    expect(text).toContain("links node_modules");
    expect(text).toContain("copies .env (2 KB in 3 entries)");
    expect(text).toContain("6 KB");
  });
  it("says so when nothing is brought or the file has not been approved", () => {
    expect(bringingText({ approved: true, copy: [], link: [], unapproved: false, estimate: { ...est, copied: [], linked: [] } }, 2)).toMatch(/clean checkout/);
    expect(bringingText({ approved: false, copy: [], link: [], unapproved: true, estimate: null }, 2)).toMatch(/not been approved/);
  });
});

describe("planBringing", () => {
  const project = { id: "p", path: "C:/w/repo", name: "repo" } as never;
  const config = (copy: string[], link: string[]) => ({ setup: [], tabs: [], copy, link, check: [], setupKey: "k" }) as never;

  it("measures only what an approved .kelpie.yaml will bring", async () => {
    vi.mocked(worktrees.configAtRef).mockResolvedValue(config([".env"], ["deps"]));
    vi.mocked(worktrees.configApproved).mockReturnValue(true);
    const b = await planBringing(project);
    expect(b).toMatchObject({ approved: true, copy: [".env"], link: ["deps"], unapproved: false });
    expect(ipc.worktreeMaterializeEstimate).toHaveBeenCalledWith("C:/w/repo", [".env"], ["deps"]);
    expect(b.estimate).not.toBeNull();
  });

  it("shows no estimate when nothing is approved, even if .worktreeinclude would copy something", async () => {
    vi.mocked(ipc.worktreeMaterializeEstimate).mockClear();
    vi.mocked(worktrees.configAtRef).mockResolvedValue(config([".env"], []));
    vi.mocked(worktrees.configApproved).mockReturnValue(false);
    const listed = await planBringing(project);
    expect(listed).toMatchObject({ approved: false, copy: [], link: [], unapproved: true, estimate: null });
    expect(bringingText(listed, 3)).toMatch(/not been approved/);
    // No .kelpie.yaml at all: the same, and a clean checkout is what the dialog says.
    vi.mocked(worktrees.configAtRef).mockResolvedValue(null);
    const none = await planBringing(project);
    expect(none).toMatchObject({ approved: false, unapproved: false, estimate: null });
    expect(bringingText(none, 3)).toBe("Each worktree starts as a clean checkout.");
    expect(ipc.worktreeMaterializeEstimate).not.toHaveBeenCalled();
  });
});
