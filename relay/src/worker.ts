// The relay's front door. A socket reaches a room only with a valid ticket, and the ticket is
// checked here so that forged traffic never costs a Durable Object call. Never serves HTML: on Cloudflare the phone app's static files
// are served beside it (wrangler.toml `[assets]`) without running it, and it answers only `/v1/*`, always in static plain text or JSON
// that repeats nothing of the request, since a page runs on this same origin.
//
// - `GET /v1/health`: "ok", nothing else.
// - `POST /v1/rooms`: makes a room. The body is the room's public key; the answer is the room id and its ticket. With a registration
//   key set (a Worker secret), only its holder may; otherwise, with `OPEN_REGISTRATION = "1"`, anyone, within limits per network and
//   in all (rate binding, then the Registrar); otherwise nobody. Never a page.
// - `GET /v1/desktop`, `GET /v1/phone`: WebSockets into a room, `?t=<ticket>` and what the room asks of each (room.ts).

import { decodeBase64Url, encodeBase64Url, hex } from "./encoding";
import type { Registrar } from "./registrar";
import type { Room } from "./room";

export interface Env {
  ROOM: DurableObjectNamespace<Room>;
  REGISTRAR: DurableObjectNamespace<Registrar>;
  TICKET_SECRET: string;
  REGISTRATION_KEY?: string;
  /** "1": rooms for anyone when no registration key is set. */
  OPEN_REGISTRATION?: string;
  /** A first, approximate shed of room making per network (Cloudflare only; skipped where there is none). */
  ROOM_RATE?: RateLimit;
  ROOMS_PER_NETWORK_DAY?: string;
  ROOMS_PER_DAY?: string;
}

// A ticket is `<room id, 32 hex>.<HMAC-SHA256 of the room id, base64url without padding>`.
const TICKET = /^([0-9a-f]{32})\.([A-Za-z0-9_-]{43})$/;
const SOCKET_PATHS = new Set(["/v1/desktop", "/v1/phone"]);
/** A P-256 public key, uncompressed (65 bytes), base64url. */
const ROOM_KEY = /^[A-Za-z0-9_-]{87}$/;
const MAX_REGISTRATION_BODY = 1024;
const FAILURES_KEPT = 4096;
/** Rooms one network may make in a UTC day without a registration key, and all networks together, unless the vars say otherwise. */
export const ROOMS_PER_NETWORK_DAY = 20;
export const ROOMS_PER_DAY = 1000;

/** Failures counted by a hash of the network (and, for sockets, the path and the room), in this isolate only: best effort, and never the
 *  address itself. A full table lets go of what expired and then of its oldest, one for each that comes, never of everything at once. */
export class Strikes {
  /** In the order counting began, which is the order they expire in. */
  private readonly seen = new Map<string, { count: number; since: number }>();

  constructor(private readonly limit: number, private readonly windowMs: number, private readonly kept = FAILURES_KEPT) {}

  out(key: string, now: number): boolean {
    const f = this.seen.get(key);
    if (f && now - f.since >= this.windowMs) this.seen.delete(key);
    return (this.seen.get(key)?.count ?? 0) >= this.limit;
  }

  note(key: string, now: number): void {
    const f = this.seen.get(key);
    if (f && now - f.since < this.windowMs) {
      f.count += 1;
      return;
    }
    this.seen.delete(key);
    for (const [k, v] of this.seen) {
      if (this.seen.size < this.kept && now - v.since < this.windowMs) break;
      this.seen.delete(k);
    }
    this.seen.set(key, { count: 1, since: now });
  }

  get size(): number {
    return this.seen.size;
  }
}

/** Where an address's failures count: an IPv6 address by its /64, which one home or phone network has whole (an address in it costs nothing
 *  to change), an IPv4 address as it is. */
export function network(ip: string): string {
  if (!ip.includes(":")) return ip;
  // An IPv4 address written as IPv6 (`::ffff:192.0.2.1`) is that IPv4 address.
  if (ip.includes(".")) return ip.slice(ip.lastIndexOf(":") + 1);
  const halves = ip.toLowerCase().split("::");
  if (halves.length > 2) return ip;
  const head = halves[0] ? halves[0].split(":") : [];
  const tail = halves.length === 2 && halves[1] ? halves[1].split(":") : [];
  const zeros = halves.length === 2 ? 8 - head.length - tail.length : 0;
  const groups = [...head, ...Array<string>(Math.max(zeros, 0)).fill("0"), ...tail];
  if (zeros < 0 || groups.length !== 8 || !groups.every((g) => /^[0-9a-f]{1,4}$/.test(g))) return ip;
  return `${groups.slice(0, 4).map((g) => parseInt(g, 16).toString(16)).join(":")}::/64`;
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

const NO_SNIFF = { "x-content-type-options": "nosniff", "cache-control": "no-store" };

function plain(status: number, body: string, reason?: string): Response {
  return new Response(body, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8", ...NO_SNIFF, ...(reason ? { "x-kelpie-reason": reason } : {}) },
  });
}

