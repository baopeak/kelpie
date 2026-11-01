import { describe, expect, it } from "vitest";
import { artifactKind, attemptUndo, markdownBlocks, safelyTicked, sourceNote, turnStats, turnsByMessage, visibleTurns, type PlanItem, type TurnList, type TurnRecord, type UndoPlan } from "./checkpoints";

describe("attemptUndo", () => {
  const item = (path: string, state: PlanItem["state"], action: PlanItem["action"] = "restore"): PlanItem => ({ path, action, state, with: [], reason: null });
  const planOf = (...items: PlanItem[]): UndoPlan => ({ items, owner_working: false, n: 1 });

  it("sends the ticked files and confirms the risky ones", async () => {
    const calls: [string[], string[]][] = [];
    const undo = async (paths: string[], confirm: string[]) => {
      calls.push([paths, confirm]);
      return { applied: true, files: paths };
    };
    const r = await attemptUndo({ plan: async () => planOf(), undo }, [item("a", "clean"), item("b", "modified"), item("c", "clean")], new Set(["a", "b"]));
    expect(r.kind).toBe("applied");
    expect(calls).toEqual([[["a", "b"], ["b"]]]);
  });

  it("drops a ticked confirmation when the undo is refused, so one more press cannot overwrite the save", async () => {
    const confirmed: string[][] = [];
    const undo = async (_paths: string[], confirm: string[]) => {
      confirmed.push(confirm);
      return { applied: false };
    };
    // The person had ticked `saved`; a save during the undo got it refused, and it is still risky.
    const fresh = planOf(item("saved", "modified"), item("other", "clean"), item("done", "clean", "skip"));
    const api = { plan: async () => fresh, undo };
    const first = await attemptUndo(api, [item("saved", "modified"), item("other", "clean")], new Set(["saved", "other"]));
    if (first.kind === "applied") throw new Error("it was refused");
    expect(first.kind).toBe("refused");
    expect(first.plan).toBe(fresh);
    expect([...first.picked]).toEqual(["other"]);
    await attemptUndo(api, first.plan!.items, first.picked);
    expect(confirmed).toEqual([["saved"], []]);
  });

  it("reports a failure with the plan as it is now, finished files no longer ticked", async () => {
    const fresh = planOf(item("put-back", "clean", "skip"), item("held", "clean"));
    const r = await attemptUndo({ plan: async () => fresh, undo: async () => Promise.reject(new Error("git restore: held")) }, [item("put-back", "clean"), item("held", "clean")], new Set(["put-back", "held"]));
    if (r.kind !== "failed") throw new Error("it failed");
    expect(String(r.error)).toContain("git restore: held");
    expect([...r.picked]).toEqual(["held"]);
  });

  it("keeps nothing ticked when the plan cannot be read again", async () => {
    const r = await attemptUndo({ plan: async () => Promise.reject(new Error("gone")), undo: async () => ({ applied: false }) }, [item("a", "modified")], new Set(["a"]));
    if (r.kind === "applied") throw new Error("it was refused");
    expect(r.plan).toBeNull();
    expect(r.picked.size).toBe(0);
  });

  it("ticks only what is safe to begin with", () => {
    expect([...safelyTicked(planOf(item("a", "clean"), item("b", "modified"), item("c", "shared"), item("d", "clean", "skip")))]).toEqual(["a"]);
  });
});

const rec = (n: number, extra: Partial<TurnRecord> = {}): TurnRecord => ({
  n, kind: n === 0 ? "baseline" : "turn", at: n * 10, started: n * 10 - 5, source: "hook", approximate: false,
  files: 0, added: 0, removed: 0, paths: [], truncated: false, undo_of: null, base_n: n ? n - 1 : null, ...extra,
});

describe("visibleTurns", () => {
  it("lists newest first", () => {
    const list: TurnList = { repo: "/r", enabled: true, repo_off: false, why: null, records: [rec(0), rec(1), rec(2)], first_is_base: false };
    expect(visibleTurns(list).map((r) => r.n)).toEqual([2, 1, 0]);
  });
  it("leaves out the base of the oldest turn once the history is full", () => {
    const list: TurnList = { repo: "/r", enabled: true, repo_off: false, why: null, records: [rec(7), rec(8), rec(9)], first_is_base: true };
    expect(visibleTurns(list).map((r) => r.n)).toEqual([9, 8]);
  });
});

