// The app lock on the phone: what its screen shows, the two WebAuthn ceremonies, and when to tell the computer the person left. The computer enforces the lock
// (`crates/kelpied/src/mobile/lock.rs`); this only asks the person. What a page shows can be changed; what it can get from the computer without the passkey cannot.
import { sha256 } from "@noble/hashes/sha2.js";
import type { Outcome } from "./link";
import { t } from "./i18n";

/** How the computer says the lock stands: `none` no lock applies here, `open` unlocked, `read_only` it may read and no more, `closed` reading over plain HTTP is off,
 *  `unlock` the passkey is asked for, `enrol` one may be set up, `no_name` this address can have none, `broken` the one on record does not verify. */
export interface LockInfo {
  state: string;
  challenge?: string;
  rp_id?: string | null;
  cred_id?: string;
  user?: string;
  name?: string;
  idle_s?: number;
  recheck?: boolean;
}

/** `unknown`: the computer has not said yet, or the person left and it has not heard. */
export type LockFace = "unknown" | "none" | "open" | "read_only" | "closed" | "unlock" | "enrol" | "no_name" | "broken";

/** A dangerous approval's assertion, as the computer reads it. */
export interface Proof extends Record<string, unknown> {
  expiry: number;
  auth_data: string;
  client_data: string;
  signature: string;
}

export interface LockEnv {
  credentials: Credentials;
  /** A request of the lock's own: its reply. */
  ask(msg: Record<string, unknown>): Promise<Outcome>;
  lockNow(): void;
  touch(): void;
  connected(): boolean;
  now(): number;
  store: { get(key: string): string | null; set(key: string, value: string): void; del(key: string): void };
}

/** What the browser does, or a test in its place. A throw is what `navigator.credentials` does: `name` says why. */
export interface Credentials {
  available(): boolean;
  create(options: CreateOptions): Promise<Enrolled>;
  get(options: GetOptions): Promise<Asserted>;
}

export interface CreateOptions {
  rpId: string;
  userId: Uint8Array;
  name: string;
  challenge: Uint8Array;
}

export interface GetOptions {
  rpId: string;
  credentialId: Uint8Array;
  challenge: Uint8Array;
}

export interface Enrolled {
  credentialId: Uint8Array;
  /** The public key as `getPublicKey()` gives it, or null where the browser has no such thing. */
  spki: Uint8Array | null;
  /** `getPublicKeyAlgorithm()`: -7 is the one kind the computer checks. */
  algorithm: number;
  authData: Uint8Array;
  clientData: Uint8Array;
}

export interface Asserted {
  authData: Uint8Array;
  clientData: Uint8Array;
  signature: Uint8Array;
}

/** What this page last heard about the lock, for what it may paint before the computer speaks: whatever is not "no lock here" counts as locked. */
const SEEN = "kelpie-lock-seen";
/** Touches go to the computer at most this often. */
const TOUCH_MS = 30_000;
/** A challenge counts for 60 s at the computer: one older than this is replaced before the person can reach for it. */
const CHALLENGE_FRESH_MS = 45_000;
/** A dangerous approval's assertion holds this long. */
const RECHECK_MS = 60_000;

const encoder = new TextEncoder();

export function toB64(bytes: Uint8Array): string {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function fromB64(text: string): Uint8Array {
  return Uint8Array.from(atob(text.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));
}

/** What a dangerous approval's assertion is over: this request, this command, until this time. The computer works out the same from what it holds. */
export function recheckChallenge(request: string, command: string, expiry: number): Uint8Array {
  const parts = [encoder.encode(request), new Uint8Array([0]), sha256(encoder.encode(command)), encoder.encode(String(expiry))];
  const all = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) {
    all.set(p, at);
    at += p.length;
  }
  return sha256(all);
}

export class LockModel {
  face: LockFace;
  /** A ceremony is under way: the system's own sheet may hide this page meanwhile, which is not the person leaving. */
  asking = false;
  error: string | null = null;
  /** An opaque cover over everything: up from the moment the page is hidden. */
  covered = false;
  private info: LockInfo = { state: "unknown" };
  private infoAt = 0;
  /** The person left while the phone was open, and the computer has not been heard saying it is locked. */
  private leftOpen = false;
  private touchedAt = -Infinity;
  private clockOffset = 0;

