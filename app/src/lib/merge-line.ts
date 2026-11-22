// One answer about a pull request: can it be merged, and if not, why. The same answer is for a task's card and for the phone, so it
// is a value with a reason code, not a sentence: each view words it in its own language.
//
// The answer follows GitHub's own state (what GitHub will allow), then what Kelpie adds: a change that touches what checks it (tests,
// workflows, check scripts) holds the merge until a person accepts it for that commit. Nothing here reads a log.

/** One check of a pull request's head commit. */
export interface PrCheck {
  /** The job, as GitHub names it (`build (windows-latest)`). */
  name: string;
  /** Whether the base branch's protection requires it. */
  required: boolean;
  state: "passing" | "failing" | "pending";
  /** The step that failed, when it is known. Never log text. */
  step?: string;
  /** How many times it ran on this commit; more than one means it was run again. */
  attempts: number;
}

/** What was read of a pull request, or why it could not be read. */
export type PrRead =
  | { ok: false; why: string }
  | {
      ok: true;
      state: "OPEN" | "CLOSED" | "MERGED";
      draft: boolean;
      base: string;
      /** GitHub's `mergeable`. */
      mergeable: "MERGEABLE" | "CONFLICTING" | "UNKNOWN";
      /** GitHub's `mergeStateStatus`: `CLEAN`, `BLOCKED`, `BEHIND`, `DIRTY`, `UNSTABLE`, `HAS_HOOKS`, `DRAFT`, `UNKNOWN`. */
      mergeState: string;
      checks: PrCheck[];
    };

/** Something in the change that a person should accept before it is merged. */
export interface Flag {
  /** `checks`: what decides the checks (a workflow, a check script). `tests`: tests removed or switched off. */
  kind: "checks" | "tests";
  /** The file. */
  path: string;
  /** What about it, as a code a view words: `workflow`, `check-script`, `baseline`, `test-removed`, `test-off`, `test-file-removed`. */
  what: string;
}

export type MergeLine =
  | { can: true; /** Real-app checks still owed for it. */ owed: number; /** Checks that passed only when run again. */ rerun: { name: string; attempts: number }[] }
  | { can: false; reason: "unknown"; detail: string }
  | { can: false; reason: "not-open"; detail: "CLOSED" | "MERGED" }
  | { can: false; reason: "draft" }
  | { can: false; reason: "conflicts"; detail: string }
  | { can: false; reason: "checks-failing"; detail: string }
  | { can: false; reason: "checks-running"; detail: string }
  | { can: false; reason: "touches"; flags: Flag[] }
  | { can: false; reason: "blocked"; detail: string };

/**
 * Whether a pull request can be merged. `open` are the flags no person has accepted for this head commit; `owed` is how many
 * real-app checks are still written down for it. The first reason that applies is the one given.
 */
export function mergeLine(pr: PrRead, open: Flag[], owed: number): MergeLine {
  // A failed read is never "can merge".
  if (!pr.ok) return { can: false, reason: "unknown", detail: pr.why };
  if (pr.state !== "OPEN") return { can: false, reason: "not-open", detail: pr.state };
  const required = pr.checks.filter((c) => c.required);
  const failing = required.find((c) => c.state === "failing");
  if (failing) return { can: false, reason: "checks-failing", detail: failing.step ? `${failing.name} / ${failing.step}` : failing.name };
  const running = required.filter((c) => c.state === "pending");
  if (running.length) return { can: false, reason: "checks-running", detail: running.map((c) => c.name).join(", ") };
  if (open.length) return { can: false, reason: "touches", flags: open };
  if (pr.draft) return { can: false, reason: "draft" };
  if (pr.mergeable === "CONFLICTING" || pr.mergeState === "DIRTY") return { can: false, reason: "conflicts", detail: pr.base };
  // GitHub has not worked it out yet, or holds it for a reason this code does not know (a review rule, a merge queue).
  if (pr.mergeable === "UNKNOWN" || pr.mergeState === "UNKNOWN") return { can: false, reason: "unknown", detail: "GitHub has not said yet" };
  if (!["CLEAN", "UNSTABLE", "HAS_HOOKS"].includes(pr.mergeState)) return { can: false, reason: "blocked", detail: pr.mergeState };
  const rerun = required.filter((c) => c.attempts > 1).map((c) => ({ name: c.name, attempts: c.attempts }));
  return { can: true, owed: Math.max(0, owed), rerun };
}

