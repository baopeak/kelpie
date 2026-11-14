import { describe, expect, it } from "vitest";
import { CHUNK, CHUNK_HEADER, DISCONNECTED_MS, GATHER_MAX_MS, GATHER_MS, OPEN_MS, pathFromStats, Reassembly, ScreenPeer, VideoOrder, type OfferReply, type PeerEnv } from "./screen-rtc";

/** Chunks as the computer cuts them (`kelpie_rtc::frame::chunks`). */
function chunks(seq: number, msg: Uint8Array, room = 16 * 1024 - CHUNK_HEADER): Uint8Array[] {
  const count = Math.max(1, Math.ceil(msg.length / room));
  return Array.from({ length: count }, (_, i) => {
    const part = msg.subarray(i * room, Math.min(msg.length, (i + 1) * room));
    const c = new Uint8Array(CHUNK_HEADER + part.length);
    const v = new DataView(c.buffer);
    c[0] = CHUNK;
    v.setUint32(1, seq, true);
    v.setUint16(5, i, true);
    v.setUint16(7, count, true);
    c.set(part, CHUNK_HEADER);
    return c;
  });
}

const message = (len: number) => Uint8Array.from({ length: len }, (_, i) => (i * 31) % 251);

describe("media messages put back together", () => {
  it("every size from a byte to a megabyte comes back whole, and only the last chunk completes it", () => {
    const r = new Reassembly();
    for (const [n, len] of [1, 16375, 16376, 40_000, 1024 * 1024].entries()) {
      const msg = message(len);
      const parts = chunks(n, msg);
      const got = parts.map((p) => r.push(p));
      expect(got.slice(0, -1).every((g) => g === null)).toBe(true);
      const whole = got.at(-1)!;
      // Compared byte by byte without the matcher's diff, which is slow on a megabyte.
      expect(whole.length).toBe(len);
      expect(whole.every((b, i) => b === msg[i])).toBe(true);
    }
  });

  it("a message cut short by a newer one is dropped whole, and junk is ignored", () => {
    const r = new Reassembly();
    const old = chunks(1, message(40_000));
    const next = chunks(2, message(20_000));
    expect(r.push(old[0])).toBeNull();
    expect(r.push(next[0])).toBeNull();
    expect(r.push(old[1]), "the old message's chunk after the newer started").toBeNull();
    expect(r.push(next[1]), "the newer lost its place too").toBeNull();
    const again = chunks(3, message(20_000));
    r.push(again[0]);
    expect(r.push(again[1])).toEqual(message(20_000));
    expect(r.push(new Uint8Array([0x01, 1, 0, 0, 0, 0, 0, 1, 0]))).toBeNull();
    expect(r.push(new Uint8Array([CHUNK, 0, 0, 0, 0, 2, 0, 1, 0])), "an index past the count").toBeNull();
  });
});

describe("the way, from the browser's stats", () => {
  it("reads the selected pair: P2P, or TURN when either side is a relay candidate", () => {
    const report = [
      { id: "T", type: "transport", selectedCandidatePairId: "P" },
      { id: "P", type: "candidate-pair", localCandidateId: "L", remoteCandidateId: "R", currentRoundTripTime: 0.048, bytesReceived: 5000 },
      { id: "L", type: "local-candidate", candidateType: "srflx" },
      { id: "R", type: "remote-candidate", candidateType: "host" },
    ];
    expect(pathFromStats(report)).toEqual({ path: "p2p", local: "srflx", remote: "host", relayProtocol: null, rttMs: 48, bytesReceived: 5000 });
    const turn = [
      { id: "P", type: "candidate-pair", nominated: true, state: "succeeded", localCandidateId: "L", remoteCandidateId: "R" },
      { id: "L", type: "local-candidate", candidateType: "relay", relayProtocol: "tls" },
      { id: "R", type: "remote-candidate", candidateType: "srflx" },
    ];
    expect(pathFromStats(turn)).toMatchObject({ path: "turn", relayProtocol: "tls", rttMs: null });
    expect(pathFromStats([{ id: "x", type: "codec" }]), "no pair chosen yet").toBeNull();
  });
});

