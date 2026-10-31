// Pairing this phone from the QR code's link (`#p=`): a kelpie-link IKpsk1 handshake with the computer's key and the code's
// single-use secret. The first message carries nothing; the computer answers only if the secret is live. The phone then says
// what it calls itself in its first sealed message, shows six digits (from the handshake) and its key's fingerprint for the
// person to check and type on the computer, and waits for `paired`, `declined` or `timeout` over the encrypted channel.
// A browser that already holds a pairing with this computer pairs again under the same key: the computer keeps one record per key, so
// the new record replaces the old one, and only once it is approved.
import { decodePairPayload, fingerprint, Initiator, keypair, MODE_PAIR, type Session } from "./noise";
import type { Pairing } from "./pairing-store";
import { t } from "./i18n";

export interface PairSocket {
  binaryType: string;
  onopen: ((e: unknown) => void) | null;
  onmessage: ((e: { data: unknown }) => void) | null;
  onclose: ((e: { code: number }) => void) | null;
  onerror: ((e: unknown) => void) | null;
  send(data: Uint8Array): void;
  close(): void;
}

export type PairResult = { ok: true; pairing: Pairing } | { ok: false; message: string };

/** How long the computer may take to answer the first message. */
const ANSWER_MS = 8000;
/** How long the person may take once the phone shows its digits: the computer gives up on the request after 120 s, and says so. */
const APPROVAL_MS = 135_000;

/** Close codes before a session exists (crates/kelpied/src/mobile/link.rs). */
function closedBecause(code: number): string {
  switch (code) {
    case 4403:
      return t("This pairing code is not valid any more: show a new one in Kelpie on your computer.");
    case 4429:
      return t("Too many pairing attempts. Wait a minute and try again.");
    case 4503:
      return t("Your computer is busy. Try again in a few seconds.");
    // A code that is not live is simply not answered (its check fails before anything else happens).
    default:
      return t("The computer did not answer: this code may have expired, or the connection was lost. Show a new QR code in Kelpie and try again.");
  }
}

const sameBytes = (a: Uint8Array, b: Uint8Array) => a.length === b.length && a.every((x, i) => x === b[i]);

/** Pairs with the computer the link came from. `code` is told the six digits and this phone's key fingerprint to show once
 *  the computer answered. `held`: the pairing this browser holds, if any. */
export function pair(payloadText: string, name: string, socket: () => PairSocket, code: (digits: string, key: string) => void, held?: Pairing | null): Promise<PairResult> {
  const payload = decodePairPayload(payloadText);
  if (!payload) return Promise.resolve({ ok: false, message: t("This pairing link is not valid. Show a new QR code in Kelpie on your computer.") });
  const me = held && sameBytes(held.dpk, payload.dpk) ? keypair(held.priv) : keypair();
  const hello = new TextEncoder().encode(JSON.stringify({ type: "pair", name: name.trim().slice(0, 40) || t("Phone"), want: "read" }));
  return new Promise((resolve) => {
    let done = false;
    let session: Session | null = null;
    let init: Initiator | null = null;
    let ws: PairSocket;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const wait = (ms: number) => {
      clearTimeout(timer);
      timer = setTimeout(() => finish({ ok: false, message: closedBecause(1006) }), ms);
    };
    const finish = (r: PairResult) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      ws.onopen = ws.onmessage = ws.onclose = ws.onerror = null;
      try {
        ws.close();
      } catch {
        /* gone */
      }
      resolve(r);
    };
    try {
      ws = socket();
    } catch {
      return resolve({ ok: false, message: closedBecause(1006) });
    }
    ws.binaryType = "arraybuffer";
    wait(ANSWER_MS);
    ws.onopen = () => {
      init = new Initiator(MODE_PAIR, me, payload.dpk, payload.psk);
      ws.send(init.first);
    };
    ws.onmessage = (e) => {
      if (!(e.data instanceof ArrayBuffer)) return finish({ ok: false, message: closedBecause(1006) });
      const bytes = new Uint8Array(e.data);
      try {
        if (!session) {
          session = init!.finish(bytes).session;
          for (const frame of session.seal(hello)) ws.send(frame);
          code(session.pairCode(), fingerprint(me.pub));
          wait(APPROVAL_MS);
          return;
        }
        const m = session.open(bytes);
        if (!m) return;
        const msg = JSON.parse(new TextDecoder().decode(m));
        if (msg.type === "paired") {
          const d = msg.device ?? {};
          finish({ ok: true, pairing: { priv: me.priv, pub: me.pub, dpk: payload.dpk, deviceId: String(d.id), name: String(d.name ?? name), computer: payload.name, endpoints: payload.endpoints, addrsSeq: 0 } });
        } else if (msg.type === "declined") {
          finish({ ok: false, message: t("Declined on the computer.") });
        } else if (msg.type === "timeout") {
          finish({ ok: false, message: t("Not approved in time. Show a new QR code in Kelpie and try again.") });
        }
      } catch {
        finish({ ok: false, message: t("This is not the computer the QR code came from.") });
      }
    };
    ws.onclose = (e) => finish({ ok: false, message: closedBecause(e.code) });
    ws.onerror = () => {};
  });
}
