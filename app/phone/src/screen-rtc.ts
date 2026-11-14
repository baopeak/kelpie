// The computer's screen peer to peer: WebRTC data channels straight between this phone and the computer, set up inside the encrypted link
// (the offer and the answer, and so each side's DTLS fingerprint, travel only there). Three channels, negotiated by number as the computer has
// them: 0 "m" media in chunks (reliable, ordered), 1 "c" the cursor (unordered, never sent again), 2 "i" this phone's acknowledgements and input
// (reliable, ordered). The first message on "i" is the hello with the computer's nonce, which ties the channels to the screen.
// Pure apart from the RTCPeerConnection the caller hands in: tested with a fake one.

/** A chunk of a media message on "m": `[0x02, msg_seq u32 LE, idx u16 LE, count u16 LE]` then its piece. */
export const CHUNK = 0x02;
export const CHUNK_HEADER = 9;
/** ICE gathering is waited for this long before the offer goes (later candidates follow with `sc_rtc_ice`), and longer while it has found none
 *  at all (a browser's first gathering can be slow: Firefox's first took over 2 s, and an offer without a candidate fails at once), up to
 *  `GATHER_MAX_MS`. */
export const GATHER_MS = 2000;
export const GATHER_MAX_MS = 6000;
/** The channels must open within this. */
export const OPEN_MS = 10_000;
/** A connection disconnected this long has failed. */
export const DISCONNECTED_MS = 3000;
/** A way that failed is not tried again on its own for this long (the person can still choose it). */
export const P2P_RETRY_MS = 10 * 60_000;
/** Cloudflare's STUN server, used until the computer names its own. */
export const STUN = "stun:stun.cloudflare.com:3478";

/** Puts the chunks of "m" back into media messages; a message cut short by a newer one is dropped whole. */
export class Reassembly {
  private seq: number | null = null;
  private count = 0;
  private next = 0;
  private parts: Uint8Array[] = [];

  push(c: Uint8Array): Uint8Array | null {
    if (c.length < CHUNK_HEADER || c[0] !== CHUNK) return null;
    const v = new DataView(c.buffer, c.byteOffset, c.byteLength);
    const seq = v.getUint32(1, true);
    const idx = v.getUint16(5, true);
    const count = v.getUint16(7, true);
    if (count === 0 || idx >= count) return null;
    if (idx === 0) {
      this.seq = seq;
      this.count = count;
      this.next = 0;
      this.parts = [];
    } else if (this.seq !== seq || this.count !== count || this.next !== idx) {
      this.seq = null;
      this.parts = [];
      return null;
    }
    this.parts.push(c.subarray(CHUNK_HEADER));
    this.next = idx + 1;
    if (this.next < count) return null;
    this.seq = null;
    const len = this.parts.reduce((n, p) => n + p.length, 0);
    const whole = new Uint8Array(len);
    let at = 0;
    for (const p of this.parts) {
      whole.set(p, at);
      at += p.length;
    }
    this.parts = [];
    return whole;
  }
}

/** The way the packets take and how fast, from `getStats()`: `turn` through a TURN server, else `p2p`. */
export interface PathStats {
  path: "p2p" | "turn";
  local: string;
  remote: string;
  /** `udp`, `tcp` or `tls` to the TURN server, when through one. */
  relayProtocol: string | null;
  rttMs: number | null;
  bytesReceived: number;
}

type StatsEntry = { id: string; type: string; [k: string]: unknown };