describe("video in order whichever way it came", () => {
  it("drops a frame older than one taken, and lets everything else through", () => {
    const o = new VideoOrder();
    expect([o.take(3), o.take(2), o.take(3), o.take(4), o.take(null)]).toEqual([true, false, false, true, true]);
    o.reset();
    expect(o.take(1)).toBe(true);
  });
});

/** A fake RTCPeerConnection: its channels, candidates and states are driven by the test. */
class FakeChannel {
  readyState = "connecting";
  binaryType = "blob";
  bufferedAmount = 0;
  sent: string[] = [];
  onopen: (() => void) | null = null;
  onclose: (() => void) | null = null;
  onmessage: ((e: { data: unknown }) => void) | null = null;
  constructor(public label: string, public init: RTCDataChannelInit) {}
  send(s: string) {
    this.sent.push(s);
  }
  open() {
    this.readyState = "open";
    this.onopen?.();
  }
}

class FakePc {
  static last: FakePc | null = null;
  channels: FakeChannel[] = [];
  iceGatheringState = "gathering";
  connectionState = "new";
  localDescription: { sdp: string } | null = null;
  remote: { type: string; sdp: string } | null = null;
  closed = false;
  config: RTCConfiguration[] = [];
  onicecandidate: ((e: { candidate: { candidate: string } | null }) => void) | null = null;
  onicegatheringstatechange: (() => void) | null = null;
  onconnectionstatechange: (() => void) | null = null;
  constructor(config: RTCConfiguration) {
    this.config.push(config);
    FakePc.last = this;
  }
  createDataChannel(label: string, init: RTCDataChannelInit) {
    const ch = new FakeChannel(label, init);
    this.channels.push(ch);
    return ch;
  }
  async createOffer() {
    return { type: "offer", sdp: "v=0 offer" };
  }
  async setLocalDescription(d: { sdp: string }) {
    this.localDescription = d;
  }
  async setRemoteDescription(d: { type: string; sdp: string }) {
    this.remote = d;
  }
  setConfiguration(c: RTCConfiguration) {
    this.config.push(c);
  }
  async getStats() {
    return new Map<string, unknown>();
  }
  close() {
    this.closed = true;
  }
  gather() {
    this.iceGatheringState = "complete";
    this.onicegatheringstatechange?.();
  }
  state(s: string) {
    this.connectionState = s;
    this.onconnectionstatechange?.();
  }
}

function peerRig(reply: OfferReply = { t: "ack", sdp: "v=0 answer", nonce: "n1", ice_servers: [{ urls: "stun:x" }] }) {
  const timers: { fn: () => void; ms: number; id: number }[] = [];
  let id = 0;
  const env: PeerEnv = {
    RTCPeerConnection: FakePc as unknown as PeerEnv["RTCPeerConnection"],
    setTimeout: (fn, ms) => {
      timers.push({ fn, ms, id: ++id });
      return id;
    },
    clearTimeout: (t) => {
      const i = timers.findIndex((x) => x.id === t);
      if (i >= 0) timers.splice(i, 1);
    },
  };
  const offers: string[] = [];
  const ice: string[] = [];
  const media: Uint8Array[] = [];
  const failed: string[] = [];
  let opened = 0;
  const peer = new ScreenPeer(env, { offer: async (_sc, sdp) => (offers.push(sdp), reply), ice: (_sc, c) => ice.push(c) }, "sc1", { media: (b) => media.push(b), open: () => opened++, failed: (w) => failed.push(w) });
  const fire = (ms: number) => {
    for (const t of timers.filter((x) => x.ms === ms)) {
      env.clearTimeout(t.id);
      t.fn();
    }
  };
  return { peer, timers, offers, ice, media, failed, opened: () => opened, fire };
}

const flush = () => new Promise((r) => setTimeout(r, 0));