/** One file of a change. `added` and `removed` are its changed lines without their `+` and `-`. */
export interface ChangedFile {
  path: string;
  status: "added" | "modified" | "removed" | "renamed";
  added: string[];
  removed: string[];
}

/** Which paths decide the checks, and which are tests. `**` is any folders, `*` anything but a slash. */
export interface FlagPaths {
  workflows: string[];
  checkScripts: string[];
  baselines: string[];
  testFiles: string[];
}

/** The defaults, from how this kind of repository is laid out. */
export const DEFAULT_FLAG_PATHS: FlagPaths = {
  workflows: [".github/workflows/**", ".gitlab-ci.yml"],
  checkScripts: ["scripts/check-*", ".kelpie.yaml", "**/vitest.config.*", "**/jest.config.*", "**/playwright.config.*"],
  baselines: ["scripts/perf/baseline.json", "**/__snapshots__/**"],
  testFiles: ["**/tests.rs", "**/tests/**", "**/*.test.*", "**/*.spec.*", "**/test_*.py", "**/*_test.go", "scripts/e2e/**"],
};

/** A path pattern as a regular expression: `**` any folders (none included), `*` anything but a slash. */
export function pathPattern(glob: string): RegExp {
  let re = "";
  for (let i = 0; i < glob.length; i++) {
    const c = glob[i];
    if (c === "*" && glob[i + 1] === "*") {
      // `**/` also stands for no folder at all.
      if (glob[i + 2] === "/") {
        re += "(?:.*/)?";
        i += 2;
      } else {
        re += ".*";
        i += 1;
      }
    } else if (c === "*") re += "[^/]*";
    else re += c.replace(/[.+?^${}()|[\]\\]/g, "\\$&");
  }
  return new RegExp(`^${re}$`);
}

const matches = (path: string, globs: string[]) => globs.some((g) => pathPattern(g).test(path.replace(/\\/g, "/")));

/** A line that declares a test (Rust, JavaScript and TypeScript, Python, Go). */
const DECLARES_TEST = /#\[(?:tokio::)?test\b|^\s*(?:it|test)\s*\(|^\s*def test_|^\s*func Test[A-Z_]/;
/** A line that switches a test off. */
const SWITCHES_OFF = /#\[ignore\b|\b(?:it|test|describe)\.(?:skip|todo)\s*\(|^\s*x(?:it|describe)\s*\(|@pytest\.mark\.skip|\bt\.Skip\(/;

/**
 * What in a change a person should look at before it is merged. A hint from paths and changed lines, not a proof: a test can be
 * emptied without matching any of this.
 */
export function changeFlags(files: ChangedFile[], paths: FlagPaths = DEFAULT_FLAG_PATHS): Flag[] {
  const out: Flag[] = [];
  for (const f of files) {
    const path = f.path.replace(/\\/g, "/");
    if (matches(path, paths.workflows)) out.push({ kind: "checks", path, what: "workflow" });
    else if (matches(path, paths.checkScripts)) out.push({ kind: "checks", path, what: "check-script" });
    else if (matches(path, paths.baselines)) out.push({ kind: "checks", path, what: "baseline" });
    if (f.status === "removed" && matches(path, paths.testFiles)) {
      out.push({ kind: "tests", path, what: "test-file-removed" });
      continue;
    }
    // Tests live inside source files too (a `#[cfg(test)]` module), so the lines are looked at whatever the path.
    const declared = (lines: string[]) => lines.filter((l) => DECLARES_TEST.test(l)).length;
    if (declared(f.removed) > declared(f.added)) out.push({ kind: "tests", path, what: "test-removed" });
    if (f.added.some((l) => SWITCHES_OFF.test(l))) out.push({ kind: "tests", path, what: "test-off" });
  }
  return out;
}

/** The flags no person has accepted. An acceptance is for one head commit: a new commit asks again. */
export function openFlags(flags: Flag[], accepted: { head: string; path: string; what: string }[], head: string): Flag[] {
  return flags.filter((f) => !accepted.some((a) => a.head === head && a.path === f.path && a.what === f.what));
}
