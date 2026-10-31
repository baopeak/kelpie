import { describe, expect, it } from "vitest";
import type { ChatItem } from "./chat-model";
import { lastPromptBefore, pinnedIndex, pinnedPromptId, renderedPrompts } from "./pinned-prompt";

const box = (top: number, bottom: number) => ({ top, bottom });

describe("pinnedIndex", () => {
  it("has nothing to pin in an empty list", () => {
    expect(pinnedIndex([], 0)).toBe(-1);
    expect(pinnedIndex([], 500)).toBe(-1);
  });

  it("pins nothing before the first prompt has ended", () => {
    const prompts = [box(100, 150)];
    expect(pinnedIndex(prompts, 0)).toBe(-1);
    expect(pinnedIndex(prompts, 100)).toBe(-1);
    expect(pinnedIndex(prompts, 149)).toBe(-1);
  });

  it("counts a prompt whose bottom equals scrollTop as off-screen", () => {
    expect(pinnedIndex([box(100, 150)], 150)).toBe(0);
    expect(pinnedIndex([box(100, 150), box(400, 450)], 450)).toBe(1);
  });

  it("pins nothing while a later prompt straddles the top, since it is still on screen", () => {
    const prompts = [box(100, 150), box(400, 450)];
    expect(pinnedIndex(prompts, 400)).toBe(-1);
    expect(pinnedIndex(prompts, 449)).toBe(-1);
  });

  it("keeps the earlier prompt until the next one reaches the top", () => {
    const prompts = [box(100, 150), box(400, 450)];
    expect(pinnedIndex(prompts, 399)).toBe(0);
  });

  it("pins the last prompt once it is past", () => {
    expect(pinnedIndex([box(100, 150), box(400, 450), box(800, 850)], 5000)).toBe(2);
  });

  it("agrees with a linear scan over many prompts", () => {
    const prompts = Array.from({ length: 1001 }, (_, i) => box(i * 100, i * 100 + 40));
    const linear = (scrollTop: number) => {
      let last = -1;
      for (let i = 0; i < prompts.length; i++) if (prompts[i].bottom <= scrollTop) last = i;
      const next = prompts[last + 1];
      return next && next.top <= scrollTop ? -1 : last;
    };
    for (let scrollTop = 0; scrollTop < 101000; scrollTop += 7) expect(pinnedIndex(prompts, scrollTop)).toBe(linear(scrollTop));
  });
});

describe("pinnedPromptId", () => {
  const rendered = [{ id: "b", ...box(100, 150) }, { id: "c", ...box(400, 450) }];

  it("names the rendered prompt that ended above the top", () => {
    expect(pinnedPromptId(rendered, 200, "a")).toBe("b");
    expect(pinnedPromptId(rendered, 900, "a")).toBe("c");
  });

  it("falls back to the earlier prompt above the first rendered one", () => {
    expect(pinnedPromptId(rendered, 50, "a")).toBe("a");
    expect(pinnedPromptId([], 50, "a")).toBe("a");
  });

  it("does not fall back while a rendered prompt straddles the top: it is still on screen", () => {
    expect(pinnedPromptId(rendered, 120, "a")).toBeNull();
    expect(pinnedPromptId(rendered, 420, "a")).toBeNull();
  });

  it("leaves the bar off while the next prompt is right under it, where a jump lands it", () => {
    expect(pinnedPromptId(rendered, 350, "a", 52)).toBeNull();
    expect(pinnedPromptId(rendered, 340, "a", 52)).toBe("b");
    expect(pinnedPromptId(rendered, 60, "a", 52)).toBeNull();
    expect(pinnedPromptId(rendered, 40, "a", 52)).toBe("a");
  });

  it("is empty with no user message above the viewport", () => {
    expect(pinnedPromptId(rendered, 50, null)).toBeNull();
    expect(pinnedPromptId([], 0, null)).toBeNull();
  });
});

describe("lastPromptBefore", () => {
  const user = (id: string, text = "hi"): ChatItem => ({ kind: "user", id, text });
  const reply = (id: string): ChatItem => ({ kind: "assistant", id, text: "ok" });

  it("finds the closest prompt before the rendered window", () => {
    expect(lastPromptBefore([user("a"), reply("r"), user("b"), reply("s"), user("c")], 4)).toBe("b");
  });

  it("skips a prompt with no text, which has no row to jump to", () => {
    expect(lastPromptBefore([user("a"), user("b", "")], 2)).toBe("a");
  });

  it("is empty when everything is rendered or nothing earlier is a prompt", () => {
    expect(lastPromptBefore([user("a")], 0)).toBeNull();
    expect(lastPromptBefore([reply("r"), user("a")], 1)).toBeNull();
  });
});

describe("renderedPrompts", () => {
  it("keeps only user items that show a row, in order", () => {
    const items: ChatItem[] = [
      { kind: "user", id: "a", text: "one" },
      { kind: "assistant", id: "r", text: "reply" },
      { kind: "user", id: "b", text: "" },
      { kind: "user", id: "c", text: "two" },
    ];
    expect(renderedPrompts(items).map((u) => u.id)).toEqual(["a", "c"]);
  });

  it("counts a picture without text as a row", () => {
    const items: ChatItem[] = [
      { kind: "user", id: "a", text: "", images: [{ name: "x.png" }] },
      { kind: "user", id: "b", text: "", images: [] },
    ];
    expect(renderedPrompts(items).map((u) => u.id)).toEqual(["a"]);
    expect(lastPromptBefore(items, 2)).toBe("a");
  });
});
