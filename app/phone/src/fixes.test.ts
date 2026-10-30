import { afterEach, describe, expect, it, vi } from "vitest";
import { ChatController } from "../../src/chat/chat-model";
import { createClaudeAdapter } from "../../src/chat/claude-adapter";
import { Composer, looksLikePasswordPrompt, SendLatch, unsentRest } from "../../src/lib/composer-core";
import { throttled } from "./cache";
import { ChatFeed } from "./chat-feed";
import { answerNotice, NOTICE_SHOWN_MS, NoticeBoard, noticeFor } from "./notice";
import { forgetHistory, history } from "./history";
import { promptKey } from "./screen-sync";

const INIT = JSON.stringify({ type: "system", subtype: "init", session_id: "s", model: "m", permissionMode: "default" });
const line = (text: string) => JSON.stringify({ type: "user", message: { role: "user", content: text } });

describe("a chat line after the person left the chat", () => {
  const ctrl = () => new ChatController("c1", "/w", null, (c) => createClaudeAdapter(c), { observer: true });

  it("does not move the position, so reopening asks for the whole log and shows all of it", () => {
    const feed = new ChatFeed();
    feed.open("c1", ctrl());
    feed.apply({ id: "c1", mode: "reset", seq: 10, lines: [INIT, line("one")] });
    expect(feed.since("c1")).toBe(10);
    feed.close("c1");
    // A chunk that was already on its way when Back was tapped.
    expect(feed.apply({ id: "c1", mode: "live", seq: 99, lines: [line("late")] })).toBe(false);
    const reopened = ctrl();
    feed.open("c1", reopened);
    expect(feed.since("c1")).toBeNull();
    feed.apply({ id: "c1", mode: "reset", seq: 99, lines: [INIT, line("one"), line("late")] });
    expect(reopened.state.items.filter((i) => i.kind === "user")).toHaveLength(2);
  });

  it("a restart forgets the positions, and a chat nobody follows has none", () => {
    const feed = new ChatFeed();
    feed.open("c1", ctrl());
    feed.apply({ id: "c1", mode: "reset", seq: 5, lines: [INIT] });
    feed.forgetPositions();
    expect(feed.since("c1")).toBeNull();
    expect(feed.since("nope")).toBeNull();
  });
});

describe("one send at a time", () => {
  it("a second Enter while the first send is on its way does nothing", async () => {
    const latch = new SendLatch();
    let release!: () => void;
    const sends: string[] = [];
    const first = latch.run(() => new Promise<void>((r) => ((release = r), sends.push("git push"))));
    expect(latch.sending).toBe(true);
    expect(await latch.run(async () => sends.push("git push"))).toBeUndefined();
    release();
    await first;
    expect(sends).toEqual(["git push"]);
    expect(latch.sending).toBe(false);
    await latch.run(async () => sends.push("next"));
    expect(sends).toEqual(["git push", "next"]);
  });

  it("a failing send frees the latch", async () => {
    const latch = new SendLatch();
    await expect(latch.run(async () => Promise.reject(new Error("x")))).rejects.toThrow("x");
    expect(latch.sending).toBe(false);
  });

  it("clears only what was sent and keeps what was typed after it", () => {
    expect(unsentRest("ls -la", "ls -la")).toBe("");
    expect(unsentRest("ls -la\nnext", "ls -la")).toBe("next");
    expect(unsentRest("ls -la and more", "ls -la")).toBe(" and more");
    expect(unsentRest("something else", "ls -la")).toBe("something else");
  });
});

describe("secrets are not remembered", () => {
  it("knows a prompt for a password, passphrase or token on the last line", () => {
    for (const row of ["[sudo] password for me:", "Enter passphrase for key '/home/me/.ssh/id_ed25519':", "Password:", "Paste your API key:", "Enter PIN:"]) {
      expect(looksLikePasswordPrompt(["output", row, "", ""]), row).toBe(true);
    }
    for (const row of ["PS C:\\proj>", "Proceed? [y/n]", "the password file is ready", "$ "]) expect(looksLikePasswordPrompt([row, ""]), row).toBe(false);
  });
});

