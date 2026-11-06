import { describe, expect, it, vi } from "vitest";

vi.mock("./ipc", () => ({ onEvent: vi.fn(), xferList: vi.fn() }));

import { activeCount, applyJob, chipText, directionText, etaText, heldText, jobTitle, nextQuestion, progress, removeJob, speedText, stateText, updateMayInstall } from "./transfers";
import type { XferJob, XferQuestion } from "./types";

const meta = { kind: "file" as const, size: 1, mtime: 1, mode: 0o644, uid: null, gid: null };
const q = (id: number): XferQuestion => ({ id, path: `/x/${id}`, src: meta, dst: meta, identical: false, kind_clash: false, changed: false });

const job = (over: Partial<XferJob> = {}): XferJob => ({
  id: "j", kind: "copy", src: {}, dst: { dest: "prod" }, items: ["C:\\a\\report.pdf"], dst_dir: "/srv", conflict: "ask", preserve: true, mode: null, dirs_only_x: false, expect: null,
  state: "running", attempt: 0, next_s: 0, totals: { files: 1, bytes: 200, done_files: 0, done_bytes: 50, skipped: 0, failed: 0 }, speed_bps: 0, eta_s: null, current: null,
  questions: [], error: null, failures: [], walk: { folder_links: 0, broken_links: 0, unreadable_names: 0, special: 0, unlistable: 0 }, renamed: [], created: 1, updated: 1, finished: null, ...over,
});

describe("the job store", () => {
  it("keeps the newest state, whatever order updates arrive in", () => {
    let jobs = applyJob({}, job({ updated: 5, state: "running" }));
    jobs = applyJob(jobs, job({ updated: 3, state: "queued" }));
    expect(jobs.j.state).toBe("running");
    jobs = applyJob(jobs, job({ updated: 7, state: "done" }));
    expect(jobs.j.state).toBe("done");
    expect(removeJob(jobs, "j")).toEqual({});
    expect(removeJob(jobs, "other")).toBe(jobs);
  });
});

describe("words", () => {
  it("formats speed and ETA, including 0, unknown and more than a day", () => {
    expect(speedText(0)).toBe("");
    expect(speedText(3 * 1024 * 1024)).toBe("3.0 MB/s");
    expect(etaText(null)).toBe("");
    expect(etaText(0)).toBe("1 s");
    expect(etaText(45)).toBe("45 s");
    expect(etaText(600)).toBe("10 min");
    expect(etaText(3 * 3600 + 300)).toBe("3 h 05 min");
    expect(etaText(2 * 86400)).toBe("> 1 day");
  });
  it("progress is unknown until the size is", () => {
    expect(progress(job())).toBe(0.25);
    expect(progress(job({ totals: { files: 0, bytes: 0, done_files: 0, done_bytes: 0, skipped: 0, failed: 0 } }))).toBeNull();
    expect(progress(job({ kind: "delete", totals: { files: 4, bytes: 0, done_files: 1, done_bytes: 0, skipped: 0, failed: 0 } }))).toBe(0.25);
  });
  it("names jobs, directions and states", () => {
    expect(jobTitle(job())).toBe("report.pdf");
    expect(jobTitle(job({ items: ["/a", "/b", "/c"] }))).toBe("3 items");
    expect(directionText(job(), (a) => a.toUpperCase())).toBe("this computer → PROD");
    expect(directionText(job({ kind: "delete", src: { dest: "prod" } }))).toBe("delete on prod");
    expect(stateText(job({ state: "reconnecting", next_s: 5, attempt: 2 }))).toBe("Connection lost: trying again in 5 s (attempt 2)");
    expect(stateText(job({ state: "done", totals: { files: 3, bytes: 3, done_files: 2, done_bytes: 3, skipped: 0, failed: 1 } }))).toBe("Done, 1 could not be copied");
  });
});

describe("updates wait for transfers", () => {
  it("counts only transfers that run or wait to run", () => {
    expect(activeCount({ a: job({ id: "a" }), b: job({ id: "b", state: "paused" }), c: job({ id: "c", state: "interrupted" }), d: job({ id: "d", state: "waiting" }) })).toBe(2);
  });
  it("holds only an update that stops the daemon, and only while transfers run", () => {
    expect(updateMayInstall({ daemonStops: false, transfers: 3 })).toBe(true);
    expect(updateMayInstall({ daemonStops: true, transfers: 3 })).toBe(false);
    expect(updateMayInstall({ daemonStops: true, transfers: 0 })).toBe(true);
  });
  it("says what holds it", () => {
    expect(heldText(0)).toBeNull();
    expect(heldText(1)).toBe("Waiting for 1 file transfer to finish");
    expect(heldText(4)).toBe("Waiting for 4 file transfers to finish");
  });
});

describe("questions and the chip", () => {
  it("shows one question at a time, oldest job first", () => {
    const jobs = { a: job({ id: "a", created: 2, questions: [q(7)] }), b: job({ id: "b", created: 1, questions: [q(3), q(4)] }), c: job({ id: "c" }) };
    expect(nextQuestion(jobs)).toEqual({ job: jobs.b, q: q(3) });
    expect(nextQuestion({ c: job({ id: "c" }) })).toBeNull();
  });
  it("counts running jobs and their overall progress", () => {
    expect(chipText({})).toBeNull();
    expect(chipText({ a: job({ id: "a", state: "done" }) })).toBeNull();
    const jobs = { a: job({ id: "a" }), b: job({ id: "b", totals: { files: 1, bytes: 200, done_files: 0, done_bytes: 130, skipped: 0, failed: 0 } }) };
    expect(chipText(jobs)).toBe("↑↓ 2 · 45 %");
    expect(chipText({ a: job({ id: "a", state: "scanning", totals: { files: 0, bytes: 0, done_files: 0, done_bytes: 0, skipped: 0, failed: 0 } }) })).toBe("↑↓ 1");
  });
});