/** The selected candidate pair of a stats report, as a path. Null before one is chosen. */
export function pathFromStats(entries: Iterable<StatsEntry>): PathStats | null {
  const all = [...entries];
  const byId = new Map(all.map((e) => [e.id, e]));
  const transport = all.find((e) => e.type === "transport" && typeof e.selectedCandidatePairId === "string");
  const pair =
    (transport && byId.get(transport.selectedCandidatePairId as string)) ??
    all.find((e) => e.type === "candidate-pair" && (e.selected === true || (e.nominated === true && e.state === "succeeded")));
  if (!pair) return null;
  const local = byId.get(pair.localCandidateId as string);
  const remote = byId.get(pair.remoteCandidateId as string);
  const localType = typeof local?.candidateType === "string" ? local.candidateType : "";
  const remoteType = typeof remote?.candidateType === "string" ? remote.candidateType : "";
  const rtt = typeof pair.currentRoundTripTime === "number" ? Math.round(pair.currentRoundTripTime * 1000) : null;
  return {
    path: localType === "relay" || remoteType === "relay" ? "turn" : "p2p",
    local: localType,
    remote: remoteType,
    relayProtocol: typeof local?.relayProtocol === "string" ? local.relayProtocol : null,
    rttMs: rtt,
    bytesReceived: typeof pair.bytesReceived === "number" ? pair.bytesReceived : 0,
  };
}

/** The computer's answer to an offer, or why not. */
export type OfferReply = { t: "ack"; sdp: string; nonce: string; ice_servers?: RTCIceServer[] } | { t: "error"; code: string; message: string } | { t: "offline" };

/** How the offer and late candidates reach the computer: on the link. */
export interface RtcPort {
  offer(sc: string, sdp: string): Promise<OfferReply>;
  ice(sc: string, cand: string): void;
}

export interface PeerEvents {
  /** A whole media message (`0x01` first), from "m" or "c". */
  media(bytes: Uint8Array): void;
  /** The channels are open and the hello is said: the computer moves the picture onto them. */
  open(): void;
  failed(why: string): void;
}

/** What the peer needs from the page: the constructor, and timers. */
export interface PeerEnv {
  RTCPeerConnection: new (config: RTCConfiguration) => RTCPeerConnection;
  setTimeout(fn: () => void, ms: number): unknown;
  clearTimeout(t: unknown): void;
}

/** One peer for one screen: offers, says the hello, carries media in and acknowledgements and input out. */
export class ScreenPeer {
  private pc: RTCPeerConnection | null = null;
  private input: RTCDataChannel | null = null;
  private opened = 0;
  private done = false;
  private timers: unknown[] = [];
  private disconnectedTimer: unknown = null;
  /** Candidates gathered so far, and who waits for the first. */
  private found = 0;
  private onFound: (() => void) | null = null;

  constructor(private env: PeerEnv, private port: RtcPort, private sc: string, private ev: PeerEvents) {}

  /** Offers, and goes on once answered; `failed` is said when it does not work. */
  async start(): Promise<void> {
    try {
      await this.run();
    } catch (e) {
      this.fail(e instanceof Error ? e.message : "error");
    }
  }

  private async run() {
    const pc = new this.env.RTCPeerConnection({ iceServers: [{ urls: STUN }], iceTransportPolicy: "all" });
    this.pc = pc;
    const media = pc.createDataChannel("m", { negotiated: true, id: 0 });
    const cursor = pc.createDataChannel("c", { negotiated: true, id: 1, ordered: false, maxRetransmits: 0 });
    const input = pc.createDataChannel("i", { negotiated: true, id: 2 });
    this.input = input;
    media.binaryType = cursor.binaryType = "arraybuffer";
    const parts = new Reassembly();
    media.onmessage = (e) => {
      const whole = e.data instanceof ArrayBuffer ? parts.push(new Uint8Array(e.data)) : null;
      if (whole && !this.done) this.ev.media(whole);
    };
    cursor.onmessage = (e) => {
      if (e.data instanceof ArrayBuffer && !this.done) this.ev.media(new Uint8Array(e.data));
    };
    let nonce: string | null = null;
    const opened = () => {
      if (++this.opened === 3 && nonce !== null) this.hello(nonce);
    };
    for (const ch of [media, cursor, input]) {
      ch.onopen = opened;
      ch.onclose = () => this.fail("closed");
    }
    let offered = false;
    pc.onicecandidate = (e) => {
      if (!e.candidate?.candidate) return;
      this.found++;
      this.onFound?.();
      if (offered) this.port.ice(this.sc, e.candidate.candidate);
    };
    pc.onconnectionstatechange = () => {
      const s = pc.connectionState;
      if (s === "failed" || s === "closed") this.fail("ice_failed");
      else if (s === "disconnected") {
        if (this.disconnectedTimer === null) this.disconnectedTimer = this.env.setTimeout(() => this.fail("ice_disconnected"), DISCONNECTED_MS);
      } else if (this.disconnectedTimer !== null) {
        this.env.clearTimeout(this.disconnectedTimer);
        this.disconnectedTimer = null;
      }
    };
    await pc.setLocalDescription(await pc.createOffer());
    await this.gathered(pc);
    if (this.done) return;
    offered = true;
    const sdp = pc.localDescription?.sdp;
    if (!sdp) throw new Error("no_offer");
    this.timers.push(this.env.setTimeout(() => this.opened < 3 && this.fail("not_open"), OPEN_MS));
    const r = await this.port.offer(this.sc, sdp);
    if (this.done) return;
    if (r.t !== "ack") return this.fail(r.t === "error" ? r.code : "offline");

    await pc.setRemoteDescription({ type: "answer", sdp: r.sdp });
    nonce = r.nonce;
    if (this.opened === 3) this.hello(nonce);
  }

