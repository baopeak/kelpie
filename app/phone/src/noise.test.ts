import { describe, expect, it } from "vitest";
import cacophony from "../../../crates/kelpie-link/tests/cacophony-ik.json";
import vectors from "../../../crates/kelpie-link/tests/vectors.json";
import { b64url, Cipher, concat, decodePairPayload, FIRST_HEADER, fingerprint, Initiator, keypair, MAX_APP, MAX_CHUNK, MAX_NOISE, MODE_PAIR, MODE_RECONNECT, pairCode, randomBytes, Session, unb64url } from "./noise";

const hex = (s: string) => Uint8Array.from(s.match(/../g) ?? [], (b) => parseInt(b, 16));
const toHex = (b: Uint8Array) => [...b].map((x) => x.toString(16).padStart(2, "0")).join("");
const text = (b: Uint8Array | null) => new TextDecoder().decode(b ?? new Uint8Array(0));
const bytes = (s: string) => new TextEncoder().encode(s);

describe("kelpie-link on the phone", () => {
  it("makes the same bytes as the Rust crate (shared vectors)", () => {
    const phone = keypair(hex(vectors.phone_private));
    expect(toHex(phone.pub)).toBe(vectors.phone_public);
    for (const [name, mode] of [["pair", MODE_PAIR], ["reconnect", MODE_RECONNECT]] as const) {
      const c = vectors[name];
      const init = new Initiator(mode, phone, hex(vectors.desktop_public), mode === MODE_PAIR ? hex(vectors.psk) : null, { ephemeral: hex(vectors.phone_ephemeral) });
      expect(toHex(init.first), name).toBe(c.first);
      const { session, payload } = init.finish(hex(c.reply));
      expect(payload.length).toBe(0);
      expect(toHex(session.hash)).toBe(c.hash);
      expect(session.pairCode()).toBe(c.code);
      // Both ends rekey every 2 messages in the vectors, each counting its own.
      session.rekeyEvery = c.rekey_every;
      c.to_phone.forEach((f, i) => expect(text(session.open(hex(f)))).toBe(c.to_phone_plain[i]));
      c.to_desktop.forEach((f, i) => expect(toHex(session.seal(bytes(c.to_desktop_plain[i]))[0])).toBe(f));
    }
  });

  it("matches the published Noise_IK and Noise_IKpsk1 vectors (cacophony, as shipped with snow)", () => {
    expect(cacophony.vectors.length).toBe(2);
    for (const v of cacophony.vectors as any[]) {
      const psk = v.init_psks?.length ? hex(v.init_psks[0]) : null;
      const m = v.messages;
      const init = new Initiator(psk ? MODE_PAIR : MODE_RECONNECT, keypair(hex(v.init_static)), hex(v.init_remote_static), psk, {
        ephemeral: hex(v.init_ephemeral), payload: hex(m[0].payload), prologue: hex(v.init_prologue),
      });
      expect(toHex(init.first.slice(FIRST_HEADER)), v.protocol_name).toBe(m[0].ciphertext);
      const { session, payload } = init.finish(hex(m[1].ciphertext), true);
      expect(toHex(payload)).toBe(m[1].payload);
      expect(toHex(session.hash)).toBe(v.handshake_hash);
      m.slice(2).forEach((msg: { payload: string; ciphertext: string }, i: number) => {
        if (i % 2 === 0) expect(toHex(session.tx.encrypt(new Uint8Array(0), hex(msg.payload)))).toBe(msg.ciphertext);
        else expect(toHex(session.rx.decrypt(new Uint8Array(0), hex(msg.ciphertext)))).toBe(msg.payload);
      });
    }
  });

  it("refuses a reply that is not from the desktop it named, and frames that were altered, repeated or reordered", () => {
    const phone = keypair();
    const desktop = keypair();
    const init = new Initiator(MODE_RECONNECT, phone, desktop.pub, null);
    expect(() => init.finish(concat(keypair().pub, randomBytes(16)))).toThrow();
    // Two sessions that agree (made by hand from one handshake's shape): a loopback through the vectors.
    const c = vectors.pair;
    const make = () => {
      const s = new Initiator(MODE_PAIR, keypair(hex(vectors.phone_private)), hex(vectors.desktop_public), hex(vectors.psk), { ephemeral: hex(vectors.phone_ephemeral) }).finish(hex(c.reply)).session;
      s.rekeyEvery = c.rekey_every;
      return s;
    };
    const s = make();
    const tampered = hex(c.to_phone[0]);
    tampered[3] ^= 1;
    expect(() => s.open(tampered)).toThrow();
    const r = make();
    expect(text(r.open(hex(c.to_phone[0])))).toBe("hello phone");
    expect(() => r.open(hex(c.to_phone[0]))).toThrow(); // replayed
    const o = make();
    expect(() => o.open(hex(c.to_phone[1]))).toThrow(); // out of order
  });

  it("chunks a long message into Noise-sized frames and puts it back together", () => {
    // Two ends that share one key each way.
    const pairUp = () => {
      const k = randomBytes(32);
      const mk = () => new Session(new Cipher(k), new Cipher(k), new Uint8Array(32));
      return [mk(), mk()] as const;
    };
    const [a, b] = pairUp();
    a.rekeyEvery = 3;
    b.rekeyEvery = 3;
    const big = Uint8Array.from({ length: 200_000 }, (_, i) => i % 251);
    const frames = a.seal(big);
    expect(frames.length).toBe(Math.ceil(big.length / MAX_CHUNK));
    expect(frames.every((f) => f.length <= MAX_NOISE)).toBe(true);
    let got: Uint8Array | null = null;
    frames.forEach((f, i) => {
      got = b.open(f, 0);
      expect(got === null).toBe(i < frames.length - 1);
    });
    expect(toHex(got!)).toBe(toHex(big));
    expect(() => a.seal(new Uint8Array(MAX_APP + 1))).toThrow();
    // Chunks that stop coming: given up on after 10 s.
    const [c, d] = pairUp();
    const two = c.seal(new Uint8Array(MAX_CHUNK + 5));
    expect(d.open(two[0], 0)).toBeNull();
    expect(() => d.open(two[1], 10_001)).toThrow();
  });

  it("reads the pairing QR payload and nothing else", () => {
    const dpk = randomBytes(32);
    const psk = randomBytes(32);
    const p = b64url(bytes(JSON.stringify({ v: 1, dpk: b64url(dpk), psk: b64url(psk), ep: [{ t: "lan", a: "192.168.1.5:4817" }, { t: "relay", a: "wss://r.example", r: "room" }, { t: 5 }], name: "Desk" })));
    const got = decodePairPayload(p)!;
    expect(toHex(got.dpk)).toBe(toHex(dpk));
    expect(toHex(got.psk)).toBe(toHex(psk));
    expect(got.endpoints).toEqual([{ t: "lan", a: "192.168.1.5:4817" }, { t: "relay", a: "wss://r.example", r: "room" }]);
    expect(got.name).toBe("Desk");
    for (const bad of ["", "!!", b64url(bytes("{}")), b64url(bytes(JSON.stringify({ v: 2, dpk: b64url(dpk), psk: b64url(psk) }))), b64url(bytes(JSON.stringify({ v: 1, dpk: "AAAA", psk: b64url(psk) })))]) {
      expect(decodePairPayload(bad)).toBeNull();
    }
    expect(toHex(unb64url(b64url(dpk)))).toBe(toHex(dpk));
    expect(pairCode(new Uint8Array(32))).toMatch(/^\d{6}$/);
    expect(fingerprint(new Uint8Array(32))).toBe("6668 7AAD F862 BD77");
  });
});