describe("one peer for one screen", () => {
  it("offers after gathering, says the hello once the three negotiated channels open, and carries media and input", async () => {
    const r = peerRig();
    const started = r.peer.start();
    await flush();
    const pc = FakePc.last!;
    expect(pc.channels.map((c) => [c.label, c.init])).toEqual([
      ["m", { negotiated: true, id: 0 }],
      ["c", { negotiated: true, id: 1, ordered: false, maxRetransmits: 0 }],
      ["i", { negotiated: true, id: 2 }],
    ]);
    // A candidate before the offer goes inside it; one after is sent on its own.
    pc.onicecandidate?.({ candidate: { candidate: "candidate:early" } });
    pc.gather();
    await started;
    expect(r.offers).toEqual(["v=0 offer"]);
    expect(pc.remote).toEqual({ type: "answer", sdp: "v=0 answer" });
    pc.onicecandidate?.({ candidate: { candidate: "candidate:late" } });
    expect(r.ice).toEqual(["candidate:late"]);
    expect(r.peer.send({ type: "sc_ack", seq: 1 }), "nothing on \"i\" before the hello").toBe(false);
    for (const ch of pc.channels) ch.open();
    expect(r.opened()).toBe(1);
    const i = pc.channels[2];
    expect(JSON.parse(i.sent[0])).toEqual({ type: "rtc_hello", sc: "sc1", nonce: "n1" });
    expect(r.peer.send({ type: "sc_ack", seq: 1 })).toBe(true);
    expect(JSON.parse(i.sent[1])).toEqual({ type: "sc_ack", seq: 1 });
    i.bufferedAmount = 100_000;
    expect(r.peer.send({ type: "sc_ack", seq: 2 }), "a backed-up channel: the link takes it").toBe(false);
    // Media: chunks on "m" put together; the cursor on "c" as it comes.
    for (const c of chunks(7, message(20_000))) pc.channels[0].onmessage?.({ data: c.slice().buffer });
    pc.channels[1].onmessage?.({ data: new Uint8Array([0x01, 3]).buffer });
    expect(r.media.map((m) => m.length)).toEqual([20_000, 2]);
    expect(r.timers.some((t) => t.ms === OPEN_MS), "the open limit is cleared once open").toBe(false);
    r.peer.close();
    expect(pc.closed).toBe(true);
    expect(r.failed).toEqual([]);
  });

  it("gives up gathering after its time, and fails when the channels do not open, the computer refuses, or the connection stays down", async () => {
    const slow = peerRig();
    const s = slow.peer.start();
    await flush();
    slow.fire(GATHER_MS);
    await flush();
    expect(slow.offers, "no candidate yet: it waits on").toHaveLength(0);
    FakePc.last!.onicecandidate?.({ candidate: { candidate: "candidate:first" } });
    await s;
    expect(slow.offers, "the first candidate after its time sends the offer").toHaveLength(1);
    const none = peerRig();
    const n = none.peer.start();
    await flush();
    none.fire(GATHER_MS);
    none.fire(GATHER_MAX_MS);
    await n;
    expect(none.offers, "at the latest after the longest wait").toHaveLength(1);
    none.peer.close();
    slow.fire(OPEN_MS);
    expect(slow.failed).toEqual(["not_open"]);
    expect(FakePc.last!.closed).toBe(true);

    const refused = peerRig({ t: "error", code: "method_off", message: "off" });
    const r = refused.peer.start();
    await flush();
    FakePc.last!.gather();
    await r;
    expect(refused.failed).toEqual(["method_off"]);

    const down = peerRig();
    const d = down.peer.start();
    await flush();
    const pc = FakePc.last!;
    pc.gather();
    await d;
    for (const ch of pc.channels) ch.open();
    pc.state("disconnected");
    pc.state("connected");
    expect(down.timers.some((t) => t.ms === DISCONNECTED_MS), "back within the time: nothing failed").toBe(false);
    pc.state("disconnected");
    down.fire(DISCONNECTED_MS);
    expect(down.failed).toEqual(["ice_disconnected"]);
    expect(down.peer.send({ type: "sc_ack", seq: 1 })).toBe(false);
  });

  it("a channel that closes or a failed connection ends it once", async () => {
    const r = peerRig();
    const s = r.peer.start();
    await flush();
    const pc = FakePc.last!;
    pc.gather();
    await s;
    pc.state("failed");
    pc.channels[0].onclose?.();
    expect(r.failed).toEqual(["ice_failed"]);
  });
});
