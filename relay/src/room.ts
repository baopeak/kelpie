// One Durable Object per room. It forwards binary frames it cannot read between one desktop and up to eight phones, and
// stores only what admits them: the room's public key, the hashes of the phones' tokens, the hashes of live pairing tokens,
// the page origins phones may come from, and how much the room has carried this month. Everything a socket needs to survive
// hibernation lives in its attachment, and all waiting is left to the runtime, so that an idle room costs nothing.
//
// - **Desktop**: `/v1/desktop?t=&ts=&sig=`, `sig` being an ECDSA P-256 signature with the room key over
//   `kelpie-relay/1 desktop <room> <ts>`; `ts` is the desktop's clock in ms, within 5 minutes of ours and above the last one
//   used, so a signature cannot be used twice. No `Origin`. The newest desktop wins. Its 101 says which version of this protocol the
//   room speaks (`x-kelpie-relay-version`), so that a desktop never sends a message an older room would close it for.
// - **Phone**: `/v1/phone?t=&k=<token>` with a token whose SHA-256 the desktop put on the allow-list, or `&p=<pair token>`
//   once. An `Origin`, when there is one, must be one of those the desktop registered: a guard against other pages only, the token
//   being what admits.
// - **Frames**: binary only, at most 64 KiB. A phone's frame reaches the desktop as `[slot, u16 big-endian] + frame`; the
//   desktop's goes to the slot it names. Nothing is queued: with no desktop ready a phone's frame is dropped, uncounted. A slot is a number
//   for one phone socket, never given to another while any socket still holds it, so a late close, frame or kick of an old socket
//   cannot reach a new one. The desktop's frames are limited per slot: one phone's traffic closes that phone, not the room.
// - **The desktop's word** (JSON text): `allow` (the whole allow-list; phones whose token left it are closed, and the desktop
//   hears of phones only after it), `pair`, `origins` (up to four; `origin`, one, from desktops before it), `kick`, `drop`, heard from
//   the newest desktop socket only. It hears `open` for each phone slot, with the hash the phone came in with (`k`) and the page
//   origin it came from (`o`, none for an app that sends none), and `close`, with the phone's close code (`c`).
//   Phones hear `desktop_offline` and `desktop_online`, which a relay can forge: hints only.

import { DurableObject } from "cloudflare:workers";
import { decodeBase64Url, hex } from "./encoding";

interface RoomEnv {
  RELAY_VERSION?: string;
}

interface Attachment {
  role: "desktop" | "phone";
  slot: number;
  entry: string;
  /** Phone: how it was let in, and the hash of its token (or of its pairing token), so that a revoke can close it. */
  via?: "token" | "pair";
  cred?: string;
  /** Phone: the page origin it came from, one the desktop registered; none for an app that sends none. */
  origin?: string;
  /** Desktop: it has sent its allow-list on this socket, so phones may reach it. */
  ready?: boolean;
  /** Desktop: a newer desktop took the room; what this socket still says is not heard. */
  replaced?: boolean;
}

interface PairToken {
  h: string;
  until: number;
}

interface Usage {
  month: string;
  bytes: number;
}