describe("a card for the next prompt starts afresh", () => {
  it("two prompts back to back have different identities; the same prompt keeps its own", () => {
    const menu = { keys: ["1", "2", "3"], question: "Do you want to proceed?", line: false };
    expect(promptKey(menu, 100)).toBe(promptKey({ ...menu }, 100));
    expect(promptKey(menu, 100)).not.toBe(promptKey(menu, 250));
    expect(promptKey(menu, 100)).not.toBe(promptKey({ ...menu, question: "Another?" }, 100));
    expect(promptKey(menu, null)).not.toBe(promptKey({ ...menu, keys: ["y", "n"] }, null));
  });
});

describe("the notice that outlives the card", () => {
  it("is shown for the session it is about, for at least 5 s, and goes when dismissed", () => {
    const board = new NoticeBoard();
    const n = board.show("t1", "Already answered on the desktop 2 min ago", 1000);
    expect(NOTICE_SHOWN_MS).toBeGreaterThanOrEqual(5000);
    expect(board.visible("t1", 1000 + 5000)?.text).toMatch(/desktop/);
    expect(board.visible("other", 1000)).toBeNull();
    expect(board.visible("t1", 1000 + NOTICE_SHOWN_MS)).toBeNull();
    board.show("t1", "again", 20_000);
    board.dismiss(n.id);
    expect(board.visible("t1", 20_001)?.text).toBe("again");
    board.dismiss();
    expect(board.visible("t1", 20_001)).toBeNull();
  });

  it("is made for a stale or repeated answer, not for an accepted one", () => {
    expect(answerNotice({ t: "error", result: "stale", answered_by: "desktop", at: 5 })).toEqual({ kind: "stale", by: "desktop", at: 5 });
    expect(answerNotice({ t: "error", result: "dup" })).toEqual({ kind: "dup" });
    expect(answerNotice({ t: "ack", result: "ok" })).toBeNull();
    expect(answerNotice({ t: "error", result: "refused" })).toBeNull();
    expect(answerNotice({ t: "unknown" })).toBeNull();
  });
});

describe("cache writes waiting when the device is forgotten", () => {
  afterEach(() => vi.useRealTimers());

  it("are dropped, so they cannot bring the list back after it was cleared", () => {
    vi.useFakeTimers();
    const written: string[] = [];
    const writes = throttled(1500, (key) => void written.push(key));
    writes.put("list", { a: 1 });
    writes.put("screen:t1", { rows: ["secret"] });
    vi.advanceTimersByTime(1000);
    writes.cancel();
    vi.advanceTimersByTime(5000);
    expect(written).toEqual([]);
    writes.put("list", { a: 2 });
    vi.advanceTimersByTime(1500);
    expect(written).toEqual(["list"]);
  });
});

describe("after the device is forgotten", () => {
  it("the typing history of the page is empty too", () => {
    history.sent("echo secret");
    expect(history.step(1, "")).toBe("echo secret");
    forgetHistory();
    expect(history.history).toEqual([]);
    expect(history.step(1, "")).toBeNull();
    expect(new Composer(["a"]).history).toEqual(["a"]);
  });

  it("the chats forget the answers saved for a retry when the daemon restarts", () => {
    const feed = new ChatFeed();
    const ctrl = new ChatController("c1", "/w", null, (c) => createClaudeAdapter(c), { observer: true });
    const forgot = vi.spyOn(ctrl, "forgetSentAnswers");
    feed.open("c1", ctrl);
    feed.forgetPositions();
    expect(forgot).toHaveBeenCalledTimes(1);
  });
});

describe("the notice and the card do not say the same thing twice", () => {
  const stale = { t: "error", result: "stale", answered_by: "desktop", at: 5 };
  it("a card that is still shown says it itself; a card that is gone leaves the notice", () => {
    expect(noticeFor(stale, true)).toBeNull();
    expect(noticeFor({ t: "error", result: "dup" }, true)).toBeNull();
    expect(noticeFor(stale, false)).toEqual({ kind: "stale", by: "desktop", at: 5 });
    expect(noticeFor({ t: "error", result: "dup" }, false)).toEqual({ kind: "dup" });
  });
});
