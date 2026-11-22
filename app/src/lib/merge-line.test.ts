import { describe, expect, it } from "vitest";
import { changeFlags, mergeLine, openFlags, pathPattern, prRead, type ChangedFile, type Flag, type PrCheck, type PrRead } from "./merge-line";

const check = (name: string, state: PrCheck["state"], more: Partial<PrCheck> = {}): PrCheck => ({ name, required: true, state, attempts: 1, ...more });
const pr = (more: Partial<Extract<PrRead, { ok: true }>> = {}): PrRead => ({
  ok: true,
  state: "OPEN",
  draft: false,
  base: "master",
  mergeable: "MERGEABLE",
  mergeState: "CLEAN",
  checks: [check("build (windows-latest)", "passing"), check("build (ubuntu-22.04)", "passing")],
  ...more,
});
const file = (path: string, more: Partial<ChangedFile> = {}): ChangedFile => ({ path, status: "modified", added: [], removed: [], ...more });
const workflow: Flag = { kind: "checks", path: ".github/workflows/ci.yml", what: "workflow" };

describe("mergeLine", () => {
  it("says a green pull request can be merged, with what is still owed", () => {
    expect(mergeLine(pr(), [], 2)).toEqual({ can: true, owed: 2, rerun: [] });
    expect(mergeLine(pr(), [], -1)).toEqual({ can: true, owed: 0, rerun: [] });
  });

  it("never says can merge when the read failed or GitHub has not answered", () => {
    expect(mergeLine({ ok: false, why: "gh timed out" }, [], 0)).toEqual({ can: false, reason: "unknown", detail: "gh timed out" });
    expect(mergeLine(pr({ mergeable: "UNKNOWN" }), [], 0)).toMatchObject({ can: false, reason: "unknown" });
    expect(mergeLine(pr({ mergeState: "UNKNOWN" }), [], 0)).toMatchObject({ can: false, reason: "unknown" });
  });

  it("names the failing required job and its step, and nothing from a log", () => {
    const checks = [check("build (windows-latest)", "failing", { step: "Rust tests" }), check("build (macos-latest)", "pending")];
    expect(mergeLine(pr({ checks }), [], 0)).toEqual({ can: false, reason: "checks-failing", detail: "build (windows-latest) / Rust tests" });
    expect(mergeLine(pr({ checks: [check("build", "failing")] }), [], 0)).toEqual({ can: false, reason: "checks-failing", detail: "build" });
  });

  it("waits for required checks that still run", () => {
    const checks = [check("a", "passing"), check("b", "pending"), check("c", "pending")];
    expect(mergeLine(pr({ checks }), [], 0)).toEqual({ can: false, reason: "checks-running", detail: "b, c" });
  });

  it("lets a check that is not required fail or run", () => {
    const checks = [check("build", "passing"), check("relay", "failing", { required: false }), check("docs", "pending", { required: false })];
    expect(mergeLine(pr({ checks, mergeState: "UNSTABLE" }), [], 0)).toMatchObject({ can: true });
  });

  it("holds a change that touches what checks it until a person accepts it", () => {
    expect(mergeLine(pr(), [workflow], 0)).toEqual({ can: false, reason: "touches", flags: [workflow] });
    // A failing check is the more useful thing to say first.
    expect(mergeLine(pr({ checks: [check("build", "failing")] }), [workflow], 0)).toMatchObject({ reason: "checks-failing" });
  });

  it("says draft, conflicts, closed and merged", () => {
    expect(mergeLine(pr({ draft: true, mergeState: "DRAFT" }), [], 0)).toEqual({ can: false, reason: "draft" });
    expect(mergeLine(pr({ mergeable: "CONFLICTING", mergeState: "DIRTY" }), [], 0)).toEqual({ can: false, reason: "conflicts", detail: "master" });
    expect(mergeLine(pr({ state: "MERGED" }), [], 0)).toEqual({ can: false, reason: "not-open", detail: "MERGED" });
    expect(mergeLine(pr({ state: "CLOSED" }), [], 0)).toEqual({ can: false, reason: "not-open", detail: "CLOSED" });
  });

  it("follows GitHub when it blocks for a reason this code does not know", () => {
    expect(mergeLine(pr({ mergeState: "BLOCKED" }), [], 0)).toEqual({ can: false, reason: "blocked", detail: "BLOCKED" });
    expect(mergeLine(pr({ mergeState: "BEHIND" }), [], 0)).toEqual({ can: false, reason: "blocked", detail: "BEHIND" });
  });

  it("counts a required check that passed only when run again", () => {
    const checks = [check("build (windows-latest)", "passing", { attempts: 3 }), check("build (ubuntu-22.04)", "passing")];
    expect(mergeLine(pr({ checks }), [], 0)).toEqual({ can: true, owed: 0, rerun: [{ name: "build (windows-latest)", attempts: 3 }] });
  });
});

