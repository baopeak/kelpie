// The phone's side of kelpie-link (crates/kelpie-link): Noise IKpsk1 to pair, IK to reconnect, both
// 25519 / ChaChaPoly / BLAKE2s, and the session framing (chunks with a flags byte; each side rekeys by counting messages).
//
// Built on the audited, dependency-free @noble libraries rather than WebCrypto: `crypto.subtle` exists only on secure
// origins, and the phone page is usually served over plain http:// on the LAN; WebCrypto has neither ChaCha20-Poly1305
// nor BLAKE2s anyway. Only `crypto.getRandomValues` (available everywhere) is taken from the browser. The bytes this file
// produces are checked against the Rust crate's own vectors (noise.test.ts, crates/kelpie-link/tests/vectors.json).
import { chacha20poly1305 } from "@noble/ciphers/chacha.js";
import { x25519 } from "@noble/curves/ed25519.js";
import { blake2s } from "@noble/hashes/blake2.js";
import { hkdf as rfc5869 } from "@noble/hashes/hkdf.js";
import { hmac } from "@noble/hashes/hmac.js";
import { sha256 } from "@noble/hashes/sha2.js";

/** The protocol's version: in the first message and the Noise prologue. Fixed: no negotiation, and never a retry at a lower one. */
export const VERSION = 1;
export const MODE_PAIR = 1;
export const MODE_RECONNECT = 2;
/** What precedes the Noise message in a first message: version, mode, mac1. */
export const FIRST_HEADER = 18;
export const MAX_NOISE = 65535;
const TAG = 16;
export const MAX_CHUNK = MAX_NOISE - TAG - 1;
export const MAX_APP = 1 << 20;
const REKEY_EVERY = 1 << 16;
/** A message whose chunks stop arriving for this long ends the connection. */
export const REASSEMBLY_MS = 10_000;
const MORE = 1;
const HASHLEN = 32;
const enc = new TextEncoder();
const PROLOGUE = enc.encode(`kelpie-link/${VERSION}`);

export interface Keys {
  priv: Uint8Array;
  pub: Uint8Array;
}

export function randomBytes(n: number): Uint8Array {
  const b = new Uint8Array(n);
  crypto.getRandomValues(b);
  return b;
}

export function keypair(priv?: Uint8Array): Keys {
  const p = priv ?? randomBytes(32);
  return { priv: p, pub: x25519.getPublicKey(p) };
}

export function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    out.set(p, at);
    at += p.length;
  }
  return out;
}

