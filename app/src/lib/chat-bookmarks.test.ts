// Saved replies against an in-memory file: what is kept, what is refused, that a file which could not
// be read is never written, and what a click on one finds. The file's own rules (the limit, keeping what
// others wrote) belong to the native command and its tests.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./store", () => ({ store: { notify: vi.fn() } }));
vi.mock("./ipc", () => ({ loadBookmarks: vi.fn(), applyBookmarks: vi.fn() }));

import { createClaudeAdapter } from "../chat/claude-adapter";
import { ChatController, mountAttach, type ChatItem } from "../chat/chat-model";
import { MAX_TEXT_BYTES, REVEAL_WAIT_MS, anchorOf, createBookmarks, findReply, sessionFor, type BookmarkChange, type ChatBookmark } from "./chat-bookmarks";

const clone = <T,>(v: T): T => JSON.parse(JSON.stringify(v));
const session = (id = "s1", conversationId?: string | null, name = "Claude 1") => ({ id, name, conversationId });
const project = (id = "p1") => ({ id });

/** The native command in small: removes by id, adds what is not there yet. */
function setup(initial: unknown = [], loadError?: string) {
  const disk = { items: clone(initial) as any[] };
  const notify = vi.fn();
  let seq = 0;
  const load = vi.fn(async () => {
    if (loadError) throw new Error(loadError);
    return clone(disk.items) as unknown;
  });
  const apply = vi.fn(async (change: BookmarkChange) => {
    if (change.remove) disk.items = disk.items.filter((b) => b.id !== change.remove);
    if (change.add && !disk.items.some((b) => b.id === change.add!.id)) disk.items = [...disk.items, clone(change.add)];
    return { items: clone(disk.items) as unknown[], at_limit: null as number | null };
  });
  const open = () => createBookmarks({ load, apply, notify, now: () => 1000 + seq, newId: () => `b${++seq}` });
  return { disk, notify, load, apply, open, marks: open() };
}

const isSaved = (marks: ReturnType<typeof createBookmarks>, sessionId: string, text: string, conversationId?: string | null) => marks.isSaved({ id: sessionId, conversationId }, anchorOf(text));

const saved = (over: Partial<ChatBookmark> = {}): ChatBookmark => ({
  id: "x",
  projectId: "p1",
  sessionId: "s1",
  sessionName: "Claude 1",
  anchor: anchorOf("hello"),
  text: "hello",
  at: 1,
  ...over,
});

describe("anchorOf", () => {
  it("is the same for a reply replayed with other white space, and differs for other text", () => {
    expect(anchorOf("  one\r\ntwo  \n")).toBe(anchorOf("one\ntwo"));
    expect(anchorOf("one two")).toBe(anchorOf("one\n\n  two"));
    expect(anchorOf("one two")).not.toBe(anchorOf("one three"));
  });
});

