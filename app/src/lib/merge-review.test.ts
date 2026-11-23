import { describe, expect, it } from "vitest";
import { MAX_ACCEPTANCES, mergeChip, parseChange, patchOf, reviewOf, withAcceptances, type FlagRead } from "./merge-review";
import { changeFlags, type Flag } from "./merge-line";
import type { PullRequest } from "./ipc";

const patch = [
  "diff --git a/.github/workflows/ci.yml b/.github/workflows/ci.yml",
  "index 111..222 100644",
  "--- a/.github/workflows/ci.yml",
  "+++ b/.github/workflows/ci.yml",
  "@@ -1,2 +1,2 @@",
  "-run: cargo test",
  "+run: cargo build",
  " keep",
  "diff --git a/src/old.test.ts b/src/old.test.ts",
  "deleted file mode 100644",
  "index 333..000",
  "--- a/src/old.test.ts",
  "+++ /dev/null",
  "@@ -1,2 +0,0 @@",
  "-it(\"works\", () => {});",
  "-// --- not a header",
  "diff --git a/src/new.ts b/src/new.ts",
  "new file mode 100644",
  "index 000..444",
  "--- /dev/null",
  "+++ b/src/new.ts",
  "@@ -0,0 +1 @@",
  "+export const x = 1;",
  "diff --git a/src/before.ts b/src/after.ts",
  "similarity index 100%",
  "rename from src/before.ts",
  "rename to src/after.ts",
  "diff --git a/run.sh b/run.sh",
  "old mode 100644",
  "new mode 100755",
  "diff --git \"a/\\303\\274.txt\" \"b/\\303\\274.txt\"",
  "--- \"a/\\303\\274.txt\"",
  "+++ \"b/\\303\\274.txt\"",
  "@@ -1 +1 @@",
  "-a",
  "+b",
].join("\n");

describe("parseChange", () => {
  const files = parseChange(patch);

  it("reads each file with its status and changed lines", () => {
    expect(files.map((f) => [f.path, f.status])).toEqual([
      [".github/workflows/ci.yml", "modified"],
      ["src/old.test.ts", "removed"],
      ["src/new.ts", "added"],
      ["src/after.ts", "renamed"],
      ["run.sh", "modified"],
      ["ü.txt", "modified"],
    ]);
    expect(files[0]).toMatchObject({ added: ["run: cargo build"], removed: ["run: cargo test"] });
    expect(files[1].removed).toEqual(["it(\"works\", () => {});", "// --- not a header"]);
    expect(files[2].added).toEqual(["export const x = 1;"]);
  });

  it("gives nothing for an empty change", () => {
    expect(parseChange("")).toEqual([]);
  });

  it("feeds the flags the way the rules expect", () => {
    expect(changeFlags(files).map((f) => `${f.path}:${f.what}`)).toEqual([".github/workflows/ci.yml:workflow", "src/old.test.ts:test-file-removed"]);
  });
});

describe("patchOf", () => {
  it("takes the patch after the commits and the stat", () => {
    const text = `Commits:\n- a message\ndiff --git a/fake b/fake\n\nFiles:\n a.ts | 2 +-\n 1 file changed\n\n${patch}`;
    expect(patchOf(text)?.patch.startsWith("diff --git a/.github/workflows/ci.yml")).toBe(true);
  });

  it("gives an empty patch when nothing changed, and says when the text was cut", () => {
    expect(patchOf("Commits:\n\nFiles:\n\n")).toEqual({ patch: "", truncated: false });
    expect(patchOf(`Commits:\n\nFiles:\n a | 1 +\n\n${patch}\n… (truncated)`)?.truncated).toBe(true);
  });

  it("refuses text that is not a branch change", () => {
    expect(patchOf("nothing here")).toBeNull();
  });
});

const flag = (path: string, what = "workflow"): Flag => ({ kind: "checks", path, what });
const read = (flags: Flag[], more: Partial<FlagRead> = {}): FlagRead => ({ head: "h1", base: "master", flags, ...more });

describe("acceptances", () => {
  it("accepts each flag once for one head and project", () => {
    const a = withAcceptances(undefined, "p1", "h1", [flag("a.yml")]);
    expect(withAcceptances(a, "p1", "h1", [flag("a.yml"), flag("b.yml")])).toEqual([
      { project: "p1", head: "h1", path: "a.yml", what: "workflow" },
      { project: "p1", head: "h1", path: "b.yml", what: "workflow" },
    ]);
  });

  it("drops the oldest beyond the bound", () => {
    const flags = Array.from({ length: MAX_ACCEPTANCES + 5 }, (_, i) => flag(`f${i}.yml`));
    const list = withAcceptances(undefined, "p1", "h1", flags);
    expect(list).toHaveLength(MAX_ACCEPTANCES);
    expect(list[0].path).toBe("f5.yml");
  });

  it("holds for one head commit and one project only", () => {
    const accepted = withAcceptances(undefined, "p1", "h1", [flag("a.yml")]);
    expect(reviewOf(read([flag("a.yml")]), accepted, "p1", "h1")).toEqual({ open: [], accepted: [flag("a.yml")], error: undefined });
    // A new commit asks again; another project is a different question.
    expect(reviewOf(read([flag("a.yml")], { head: "h2" }), accepted, "p1", "h2").open).toEqual([flag("a.yml")]);
    expect(reviewOf(read([flag("a.yml")]), accepted, "p2", "h1").open).toEqual([flag("a.yml")]);
  });

  it("knows nothing for a read of another head", () => {
    expect(reviewOf(read([flag("a.yml")]), [], "p1", "h2")).toEqual({ open: [], accepted: [] });
    expect(reviewOf(undefined, [], "p1", "h1")).toEqual({ open: [], accepted: [] });
  });
});

const openPr = (): PullRequest => ({
  number: 7, title: "t", url: "u", state: "OPEN", draft: false, base: "master", checks: "passing", provider: "github", head: "h1",
  mergeable: "MERGEABLE", merge_state: "CLEAN", check_list: [{ name: "build", state: "passing", required: true }],
});

describe("mergeChip with a review", () => {
  it("cannot merge while a change needs a look, and can once it is accepted, keeping it in the title", () => {
    const flags = [flag(".github/workflows/ci.yml")];
    const open = mergeChip(openPr(), reviewOf(read(flags), [], "p1", "h1"));
    expect(open).toMatchObject({ cls: "checks-failing" });
    expect(open?.text).toContain("a change needs a look");
    const accepted = mergeChip(openPr(), reviewOf(read(flags), withAcceptances([], "p1", "h1", flags), "p1", "h1"));
    expect(accepted?.text).toBe("can merge");
    expect(accepted?.title).toContain(".github/workflows/ci.yml (workflow): accepted for this commit");
  });

  it("shows the line without flags and the reason in the title when the change could not be read", () => {
    const chip = mergeChip(openPr(), reviewOf(read([], { error: "origin/master: no such ref" }), [], "p1", "h1"));
    expect(chip?.text).toBe("can merge");
    expect(chip?.title).toContain("origin/master: no such ref");
  });
});