export function b64url(b: Uint8Array): string {
  let s = "";
  for (const x of b) s += String.fromCharCode(x);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function unb64url(s: string): Uint8Array {
  const t = s.trim().replace(/-/g, "+").replace(/_/g, "/");
  const bin = atob(t + "=".repeat((4 - (t.length % 4)) % 4));
  return Uint8Array.from(bin, (c) => c.charCodeAt(0));
}

const hash = (d: Uint8Array) => blake2s(d);
const mac = (k: Uint8Array, d: Uint8Array) => hmac(blake2s, k, d);

function hkdf(ck: Uint8Array, ikm: Uint8Array, n: 2 | 3): Uint8Array[] {
  const temp = mac(ck, ikm);
  const out: Uint8Array[] = [];
  let prev: Uint8Array = new Uint8Array(0);
  for (let i = 1; i <= n; i++) {
    prev = mac(temp, concat(prev, Uint8Array.of(i)));
    out.push(prev);
  }
  return out;
}

const dh = (priv: Uint8Array, pub: Uint8Array) => x25519.getSharedSecret(priv, pub);

export class Cipher {
  n = 0n;
  constructor(public k: Uint8Array | null = null) {}

  private static nonce(n: bigint): Uint8Array {
    const b = new Uint8Array(12);
    new DataView(b.buffer).setBigUint64(4, n, true);
    return b;
  }

  encrypt(ad: Uint8Array, p: Uint8Array): Uint8Array {
    if (!this.k) return p;
    const c = chacha20poly1305(this.k, Cipher.nonce(this.n), ad).encrypt(p);
    this.n++;
    return c;
  }

  /** Throws when the message does not authenticate. */
  decrypt(ad: Uint8Array, c: Uint8Array): Uint8Array {
    if (!this.k) return c;
    const p = chacha20poly1305(this.k, Cipher.nonce(this.n), ad).decrypt(c);
    this.n++;
    return p;
  }

  rekey() {
    this.k = chacha20poly1305(this.k!, Cipher.nonce(0xffffffffffffffffn), new Uint8Array(0)).encrypt(new Uint8Array(32)).slice(0, 32);
  }
}

class Symmetric {
  h: Uint8Array;
  ck: Uint8Array;
  c = new Cipher();

  constructor(name: string) {
    const n = enc.encode(name);
    this.h = n.length <= HASHLEN ? concat(n, new Uint8Array(HASHLEN - n.length)) : hash(n);
    this.ck = this.h;
  }

  mixHash(d: Uint8Array) {
    this.h = hash(concat(this.h, d));
  }

  mixKey(ikm: Uint8Array) {
    const [ck, k] = hkdf(this.ck, ikm, 2);
    this.ck = ck;
    this.c = new Cipher(k.slice(0, 32));
  }

  mixKeyAndHash(ikm: Uint8Array) {
    const [ck, th, k] = hkdf(this.ck, ikm, 3);
    this.ck = ck;
    this.mixHash(th);
    this.c = new Cipher(k.slice(0, 32));
  }

  encryptAndHash(p: Uint8Array): Uint8Array {
    const c = this.c.encrypt(this.h, p);
    this.mixHash(c);
    return c;
  }

  decryptAndHash(c: Uint8Array): Uint8Array {
    const p = this.c.decrypt(this.h, c);
    this.mixHash(c);
    return p;
  }

  split(): [Cipher, Cipher] {
    const [k1, k2] = hkdf(this.ck, new Uint8Array(0), 2);
    return [new Cipher(k1.slice(0, 32)), new Cipher(k2.slice(0, 32))];
  }
}

/** A key as people compare it: the start of its SHA-256 in groups of four (Kelpie's approval dialog shows the same). */
export function fingerprint(pub: Uint8Array): string {
  const hex = [...sha256(pub).slice(0, 8)].map((b) => b.toString(16).padStart(2, "0").toUpperCase()).join("");
  return hex.match(/.{4}/g)!.join(" ");
}

/** The six digits both ends derive from a handshake (HKDF-BLAKE2s of the handshake hash, info "kelpie-sas"): the phone shows
 *  them, the person types them on the computer, so the approval is bound to this very handshake. */
export function pairCode(h: Uint8Array): string {
  const d = rfc5869(blake2s, h, undefined, enc.encode("kelpie-sas"), 4);
  const n = (((d[0] << 24) >>> 0) + (d[1] << 16) + (d[2] << 8) + d[3]) % 1_000_000;
  return String(n).padStart(6, "0");
}

/** The first message's prefilter (a keyed BLAKE2s of 16 bytes), checked by the computer before any Diffie-Hellman: keyed from its
 *  public key, or from the pairing secret. */
function mac1(material: Uint8Array, label: string, mode: number, msg: Uint8Array): Uint8Array {
  const key = blake2s(concat(enc.encode(label), material));
  return blake2s(concat(Uint8Array.of(VERSION, mode), msg), { key, dkLen: 16 });
}

/** The phone's half of a handshake: `first` is the first socket message, `finish(reply)` the desktop's answer. */
export class Initiator {
  readonly first: Uint8Array;
  private ss: Symmetric;
  private e: Keys;
  private psk: boolean;

  /** The first message carries nothing (it is replayable): `test` sets a payload, an ephemeral key and a prologue only to
   *  check this state machine against the published Noise vectors. */
  constructor(mode: number, private s: Keys, desktop: Uint8Array, psk: Uint8Array | null, test?: { ephemeral?: Uint8Array; payload?: Uint8Array; prologue?: Uint8Array }) {
    if ((mode === MODE_PAIR) !== (psk !== null)) throw new Error("a pairing needs its code, a reconnect none");
    this.psk = psk !== null;
    this.ss = new Symmetric(this.psk ? "Noise_IKpsk1_25519_ChaChaPoly_BLAKE2s" : "Noise_IK_25519_ChaChaPoly_BLAKE2s");
    this.ss.mixHash(test?.prologue ?? concat(PROLOGUE, Uint8Array.of(mode)));
    const ephemeral = test?.ephemeral;
    const payload = test?.payload ?? new Uint8Array(0);
    this.ss.mixHash(desktop);
    this.e = keypair(ephemeral);
    this.ss.mixHash(this.e.pub);
    if (this.psk) this.ss.mixKey(this.e.pub);
    this.ss.mixKey(dh(this.e.priv, desktop)); // es
    const sEnc = this.ss.encryptAndHash(s.pub); // s
    this.ss.mixKey(dh(s.priv, desktop)); // ss
    if (psk) this.ss.mixKeyAndHash(psk);
    const pEnc = this.ss.encryptAndHash(payload);
    const msg = concat(this.e.pub, sEnc, pEnc);
    const tag = psk ? mac1(psk, "kelpie-link mac1 psk", mode, msg) : mac1(desktop, "kelpie-link mac1 desktop", mode, msg);
    this.first = concat(Uint8Array.of(VERSION, mode), tag, msg);
  }

  /** Throws when the reply is not from the desktop this phone named, or carries anything. */
  finish(reply: Uint8Array, payloadAllowed = false): { session: Session; payload: Uint8Array } {
    if (reply.length < 32 + TAG) throw new Error("short reply");
    const re = reply.slice(0, 32);
    this.ss.mixHash(re);
    if (this.psk) this.ss.mixKey(re);
    this.ss.mixKey(dh(this.e.priv, re)); // ee
    this.ss.mixKey(dh(this.s.priv, re)); // se
    const payload = this.ss.decryptAndHash(reply.slice(32));
    if (payload.length && !payloadAllowed) throw new Error("a reply that carries anything is not kelpie-link");
    const [send, recv] = this.ss.split();
    return { session: new Session(send, recv, this.ss.h), payload };
  }
}

/** An established link. Any error from `open` means the connection is over (drop it and handshake again). */
export class Session {
  private sentSinceRekey = 0;
  private receivedSinceRekey = 0;
  /** Messages between rekeys, each direction counting its own (2^16; smaller in tests). */
  rekeyEvery = REKEY_EVERY;
  private partial: Uint8Array[] = [];
  private partialSize = 0;
  private partialSince: number | null = null;

  /** `tx` / `rx`: the raw transport ciphers (the published vectors check them). */
  constructor(readonly tx: Cipher, readonly rx: Cipher, readonly hash: Uint8Array) {}

  pairCode(): string {
    return pairCode(this.hash);
  }

  /** One application message as the socket messages to send, in order. */
  seal(msg: Uint8Array): Uint8Array[] {
    if (msg.length > MAX_APP) throw new Error("message too large");
    const chunks: Uint8Array[] = [];
    for (let at = 0; at < msg.length; at += MAX_CHUNK) chunks.push(msg.subarray(at, at + MAX_CHUNK));
    if (!chunks.length) chunks.push(new Uint8Array(0));
    return chunks.map((chunk, i) => {
      const frame = this.tx.encrypt(new Uint8Array(0), concat(Uint8Array.of(i < chunks.length - 1 ? MORE : 0), chunk));
      if (++this.sentSinceRekey >= this.rekeyEvery) {
        this.tx.rekey();
        this.sentSinceRekey = 0;
      }
      return frame;
    });
  }

  /** One socket message: the whole application message once its last chunk is in, `null` while more are to come. */
  open(frame: Uint8Array, now = Date.now()): Uint8Array | null {
    if (frame.length > MAX_NOISE || frame.length < TAG + 1) throw new Error("bad frame");
    if (this.partialSince !== null && now - this.partialSince > REASSEMBLY_MS) throw new Error("a message was cut off");
    const plain = this.rx.decrypt(new Uint8Array(0), frame);
    if (++this.receivedSinceRekey >= this.rekeyEvery) {
      this.rx.rekey();
      this.receivedSinceRekey = 0;
    }
    const flags = plain[0];
    if (flags & ~MORE) throw new Error("bad flags");
    const chunk = plain.subarray(1);
    if (this.partialSize + chunk.length > MAX_APP) throw new Error("message too large");
    this.partial.push(chunk);
    this.partialSize += chunk.length;
    if (flags & MORE) {
      this.partialSince ??= now;
      return null;
    }
    const whole = this.partial.length === 1 ? this.partial[0] : concat(...this.partial);
    this.partial = [];
    this.partialSize = 0;
    this.partialSince = null;
    return whole;
  }
}

/** A way to reach the computer (`t`: `lan` today, `a` being `ip:port`; a kind this client does not know is kept and skipped). */
export interface Endpoint {
  t: string;
  a: string;
  r?: string;
}

/** What the computer's pairing QR code carries (`#p=` of the link): see `QrPayload` in kelpie-link. */
export interface PairPayload {
  dpk: Uint8Array;
  psk: Uint8Array;
  endpoints: Endpoint[];
  name: string;
}

export function decodePairPayload(text: string): PairPayload | null {
  try {
    const raw = new TextDecoder().decode(unb64url(text));
    const v = JSON.parse(raw);
    if (v?.v !== 1 || typeof v.dpk !== "string" || typeof v.psk !== "string") return null;
    const dpk = unb64url(v.dpk);
    const psk = unb64url(v.psk);
    if (dpk.length !== 32 || psk.length !== 32) return null;
    const endpoints: Endpoint[] = Array.isArray(v.ep) ? v.ep.filter((e: Endpoint) => typeof e?.t === "string" && typeof e?.a === "string").slice(0, 32) : [];
    return { dpk, psk, endpoints, name: typeof v.name === "string" ? v.name.slice(0, 64) : "" };
  } catch {
    return null;
  }
}
