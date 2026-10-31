import { describe, expect, it } from "vitest";
import { ACK_AFTER_MS, HELD_FRAMES, ScreenSync, type ScreenFrame } from "./screen-sync";

function rig() {
  const acks: [string, number][] = [];
  const refollows: string[] = [];
  const timers: { fn: () => void; ms: number; id: number }[] = [];
  let id = 0;
  const sync = new ScreenSync("t1", {
    ack: (i, rev) => acks.push([i, rev]),
    refollow: (i) => refollows.push(i),
    setTimeout: (fn, ms) => (timers.push({ fn, ms, id: ++id }), id),
    clearTimeout: (h) => {
      const i = timers.findIndex((t) => t.id === h);
      if (i >= 0) timers.splice(i, 1);
    },
  });
  return { sync, acks, refollows, timers };
}

const frame = (over: Partial<ScreenFrame>): ScreenFrame => ({
  id: "t1", rev: 1, base: null, shift: 0, rows: [], height: 3, cursor: null, prompt: null, waiting_since: null, title: null, server_ts: 1, level: 0, ...over,
});

describe("ScreenSync frames", () => {
  it("keeps a screen the daemon may still build on while acknowledgements are on their way", () => {
    const { sync, acks, refollows } = rig();
    // Whole frames, because the daemon has not heard an acknowledgement yet.
    sync.apply(frame({ rev: 309, rows: [[0, "309"]] }));
    sync.apply(frame({ rev: 310, rows: [[0, "310"]] }));
    expect(acks.at(-1)).toEqual(["t1", 310]);
    sync.apply(frame({ rev: 311, rows: [[0, "311"]] }));
    sync.apply(frame({ rev: 312, rows: [[0, "312"]] }));
    expect(acks.at(-1)).toEqual(["t1", 312]);
    // It had heard only 310 when it made these.
    expect(sync.apply(frame({ rev: 313, base: 310, rows: [[1, "313"]] }))).toBe(true);
    expect(sync.apply(frame({ rev: 314, base: 310, rows: [[1, "314"]] }))).toBe(true);
    expect(refollows).toEqual([]);
    expect(sync.view.rows).toEqual(["310", "314", ""]);
    // A later base lets the older ones go, and the frame it names is applied.
    expect(sync.apply(frame({ rev: 315, base: 312, rows: [[1, "315"]] }))).toBe(true);
    expect(sync.view.rows).toEqual(["312", "315", ""]);
    expect(sync.apply(frame({ rev: 316, base: 310, rows: [] }))).toBe(false);
    expect(refollows).toEqual(["t1"]);
  });

  it("carrying on after a reconnect shows the acknowledged screen, not a later prompt with its rows", () => {
    const { sync } = rig();
    sync.apply(frame({ rev: 1, rows: [[0, "a"]], title: "one" }));
    sync.ackNow();
    const asked = { keys: ["1"], line: false };
    sync.apply(frame({ rev: 2, base: 1, rows: [[0, "b"]], prompt: asked, title: "two", waiting_since: 5 }));
    expect(sync.view.prompt).toEqual(asked);
    sync.resume();
    expect(sync.view).toMatchObject({ rows: ["a", "", ""], prompt: null, title: "one", waitingSince: null });
    expect(sync.rev).toBe(1);
  });

  it("lets the first to arrive go past the bound, however they are numbered", () => {
    const { sync, refollows } = rig();
    for (let rev = 50; rev < 50 + HELD_FRAMES; rev++) sync.apply(frame({ rev, rows: [[0, String(rev)]] }));
    // A daemon of another run may number lower than what is held: that is what it builds on next.
    sync.apply(frame({ rev: 3, rows: [[0, "three"]] }));
    expect(sync.apply(frame({ rev: 4, base: 3, rows: [[1, "four"]] }))).toBe(true);
    expect(refollows).toEqual([]);
    expect(sync.view.rows).toEqual(["three", "four", ""]);
  });

  it("holds a bounded number of screens whatever the daemon sends, and always the newest", () => {
    const { sync } = rig();
    for (let rev = 1; rev <= 100; rev++) sync.apply(frame({ rev, rows: [[0, String(rev)]] }));
    const held = (sync as unknown as { held: Map<number, unknown> }).held;
    expect(held.size).toBe(HELD_FRAMES);
    expect([...held.keys()].sort((a, b) => a - b)).toEqual(Array.from({ length: HELD_FRAMES }, (_, i) => 100 - HELD_FRAMES + 1 + i));
    expect(sync.haveRev()).toBe(100);
    // Past the bound the oldest is gone, and asking for it starts over: slow, never wrong.
    expect(sync.apply(frame({ rev: 101, base: 100 - HELD_FRAMES, rows: [] }))).toBe(false);
  });

  it("a whole frame (base null) builds the rows from nothing", () => {
    const { sync } = rig();
    expect(sync.apply(frame({ rows: [[0, "a"], [2, "c"]] }))).toBe(true);
    expect(sync.view.rows).toEqual(["a", "", "c"]);
    expect(sync.rev).toBe(1);
  });

  it("a diff scrolls by shift, then replaces the rows it names", () => {
    const { sync } = rig();
    sync.apply(frame({ rows: [[0, "one"], [1, "two"], [2, "three"]] }));
    sync.apply(frame({ rev: 2, base: 1, shift: 1, rows: [[2, "four"]] }));
    expect(sync.view.rows).toEqual(["two", "three", "four"]);
  });

  it("resizes to the frame's height", () => {
    const { sync } = rig();
    sync.apply(frame({ rows: [[0, "a"], [1, "b"], [2, "c"]] }));
    sync.apply(frame({ rev: 2, base: 1, height: 2, rows: [] }));
    expect(sync.view.rows).toEqual(["a", "b"]);
  });

  it("carries the prompt, waiting time and title of the newest frame", () => {
    const { sync } = rig();
    sync.apply(frame({ prompt: { keys: ["1", "2"], question: "Proceed?", line: false }, waiting_since: 77, title: "T" }));
    expect(sync.view).toMatchObject({ prompt: { keys: ["1", "2"] }, waitingSince: 77, title: "T" });
  });

  it("a frame that follows one it does not hold is not applied: it follows again from nothing", () => {
    const { sync, refollows } = rig();
    sync.apply(frame({ rows: [[0, "a"]] }));
    expect(sync.apply(frame({ rev: 5, base: 4, rows: [[0, "x"]] }))).toBe(false);
    expect(sync.view.rows[0]).toBe("a");
    expect(refollows).toEqual(["t1"]);
    expect(sync.haveRev()).toBeNull();
  });

  it("keeps only the screens from the one the frame builds on", () => {
    const { sync } = rig();
    sync.apply(frame({ rows: [[0, "a"]] }));
    sync.ackNow();
    sync.apply(frame({ rev: 2, base: 1, rows: [[0, "b"]] }));
    sync.apply(frame({ rev: 3, base: 2, rows: [[0, "c"]] }));
    // Frame 3 built on 2, so frame 1 is gone; a frame built on 3 drops 2 as well.
    expect(sync.apply(frame({ rev: 4, base: 3, rows: [] }))).toBe(true);
    expect(sync.apply(frame({ rev: 5, base: 2, rows: [] }))).toBe(false);
  });
});