describe("pathPattern", () => {
  it("reads ** as any folders, none included, and * as one name", () => {
    expect(pathPattern("**/tests.rs").test("tests.rs")).toBe(true);
    expect(pathPattern("**/tests.rs").test("crates/kelpied/src/notify/tests.rs")).toBe(true);
    expect(pathPattern("**/tests.rs").test("crates/kelpied/src/notify/tests.rs.bak")).toBe(false);
    expect(pathPattern("scripts/check-*").test("scripts/check-phone-guard.sh")).toBe(true);
    expect(pathPattern("scripts/check-*").test("scripts/sub/check-x.sh")).toBe(false);
    expect(pathPattern(".github/workflows/**").test(".github/workflows/ci.yml")).toBe(true);
    expect(pathPattern(".github/workflows/**").test("xgithub/workflows/ci.yml")).toBe(false);
    expect(pathPattern("**/*.test.*").test("app/src/lib/layout.test.ts")).toBe(true);
    expect(pathPattern("**/*.test.*").test("app/src/lib/layout.ts")).toBe(false);
  });
});

describe("changeFlags", () => {
  const whats = (files: ChangedFile[]) => changeFlags(files).map((f) => `${f.kind}:${f.what}:${f.path}`);

  it("flags what decides the checks", () => {
    expect(whats([file(".github/workflows/ci.yml"), file("scripts/check-phone-guard.sh"), file("scripts/perf/baseline.json"), file("app/vitest.config.ts"), file("app/src/lib/store.ts")])).toEqual([
      "checks:workflow:.github/workflows/ci.yml",
      "checks:check-script:scripts/check-phone-guard.sh",
      "checks:baseline:scripts/perf/baseline.json",
      "checks:check-script:app/vitest.config.ts",
    ]);
  });

  it("flags a removed test file, and reads Windows paths", () => {
    expect(whats([file("crates\\kelpied\\src\\notify\\tests.rs", { status: "removed" }), file("scripts/e2e/finish.py", { status: "removed" }), file("docs/old.md", { status: "removed" })])).toEqual([
      "tests:test-file-removed:crates/kelpied/src/notify/tests.rs",
      "tests:test-file-removed:scripts/e2e/finish.py",
    ]);
  });

  it("flags tests removed from inside a source file, where Rust keeps them", () => {
    const removed = ["    #[test]", "    fn a_reused_id_is_refused() {", "    #[tokio::test]", "    async fn it_waits() {"];
    expect(whats([file("crates/kelpied/src/runs.rs", { removed })])).toEqual(["tests:test-removed:crates/kelpied/src/runs.rs"]);
    // A test that was rewritten declares as many as it removes.
    expect(whats([file("crates/kelpied/src/runs.rs", { removed, added: ["    #[test]", "    #[test]"] })])).toEqual([]);
    expect(whats([file("app/src/lib/layout.test.ts", { removed: ['  it("keeps shares", () => {'] })])).toEqual(["tests:test-removed:app/src/lib/layout.test.ts"]);
  });

  it("flags a test that is switched off", () => {
    expect(whats([file("crates/kelpied/src/files/queue.rs", { added: ["    #[ignore = \"flaky\"]"] })])).toEqual(["tests:test-off:crates/kelpied/src/files/queue.rs"]);
    expect(whats([file("app/src/lib/a.test.ts", { added: ['  it.skip("works", () => {'] })])).toEqual(["tests:test-off:app/src/lib/a.test.ts"]);
    expect(whats([file("app/src/lib/a.test.ts", { added: ['  it("skips nothing", () => {'] })])).toEqual([]);
  });

  it("says nothing about an ordinary change", () => {
    expect(whats([file("app/src/lib/store.ts", { added: ["const it = 1;", "test(x)"], removed: ["const was = 0;"] }), file("README.md")])).toEqual([]);
  });
});

describe("openFlags", () => {
  it("asks again after a new commit", () => {
    const accepted = [{ head: "abc", path: workflow.path, what: workflow.what }];
    expect(openFlags([workflow], accepted, "abc")).toEqual([]);
    expect(openFlags([workflow], accepted, "def")).toEqual([workflow]);
    expect(openFlags([workflow, { kind: "tests", path: "a.rs", what: "test-off" }], accepted, "abc")).toEqual([{ kind: "tests", path: "a.rs", what: "test-off" }]);
  });
});

describe("prRead", () => {
  const base = { number: 7, title: "t", url: "u", state: "OPEN" as const, draft: false, base: "master", checks: "passing" as const };

  it("gives no line where GitHub's merge state did not come with the pull request", () => {
    expect(prRead(base)).toBeNull();
    expect(prRead({ ...base, mergeable: "MERGEABLE", merge_state: "CLEAN" })).toBeNull();
  });

  it("carries the merge state and the checks", () => {
    const read = prRead({ ...base, mergeable: "MERGEABLE", merge_state: "BLOCKED", head: "abc", check_list: [{ name: "build", state: "failing", required: true }] });
    expect(read).toEqual({ ok: true, state: "OPEN", draft: false, base: "master", mergeable: "MERGEABLE", mergeState: "BLOCKED", checks: [{ name: "build", required: true, state: "failing", attempts: 1 }] });
    expect(mergeLine(read!, [], 0)).toEqual({ can: false, reason: "checks-failing", detail: "build" });
  });
});