describe("artifactKind", () => {
  it("knows what renders", () => {
    expect(artifactKind("docs/Plan.MD")).toBe("markdown");
    expect(artifactKind("a/b/shot.png")).toBe("image");
    expect(artifactKind("flow.mmd")).toBe("mermaid");
    expect(artifactKind("data.tsv")).toBe("csv");
    expect(artifactKind("src/main.rs")).toBeNull();
    expect(artifactKind("Makefile")).toBeNull();
  });
});

describe("markdownBlocks", () => {
  it("gives each block the line of the source it starts on", () => {
    const src = "# Title\n\nFirst paragraph\nspanning two lines.\n\n- one\n- two\n\n```mermaid\ngraph TD\n```\n";
    const blocks = markdownBlocks(src);
    expect(blocks.map((b) => b.line)).toEqual([1, 3, 6, 9]);
    expect(blocks[0].raw.startsWith("# Title")).toBe(true);
    expect(src.split("\n")[blocks[2].line - 1]).toBe("- one");
  });
  it("copes with a document without a trailing newline and with leading blank lines", () => {
    expect(markdownBlocks("\n\nHello").map((b) => b.line)).toEqual([3]);
    expect(markdownBlocks("")).toEqual([]);
  });
});

describe("turn text", () => {
  it("sums up a turn", () => {
    expect(turnStats({ added: 20, removed: 4 }, "3 files")).toBe("3 files +20 −4");
    expect(turnStats({ added: 0, removed: 0 }, "1 file")).toBe("1 file");
  });
  it("explains approximate and codex boundaries, and says nothing for exact ones", () => {
    expect(sourceNote(rec(1, { approximate: true, source: "idle" }), "gemini")).toMatch(/merged/);
    expect(sourceNote(rec(1), "codex")).toMatch(/turn-complete/);
    expect(sourceNote(rec(1), "claude")).toBe("");
    expect(sourceNote(rec(0, { source: "restart" }), "claude")).toMatch(/resumed/);
  });
});

describe("turnsByMessage", () => {
  const list = (records: TurnRecord[], first_is_base = false): TurnList => ({ repo: "/r", enabled: true, repo_off: false, why: null, records, first_is_base });
  const turn = (n: number, started: number, files = 1, at = started + 3000) => rec(n, { started, at, files });
  const owners = (map: Map<string, TurnRecord[]>) => [...map.entries()].map(([id, recs]) => [id, recs.map((r) => r.n)]);
  it("gives each turn with changes to the last message sent before it started", () => {
    const msgs = [{ id: "u1", at: 100 }, { id: "u2", at: 5000 }, { id: "old" }];
    const map = turnsByMessage(list([rec(0, { at: 50 }), turn(1, 150), turn(2, 160, 0), turn(3, 5100), turn(4, 5200)]), msgs);
    expect(owners(map)).toEqual([["u1", [1]], ["u2", [3, 4]]]);
  });
  it("keeps a turn on its own message when the next one went out before its snapshot finished", () => {
    // A queued message is sent as the turn ends; the record's time is the later snapshot end.
    const map = turnsByMessage(list([turn(1, 150, 1, 2600)]), [{ id: "u1", at: 100 }, { id: "queued", at: 2400 }]);
    expect(owners(map)).toEqual([["u1", [1]]]);
  });
  it("allows a replayed message stamped up to a second after its turn started", () => {
    expect(owners(turnsByMessage(list([turn(1, 1000)]), [{ id: "u1", at: 1800 }]))).toEqual([["u1", [1]]]);
  });
  it("falls back to the record's time when the start is unknown", () => {
    expect(owners(turnsByMessage(list([rec(1, { started: 0, at: 900, files: 1 })]), [{ id: "u1", at: 100 }]))).toEqual([["u1", [1]]]);
  });
  it("matches nothing for earlier turns, undos, undone turns or the base of a full history", () => {
    const records = [turn(1, 3000), turn(2, 5200), turn(3, 5300), rec(4, { kind: "undo", at: 9000, files: 2, undo_of: 3 })];
    expect(turnsByMessage(list(records), [{ id: "u1", at: 5000 }]).get("u1")!.map((r) => r.n)).toEqual([2]);
    expect(turnsByMessage(list([turn(5, 200), turn(6, 300)], true), [{ id: "u1", at: 100 }]).get("u1")!.map((r) => r.n)).toEqual([6]);
  });
});