describe("toggle", () => {
  it("saves a reply with what the list shows, and takes it away again", async () => {
    const { marks, disk, apply } = setup();
    await marks.load();

    await marks.toggle({ text: "hello" }, session(), project());
    expect(isSaved(marks, "s1", "hello")).toBe(true);
    expect(disk.items).toEqual([{ id: "b1", projectId: "p1", sessionId: "s1", sessionName: "Claude 1", anchor: anchorOf("hello"), text: "hello", at: 1000 }]);

    await marks.toggle({ text: "hello" }, session(), project());
    expect(isSaved(marks, "s1", "hello")).toBe(false);
    expect(disk.items).toEqual([]);
    expect(apply.mock.calls.map(([c]) => Object.keys(c))).toEqual([["add"], ["remove"]]);
  });

  it("treats the same text as one bookmark in a session, and as its own in another", async () => {
    const { marks, disk } = setup();
    await marks.load();

    await marks.toggle({ text: "same" }, session("s1"), project());
    await marks.toggle({ text: "same" }, session("s2"), project());
    expect(disk.items).toHaveLength(2);
    expect(isSaved(marks, "s2", "same")).toBe(true);

    await marks.toggle({ text: " same\n" }, session("s1"), project());
    expect(isSaved(marks, "s1", "same")).toBe(false);
    expect(isSaved(marks, "s2", "same")).toBe(true);
  });

  it("saves nothing for a reply of only white space", async () => {
    const { marks, apply } = setup();
    await marks.load();
    await marks.toggle({ text: " \n " }, session(), project());
    expect(apply).not.toHaveBeenCalled();
  });

  it("cuts a long reply on a whole character, says so, and still knows it by its full text", async () => {
    const { marks, disk } = setup();
    await marks.load();
    const long = "ệ".repeat(MAX_TEXT_BYTES);

    await marks.toggle({ text: long }, session(), project());
    const [item] = disk.items as ChatBookmark[];
    expect(item.truncated).toBe(true);
    expect(new TextEncoder().encode(item.text).length).toBeLessThanOrEqual(MAX_TEXT_BYTES);
    expect(item.text.length).toBeGreaterThan(MAX_TEXT_BYTES / 4);
    expect(item.text.endsWith("ệ")).toBe(true);
    expect(isSaved(marks, "s1", long)).toBe(true);

    await marks.toggle({ text: long }, session(), project());
    expect(disk.items).toEqual([]);
  });

  it("does not mark a reply that fits exactly", async () => {
    const { marks, disk } = setup();
    await marks.load();
    await marks.toggle({ text: "a".repeat(MAX_TEXT_BYTES) }, session(), project());
    const [item] = disk.items as ChatBookmark[];
    expect(item.truncated).toBeUndefined();
    expect(item.text).toHaveLength(MAX_TEXT_BYTES);
  });

  it("says so when the native side refuses one more, and shows the file as it came back", async () => {
    const { marks, apply, notify } = setup();
    await marks.load();
    apply.mockResolvedValueOnce({ items: [saved({ id: "theirs", text: "from another window", anchor: anchorOf("from another window") })], at_limit: 500 });

    await marks.toggle({ text: "one too many" }, session(), project());
    expect(notify).toHaveBeenCalledTimes(1);
    expect(notify.mock.calls[0][0]).toContain("500");
    expect(isSaved(marks, "s1", "one too many")).toBe(false);
    expect(isSaved(marks, "s1", "from another window")).toBe(true);
  });

  it("is still saved after a reload", async () => {
    const { marks, open } = setup();
    await marks.load();
    await marks.toggle({ text: "keep me" }, session(), project());

    const again = open();
    expect(isSaved(again, "s1", "keep me")).toBe(false);
    await again.load();
    expect(isSaved(again, "s1", "keep me")).toBe(true);
    expect(isSaved(again, "other", "keep me")).toBe(false);
  });

  it("changes nothing when the write fails, says so, and can be tried again", async () => {
    const { marks, apply, notify, disk } = setup();
    await marks.load();
    apply.mockRejectedValueOnce(new Error("disk full"));

    await marks.toggle({ text: "hello" }, session(), project());
    expect(isSaved(marks, "s1", "hello")).toBe(false);
    expect(notify).toHaveBeenCalledTimes(1);
    expect(notify.mock.calls[0][0]).toContain("disk full");
    expect(marks.readOnly()).toBe(false);

    await marks.toggle({ text: "hello" }, session(), project());
    expect(isSaved(marks, "s1", "hello")).toBe(true);
    expect(disk.items).toHaveLength(1);
  });

  it("asks one change at a time, each knowing the one before: quick clicks undo each other", async () => {
    const { marks, disk, apply } = setup();
    await marks.load();
    let running = 0;
    let most = 0;
    const real = apply.getMockImplementation()!;
    apply.mockImplementation(async (change) => {
      most = Math.max(most, ++running);
      await new Promise((r) => setTimeout(r, 1));
      const done = await real(change);
      running--;
      return done;
    });

    await Promise.all([
      marks.toggle({ text: "a" }, session(), project()),
      marks.toggle({ text: "b" }, session(), project()),
      marks.toggle({ text: "a" }, session(), project()),
    ]);
    expect(most).toBe(1);
    expect(disk.items.map((b) => b.text)).toEqual(["b"]);
    expect(isSaved(marks, "s1", "a")).toBe(false);
    expect(isSaved(marks, "s1", "b")).toBe(true);
  });

  it("waits for the file: a click during the first read knows what is there", async () => {
    const { marks, disk } = setup([saved()]);
    const loading = marks.load();
    const click = marks.toggle({ text: "new" }, session(), project());
    await Promise.all([loading, click]);
    expect(disk.items.map((b) => b.text)).toEqual(["hello", "new"]);
  });

  it("changes nothing before the file was asked for", async () => {
    const { marks, apply } = setup([saved()]);
    await marks.toggle({ text: "new" }, session(), project());
    await marks.remove(saved());
    expect(apply).not.toHaveBeenCalled();
  });

  it("takes in what another window saved, as of the last change", async () => {
    const { marks, disk } = setup();
    await marks.load();
    disk.items = [saved({ id: "theirs" })];
    await marks.toggle({ text: "mine" }, session(), project());
    expect(isSaved(marks, "s1", "hello")).toBe(true);
    expect(disk.items.map((b) => b.id)).toEqual(["theirs", "b1"]);
  });
});