  private gathered(pc: RTCPeerConnection): Promise<void> {
    return new Promise((resolve) => {
      if (pc.iceGatheringState === "complete") return resolve();
      const timers: unknown[] = [];
      const done = () => {
        for (const t of timers) this.env.clearTimeout(t);
        this.onFound = null;
        resolve();
      };
      // After its time, as soon as there is a candidate; at the latest after the longest wait.
      timers.push(this.env.setTimeout(() => (this.found > 0 ? done() : (this.onFound = done)), GATHER_MS));
      timers.push(this.env.setTimeout(done, GATHER_MAX_MS));
      pc.onicegatheringstatechange = () => {
        if (pc.iceGatheringState === "complete") done();
      };
    });
  }

  private hello(nonce: string) {
    if (this.done || !this.input) return;
    this.input.send(JSON.stringify({ type: "rtc_hello", sc: this.sc, nonce }));
    for (const t of this.timers) this.env.clearTimeout(t);
    this.timers = [];
    this.ev.open();
  }

  /** A message on "i" (an acknowledgement or input); false when the channel cannot take it now (it goes on the link instead). */
  send(msg: object): boolean {
    if (this.done || this.opened < 3 || this.input?.readyState !== "open") return false;
    // A channel that backs up this much is no faster than the link: it goes there.
    if (this.input.bufferedAmount > 64 * 1024) return false;
    try {
      this.input.send(JSON.stringify(msg));
      return true;
    } catch {
      return false;
    }
  }

  /** The way the packets take now, from the browser's stats. */
  async stats(): Promise<PathStats | null> {
    if (!this.pc || this.done) return null;
    try {
      const report = await this.pc.getStats();
      return pathFromStats(report.values() as Iterable<StatsEntry>);
    } catch {
      return null;
    }
  }

  close() {
    if (this.done) return;
    this.done = true;
    for (const t of this.timers) this.env.clearTimeout(t);
    if (this.disconnectedTimer !== null) this.env.clearTimeout(this.disconnectedTimer);
    try {
      this.pc?.close();
    } catch {
      /* already closed */
    }
  }

  private fail(why: string) {
    if (this.done) return;
    this.close();
    this.ev.failed(why);
  }
}

/** Video messages that come after a newer one are dropped: during a switch the old way's last frames can arrive after the new way's first. */
export class VideoOrder {
  private last = -1;

  /** Whether a media message may go to the decoder (`seq` of a video message, null for anything else). */
  take(seq: number | null): boolean {
    if (seq === null) return true;
    if (seq <= this.last) return false;
    this.last = seq;
    return true;
  }

  reset() {
    this.last = -1;
  }
}
