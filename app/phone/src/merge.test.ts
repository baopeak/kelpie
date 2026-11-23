import { describe, expect, it, vi } from "vitest";
import type { Outcome } from "./link";
import { recheckChallenge } from "./lock";
import { challengeOutcome, DEFAULT_METHOD, isMergeMethod, MERGE_METHODS, mergeBlockWords, mergeCommand, mergeOffered, mergeOutcome, mergeRequestId, mergeRequestOf, methodWords, refusalWords, shortHead, type MergeRequest } from "./merge";
import type { PullInfo } from "./pull-line";

vi.mock("./i18n", () => ({ t: (s: string, vars?: Record<string, string | number>) => (vars ? s.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : s) }));

const HEAD = "0123456789abcdef0123456789abcdef01234567";
const pr: PullInfo = { repo: "o/n", number: 12, title: "Fix the thing", url: "https://github.com/o/n/pull/12", base: "master", head: HEAD, can: true, reason: null, detail: "" };
const asked: MergeRequest = mergeRequestOf("p1", pr, DEFAULT_METHOD);

describe("merging a pull request from the phone", () => {
  it("asks for what the line showed, squashing unless the person chooses another way", () => {
    expect(asked).toEqual({ project: "p1", repo: "o/n", number: 12, head: HEAD, method: "squash" });
    expect(DEFAULT_METHOD).toBe("squash");
    expect(MERGE_METHODS).toEqual(["squash", "merge", "rebase"]);
    expect(MERGE_METHODS.every(isMergeMethod)).toBe(true);
    for (const other of ["", "Squash", "--admin", "fast-forward"]) expect(isMergeMethod(other), other).toBe(false);
    expect(shortHead(HEAD)).toBe("0123456");
    expect(MERGE_METHODS.map(methodWords)).toEqual(["Squash and merge", "Create a merge commit", "Rebase and merge"]);
  });

  it("builds the line the passkey signs as the computer does: host, repository, pull request, head commit and method", () => {
    // The same string `pulls::bound_command` builds; a change to either breaks every merge, so it is pinned.
    expect(mergeCommand(asked)).toBe(`merge github.com o/n #12 ${HEAD} squash`);
    const base = mergeCommand(asked);
    for (const other of [{ ...asked, repo: "o/m" }, { ...asked, number: 13 }, { ...asked, head: "f".repeat(40) }, { ...asked, method: "rebase" as const }]) expect(mergeCommand(other)).not.toBe(base);
    expect(mergeRequestId("abc")).toBe("merge:abc");
    // The assertion is over this request, this line and this time: a different merge's, or a dangerous approval's, is another challenge.
    const hex = (b: Uint8Array) => [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
    const mine = hex(recheckChallenge(mergeRequestId("n1"), base, 1000));
    expect(mine).toHaveLength(64);
    expect(hex(recheckChallenge(mergeRequestId("n2"), base, 1000))).not.toBe(mine);
    expect(hex(recheckChallenge(mergeRequestId("n1"), mergeCommand({ ...asked, method: "merge" }), 1000))).not.toBe(mine);
    expect(hex(recheckChallenge(mergeRequestId("n1"), base, 1001))).not.toBe(mine);
    expect(hex(recheckChallenge("1700000000000", base, 1000))).not.toBe(mine);
  });

  it("offers Merge only while the line says it can be merged and this phone may merge", () => {
    expect(mergeOffered({ pr, control: true, api: true })).toBe(true);
    expect(mergeOffered({ pr, control: false, api: true })).toBe(false);
    expect(mergeOffered({ pr, control: true, api: false })).toBe(false);
    expect(mergeOffered({ pr: null, control: true, api: true })).toBe(false);
    expect(mergeOffered({ pr: { ...pr, can: false, reason: "draft" }, control: true, api: true })).toBe(false);
  });

  it("signs nothing unless the computer describes the merge that was chosen", () => {
    // The computer's answer: what it read itself, and the line made from it.
    const said_ = (m: MergeRequest) => ({ repo: m.repo, number: m.number, head: m.head, method: m.method, title: "Fix the thing", command: mergeCommand(m) });
    const ok = (over: Partial<Extract<Outcome, { t: "ack" }>> = {}): Outcome => ({ t: "ack", result: "ok", nonce: "n1", expiry: 61_000, ...said_(asked), ...over });
    expect(challengeOutcome(ok(), asked)).toEqual({ ok: true, nonce: "n1", expiry: 61_000, command: mergeCommand(asked), request: asked, title: "Fix the thing" });
    // The pull request or head it read is not the one the person tapped (it changed since the line was shown): refused, whatever line is signed for it.
    for (const other of [{ ...asked, number: 13 }, { ...asked, head: "f".repeat(40) }, { ...asked, repo: "o/m" }]) {
      const r = challengeOutcome(ok(said_(other)), asked);
      expect(r.ok, mergeCommand(other)).toBe(false);
      expect(r.ok === false && r.message).toContain("changed since you looked");
    }
    // Another method, or a line that does not describe what the answer says.
    for (const other of [ok(said_({ ...asked, method: "rebase" })), ok({ command: mergeCommand({ ...asked, number: 13 }) }), ok({ command: mergeCommand({ ...asked, method: "rebase" }) })]) {
      const r = challengeOutcome(other, asked);
      expect(r.ok).toBe(false);
      expect(r.ok === false && r.message).toContain("another merge");
    }
    for (const broken of [ok({ nonce: undefined }), ok({ nonce: "" }), ok({ expiry: undefined }), ok({ command: undefined }), ok({ repo: undefined }), ok({ number: undefined }), ok({ head: undefined }), ok({ title: undefined })]) expect(challengeOutcome(broken, asked).ok).toBe(false);
    const said = (o: Outcome) => {
      const r = challengeOutcome(o, asked);
      return r.ok ? "ok" : r.message;
    };
    expect(said({ t: "offline" })).toContain("nothing was merged");
    expect(said({ t: "unknown" })).toContain("did not answer");
    expect(said({ t: "error", code: "no_control", message: "English" })).toContain("control");
    expect(said({ t: "error", code: "no_passkey", message: "English" })).toContain("passkey");
    expect(said({ t: "error", code: "merges_waiting", message: "English" })).toContain("waiting for a passkey");
  });

  it("says why Merge is not offered, from what the computer says about control and whether the passkey opened this page", () => {
    expect(mergeBlockWords("ok", "control", true)).toBeNull();
    expect(mergeBlockWords("ok", "control", false)).toContain("unlock");
    for (const [control, words] of [["hosted_app", "HTTPS"], ["plain_page", "HTTPS"], ["via_relay", "HTTPS"], ["not_granted", "allow control"], ["lock_off", "app lock"], ["no_passkey", "passkey"], ["locked", "unlock"]] as const) {
      expect(mergeBlockWords(control, "control", true), control).toContain(words);
    }
    // A computer that says nothing about it, or something this app does not know: the device's permission decides.
    for (const control of [undefined, "a_later_reason"]) {
      expect(mergeBlockWords(control, "control", true), String(control)).toBeNull();
      expect(mergeBlockWords(control, "write", true), String(control)).toContain("allow control");
      expect(mergeBlockWords(control, undefined, true), String(control)).toContain("allow control");
    }
  });

  it("says what became of a merge, and says so when it cannot know", () => {
    expect(mergeOutcome({ t: "ack", result: "merged" })).toEqual({ ok: true });
    // GitHub's own refusal is shown as it came.
    expect(mergeOutcome({ t: "error", code: "merge_refused", message: "Pull request is not mergeable: the base branch policy prohibits the merge." })).toEqual({ ok: false, message: "Pull request is not mergeable: the base branch policy prohibits the merge.", unsure: false });
    expect(mergeOutcome({ t: "error", code: "challenge", message: "English" })).toMatchObject({ ok: false, unsure: false });
    expect(refusalWords("challenge", "English")).toContain("Start again");
    expect(refusalWords("recheck", "English")).toContain("did not confirm");
    expect(refusalWords("lock_refused", "English")).toContain("did not accept");
    expect(refusalWords("something_new", "English from the computer")).toBe("English from the computer");
    // Nothing went out, or it went out and nothing came back: the second is never said as "not merged".
    expect(mergeOutcome({ t: "offline" })).toMatchObject({ ok: false, unsure: false });
    const lost = mergeOutcome({ t: "unknown" });
    expect(lost).toMatchObject({ ok: false, unsure: true });
    expect(lost.ok === false && lost.message).toContain("not known whether");
    // An answer that is neither a merge nor an error is not taken for one.
    expect(mergeOutcome({ t: "ack", result: "ok" })).toMatchObject({ ok: false, unsure: true });
    // The computer took the merge and the pull request is not merged (a merge queue), or it is still carrying out the first of two identical requests.
    const queued = mergeOutcome({ t: "ack", result: "pending", state: "open" });
    expect(queued).toMatchObject({ ok: false, unsure: true });
    expect(queued.ok === false && queued.message).toContain("not merged yet");
    expect(mergeOutcome({ t: "error", code: "in_progress", message: "English" })).toMatchObject({ ok: false, unsure: true });
  });
});