describe("the same conversation in a new session", () => {
  it("knows a reply again by its conversation: a resume or a mode switch makes a new session id", async () => {
    const { marks, disk } = setup();
    await marks.load();
    await marks.toggle({ text: "hello" }, session("old", "conv-1"), project());
    expect((disk.items[0] as ChatBookmark).conversationId).toBe("conv-1");
    expect(isSaved(marks, "new", "hello", "conv-1")).toBe(true);
    expect(isSaved(marks, "new", "hello", "conv-2")).toBe(false);

    // A click there takes it away; it does not save a second one.
    await marks.toggle({ text: "hello" }, session("new", "conv-1"), project());
    expect(disk.items).toEqual([]);
  });

  it("does not take the placeholder for a conversation", async () => {
    const { marks, disk } = setup();
    await marks.load();
    await marks.toggle({ text: "hello" }, session("old", "latest"), project());
    expect((disk.items[0] as ChatBookmark).conversationId).toBeUndefined();
    expect(isSaved(marks, "new", "hello", "latest")).toBe(false);
    expect(isSaved(marks, "old", "hello", "latest")).toBe(true);
  });

  it("keeps entries from before conversations were stored working, by their session", async () => {
    const { marks, disk } = setup([saved()]);
    await marks.load();
    expect(isSaved(marks, "s1", "hello", "conv-1")).toBe(true);
    await marks.toggle({ text: "hello" }, session("s1", "conv-1"), project());
    expect(disk.items).toEqual([]);
  });

  it("finds the chat to reveal in: its own while it exists, else one with its conversation", () => {
    const chat = (id: string, agent_session_id: string | null, isChat = true) => ({ id, agent_session_id, chat: isChat });
    const b = saved({ sessionId: "gone", conversationId: "conv-1" });

    expect(sessionFor(b, {})).toBeUndefined();
    expect(sessionFor(b, { a: chat("a", "conv-2"), t: chat("t", "conv-1", false) })).toBeUndefined();
    expect(sessionFor(b, { a: chat("a", "conv-2"), b: chat("b", "conv-1") })?.id).toBe("b");
    expect(sessionFor(saved({ sessionId: "b", conversationId: "conv-1" }), { a: chat("a", "conv-1"), b: chat("b", "conv-2") })?.id).toBe("b");
    expect(sessionFor(saved({ sessionId: "gone" }), { a: chat("a", null) })).toBeUndefined();
  });
});

describe("a file that could not be read", () => {
  const nothingChanges = async (h: ReturnType<typeof setup>) => {
    await h.marks.load();
    expect(h.marks.readOnly()).toBe(true);
    await h.marks.toggle({ text: "hello" }, session(), project());
    await h.marks.toggle({ text: "again" }, session(), project());
    await h.marks.remove(saved());
    expect(h.apply).not.toHaveBeenCalled();
    expect(isSaved(h.marks, "s1", "hello")).toBe(false);
    expect(h.marks.inProject("p1")).toEqual([]);
    expect(h.notify).toHaveBeenCalledTimes(1);
  };

  it("makes toggle and remove do nothing, changes nothing, and says so once", async () => {
    const h = setup([], "expected value at line 1 column 1");
    await nothingChanges(h);
    expect(h.notify.mock.calls[0][0]).toContain("expected value at line 1 column 1");
  });

  it("counts anything but a list as unreadable", async () => {
    await nothingChanges(setup({ version: 1 }));
    await nothingChanges(setup(null));
  });
});

