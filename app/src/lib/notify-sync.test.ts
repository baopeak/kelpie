import { describe, expect, it, vi } from "vitest";

vi.mock("./store", () => ({ store: {} }));
vi.mock("./ipc", () => ({ daemon: vi.fn() }));

const { makeReporter } = await import("./notify-sync");

describe("makeReporter", () => {
  it("sends the window's state once and again only when it changes", () => {
    const sent: unknown[] = [];
    const r = makeReporter((x) => sent.push(x));
    const t = 1_000_000;
    r.report(true, ["b", "a"], t);
    r.report(true, ["a", "b"], t + 5_000);
    expect(sent).toEqual([{ focused: true, visible: ["a", "b"], active_at: t }]);
    r.report(true, ["a"], t);
    r.report(false, ["a"], t);
    r.report(false, ["z"], t);
    expect(sent).toEqual([
      { focused: true, visible: ["a", "b"], active_at: t },
      { focused: true, visible: ["a"], active_at: t },
      { focused: false, visible: [], active_at: t },
    ]);
  });

  it("an unfocused window shows nothing, whatever is laid out", () => {
    const sent: { visible: string[] }[] = [];
    makeReporter((x) => sent.push(x)).report(false, ["a", "b"], 1);
    expect(sent[0].visible).toEqual([]);
  });

  it("sends again after a reconnection even when nothing changed", () => {
    const sent: unknown[] = [];
    const r = makeReporter((x) => sent.push(x));
    r.report(true, ["a"], 1);
    r.reset();
    r.report(true, ["a"], 1);
    expect(sent).toHaveLength(2);
  });

  it("sends again when the person has been active a minute later, so the daemon's idea of away stays current", () => {
    const sent: { active_at: number }[] = [];
    const r = makeReporter((x) => sent.push(x));
    r.report(true, ["a"], 960_000);
    r.report(true, ["a"], 990_000);
    r.report(true, ["a"], 1_020_000);
    expect(sent.map((x) => x.active_at)).toEqual([960_000, 1_020_000]);
  });
});
