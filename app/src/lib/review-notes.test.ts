import { describe, expect, it, vi } from "vitest";
import { mergeNotes, migrateNotes, notesForKey, pendingNotes } from "./review-notes";
import type { ReviewNote } from "./types";

const note = (id: string, path = "a.ts", line = 1): ReviewNote => ({ id, path, line, side: "new", text: "code", note: `note ${id}`, at: 1 });

describe("notesForKey", () => {
  const byKey = { "c:/w/app": [note("1")], "c:/w/app/.claude/worktrees/x": [note("2")], "c:/w/other": [note("3")] };
  it("reads the notes filed under exactly that key", () => {
    expect(notesForKey(byKey, "c:/w/app").map((n) => n.id)).toEqual(["1"]);
    expect(notesForKey(byKey, "c:/w/app/.claude/worktrees/x").map((n) => n.id)).toEqual(["2"]);
  });
  it("never shows the notes of the folder around a worktree, or of a sibling that shares a prefix", () => {
    expect(notesForKey(byKey, "c:/w/app/.claude/worktrees/y")).toEqual([]);
    expect(notesForKey(byKey, "c:/w/app/src")).toEqual([]);
    expect(notesForKey(byKey, "c:/w/application")).toEqual([]);
    expect(notesForKey({}, "c:/w/app")).toEqual([]);
  });
});

describe("mergeNotes", () => {
  it("lists the daemon's notes first and adds the app's that the daemon lacks, once", () => {
    expect(mergeNotes([note("1"), note("2")], [note("2"), note("3")]).map((n) => n.id)).toEqual(["1", "2", "3"]);
  });
});

describe("pendingNotes", () => {
  it("keeps the notes of projects that are still in the sidebar", () => {
    const local = { p1: [note("1")], p2: [], gone: [note("9")] };
    expect(pendingNotes(local, [{ id: "p1", path: "C:/w/a" }, { id: "p2", path: "C:/w/b" }])).toEqual([{ projectId: "p1", path: "C:/w/a", notes: [note("1")] }]);
    expect(pendingNotes(undefined, [{ id: "p1", path: "C:/w/a" }])).toEqual([]);
  });
});

describe("migrating the notes of the app's state to the daemon", () => {
  const pending = [
    { projectId: "p1", path: "C:/w/a", notes: [note("1"), note("2", "b.ts", 4)] },
    { projectId: "p2", path: "C:/w/b", notes: [note("3")] },
  ];

  it("copies the state first, moves every note with its id, and only then lets a project's go", async () => {
    const order: string[] = [];
    const io = {
      backup: vi.fn(async () => (order.push("backup"), "C:/state.notes.bak.json")),
      add: vi.fn(async (path: string, n: ReviewNote) => void order.push(`add ${path} ${n.id}`)),
      drop: vi.fn((id: string) => void order.push(`drop ${id}`)),
    };
    expect(await migrateNotes(pending, io)).toEqual({ moved: 3, failed: [], noBackup: false });
    expect(order).toEqual(["backup", "add C:/w/a 1", "add C:/w/a 2", "drop p1", "add C:/w/b 3", "drop p2"]);
  });

  it("moves nothing when the state could not be backed up", async () => {
    const io = { backup: async () => null, add: vi.fn(), drop: vi.fn() };
    expect(await migrateNotes(pending, io)).toEqual({ moved: 0, failed: [], noBackup: true });
    expect(io.add).not.toHaveBeenCalled();
    expect(io.drop).not.toHaveBeenCalled();
  });

  it("keeps a project's notes when the daemon refuses one, and goes on with the others", async () => {
    const dropped: string[] = [];
    const io = {
      backup: async () => "bak",
      add: async (_path: string, n: ReviewNote) => {
        if (n.id === "2") throw new Error("refused");
      },
      drop: (id: string) => void dropped.push(id),
    };
    expect(await migrateNotes(pending, io)).toEqual({ moved: 1, failed: ["p1"], noBackup: false });
    expect(dropped).toEqual(["p2"]);
  });

  it("does nothing, not even a backup, when there is nothing to move", async () => {
    const io = { backup: vi.fn(), add: vi.fn(), drop: vi.fn() };
    expect(await migrateNotes([], io)).toEqual({ moved: 0, failed: [], noBackup: false });
    expect(io.backup).not.toHaveBeenCalled();
  });
});
