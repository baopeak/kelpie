import { afterEach, beforeEach, describe, expect, it } from "vitest";
import type { PendingSend } from "./chat-send";
import { forgetGoneChats, KEPT_PER_CHAT, KEPT_PREFIX, KEPT_TOTAL, loadKept, saveKept } from "./kept-messages";

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
    (globalThis as { localStorage?: unknown }).localStorage = fakeStorage(80);
    expect(saveKept("chat-a", [msg("old")])).toEqual([]);
    expect(saveKept("chat-a", [msg("old"), msg("new2")])).toEqual(["old", "new2"]);
    expect(loadKept("chat-a")).toEqual([]);
  });

  it("drops the copies for chats the computer no longer has, and nothing on an empty list", () => {
    saveKept("kept", [msg("k")]);
    saveKept("gone", [msg("g")]);
    forgetGoneChats([]);
    expect(loadKept("gone")).toHaveLength(1);
    forgetGoneChats(["kept", "a-terminal"]);
    expect(loadKept("kept")).toHaveLength(1);
    expect(loadKept("gone")).toEqual([]);
  });
});
