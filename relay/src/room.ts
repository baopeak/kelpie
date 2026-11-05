// One Durable Object per room. It forwards binary frames it cannot read between one desktop and up to eight phones, and
// stores only what admits them: the room's public key, the hashes of the phones' tokens, the hashes of live pairing tokens,
// the page origin phones may come from, and how much the room has carried this month. Everything a socket needs to survive
// hibernation lives in its attachment, and all waiting is left to the runtime, so that an idle room costs nothing.
//
// - **Desktop**: `/v1/desktop?t=&ts=&sig=`, `sig` being an ECDSA P-256 signature with the room key over
//   `kelpie-relay/1 desktop <room> <ts>`; `ts` is the desktop's clock in ms, within 5 minutes of ours and above the last one
//   used, so a signature cannot be used twice. No `Origin`. The newest desktop wins.
// - **Phone**: `/v1/phone?t=&k=<token>` with a token whose SHA-256 the desktop put on the allow-list, or `&p=<pair token>`
//   once. An `Origin`, when there is one, must be the one the desktop registered.
// - **Frames**: binary only, at most 64 KiB. A phone's frame reaches the desktop as `[slot, u16 big-endian] + frame`; the
//   desktop's goes to the slot it names. Nothing is queued: with no desktop ready a phone's frame is dropped. A slot is a number
//   for one phone socket, never given to another while any socket still holds it, so a late close, frame or kick of an old socket
//   cannot reach a new one. The desktop's frames are limited per slot: one phone's traffic closes that phone, not the room.
// - **The desktop's word** (JSON text): `allow` (the whole allow-list; phones whose token left it are closed, and the desktop
//   hears of phones only after it), `pair`, `origin`, `kick`, `drop`. It hears `open` and `close` for each phone slot.
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
  /** Phone: how it was let in, and the hash of its token, so that a revoke can close it. */
  via?: "token" | "pair";
  cred?: string;
  /** Desktop: it has sent its allow-list on this socket, so phones may reach it. */
  ready?: boolean;
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
/** More than this many messages in a second from one socket closes it. */
export const MESSAGES_PER_SECOND = 200;
/** What the room carries at most, as a burst and then each second. */
export const ROOM_BURST_BYTES = 4 * 1024 * 1024;
/** A soft cap on what a room carries in a calendar month (UTC). */
export const MONTHLY_BYTES = 2 * 1024 * 1024 * 1024;
const USAGE_FLUSH_MS = 5000;
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
    // Last desktop wins, so a half-open old socket cannot hold the room.
    for (const old of this.ctx.getWebSockets("desktop")) old.close(CLOSE_REPLACED, "replaced");
    this.ctx.acceptWebSocket(server, ["desktop"]);
    server.serializeAttachment({ role: "desktop", slot: 0, entry, ready: false } satisfies Attachment);
    return new Response(null, { status: 101, webSocket: client });
  }

  private async phone(url: URL, request: Request, entry: string, now: number): Promise<Response> {
    const origin = request.headers.get("origin");
    if (origin !== null && origin !== (await this.ctx.storage.get<string>("origin"))) return plain(403, "origin\n");
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
    } else {
      return plain(403, "not allowed\n");
    }
    if (cred !== undefined) {
      // One socket per token: a phone that connects again replaces its older socket.
      for (const old of this.phones()) if (attachment(old).cred === cred) old.close(CLOSE_REPLACED, "replaced");
    }
    if (this.phones().length >= MAX_PHONES) return plain(409, "room full\n");
    const slot = await this.newSlot();
    if (slot < 0) return plain(409, "room full\n");

    const [client, server] = Object.values(new WebSocketPair());
    this.ctx.acceptWebSocket(server, ["phone", slotTag(slot)]);
    server.serializeAttachment({ role: "phone", slot, entry, via, cred } satisfies Attachment);
    await this.ctx.storage.put({ lastUsed: now, nextSlot: (slot + 1) % SLOTS });
    const desktop = this.readyDesktop();
    if (desktop) desktop.send(JSON.stringify({ t: "open", s: slot, via }));
    else server.send(JSON.stringify({ t: "desktop_offline" }));
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    const from = attachment(ws);
    if (typeof message === "string") {
      if (!this.withinRate(from.role === "desktop" ? "desktop" : slotTag(from.slot))) return ws.close(CLOSE_TOO_MANY, "too many messages");
      if (from.role === "desktop") return this.control(ws, from, message);
      return ws.close(CLOSE_TEXT_NOT_ACCEPTED, "binary frames only");
    }
    if (from.role === "phone") {
      if (!this.withinRate(slotTag(from.slot))) return ws.close(CLOSE_TOO_MANY, "too many messages");
      if (message.byteLength > MAX_FRAME) return ws.close(CLOSE_TOO_LARGE, "frame too large");
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
    this.gone(ws);
    try {
      ws.close(code >= 3000 && code <= 4999 ? code : 1000, reason.slice(0, 64));
    } catch {
      /* already closed */
    }
  }

  async webSocketError(ws: WebSocket): Promise<void> {
    this.gone(ws);
  }

  /** The room has not been used for its lifetime: it goes. Set again on each desktop connection, never in a loop. */
  async alarm(): Promise<void> {
    // A desktop that stays connected is using the room, however long ago it signed in.
    if (this.ctx.getWebSockets("desktop").length) await this.ctx.storage.put("lastUsed", Date.now());
    const lastUsed = (await this.ctx.storage.get<number>("lastUsed")) ?? 0;
    if (Date.now() - lastUsed >= ROOM_TTL_MS) await this.dropRoom();
    else await this.ctx.storage.setAlarm(lastUsed + ROOM_TTL_MS);
  }

  /** A socket left: the other side hears of it. */
  private gone(ws: WebSocket): void {
    const left = attachment(ws);
    if (left.role === "phone") {
      this.readyDesktop(ws)?.send(JSON.stringify({ t: "close", s: left.slot }));
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
            ws.send(JSON.stringify({ t: "open", s: a.slot, via: a.via }));
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
      case "origin": {
        const o = msg.o;
        if (typeof o !== "string" || o.length > MAX_ORIGIN || !/^https:\/\/[^/?#@\s\\]+$/.test(o)) {
          return ws.close(CLOSE_TEXT_NOT_ACCEPTED, "origin");
        }
        await this.ctx.storage.put("origin", o);
        return;
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

  /** The next slot number no socket holds, open or closing: from a counter kept with the room, so numbers come round only after 65,536. */
  private async newSlot(): Promise<number> {
    const start = (await this.ctx.storage.get<number>("nextSlot")) ?? 0;
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
  return new Response(body, { status, headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" } });
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
