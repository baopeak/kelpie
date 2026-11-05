// The relay's front door. A socket reaches a room only with a valid ticket, and the ticket is
// checked here so that forged traffic never costs a Durable Object call. Never serves HTML.
//
// - `GET /v1/health`: "ok", nothing else.
// - `POST /v1/rooms`: makes a room, for whoever holds the registration key (a Worker secret). The body is the room's public key;
//   the answer is the room id and its ticket. Strangers cannot make rooms, which is also the first guard on the bill.
// - `GET /v1/desktop`, `GET /v1/phone`: WebSockets into a room, `?t=<ticket>` and what the room asks of each (room.ts).

import { decodeBase64Url, encodeBase64Url, hex } from "./encoding";
import type { Room } from "./room";

export interface Env {
  ROOM: DurableObjectNamespace<Room>;
  TICKET_SECRET: string;
  REGISTRATION_KEY?: string;
}

// A ticket is `<room id, 32 hex>.<HMAC-SHA256 of the room id, base64url without padding>`.
const TICKET = /^([0-9a-f]{32})\.([A-Za-z0-9_-]{43})$/;
const SOCKET_PATHS = new Set(["/v1/desktop", "/v1/phone"]);
/** A P-256 public key, uncompressed (65 bytes), base64url. */
const ROOM_KEY = /^[A-Za-z0-9_-]{87}$/;
const MAX_REGISTRATION_BODY = 1024;
const FAILURES_KEPT = 4096;

/** Failures counted by a hash of the address (and, for sockets, the room), in this isolate only: best effort, and never the address itself. */
class Strikes {
  private readonly seen = new Map<string, { count: number; since: number }>();

  constructor(private readonly limit: number, private readonly windowMs: number) {}

  out(key: string, now: number): boolean {
    const f = this.seen.get(key);
    if (f && now - f.since >= this.windowMs) this.seen.delete(key);
    return (this.seen.get(key)?.count ?? 0) >= this.limit;
  }

  note(key: string, now: number): void {
    if (this.seen.size >= FAILURES_KEPT && !this.seen.has(key)) this.seen.clear();
    const f = this.seen.get(key) ?? { count: 0, since: now };
    f.count += 1;
    this.seen.set(key, f);
  }
}

/** Wrong registration keys one address may send in a day. */
const registrations = new Strikes(10, 24 * 3600 * 1000);
/** Sockets a room refused one address in 10 minutes: a ticket is no secret (every QR code has one), so with one a stranger could
 *  otherwise make the room look at every guess. Past this, the router refuses without asking the room. */
export const ADMISSION_REFUSALS = 30;
const admissions = new Strikes(ADMISSION_REFUSALS, 10 * 60 * 1000);

let cachedKey: { secret: string; key: Promise<CryptoKey> } | undefined;

function ticketKey(secret: string): Promise<CryptoKey> {
  if (cachedKey?.secret !== secret) {
    const key = crypto.subtle.importKey(
      "raw",
      new TextEncoder().encode(secret),
      { name: "HMAC", hash: "SHA-256" },
      false,
      ["sign", "verify"],
    );
    cachedKey = { secret, key };
  }
  return cachedKey.key;
}

/** The room id when the ticket is genuine, otherwise null. */
export async function checkTicket(ticket: string | null, secret: string): Promise<string | null> {
  const match = ticket === null ? null : TICKET.exec(ticket);
  if (!match) return null;
  const [, room, mac] = match;
  const genuine = await crypto.subtle.verify(
    "HMAC",
    await ticketKey(secret),
    decodeBase64Url(mac),
    new TextEncoder().encode(room),
  );
  return genuine ? room : null;
}

export async function makeTicket(room: string, secret: string): Promise<string> {
  const mac = await crypto.subtle.sign("HMAC", await ticketKey(secret), new TextEncoder().encode(room));
  return `${room}.${encodeBase64Url(new Uint8Array(mac))}`;
}

function plain(status: number, body: string): Response {
  return new Response(body, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" },
  });
}

async function digest(text: string): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)));
}

/** Compares two secrets in time that does not depend on where they differ (their digests have one length). */
async function sameSecret(a: string, b: string): Promise<boolean> {
  const [x, y] = await Promise.all([digest(a), digest(b)]);
  return crypto.subtle.timingSafeEqual(x, y);
}

/** Where a failure counts: a hash of the address (and what else is given), never the address itself. */
async function failureKey(request: Request, also = ""): Promise<string> {
  return hex(await digest(`kelpie-relay/1 ${request.headers.get("cf-connecting-ip") ?? ""} ${also}`));
}

/** Makes a room for the holder of the registration key: the desktop sends its room's public key. */
async function register(request: Request, env: Env): Promise<Response> {
  // A page can never hold the key: nothing a browser sends gets this far.
  if (request.headers.has("origin")) return plain(403, "not from a page\n");
  if (!env.REGISTRATION_KEY || !env.TICKET_SECRET) return plain(503, "relay not configured\n");
  const now = Date.now();
  const who = await failureKey(request);
  if (registrations.out(who, now)) return plain(429, "too many attempts\n");
  const given = /^Bearer (\S{1,256})$/.exec(request.headers.get("authorization") ?? "")?.[1];
  if (!given || !(await sameSecret(given, env.REGISTRATION_KEY))) {
    registrations.note(who, now);
    return plain(401, "registration key\n");
  }
  if (Number(request.headers.get("content-length") ?? "0") > MAX_REGISTRATION_BODY) return plain(413, "too large\n");
  const body = (await request.json().catch(() => null)) as { pk?: unknown } | null;
  const pk = typeof body?.pk === "string" ? body.pk : "";
  if (!ROOM_KEY.test(pk) || !(await usableRoomKey(pk))) return plain(400, "room key\n");
  const room = hex(crypto.getRandomValues(new Uint8Array(16)));
  // No location hint: this first call, from the desktop's own request, places the room near the desktop.
  await env.ROOM.get(env.ROOM.idFromName(room)).init(room, pk);
  return new Response(JSON.stringify({ room, ticket: await makeTicket(room, env.TICKET_SECRET) }), {
    status: 201,
    headers: { "content-type": "application/json", "cache-control": "no-store" },
  });
}

async function usableRoomKey(pk: string): Promise<boolean> {
  try {
    await crypto.subtle.importKey("raw", decodeBase64Url(pk), { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
    return true;
  } catch {
    return false;
  }
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const { pathname, searchParams } = new URL(request.url);
    if (pathname === "/v1/health") return plain(200, "ok\n");
    if (pathname === "/v1/rooms") return request.method === "POST" ? register(request, env) : plain(405, "post only\n");
    if (!SOCKET_PATHS.has(pathname)) return plain(404, "not found\n");
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      return plain(426, "websocket only\n");
    }
    if (!env.TICKET_SECRET) return plain(503, "relay not configured\n");

    const room = await checkTicket(searchParams.get("t"), env.TICKET_SECRET);
    if (room === null) return plain(403, "bad ticket\n");

    const who = await failureKey(request, room);
    const now = Date.now();
    if (admissions.out(who, now)) return plain(429, "too many attempts\n");
    const stub = env.ROOM.get(env.ROOM.idFromName(room));
    const forwarded = new Request(request);
    forwarded.headers.set("x-entry-colo", String(request.cf?.colo ?? ""));
    forwarded.headers.set("x-kelpie-room", room);
    const answer = await stub.fetch(forwarded);
    if (answer.status === 403 || answer.status === 404) admissions.note(who, now);
    return answer;
  },
};
