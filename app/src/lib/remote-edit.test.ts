import { describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("./ipc", () => ({ onEvent: vi.fn(), sftpStat: vi.fn(), xferList: vi.fn(), xferStart: vi.fn() }));
vi.mock("./store", () => ({ store: { notify: vi.fn(), state: { projects: [] } } }));
vi.mock("./ssh-hosts", () => ({ checkHost: vi.fn(), hostProject: vi.fn(), isHostKeyError: () => false }));

import { afterUpload, uploadJob, type EditSession } from "./remote-edit";
import type { XferJob } from "./types";

const session: EditSession = {
  id: "s", dest: "prod", remotePath: "/srv/app/config.yml", localPath: "C:\\Users\\me\\AppData\\Local\\Kelpie\\sftp-edit\\s\\config.yml", name: "config.yml",
  base: { size: 120, mtime: 1_700_000_000 }, auto: true, state: "open", lastUpload: null, error: null,
};

const job = (over: Partial<XferJob>): XferJob => ({
  id: "j", kind: "copy", src: {}, dst: { dest: "prod" }, items: [], dst_dir: "/srv/app", conflict: "ask", preserve: false, mode: null, dirs_only_x: false, expect: null, state: "done",
  attempt: 0, next_s: 0, totals: { files: 1, bytes: 1, done_files: 1, done_bytes: 1, skipped: 0, failed: 0 }, speed_bps: 0, eta_s: null, current: null, questions: [], error: null,
  failures: [], walk: { folder_links: 0, broken_links: 0, unreadable_names: 0, special: 0, unlistable: 0 }, renamed: [], created: 1, updated: 1, finished: 1, ...over,
});

describe("uploading an edited file", () => {
  it("replaces only the server copy it was opened from", () => {
    expect(uploadJob(session)).toEqual({
      kind: "copy", src: {}, dst: { dest: "prod" }, items: [session.localPath], dst_dir: "/srv/app", conflict: "ask", preserve: false, expect: { size: 120, mtime: 1_700_000_000 },
    });
  });
  it("takes the new server copy as the base after an upload", () => {
    const next = afterUpload(session, job({}), { size: 130, mtime: 1_700_000_100 });
    expect(next.state).toBe("open");
    expect(next.base).toEqual({ size: 130, mtime: 1_700_000_100 });
    expect(next.lastUpload).toBeTypeOf("number");
  });
  it("keeps the base when the person discarded the upload, and reports a failure", () => {
    expect(afterUpload(session, job({ totals: { files: 1, bytes: 1, done_files: 0, done_bytes: 1, skipped: 1, failed: 0 } }), null)).toEqual({ state: "open" });
    expect(afterUpload(session, job({ state: "failed", error: "permission denied" }), null)).toEqual({ state: "error", error: "permission denied" });
  });
});
