import { describe, expect, it } from "vitest";
import { createRepeatGuard, describeCancelled, describeNotify, worthPopup, type NotifyEvent } from "./notify-events";

const ev = (extra: Partial<NotifyEvent> = {}): NotifyEvent => ({
  id: "e1", kind: "finished", session: "s1", project_id: "p1", alive: true, exit_code: null, detail: null, run: null, task: null, error: null, at: 1, ...extra,
});
const ctx = { label: "Claude · docs", projectName: "Kelpie" };

// The wording the app produced itself before the daemon classified status changes.
describe("describeNotify for session events", () => {
  it("finished, waiting and error read as they always did", () => {
    expect(describeNotify(ev({ kind: "finished" }), ctx)).toEqual({ kind: "finished", title: "Claude · docs · Kelpie", body: "Finished" });
    expect(describeNotify(ev({ kind: "waiting" }), ctx)).toEqual({ kind: "waiting", title: "Claude · docs · Kelpie", body: "Needs your input" });
    expect(describeNotify(ev({ kind: "error", alive: true }), ctx)).toEqual({ kind: "error", title: "Claude · docs · Kelpie", body: "Stopped with an error" });
    expect(describeNotify(ev({ kind: "error", alive: false, exit_code: 3 }), ctx).body).toBe("Exited with code 3");
  });

  it("names the host of a remote session and leaves out an unknown project", () => {
    expect(describeNotify(ev(), { label: "Codex", host: "dev@box", projectName: "Kelpie" }).title).toBe("Codex @dev@box · Kelpie");
    expect(describeNotify(ev(), { label: "Codex" }).title).toBe("Codex");
  });
});

describe("describeNotify for the other events", () => {
  it("an automation run starts or fails", () => {
    expect(describeNotify(ev({ kind: "automation", detail: "nightly", session: null }), {})).toEqual({ kind: "automation", title: "Automation · nightly", body: "Started" });
    expect(describeNotify(ev({ kind: "automation", detail: "nightly", error: "no folder" }), {})).toEqual({ kind: "error", title: "Automation · nightly", body: "Failed: no folder" });
  });

  it("a run waiting for approval needs you", () => {
    expect(describeNotify(ev({ kind: "approval", session: null, run: "Release", task: "Deploy", detail: "Release · Deploy" }), {})).toEqual({
      kind: "waiting", title: "Run · Release · Deploy", body: "Waiting for your approval to start",
    });
  });
});

describe("describeCancelled", () => {
  it("explains why the timer was dropped, and stays quiet when the user cancelled it", () => {
    for (const reason of ["exited", "restarted", "conversation", "not_limited", "input", "daemon"]) {
      const note = describeCancelled(reason, "Claude · docs");
      expect(note?.title).toBe("Claude · docs: “continue” was not sent");
      expect(note?.body).toMatch(/^Cancelled because .+\.$/);
    }
    expect(describeCancelled("input", "x")?.body).toContain("you typed");
    expect(describeCancelled("daemon", "x")?.body).toContain("daemon restarted");
    expect(describeCancelled("user", "x")).toBeNull();
    expect(describeCancelled("something else", "x")).toBeNull();
  });
});

// Desktop popups: only what needs the user, and the same one at most once per window.
describe("worthPopup", () => {
  it("pops up for input, approvals, failures and the cost cap", () => {
    for (const k of ["waiting", "error", "cost"] as const) expect(worthPopup(k, false)).toBe(true);
  });

  it("keeps a finished turn in the inbox unless asked for", () => {
    expect(worthPopup("finished", false)).toBe(false);
    expect(worthPopup("finished", true)).toBe(true);
  });

  it("never pops up for automation starts or worktree notes", () => {
    expect(worthPopup("automation", true)).toBe(false);
    expect(worthPopup("worktree", true)).toBe(false);
  });
});

describe("createRepeatGuard", () => {
  it("lets the same key through once per window", () => {
    let now = 0;
    const once = createRepeatGuard(60_000, () => now);
    expect(once("Claude · Finished")).toBe(true);
    now = 30_000;
    expect(once("Claude · Finished")).toBe(false);
    expect(once("Codex · Needs your input")).toBe(true);
    now = 60_000;
    expect(once("Claude · Finished")).toBe(true);
  });
});
