// Pairing's steps over a fake socket. The handshake itself is checked in noise.test.ts (shared vectors) and against the real
// daemon in scripts/e2e: here a fake Initiator stands in for it, so each answer from the computer can be tried.
import { describe, expect, it, vi } from "vitest";
import { b64url, keypair } from "./noise";
import { pair, type PairSocket } from "./pair";

const decoder = new TextDecoder();
const encoder = new TextEncoder();

vi.mock("./i18n", () => ({ t: (s: string) => s }));
vi.mock("./noise", async (original) => {
  const real = await original<typeof import("./noise")>();
  class FakeInitiator {
    first = new Uint8Array([1, 2, 3]);
    finish(reply: Uint8Array) {
      if (reply[0] === 0xee) throw new Error("not this computer");
      return {
        session: {
          seal: (m: Uint8Array) => [m],
          open: (f: Uint8Array) => {
            if (f[0] === 0xee) throw new Error("altered");
            return f;
          },
          pairCode: () => "123456",
        },
      };
    }
  }
  return { ...real, Initiator: FakeInitiator };
});

class FakeSocket implements PairSocket {
  binaryType = "";
  onopen: PairSocket["onopen"] = null;
  onmessage: PairSocket["onmessage"] = null;
  onclose: PairSocket["onclose"] = null;
  onerror: PairSocket["onerror"] = null;
  sent: Uint8Array[] = [];
  closed = false;
  send(data: Uint8Array) {
    this.sent.push(data);
  }
  close() {
    this.closed = true;
  }
  /** A binary message from the computer. */
  receive(bytes: Uint8Array) {
    this.onmessage?.({ data: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) });
  }
  say(msg: unknown) {
    this.receive(encoder.encode(JSON.stringify(msg)));
  }
}

const desktop = keypair();
const link = b64url(encoder.encode(JSON.stringify({ v: 1, dpk: b64url(desktop.pub), psk: b64url(new Uint8Array(32).fill(7)), ep: [{ t: "lan", a: "192.168.1.5:4817" }], name: "Desk" })));

function start(name = " Pixel ") {
  const ws = new FakeSocket();
  const shown: [string, string][] = [];
  const result = pair(link, name, () => ws, (digits, key) => shown.push([digits, key]));
  ws.onopen?.({});
  return { ws, shown, result };
}

describe("pairing a phone", () => {
  it("sends the first message, then its name, shows the code and key, and keeps the pairing once approved", async () => {
    const { ws, shown, result } = start();
    expect(ws.binaryType).toBe("arraybuffer");
    expect([...ws.sent[0]]).toEqual([1, 2, 3]);
    expect(shown).toEqual([]);
    ws.receive(new Uint8Array([9]));
    expect(JSON.parse(decoder.decode(ws.sent[1]))).toEqual({ type: "pair", name: "Pixel", want: "read" });
    expect(shown.length).toBe(1);
    expect(shown[0][0]).toBe("123456");
    expect(shown[0][1]).toMatch(/^[0-9A-F]{4}( [0-9A-F]{4}){3}$/);
    ws.say({ type: "paired", device: { id: "d1", name: "Pixel" } });
    const r = await result;
    expect(r.ok).toBe(true);
    if (!r.ok) return;
    expect(r.pairing.deviceId).toBe("d1");
    expect(r.pairing.computer).toBe("Desk");
    expect([...r.pairing.dpk]).toEqual([...desktop.pub]);
    expect(r.pairing.endpoints).toEqual([{ t: "lan", a: "192.168.1.5:4817" }]);
    expect(ws.closed).toBe(true);
  });

  it.each([
    [{ type: "declined" }, "Declined"],
    [{ type: "timeout" }, "Not approved"],
  ])("says so when the computer answers %j", async (answer, words) => {
    const { ws, result } = start();
    ws.receive(new Uint8Array([9]));
    ws.say(answer);
    const r = await result;
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.message).toContain(words);
  });

  it("gives up when the computer stays silent, and when nobody decides, however long the socket stays open", async () => {
    vi.useFakeTimers();
    try {
      const silent = start();
      await vi.advanceTimersByTimeAsync(8001);
      const r = await silent.result;
      expect(r.ok).toBe(false);
      if (!r.ok) expect(r.message).toContain("did not answer");
      expect(silent.ws.closed).toBe(true);

      const waiting = start();
      waiting.ws.receive(new Uint8Array([9]));
      let settled = false;
      void waiting.result.then(() => (settled = true));
      await vi.advanceTimersByTimeAsync(120_000);
      expect(settled).toBe(false);
      await vi.advanceTimersByTimeAsync(15_001);
      expect(settled).toBe(true);
    } finally {
      vi.useRealTimers();
    }
  });

  it.each([
    [4403, "not valid any more"],
    [4429, "Too many"],
    [4503, "busy"],
    [1006, "did not answer"],
  ])("explains a connection closed with %i", async (code, words) => {
    const { ws, result } = start();
    ws.onclose?.({ code });
    const r = await result;
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.message).toContain(words);
  });

  it("does not trust a reply that is not from the computer in the QR code", async () => {
    const { ws, shown, result } = start();
    ws.receive(new Uint8Array([0xee]));
    const r = await result;
    expect(r.ok).toBe(false);
    if (!r.ok) expect(r.message).toContain("not the computer");
    expect(shown).toEqual([]);
    expect(ws.closed).toBe(true);
  });

  it("gives up on a sealed message that does not open, and on text frames", async () => {
    const altered = start();
    altered.ws.receive(new Uint8Array([9]));
    altered.ws.receive(new Uint8Array([0xee]));
    const r = await altered.result;
    expect(r.ok).toBe(false);
    const text = start();
    text.ws.onmessage?.({ data: "plain text" });
    expect((await text.result).ok).toBe(false);
  });

  it("refuses a link it cannot read without opening a socket", async () => {
    const socket = vi.fn();
    const r = await pair("not-a-link", "Pixel", socket, () => {});
    expect(r.ok).toBe(false);
    expect(socket).not.toHaveBeenCalled();
  });
});