  constructor(private env: LockEnv, private changed: () => void) {
    this.face = env.store.get(SEEN) === "off" ? "none" : "unknown";
  }

  /** Whether what is behind the lock may be on the screen, or be painted from what is kept. */
  clear(): boolean {
    return !this.covered && ["none", "open", "read_only"].includes(this.face);
  }

  sawServerClock(serverTs: number | undefined) {
    if (typeof serverTs === "number") this.clockOffset = serverTs - this.env.now();
  }

  /** The computer says how the lock stands (its hello, or `locked`). */
  told(info: LockInfo) {
    this.info = info;
    this.infoAt = this.env.now();
    const face = info.state as LockFace;
    if (face === "open" && this.leftOpen) {
      // It reconnected as though nothing had happened: the computer has not heard that the person left, and it hears now.
      this.env.lockNow();
      this.face = "unknown";
      return this.changed();
    }
    this.leftOpen = false;
    this.face = face;
    if (face === "open" || face === "none" || face === "read_only") {
      this.covered = false;
      this.error = null;
    }
    this.env.store.set(SEEN, face === "none" || face === "read_only" ? "off" : "on");
    this.changed();
  }

  /** The page was hidden. A ceremony's own sheet does that too, and is not leaving. */
  left() {
    if (this.asking || !["open", "unlock", "enrol", "unknown"].includes(this.face)) return;
    this.covered = true;
    if (this.face === "open") {
      this.leftOpen = true;
      this.env.lockNow();
    }
    this.changed();
  }

  /** The page was shown again. Until the computer has answered, nothing behind the lock comes back. */
  back() {
    if (this.asking) return;
    this.covered = false;
    if (this.leftOpen) this.face = "unknown";
    this.changed();
  }

  /** The person did something. */
  touched() {
    const now = this.env.now();
    if (this.face !== "open" || now - this.touchedAt < TOUCH_MS) return;
    this.touchedAt = now;
    this.env.touch();
  }

  /** Once a second, while the page is shown: the challenge the button will use stays fresh, since asking for one on the tap could cost the tap's right to start a ceremony. */
  tick() {
    if (this.asking || (this.face !== "unlock" && this.face !== "enrol") || !this.env.connected()) return;
    if (this.env.now() - this.infoAt > CHALLENGE_FRESH_MS) void this.refresh();
  }

  private async refresh() {
    this.infoAt = this.env.now();
    const o = await this.env.ask({ type: "lock_challenge" });
    if (o.t === "ack" && o.lock) this.told(o.lock);
  }

  /** The button on the lock screen: the ceremony its state calls for. Deny or cancel leaves it locked, and the button is still there. */
  async go(): Promise<void> {
    if (this.asking || (this.face !== "unlock" && this.face !== "enrol")) return;
    const { challenge, rp_id: rpId, cred_id: credId, user, name } = this.info;
    if (!this.env.credentials.available()) return this.fail(t("This browser cannot use a passkey here."));
    if (!challenge || !rpId) return this.fail(t("Waiting for the computer. Try again in a moment."));
    this.asking = true;
    this.error = null;
    this.changed();
    try {
      const reply = this.face === "enrol" ? await this.enrol(rpId, fromB64(challenge), user, name) : await this.unlock(rpId, fromB64(challenge), credId);
      if (reply === null) return;
      if (reply.t !== "ack") {
        this.error = reply.t === "error" ? reply.message : t("You are offline. It locks until the connection is back.");
        // The challenge went with the attempt, and the next try has a new one.
        await this.refresh();
      }
    } catch (e) {
      this.error = whyNot(e);
    } finally {
      this.asking = false;
      this.changed();
    }
  }

  private async enrol(rpId: string, challenge: Uint8Array, user: string | undefined, name: string | undefined): Promise<Outcome | null> {
    const made = await this.env.credentials.create({ rpId, userId: user ? fromB64(user) : new Uint8Array(16), name: name ?? "Kelpie", challenge });
    if (!made.spki || made.algorithm !== -7) {
      this.error = t("This browser or authenticator gives a kind of passkey Kelpie cannot check.");
      return null;
    }
    return this.env.ask({ type: "lock_enrol", cred_id: toB64(made.credentialId), spki: toB64(made.spki), auth_data: toB64(made.authData), client_data: toB64(made.clientData) });
  }

