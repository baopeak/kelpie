import { describe, expect, it } from "vitest";
import { checkSend, Composer, EnterGuard, isLongPaste, LONG_PASTE_CHARS, MAX_SEND_CHARS } from "./composer-core";

describe("EnterGuard", () => {
  const clock = () => {
    let t = 1000;
    return { now: () => t, tick: (ms: number) => (t += ms) };
  };

  it("sends on a plain Enter and not on Shift+Enter or other keys", () => {
    const g = new EnterGuard(clock().now);
    expect(g.sends({ key: "Enter" })).toBe(true);
    expect(g.sends({ key: "Enter", ctrlKey: true })).toBe(true);
    expect(g.sends({ key: "Enter", shiftKey: true })).toBe(false);
    expect(g.sends({ key: "a" })).toBe(false);
  });

  it("an Enter that confirms a composition (Telex, VNI, Japanese) is not a send", () => {
    const c = clock();
    const g = new EnterGuard(c.now);
    g.compositionStart();
    expect(g.sends({ key: "Enter" })).toBe(false);
    expect(g.sends({ key: "Enter", isComposing: true })).toBe(false);
    g.compositionEnd();
    c.tick(500);
    expect(g.sends({ key: "Enter" })).toBe(true);
  });

  it("Safari's order (compositionend, then a keydown with isComposing false) is still the confirmation", () => {
    const c = clock();
    const g = new EnterGuard(c.now);
    g.compositionStart();
    g.compositionEnd();
    c.tick(5);
    expect(g.sends({ key: "Enter", isComposing: false })).toBe(false);
    c.tick(200);
    expect(g.sends({ key: "Enter" })).toBe(true);
  });

  it("keyCode 229 (an input method handled the key) never sends", () => {
    const g = new EnterGuard(clock().now);
    expect(g.sends({ key: "Enter", keyCode: 229 })).toBe(false);
  });
});

describe("sending and pasting", () => {
  it("refuses empty and over-long text, which the daemon would refuse too", () => {
    expect(checkSend("  \n")).toEqual({ ok: false, reason: "empty" });
    expect(checkSend("x".repeat(MAX_SEND_CHARS + 1))).toEqual({ ok: false, reason: "too_long" });
    expect(checkSend("hello")).toEqual({ ok: true, text: "hello" });
  });

  it("asks before a long or many-line paste", () => {
    expect(isLongPaste("short")).toBe(false);
    expect(isLongPaste("x".repeat(LONG_PASTE_CHARS + 1))).toBe(true);
    expect(isLongPaste("a\n".repeat(30))).toBe(true);
  });
});

describe("Composer history", () => {
  it("walks older and newer messages and brings the draft back", () => {
    const c = new Composer();
    c.sent("one");
    c.sent("two");
    expect(c.history).toEqual(["two", "one"]);
    expect(c.step(1, "my draft")).toBe("two");
    expect(c.step(1, "two")).toBe("one");
    expect(c.step(1, "one")).toBeNull();
    expect(c.step(-1, "one")).toBe("two");
    expect(c.step(-1, "two")).toBe("my draft");
    expect(c.step(-1, "my draft")).toBeNull();
  });

  it("has nothing to step to without history, and forgets a repeated message's older copy", () => {
    const c = new Composer();
    expect(c.step(1, "x")).toBeNull();
    c.sent("a");
    c.sent("b");
    c.sent("a");
    expect(c.history).toEqual(["a", "b"]);
  });

  it("editing a recalled message starts the walk again from the edit", () => {
    const c = new Composer(["b", "a"]);
    expect(c.step(1, "")).toBe("b");
    c.edited();
    expect(c.step(1, "b!")).toBe("b");
    expect(c.step(-1, "b")).toBe("b!");
  });
});