const MAX_PHONES = 8;
const SLOT_BYTES = 2;
/** How many slot numbers there are (a u16). */
const SLOTS = 65_536;
export const MAX_FRAME = 64 * 1024;
const MAX_ALLOWED = 64;
const MAX_PAIR_TOKENS = 4;
const MAX_PAIR_TTL_S = 120;
const MAX_ORIGIN = 300;
const MAX_ORIGINS = 4;
const ORIGIN = /^https:\/\/[^/?#@\s\\]+$/;
/** The protocol this room speaks: 2 added `origins` and `o`. Said on the desktop's 101, whatever the deployment calls itself. */
export const PROTOCOL_VERSION = "2";
/** More than this many messages in a second from one socket closes it. */
export const MESSAGES_PER_SECOND = 200;
/** What the room carries at most, as a burst and then each second. */
export const ROOM_BURST_BYTES = 4 * 1024 * 1024;
/** A soft cap on what a room carries in a calendar month (UTC). */
export const MONTHLY_BYTES = 2 * 1024 * 1024 * 1024;
/** Usage is written at most this often (or each MB): a minute's count lost to an eviction matters little against a monthly cap. */
export const USAGE_FLUSH_MS = 60_000;
const USAGE_FLUSH_BYTES = 1024 * 1024;
/** A room nobody connected to for this long is removed. */
export const ROOM_TTL_MS = 90 * 24 * 3600 * 1000;
const CLOCK_SKEW_MS = 5 * 60 * 1000;
const OPEN = 1;
const HEX64 = /^[0-9a-f]{64}$/;
const TOKEN = /^[A-Za-z0-9_-]{43}$/;

export const CLOSE_REPLACED = 4001;
export const CLOSE_NOT_ALLOWED = 4002;
export const CLOSE_TEXT_NOT_ACCEPTED = 4003;
export const CLOSE_ROOM_GONE = 4004;
export const CLOSE_TOO_MANY = 4008;
export const CLOSE_ROOM_RATE = 4009;
export const CLOSE_MONTHLY = 4010;
export const CLOSE_TOO_LARGE = 1009;

export class Room extends DurableObject<RoomEnv> {
  private readonly boot = crypto.randomUUID();
  private readonly bootedAt = Date.now();
  /** Per socket (the desktop, or a phone's slot), in memory only: what a waking object forgets errs towards letting through. */
  private readonly rates = new Map<string, { second: number; count: number }>();
  private bucket = { tokens: ROOM_BURST_BYTES, at: Date.now() };
  private usage?: Usage;
  /** The next slot number to try, read from storage once per start and then kept here, so that two admissions never take one number. */
  private nextSlot?: number;
  private unflushed = 0;
  private flushedAt = Date.now();

  constructor(ctx: DurableObjectState, env: RoomEnv) {
    super(ctx, env);
    // Answered by the runtime: keepalives neither wake the object nor bill duration.
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  /** Called once by the router when the room is registered. */
  async init(room: string, pk: string): Promise<void> {
    if (await this.ctx.storage.get("pk")) throw new Error("room exists");
    const now = Date.now();
    await this.ctx.storage.put({ room, pk, lastUsed: now, lastTs: 0, allow: [], pair: [] });
    await this.ctx.storage.setAlarm(now + ROOM_TTL_MS);
  }

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    const role = url.pathname === "/v1/desktop" ? "desktop" : "phone";
    const pk = await this.ctx.storage.get<string>("pk");
    const room = await this.ctx.storage.get<string>("room");
    if (!pk || !room || room !== request.headers.get("x-kelpie-room")) return plain(404, "no such room\n");
    const now = Date.now();
    const lastUsed = (await this.ctx.storage.get<number>("lastUsed")) ?? 0;
    if (now - lastUsed >= ROOM_TTL_MS) {
      await this.dropRoom();
      return plain(404, "no such room\n");
    }
    if ((await this.loadUsage()).bytes >= MONTHLY_BYTES) return plain(429, "monthly limit\n");
    const entry = request.headers.get("x-entry-colo") ?? "";
    return role === "desktop" ? this.desktop(url, request, room, pk, entry, now) : this.phone(url, request, entry, now);
  }

  private async desktop(url: URL, request: Request, room: string, pk: string, entry: string, now: number): Promise<Response> {
    if (request.headers.has("origin")) return plain(403, "not from a page\n");
    const ts = Number(url.searchParams.get("ts"));
    const sig = url.searchParams.get("sig") ?? "";
    const lastTs = (await this.ctx.storage.get<number>("lastTs")) ?? 0;
    if (!Number.isSafeInteger(ts) || Math.abs(now - ts) > CLOCK_SKEW_MS || ts <= lastTs) return plain(403, "stale signature\n");
    if (!(await verifyDesktop(pk, room, ts, sig))) return plain(403, "bad signature\n");
    // Looked at again: another request with this very signature may have got in while this one was being checked.
    if (ts <= ((await this.ctx.storage.get<number>("lastTs")) ?? 0)) return plain(403, "stale signature\n");
    await this.ctx.storage.put({ lastTs: ts, lastUsed: now });
    await this.ctx.storage.setAlarm(now + ROOM_TTL_MS);

    const [client, server] = Object.values(new WebSocketPair());
    // Last desktop wins, so a half-open old socket cannot hold the room; what it still sends (a late `allow []`) is not heard.
    for (const old of this.ctx.getWebSockets("desktop")) {
      try {
        old.serializeAttachment({ ...attachment(old), ready: false, replaced: true } satisfies Attachment);
        old.close(CLOSE_REPLACED, "replaced");
      } catch {
        /* already closed */
      }
    }
    this.ctx.acceptWebSocket(server, ["desktop"]);
    server.serializeAttachment({ role: "desktop", slot: 0, entry, ready: false } satisfies Attachment);
    return new Response(null, { status: 101, webSocket: client, headers: { "x-kelpie-relay-version": PROTOCOL_VERSION } });
  }

  /** The page origins phones may come from; a room stored before there could be several has one, as `origin`. */
  private async origins(): Promise<string[]> {
    const origins = await this.ctx.storage.get<string[]>("origins");
    if (origins) return origins;
    const origin = await this.ctx.storage.get<string>("origin");
    return origin ? [origin] : [];
  }

  private async phone(url: URL, request: Request, entry: string, now: number): Promise<Response> {
    const origin = request.headers.get("origin") ?? undefined;
    if (origin !== undefined && !(await this.origins()).includes(origin)) return plain(403, "origin\n");
    const token = url.searchParams.get("k");
    const pairToken = url.searchParams.get("p");
    let via: "token" | "pair";
    let cred: string | undefined;
    if (token !== null && TOKEN.test(token)) {
      cred = await tokenSha256(token);
      const allow = (await this.ctx.storage.get<string[]>("allow")) ?? [];
      if (!allow.includes(cred)) return plain(403, "not allowed\n");
      via = "token";
    } else if (pairToken !== null && TOKEN.test(pairToken)) {
      const h = await tokenSha256(pairToken);
      const live = ((await this.ctx.storage.get<PairToken[]>("pair")) ?? []).filter((p) => p.until > now);
      if (!live.some((p) => p.h === h)) return plain(403, "not allowed\n");
      // Single use: spent before the socket is even open.
      await this.ctx.storage.put("pair", live.filter((p) => p.h !== h));
      via = "pair";
      cred = h;
    } else {
      return plain(403, "not allowed\n");
    }
    const stored = this.nextSlot === undefined ? await this.ctx.storage.get<number>("nextSlot") : undefined;
    // Another admission may have read it meanwhile and moved on: its number stands.
    this.nextSlot ??= stored ?? 0;
    // From here to the accept nothing waits, so no other admission runs in between: the count and the slot hold.
    if (via === "token") {
      // One socket per token: a phone that connects again replaces its older socket.
      for (const old of this.phones()) if (attachment(old).cred === cred) old.close(CLOSE_REPLACED, "replaced");
    }
    if (this.phones().length >= MAX_PHONES) return plain(409, "room full\n");
    const slot = this.freeSlot(this.nextSlot);
    if (slot < 0) return plain(409, "room full\n");

    const [client, server] = Object.values(new WebSocketPair());
    this.ctx.acceptWebSocket(server, ["phone", slotTag(slot)]);
    server.serializeAttachment({ role: "phone", slot, entry, via, cred, origin } satisfies Attachment);
    this.nextSlot = (slot + 1) % SLOTS;
    const desktop = this.readyDesktop();
    if (desktop) desktop.send(this.opened(slot, via, cred, origin));
    else server.send(JSON.stringify({ t: "desktop_offline" }));
    await this.ctx.storage.put({ lastUsed: now, nextSlot: this.nextSlot });
    return new Response(null, { status: 101, webSocket: client });
  }

  /** What the desktop hears of a phone's slot: how it came in, the hash it came in with, which its limits go by (the slot is new every
   *  time), and the page origin it came from, if any. */
  private opened(slot: number, via: Attachment["via"], cred: string | undefined, origin: string | undefined): string {
    return JSON.stringify({ t: "open", s: slot, via, k: cred, o: origin });
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    const from = attachment(ws);
    // A desktop that another took the room from has no say in it: not its list, not its kicks, not its frames.
    if (from.role === "desktop" && from.replaced) return;
    if (typeof message === "string") {
      if (!this.withinRate(from.role === "desktop" ? "desktop" : slotTag(from.slot))) return ws.close(CLOSE_TOO_MANY, "too many messages");
      if (from.role === "desktop") return this.control(ws, from, message);
      return ws.close(CLOSE_TEXT_NOT_ACCEPTED, "binary frames only");
    }
    if (from.role === "phone") {
      if (!this.withinRate(slotTag(from.slot))) return ws.close(CLOSE_TOO_MANY, "too many messages");
      if (message.byteLength > MAX_FRAME) return ws.close(CLOSE_TOO_LARGE, "frame too large");
      // With nobody to carry it to, a frame is dropped before it costs the room anything.
      if (!this.readyDesktop()) return;
      if (!this.spend(message.byteLength)) return ws.close(CLOSE_ROOM_RATE, "room over its rate");
      if (!(await this.count(message.byteLength))) return this.closeAll(CLOSE_MONTHLY, "monthly limit");
      return this.toDesktop(from.slot, message);
    }
    if (message.byteLength < SLOT_BYTES || message.byteLength > MAX_FRAME + SLOT_BYTES) return ws.close(CLOSE_TOO_LARGE, "frame too large");
    // The desktop carries every phone's traffic: what one phone is sent too much of closes that phone, never the room.
    const slot = new DataView(message).getUint16(0);
    if (!this.withinRate(`to:${slot}`)) return this.closeSlot(slot, CLOSE_TOO_MANY, "too many messages");
    if (!this.spend(message.byteLength)) return this.closeSlot(slot, CLOSE_ROOM_RATE, "room over its rate");
    if (!(await this.count(message.byteLength))) return this.closeAll(CLOSE_MONTHLY, "monthly limit");
    if (from.ready) this.toPhone(slot, message.slice(SLOT_BYTES));
  }

  async webSocketClose(ws: WebSocket, code: number, reason: string): Promise<void> {
    this.gone(ws, code);
    try {
      ws.close(code >= 3000 && code <= 4999 ? code : 1000, reason.slice(0, 64));
    } catch {
      /* already closed */
    }
  }

  async webSocketError(ws: WebSocket): Promise<void> {
    this.gone(ws, 1006);
  }

  /** The room has not been used for its lifetime: it goes. Set again on each desktop connection, never in a loop. */
  async alarm(): Promise<void> {
    // A desktop that stays connected is using the room, however long ago it signed in.
    if (this.ctx.getWebSockets("desktop").length) await this.ctx.storage.put("lastUsed", Date.now());
    const lastUsed = (await this.ctx.storage.get<number>("lastUsed")) ?? 0;
    if (Date.now() - lastUsed >= ROOM_TTL_MS) await this.dropRoom();
    else await this.ctx.storage.setAlarm(lastUsed + ROOM_TTL_MS);
  }

  /** A socket left: the other side hears of it. The desktop hears the phone's code (a phone that closes the route that lost its race says so). */
  private gone(ws: WebSocket, code: number): void {
    const left = attachment(ws);
    if (left.role === "phone") {
      this.readyDesktop(ws)?.send(JSON.stringify({ t: "close", s: left.slot, c: code }));
    } else if (left.ready && !this.readyDesktop(ws)) {
      for (const phone of this.phones(ws)) phone.send(JSON.stringify({ t: "desktop_offline" }));
    }
  }

  private async control(ws: WebSocket, from: Attachment, text: string): Promise<void> {
    if (text === "info") return ws.send(this.info());
    let msg: Record<string, unknown>;
    try {
      msg = JSON.parse(text) as Record<string, unknown>;
    } catch {
      return ws.close(CLOSE_TEXT_NOT_ACCEPTED, "not a message");
    }
    switch (msg.t) {
      case "allow": {
        const list = msg.h;
        if (!Array.isArray(list) || list.length > MAX_ALLOWED || !list.every((h) => typeof h === "string" && HEX64.test(h))) {
          return ws.close(CLOSE_TEXT_NOT_ACCEPTED, "allow-list");
        }
        await this.ctx.storage.put("allow", list);
        const allowed = new Set(list as string[]);
        for (const phone of this.phones()) {
          const a = attachment(phone);
          if (a.via === "token" && !allowed.has(a.cred ?? "")) phone.close(CLOSE_NOT_ALLOWED, "not allowed");
        }
        if (!from.ready) {
          // Removals made while the desktop was away apply before it hears of any phone.
          ws.serializeAttachment({ ...from, ready: true } satisfies Attachment);
          for (const phone of this.phones()) {
            const a = attachment(phone);
            ws.send(this.opened(a.slot, a.via, a.cred, a.origin));
            phone.send(JSON.stringify({ t: "desktop_online" }));
          }
        }
        return;
      }
      case "pair": {
        const ttl = Number(msg.ttl);
        if (typeof msg.h !== "string" || !HEX64.test(msg.h) || !Number.isInteger(ttl) || ttl < 1 || ttl > MAX_PAIR_TTL_S) {
          return ws.close(CLOSE_TEXT_NOT_ACCEPTED, "pair token");
        }
        const now = Date.now();
        const live = ((await this.ctx.storage.get<PairToken[]>("pair")) ?? []).filter((p) => p.until > now);
        live.push({ h: msg.h, until: now + ttl * 1000 });
        await this.ctx.storage.put("pair", live.slice(-MAX_PAIR_TOKENS));
        return;
      }
      case "origins": {
        const list = msg.o;
        if (!Array.isArray(list) || list.length > MAX_ORIGINS || !list.every(isOrigin)) return ws.close(CLOSE_TEXT_NOT_ACCEPTED, "origins");
        return this.keepOrigins(list as string[]);
      }
      // A desktop from before there could be several says the one.
      case "origin": {
        if (!isOrigin(msg.o)) return ws.close(CLOSE_TEXT_NOT_ACCEPTED, "origin");
        return this.keepOrigins([msg.o]);
      }
      case "kick": {
        const slot = Number(msg.s);
        const code = Number(msg.c);
        const reason = typeof msg.r === "string" ? msg.r.slice(0, 64) : "";
        return this.closeSlot(slot, Number.isInteger(code) && code >= 4000 && code <= 4999 ? code : 1000, reason);
      }
      case "drop":
        return this.dropRoom();
      default:
        return ws.close(CLOSE_TEXT_NOT_ACCEPTED, "unknown message");
    }
  }

  private async keepOrigins(origins: string[]): Promise<void> {
    await this.ctx.storage.put("origins", origins);
    await this.ctx.storage.delete("origin");
  }

  /** Nothing is queued: with no desktop ready the frame is dropped. */
  private toDesktop(slot: number, frame: ArrayBuffer): void {
    const desktop = this.readyDesktop();
    if (!desktop) return;
    const framed = new Uint8Array(frame.byteLength + SLOT_BYTES);
    new DataView(framed.buffer).setUint16(0, slot);
    framed.set(new Uint8Array(frame), SLOT_BYTES);
    desktop.send(framed);
  }

  private toPhone(slot: number, payload: ArrayBuffer): void {
    for (const phone of this.ctx.getWebSockets(slotTag(slot))) {
      try {
        phone.send(payload);
      } catch {
        /* closing: nothing more goes to it */
      }
    }
  }

  private closeSlot(slot: number, code: number, reason: string): void {
    for (const phone of this.ctx.getWebSockets(slotTag(slot))) {
      try {
        phone.close(code, reason);
      } catch {
        /* already closed */
      }
    }
  }

  /** The desktop phones may reach: connected, and done with its allow-list. */
  private readyDesktop(except?: WebSocket): WebSocket | undefined {
    return this.ctx.getWebSockets("desktop").find((ws) => ws !== except && ws.readyState === OPEN && attachment(ws).ready);
  }

  private phones(except?: WebSocket): WebSocket[] {
    return this.ctx.getWebSockets("phone").filter((ws) => ws !== except && ws.readyState === OPEN);
  }

  /** The next slot number from `start` that no socket holds, open or closing: from a counter kept with the room, so numbers come round only
   *  after 65,536. Waits for nothing. */
  private freeSlot(start: number): number {
    for (let i = 0; i < SLOTS; i++) {
      const slot = (start + i) % SLOTS;
      if (this.ctx.getWebSockets(slotTag(slot)).length === 0) return slot;
    }
    return -1;
  }

  /** `key`: the desktop's word, a phone's socket, or what the desktop sends one slot. */
  private withinRate(key: string): boolean {
    const second = Math.floor(Date.now() / 1000);
    const r = this.rates.get(key);
    if (!r || r.second !== second) {
      // Slots come and go: what counted for a second gone by is forgotten.
      if (this.rates.size > 64) for (const [k, v] of this.rates) if (v.second !== second) this.rates.delete(k);
      this.rates.set(key, { second, count: 1 });
      return true;
    }
    r.count += 1;
    return r.count <= MESSAGES_PER_SECOND;
  }

  /** The room's burst budget, refilled as time passes. */
  private spend(bytes: number): boolean {
    const now = Date.now();
    this.bucket.tokens = Math.min(ROOM_BURST_BYTES, this.bucket.tokens + ((now - this.bucket.at) / 1000) * ROOM_BURST_BYTES);
    this.bucket.at = now;
    if (this.bucket.tokens < bytes) return false;
    this.bucket.tokens -= bytes;
    return true;
  }

  private async loadUsage(): Promise<Usage> {
    const month = new Date().toISOString().slice(0, 7);
    this.usage ??= (await this.ctx.storage.get<Usage>("usage")) ?? { month, bytes: 0 };
    if (this.usage.month !== month) this.usage = { month, bytes: 0 };
    return this.usage;
  }

  /** Counts what the room carried; false once it is over the month's cap. Written as messages arrive, in batches, never on a timer. */
  private async count(bytes: number): Promise<boolean> {
    const usage = await this.loadUsage();
    usage.bytes += bytes;
    this.unflushed += bytes;
    const now = Date.now();
    const over = usage.bytes >= MONTHLY_BYTES;
    if (over || this.unflushed >= USAGE_FLUSH_BYTES || now - this.flushedAt >= USAGE_FLUSH_MS) {
      await this.ctx.storage.put("usage", usage);
      this.unflushed = 0;
      this.flushedAt = now;
    }
    return !over;
  }

  private closeAll(code: number, reason: string): void {
    for (const ws of this.ctx.getWebSockets()) {
      try {
        ws.close(code, reason);
      } catch {
        /* already closed */
      }
    }
  }

  /** The room is no more: every socket closes and nothing of it is kept. */
  private async dropRoom(): Promise<void> {
    this.closeAll(CLOSE_ROOM_GONE, "room gone");
    await this.ctx.storage.deleteAlarm();
    await this.ctx.storage.deleteAll();
    this.usage = undefined;
  }

  /** Which start of this object answered: a new `boot` means it was evicted in between. Counts only, nothing a socket carried. */
  private info(): string {
    return JSON.stringify({
      id: this.ctx.id.toString(),
      boot: this.boot,
      uptime_ms: Date.now() - this.bootedAt,
      sockets: this.ctx.getWebSockets().length,
      version: this.env.RELAY_VERSION ?? "",
    });
  }
}

// A script must have a default export to load. Nothing routes here: only the binding reaches rooms.
export default {
  fetch: () => new Response("not found\n", { status: 404 }),
} satisfies ExportedHandler;

function attachment(ws: WebSocket): Attachment {
  return ws.deserializeAttachment() as Attachment;
}

function slotTag(slot: number): string {
  return `slot:${slot}`;
}

function plain(status: number, body: string): Response {
  return new Response(body, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8", "x-content-type-options": "nosniff", "cache-control": "no-store" },
  });
}

/** A page origin a phone may come from: `https://` and a host (with a port, maybe), nothing more. */
function isOrigin(o: unknown): o is string {
  return typeof o === "string" && o.length <= MAX_ORIGIN && ORIGIN.test(o);
}

/** SHA-256 of a base64url token's bytes, hex. */
export async function tokenSha256(token: string): Promise<string> {
  return hex(new Uint8Array(await crypto.subtle.digest("SHA-256", decodeBase64Url(token))));
}

/** What the desktop signs to enter its room. */
export function desktopStatement(room: string, ts: number): string {
  return `kelpie-relay/1 desktop ${room} ${ts}`;
}

export async function verifyDesktop(pk: string, room: string, ts: number, sig: string): Promise<boolean> {
  if (!/^[A-Za-z0-9_-]{86}$/.test(sig)) return false;
  try {
    const key = await crypto.subtle.importKey("raw", decodeBase64Url(pk), { name: "ECDSA", namedCurve: "P-256" }, false, ["verify"]);
    return await crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, key, decodeBase64Url(sig), new TextEncoder().encode(desktopStatement(room, ts)));
  } catch {
    return false;
  }
}
