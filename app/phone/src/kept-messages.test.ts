import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { PendingSend } from "./chat-send";
import { discardKept, forgetGoneChats, goneChats, GONE_KEEP_MS, KEPT_PER_CHAT, KEPT_PREFIX, KEPT_TOTAL, loadKept, saveKept } from "./kept-messages";

/** A browser's storage, with room for `limit` characters of values (null: unlimited). */
function fakeStorage(limit: number | null = null) {
  const data = new Map<string, string>();
  const used = () => [...data.values()].reduce((n, v) => n + v.length, 0);
  const s = {
    getItem: (k: string) => data.get(k) ?? null,
    setItem: (k: string, v: string) => {
      if (limit !== null && used() - (data.get(k)?.length ?? 0) + v.length > limit) throw new DOMException("full", "QuotaExceededError");
      data.set(k, v);
    },
    removeItem: (k: string) => void data.delete(k),
  };
  return new Proxy(s, { ownKeys: () => [...data.keys()], getOwnPropertyDescriptor: (_, k) => (data.has(String(k)) ? { enumerable: true, configurable: true, value: data.get(String(k)) } : undefined) });
}

const msg = (cid: string): PendingSend => ({ cid, text: `text ${cid}`, state: "lost", at: 1 });
const many = (prefix: string, n: number) => Array.from({ length: n }, (_, i) => msg(`${prefix}${i}`));

describe("the copies a phone keeps of what waits for a decision", () => {
  beforeEach(() => void ((globalThis as { localStorage?: unknown }).localStorage = fakeStorage()));
  afterEach(() => void delete (globalThis as { localStorage?: unknown }).localStorage);

  it("keeps them past a reload, the newest within the cap per chat, and says which it could not keep", () => {
    expect(saveKept("chat-a", [msg("a1"), msg("a2")])).toEqual([]);
    expect(loadKept("chat-a").map((p) => p.cid)).toEqual(["a1", "a2"]);
    const lots = many("x", KEPT_PER_CHAT + 3);
    expect(saveKept("chat-a", lots)).toEqual(["x0", "x1", "x2"]);
    expect(loadKept("chat-a")).toHaveLength(KEPT_PER_CHAT);
    expect(saveKept("chat-a", [])).toEqual([]);
    expect(localStorage.getItem(`${KEPT_PREFIX}chat-a`)).toBeNull();
  });

  it("keeps no more than the total cap across chats", () => {
    const chats = Math.floor(KEPT_TOTAL / KEPT_PER_CHAT);
    for (let i = 0; i < chats; i++) expect(saveKept(`chat-${i}`, many(`c${i}-`, KEPT_PER_CHAT))).toEqual([]);
    expect(saveKept("one-more", [msg("late")])).toEqual(["late"]);
    expect(loadKept("one-more")).toEqual([]);
  });

  it("says none were kept when the browser is full, and leaves no older copy behind", () => {
    // Room for one message's copy, not two.
    (globalThis as { localStorage?: unknown }).localStorage = fakeStorage(130);
    expect(saveKept("chat-a", [msg("old")])).toEqual([]);
    expect(saveKept("chat-a", [msg("old"), msg("new2")])).toEqual(["old", "new2"]);
    expect(loadKept("chat-a")).toEqual([]);
  });

  it("keeps a gone chat's copies, shown with the chat's name, for a week after they last changed", () => {
    const t0 = 1_800_000_000_000;
    saveKept("kept", [msg("k")], "Live chat", t0);
    saveKept("resumed", [msg("r1"), msg("r2")], "Fix the tests", t0);
    // An empty list deletes nothing (it may not be the whole story yet); listed, both are gone from it.
    forgetGoneChats([], t0);
    expect(loadKept("resumed")).toHaveLength(2);
    expect(goneChats([]).map((c) => c.chat).sort()).toEqual(["kept", "resumed"]);
    // The chat was resumed as a new session: the old id is gone from the computer's list. Its messages stay, and are listed.
    forgetGoneChats(["kept", "new-session", "a-terminal"], t0 + 60_000);
    expect(loadKept("resumed")).toHaveLength(2);
    expect(goneChats(["kept", "new-session"]).map((c) => [c.chat, c.name, c.items.map((p) => p.cid)])).toEqual([["resumed", "Fix the tests", ["r1", "r2"]]]);
    // A save that no longer knows the name keeps the one it had.
    saveKept("resumed", [msg("r1"), msg("r2")], undefined, t0 + 120_000);
    expect(goneChats(["kept"])[0].name).toBe("Fix the tests");
    // Discarding one leaves the other; discarding the last leaves nothing.
    discardKept("resumed", "r1");
    expect(loadKept("resumed").map((p) => p.cid)).toEqual(["r2"]);
    // A week after it last changed it goes; a chat still listed never does.
    forgetGoneChats(["kept"], t0 + 120_000 + GONE_KEEP_MS - 1);
    expect(loadKept("resumed")).toHaveLength(1);
    forgetGoneChats(["kept"], t0 + 120_000 + GONE_KEEP_MS + 1);
    expect(loadKept("resumed")).toEqual([]);
    expect(loadKept("kept")).toHaveLength(1);
    discardKept("kept", "k");
    expect(localStorage.getItem(`${KEPT_PREFIX}kept`)).toBeNull();
  });

  it("reads what the first build stored (a plain list) as a gone chat's messages too", () => {
    localStorage.setItem(`${KEPT_PREFIX}old-format`, JSON.stringify([{ ...msg("o"), at: 5 }]));
    expect(loadKept("old-format").map((p) => p.cid)).toEqual(["o"]);
    expect(goneChats(["other"]).map((c) => [c.chat, c.name])).toEqual([["old-format", undefined]]);
    forgetGoneChats(["other"], 5 + GONE_KEEP_MS + 1);
    expect(loadKept("old-format")).toEqual([]);
  });
});