/** A whole positive number from a var, or the default. */
function limit(value: string | undefined, fallback: number): number {
  const n = Number(value);
  return value && Number.isSafeInteger(n) && n > 0 ? n : fallback;
}

async function digest(text: string): Promise<Uint8Array> {
  return new Uint8Array(await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text)));
}

/** Compares two secrets in time that does not depend on where they differ (their digests have one length). */
async function sameSecret(a: string, b: string): Promise<boolean> {
  const [x, y] = await Promise.all([digest(a), digest(b)]);
  return crypto.subtle.timingSafeEqual(x, y);
}

/** Where a failure counts: a hash of the network (and what else is given), never the address itself. */
async function failureKey(request: Request, also = ""): Promise<string> {
  return hex(await digest(`kelpie-relay/1 ${network(request.headers.get("cf-connecting-ip") ?? "")} ${also}`));
}

/** The router's own refusal, which says so (`x-kelpie-reason`), so that the desktop does not take it for the room's monthly limit. */
export const TOO_MANY_REFUSED = "attempts";
/** Why a room was not made without a key: too many from this network just now, or today; or the relay made all it makes today. */
export const ROOMS_RATE = "rate";
export const ROOMS_NETWORK = "network";
export const ROOMS_FULL = "full";

/** Makes a room for the holder of the registration key, or, with registration open, for anyone within the limits: the desktop sends its
 *  room's public key. */
async function register(request: Request, env: Env): Promise<Response> {
  // A page can never make a room: nothing a browser sends gets this far.
  if (request.headers.has("origin")) return plain(403, "not from a page\n");
  if (!env.TICKET_SECRET) return plain(503, "relay not configured\n");
  const now = Date.now();
  const who = await failureKey(request);
  // A key, once set, is the only way in: it closes registration that was open.
  const open = !env.REGISTRATION_KEY;
  if (env.REGISTRATION_KEY) {
    if (registrations.out(who, now)) return plain(429, "too many attempts\n");
    const given = /^Bearer (\S{1,256})$/.exec(request.headers.get("authorization") ?? "")?.[1];
    if (!given || !(await sameSecret(given, env.REGISTRATION_KEY))) {
      registrations.note(who, now);
      return plain(401, "registration key\n");
    }
  } else if (env.OPEN_REGISTRATION !== "1") {
    return plain(503, "relay not configured\n");
  }
  if (Number(request.headers.get("content-length") ?? "0") > MAX_REGISTRATION_BODY) return plain(413, "too large\n");
  const body = (await request.json().catch(() => null)) as { pk?: unknown } | null;
  const pk = typeof body?.pk === "string" ? body.pk : "";
  if (!ROOM_KEY.test(pk) || !(await usableRoomKey(pk))) return plain(400, "room key\n");
  if (open) {
    // The binding first: it is per location and approximate, but sheds a burst before it reaches the one Registrar.
    if (env.ROOM_RATE && !(await env.ROOM_RATE.limit({ key: who })).success) return plain(429, "too many rooms just now\n", ROOMS_RATE);
    const registrar = env.REGISTRAR.get(env.REGISTRAR.idFromName("registrar"));
    const admitted = await registrar.admit(who, now, limit(env.ROOMS_PER_NETWORK_DAY, ROOMS_PER_NETWORK_DAY), limit(env.ROOMS_PER_DAY, ROOMS_PER_DAY));
    if (admitted === "network") return plain(429, "too many rooms from this network today\n", ROOMS_NETWORK);
    if (admitted === "full") return plain(503, "no new rooms today\n", ROOMS_FULL);
  }
  const room = hex(crypto.getRandomValues(new Uint8Array(16)));
  // No location hint: this first call, from the desktop's own request, places the room near the desktop.
  await env.ROOM.get(env.ROOM.idFromName(room)).init(room, pk);
  return new Response(JSON.stringify({ room, ticket: await makeTicket(room, env.TICKET_SECRET) }), {
    status: 201,
    headers: { "content-type": "application/json", ...NO_SNIFF },
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

    // Phones' refusals and the desktop's count apart: a removed phone that keeps trying from the desktop's network never shuts the desktop out.
    const who = await failureKey(request, `${pathname} ${room}`);
    const now = Date.now();
    if (admissions.out(who, now)) return plain(429, "too many refused attempts\n", TOO_MANY_REFUSED);
    const stub = env.ROOM.get(env.ROOM.idFromName(room));
    const forwarded = new Request(request);
    forwarded.headers.set("x-entry-colo", String(request.cf?.colo ?? ""));
    forwarded.headers.set("x-kelpie-room", room);
    const answer = await stub.fetch(forwarded);
    if (answer.status === 403 || answer.status === 404) admissions.note(who, now);
    return answer;
  },
};