describe("items this build does not understand", () => {
  it("leaves them out of the list and in the file", async () => {
    const { marks, disk } = setup([saved(), { id: "x2", text: 7 }, 42, { ...saved({ id: "x3" }), conversationId: 9 }]);
    await marks.load();
    expect(marks.readOnly()).toBe(false);
    expect(marks.inProject("p1").map((b) => b.id)).toEqual(["x"]);

    await marks.toggle({ text: "new" }, session(), project());
    expect(disk.items).toHaveLength(5);
    expect(marks.inProject("p1").map((b) => b.text)).toEqual(["new", "hello"]);
  });
});

describe("inProject", () => {
  it("lists a project's own bookmarks, newest first", async () => {
    const { marks } = setup([
      saved({ id: "old", at: 10 }),
      saved({ id: "other", projectId: "p2", at: 20 }),
      saved({ id: "new", at: 30 }),
      saved({ id: "mid", at: 20 }),
    ]);
    await marks.load();
    expect(marks.inProject("p1").map((b) => b.id)).toEqual(["new", "mid", "old"]);
    expect(marks.inProject("p2").map((b) => b.id)).toEqual(["other"]);
  });
});

describe("removing from the list", () => {
  it("offers to put it back as it was, in the same place", async () => {
    const { marks, disk, notify } = setup([saved({ id: "a", at: 10 }), saved({ id: "b", at: 20, anchor: "other" })]);
    await marks.load();

    await marks.remove(marks.inProject("p1")[1]);
    expect(disk.items.map((b) => b.id)).toEqual(["b"]);
    const [, action] = notify.mock.calls[0];
    expect(action.label).toBeTruthy();

    await action.run();
    expect(marks.inProject("p1").map((b) => b.id)).toEqual(["b", "a"]);
    expect((disk.items.find((b) => b.id === "a") as ChatBookmark).at).toBe(10);
  });

  it("offers nothing when the removal failed", async () => {
    const { marks, apply, notify, disk } = setup([saved()]);
    await marks.load();
    apply.mockRejectedValueOnce(new Error("disk full"));

    await marks.remove(saved());
    expect(disk.items).toHaveLength(1);
    expect(notify).toHaveBeenCalledTimes(1);
    expect(notify.mock.calls[0][1]).toBeUndefined();
  });

  it("does not offer it on the button in a reply, where the reply itself still shows", async () => {
    const { marks, notify } = setup();
    await marks.load();
    await marks.toggle({ text: "hello" }, session(), project());
    await marks.toggle({ text: "hello" }, session(), project());
    expect(notify).not.toHaveBeenCalled();
  });
});