describe("ScreenSync acknowledgements", () => {
  it("acknowledges every second frame, or 250 ms after the first one alone", () => {
    const { sync, acks, timers } = rig();
    sync.apply(frame({ rows: [[0, "a"]] }));
    expect(acks).toEqual([]);
    expect(timers).toHaveLength(1);
    expect(timers[0].ms).toBe(ACK_AFTER_MS);
    sync.apply(frame({ rev: 2, base: null, rows: [[0, "b"]] }));
    expect(acks).toEqual([["t1", 2]]);
    expect(timers).toHaveLength(0);
    sync.apply(frame({ rev: 3, base: 2, rows: [[0, "c"]] }));
    timers[0].fn();
    expect(acks).toEqual([["t1", 2], ["t1", 3]]);
  });

  it("resumes from the last acknowledged frame when the connection is made again", () => {
    const { sync } = rig();
    sync.apply(frame({ rows: [[0, "a"]] }));
    sync.ackNow();
    sync.apply(frame({ rev: 2, base: 1, rows: [[0, "b"]] }));
    expect(sync.haveRev()).toBe(1);
    sync.resume();
    expect(sync.view.rows[0]).toBe("a");
    expect(sync.rev).toBe(1);
    // The daemon then builds on frame 1.
    expect(sync.apply(frame({ rev: 3, base: 1, rows: [[1, "z"]] }))).toBe(true);
    expect(sync.view.rows).toEqual(["a", "z", ""]);
  });

  it("resume without an acknowledged frame starts from nothing; a new daemon run resets everything", () => {
    const { sync } = rig();
    sync.apply(frame({ rows: [[0, "a"]] }));
    sync.resume();
    expect(sync.haveRev()).toBeNull();
    expect(sync.view.rows).toEqual([]);
    sync.apply(frame({ rows: [[0, "a"]] }));
    sync.ackNow();
    sync.reset();
    expect(sync.haveRev()).toBeNull();
    expect(sync.rev).toBeNull();
  });
});
