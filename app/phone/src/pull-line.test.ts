import { describe, expect, it, vi } from "vitest";
import type { Outcome, PullStatus } from "./link";
import { pullHeadline, pullOf, pullSubline, pullView, pullViewWords, reasonWords, type PullInfo } from "./pull-line";

vi.mock("./i18n", () => ({ t: (s: string, vars?: Record<string, string | number>) => (vars ? s.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : s) }));

const HEAD = "0123456789abcdef0123456789abcdef01234567";
const status = (line: NonNullable<PullStatus["line"]>, rest: PullStatus = {}): Outcome => ({
  t: "ack",
  pr: { repo: "o/n", number: 12, title: "Fix the thing", url: "https://github.com/o/n/pull/12", base: "master", head: HEAD, line, ...rest },
});
const info = (outcome: Outcome): PullInfo => {
  const pr = pullOf(pullView(outcome));
  if (!pr) throw new Error("not a pull request");
  return pr;
};

describe("a project's pull request on the phone", () => {
  it("says that it can be merged, with the repository and the title under it", () => {
    const pr = info(status({ can: true, reason: null, detail: "" }));
    expect(pr).toMatchObject({ repo: "o/n", number: 12, head: HEAD, can: true, reason: null });
    expect(pullHeadline(pr)).toBe("Pull request #12: can merge");
    expect(pullSubline(pr)).toBe("o/n · Fix the thing");
    expect(pullSubline({ ...pr, title: "" })).toBe("o/n");
  });

  it("says why it cannot, in its own words, with the computer's detail only where it is data", () => {
    const why = (reason: string, detail = "") => pullHeadline(info(status({ can: false, reason, detail })));
    expect(why("not_open", "merged")).toBe("Pull request #12: cannot merge, it is already merged");
    expect(why("not_open", "closed")).toContain("it is closed");
    expect(why("not_open", "something")).toContain("it is not open");
    expect(why("draft")).toContain("it is a draft");
    expect(why("conflicts", "with master")).toBe("Pull request #12: cannot merge, it conflicts with master");
    expect(why("checks_failing", "build, test")).toContain("required checks failed: build, test");
    expect(why("checks_running", "deploy")).toContain("required checks are still running: deploy");
    expect(why("checks_running")).toContain("required checks are still running");
    expect(why("blocked", "English from the computer")).not.toContain("English from the computer");
    expect(why("blocked")).toContain("GitHub wants something more");
    expect(why("unknown")).toContain("GitHub has not said yet");
  });

  it("never turns what it does not understand into a pull request that can be merged", () => {
    // A reason this phone does not know is the computer not knowing; a line that says it cannot is never shown as one that can.
    const odd = info(status({ can: false, reason: "something_new", detail: "x" }));
    expect(odd.reason).toBe("unknown");
    expect(odd.can).toBe(false);
    expect(info(status({ can: false, reason: null, detail: "" })).can).toBe(false);
    // An answer that is missing its repository, number, head or line is "unavailable".
    for (const pr of [{}, { repo: "o/n", number: 1 }, { repo: "o/n", number: 1, head: HEAD }, { number: 1, head: HEAD, line: { can: true, reason: null, detail: "" } }, { repo: "o/n", number: "1", head: HEAD, line: { can: true, reason: null, detail: "" } }]) {
      const view = pullView({ t: "ack", pr: pr as unknown as PullStatus });
      expect(view.kind, JSON.stringify(pr)).toBe("unavailable");
    }
    expect(pullView({ t: "ack" }).kind).toBe("unavailable");
    expect(pullView({ t: "ack", pr: { repo: "o/n", number: 1, head: HEAD, line: { can: "yes" as unknown as boolean, reason: null, detail: "" } } }).kind).toBe("unavailable");
  });

  it("tells a branch with no pull request from one that could not be read", () => {
    expect(pullView({ t: "ack", pr: { none: true } })).toEqual({ kind: "none" });
    expect(pullViewWords({ kind: "none" })).toBe("No pull request for this project's branch.");
    expect(pullViewWords({ kind: "loading" })).toBe("Looking for a pull request…");
    const said = (o: Outcome) => {
      const v = pullView(o);
      return v.kind === "unavailable" ? v.message : `not unavailable: ${v.kind}`;
    };
    expect(said({ t: "offline" })).toBe("Not connected to your computer.");
    expect(said({ t: "unknown" })).toBe("Your computer did not answer.");
    expect(said({ t: "error", code: "not_ready", message: "English" })).toContain("Open Kelpie on your computer");
    expect(said({ t: "error", code: "no_project", message: "English" })).toContain("Open Kelpie on your computer");
    expect(said({ t: "error", code: "rate_limited", message: "English" })).toBe("Asking too often: wait a moment.");
    expect(said({ t: "error", code: "pr_unavailable", message: "`gh` is not signed in to GitHub: run `gh auth login` on the computer." })).toContain("gh auth login");
    expect(pullViewWords(pullView({ t: "error", code: "pr_unavailable", message: "gh is missing" }))).toBe("Could not read the pull request: gh is missing");
    expect(pullViewWords({ kind: "pr", pr: info(status({ can: true, reason: null, detail: "" })) })).toBeNull();
  });

  it("words a reason for a pull request as read", () => {
    const pr = info(status({ can: false, reason: "conflicts", detail: "with master" }));
    expect(reasonWords(pr)).toBe("it conflicts with master");
    expect(reasonWords({ ...pr, base: "main" })).toBe("it conflicts with main");
  });
});