describe("a chat mounted again, with the state of its last mount", () => {
  const REPLY = "The reply that was saved";
  const line = (text: string) => JSON.stringify({ type: "assistant", message: { id: `m-${text}`, role: "assistant", content: [{ type: "text", text }] } }) + "\n";
  const log = (...texts: string[]) => new TextEncoder().encode(texts.map(line).join(""));

  /** A real controller, whose attaches finish when the test says. */
  function chat() {
    const pending: ((snapshot: Uint8Array) => void)[] = [];
    const ctrl = new ChatController("c1", "/work", null, (ctx) => createClaudeAdapter(ctx), {
      transport: {
        send: () => {},
        update: () => {},
        attach: () => new Promise<Uint8Array>((resolve) => pending.push(resolve)),
      },
    });
    const finish = async (i: number, snapshot: Uint8Array) => {
      pending[i](snapshot);
      await new Promise((r) => setTimeout(r, 0));
    };
    return { ctrl, finish };
  }
  const reply = (items: ChatItem[]) => findReply(items, anchorOf(REPLY));

  it("is not whole until this mount's attach is done, though the old state says ready and holds the reply", async () => {
    const { ctrl, finish } = chat();
    const first = mountAttach(ctrl);
    first.attach(1);
    expect(first.whole()).toBe(false);
    await finish(0, log(REPLY));
    expect(first.whole()).toBe(true);
    expect(reply(ctrl.state.items)).toBeTruthy();
    ctrl.detach();

    const second = mountAttach(ctrl);
    expect(second.whole()).toBe(false);
    second.attach(1);
    expect(ctrl.state.ready).toBe(true);
    expect(reply(ctrl.state.items)).toBeTruthy();
    expect(second.whole()).toBe(false);

    await finish(1, log("Something before", REPLY));
    expect(second.whole()).toBe(true);
    expect(reply(ctrl.state.items)).toBeTruthy();
  });

  it("finds nothing once whole if the attach brought no such reply: the saved text is shown instead", async () => {
    const { ctrl, finish } = chat();
    const mount = mountAttach(ctrl);
    mount.attach(1);
    await finish(0, log("Something else"));
    expect(mount.whole()).toBe(true);
    expect(reply(ctrl.state.items)).toBeUndefined();
  });

  it("waits for the newest of two attaches", async () => {
    const { ctrl, finish } = chat();
    const mount = mountAttach(ctrl);
    mount.attach(1);
    mount.attach(1);
    await finish(0, log("old"));
    expect(mount.whole()).toBe(false);
    await finish(1, log(REPLY));
    expect(mount.whole()).toBe(true);
  });

  it("counts a reply still arriving as not there", () => {
    const items = [{ kind: "assistant", id: "a", text: REPLY, streaming: true }] as ChatItem[];
    expect(reply(items)).toBeUndefined();
  });
});

describe("revealing a saved reply", () => {
  beforeEach(() => void vi.useFakeTimers());
  afterEach(() => void vi.useRealTimers());

  it("is left to the chat, which says whether it found it", () => {
    const { marks } = setup();
    const b = saved();

    marks.askReveal(b, "s1");
    const asked = marks.pendingReveal()!;
    expect(asked).toEqual({ bookmark: b, sessionId: "s1" });
    marks.settleReveal(asked, true);
    expect(marks.pendingReveal()).toBeNull();
    expect(marks.shown()).toBeNull();

    marks.askReveal(b, "s1");
    marks.settleReveal(marks.pendingReveal()!, false);
    expect(marks.pendingReveal()).toBeNull();
    expect(marks.shown()).toBe(b);
  });

  it("falls back to the saved text when no chat ever answers, and only then", () => {
    const { marks } = setup();
    const b = saved();

    marks.askReveal(b, "s1");
    vi.advanceTimersByTime(REVEAL_WAIT_MS - 1);
    expect(marks.shown()).toBeNull();
    vi.advanceTimersByTime(1);
    expect(marks.pendingReveal()).toBeNull();
    expect(marks.shown()).toBe(b);

    marks.closeSaved();
    marks.askReveal(b, "s1");
    marks.settleReveal(marks.pendingReveal()!, true);
    vi.advanceTimersByTime(REVEAL_WAIT_MS);
    expect(marks.shown()).toBeNull();
  });

  it("ignores the answer about a reveal that a newer click replaced", () => {
    const { marks } = setup();
    const first = saved({ id: "first" });
    const second = saved({ id: "second" });

    marks.askReveal(first, "s1");
    const old = marks.pendingReveal()!;
    marks.askReveal(second, "s1");
    marks.settleReveal(old, false);
    expect(marks.pendingReveal()?.bookmark).toBe(second);
    expect(marks.shown()).toBeNull();

    vi.advanceTimersByTime(REVEAL_WAIT_MS);
    expect(marks.shown()).toBe(second);
  });

  it("showing the saved text ends a reveal on its way, and its timer", () => {
    const { marks } = setup();
    const first = saved({ id: "first" });
    const second = saved({ id: "second" });

    marks.askReveal(first, "s1");
    marks.showSaved(second);
    expect(marks.pendingReveal()).toBeNull();
    expect(marks.shown()).toBe(second);

    marks.closeSaved();
    vi.advanceTimersByTime(REVEAL_WAIT_MS);
    expect(marks.shown()).toBeNull();
  });
});
