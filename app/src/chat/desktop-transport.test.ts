import { beforeEach, describe, expect, it, vi } from "vitest";

// The live callback the transport hands to the IPC, and the reply its attach waits for.
const h = vi.hoisted(() => ({
  live: null as null | ((seq: number, data: Uint8Array) => void),
  reply: null as null | ((r: { seq: number; data: string }) => void),
}));
vi.mock("../lib/ipc", () => ({
  attachTerminal: (_handle: number, _since: number | null, onData: (seq: number, data: Uint8Array) => void) => {
    h.live = onData;
    return new Promise((resolve) => (h.reply = resolve));
  },
  b64ToBytes: (s: string) => new TextEncoder().encode(s),
  daemon: vi.fn(() => Promise.resolve()),
  sendInput: vi.fn(() => Promise.resolve()),
}));
vi.mock("../lib/store", () => ({ store: { sessions: { s1: { id: "s1", handle: 7 } }, notify: vi.fn() } }));
vi.mock("../lib/i18n", () => ({ errorText: String }));

import { desktopTransport } from "./desktop-transport";

const enc = (s: string) => new TextEncoder().encode(s);
const dec = (b: Uint8Array) => new TextDecoder().decode(b);

describe("the desktop chat transport's live log", () => {
  const resyncs: string[] = [];
  beforeEach(() => {
    resyncs.length = 0;
    const target = new EventTarget();
    target.addEventListener("kelpie:chat-resync", (e) => resyncs.push((e as CustomEvent).detail));
    vi.stubGlobal("window", target);
  });

  async function attached(log: string) {
    const got: string[] = [];
    const t = desktopTransport();
    const done = t.attach!(7, (b) => got.push(dec(b)));
    return { got, t, finish: async () => { h.reply!({ seq: log.length, data: log }); return done; } };
  }

  it("holds chunks until the reply, then drops what the log already has and keeps the order", async () => {
    const a = await attached("abcdef");
    h.live!(6, enc("def")); // in the reply already
    h.live!(9, enc("ghi"));
    const snapshot = await a.finish();
    h.live!(12, enc("jkl"));
    expect(dec(snapshot!)).toBe("abcdef");
    expect(a.got).toEqual(["ghi", "jkl"]);
    expect(a.t.logEnd!()).toBe(6);
  });

  it("applies only the new part of a chunk that overlaps", async () => {
    const a = await attached("abc");
    await a.finish();
    h.live!(5, enc("bcde"));
    expect(a.got).toEqual(["de"]);
  });

  it("on a gap stops feeding and asks the chat pane to load the conversation again", async () => {
    const a = await attached("abc");
    await a.finish();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    h.live!(8, enc("xyz")); // bytes 3..5 never came
    h.live!(11, enc("uvw"));
    expect(a.got).toEqual([]);
    expect(resyncs).toEqual(["s1"]);
    expect(warn).toHaveBeenCalledOnce();
    warn.mockRestore();
  });

  it("ignores chunks of an earlier attach", async () => {
    const t = desktopTransport();
    const got: string[] = [];
    const first = t.attach!(7, (b) => got.push(dec(b)));
    const oldLive = h.live!;
    h.reply!({ seq: 3, data: "abc" });
    await first;
    const second = t.attach!(7, (b) => got.push(dec(b)));
    h.reply!({ seq: 3, data: "abc" });
    await second;
    oldLive(6, enc("old"));
    h.live!(6, enc("new"));
    expect(got).toEqual(["new"]);
  });
});