  private async unlock(rpId: string, challenge: Uint8Array, credId: string | undefined): Promise<Outcome | null> {
    if (!credId) {
      this.error = t("Waiting for the computer. Try again in a moment.");
      return null;
    }
    const got = await this.env.credentials.get({ rpId, credentialId: fromB64(credId), challenge });
    return this.env.ask({ type: "lock_unlock", auth_data: toB64(got.authData), client_data: toB64(got.clientData), signature: toB64(got.signature) });
  }

  /** The assertion for a dangerous approval. Null when the person did not give it; `error` says why. */
  async recheck(request: string, command: string): Promise<Proof | null> {
    const { rp_id: rpId, cred_id: credId } = this.info;
    if (!rpId || !credId || !this.env.credentials.available()) {
      this.error = t("This browser cannot use a passkey here.");
      return null;
    }
    const expiry = this.env.now() + this.clockOffset + RECHECK_MS;
    this.asking = true;
    this.error = null;
    try {
      const got = await this.env.credentials.get({ rpId, credentialId: fromB64(credId), challenge: recheckChallenge(request, command, expiry) });
      return { expiry, auth_data: toB64(got.authData), client_data: toB64(got.clientData), signature: toB64(got.signature) };
    } catch (e) {
      this.error = whyNot(e);
      return null;
    } finally {
      this.asking = false;
    }
  }

  /** The pairing is gone: so is what this page knew of its lock. */
  forget() {
    this.env.store.del(SEEN);
    this.face = "unknown";
    this.info = { state: "unknown" };
    this.covered = false;
    this.leftOpen = false;
    this.error = null;
    this.changed();
  }

  private fail(message: string) {
    this.error = message;
    this.changed();
  }
}

/** Why a ceremony did not finish, for the person. Nothing the browser says is passed on: it means little to them. */
function whyNot(e: unknown): string {
  const name = (e as { name?: string } | null)?.name;
  if (name === "NotAllowedError" || name === "AbortError") return t("Not verified. It stays locked: try again when you are ready.");
  if (name === "SecurityError") return t("This address cannot use a passkey. Open Kelpie through Tailscale.");
  if (name === "NotSupportedError") return t("This device has no screen lock or passkey that Kelpie can use. Set up Face ID, a fingerprint or a passcode, then try again.");
  return t("That did not work. Try again.");
}

/** The browser's WebAuthn, with what the computer's lock needs: this device's own authenticator, the person verified, and no attestation. */
export function browserCredentials(): Credentials {
  return {
    available: () => typeof window !== "undefined" && window.isSecureContext && !!navigator.credentials && "PublicKeyCredential" in window,
    async create(o) {
      const c = (await navigator.credentials.create({
        publicKey: {
          rp: { id: o.rpId, name: "Kelpie" },
          user: { id: o.userId as BufferSource, name: o.name, displayName: o.name },
          challenge: o.challenge as BufferSource,
          pubKeyCredParams: [{ type: "public-key", alg: -7 }],
          authenticatorSelection: { authenticatorAttachment: "platform", residentKey: "preferred", userVerification: "required" },
          attestation: "none",
          timeout: 60_000,
        },
      })) as PublicKeyCredential;
      const r = c.response as AuthenticatorAttestationResponse;
      const spki = r.getPublicKey?.();
      return { credentialId: new Uint8Array(c.rawId), spki: spki ? new Uint8Array(spki) : null, algorithm: r.getPublicKeyAlgorithm?.() ?? 0, authData: new Uint8Array(r.getAuthenticatorData()), clientData: new Uint8Array(r.clientDataJSON) };
    },
    async get(o) {
      const c = (await navigator.credentials.get({
        publicKey: { rpId: o.rpId, challenge: o.challenge as BufferSource, allowCredentials: [{ type: "public-key", id: o.credentialId as BufferSource }], userVerification: "required", timeout: 60_000 },
      })) as PublicKeyCredential;
      const r = c.response as AuthenticatorAssertionResponse;
      return { authData: new Uint8Array(r.authenticatorData), clientData: new Uint8Array(r.clientDataJSON), signature: new Uint8Array(r.signature) };
    },
  };
}
