// One Durable Object per room. It forwards binary frames it cannot read between one desktop and
// up to eight phones, and stores nothing. Everything it needs to survive hibernation lives in
// the socket attachment, and all waiting is left to the runtime so that idle rooms cost nothing.

import { DurableObject } from "cloudflare:workers";

interface RoomEnv {
  RELAY_VERSION?: string;
}

interface Attachment {
  role: "desktop" | "phone";
  slot: number;
  entry: string;
}

const MAX_PHONES = 8;
const SLOT_BYTES = 2;
const CLOSE_REPLACED = 4001;
const CLOSE_TEXT_NOT_ACCEPTED = 4003;

export class Room extends DurableObject<RoomEnv> {
  private readonly boot = crypto.randomUUID();
  private readonly bootedAt = Date.now();
  private colo?: Promise<string>;

  constructor(ctx: DurableObjectState, env: RoomEnv) {
    super(ctx, env);
    // Answered by the runtime: keepalives neither wake the object nor bill duration.
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair("ping", "pong"));
  }

  async fetch(request: Request): Promise<Response> {
    const role = new URL(request.url).pathname === "/v1/desktop" ? "desktop" : "phone";
    const slot = role === "phone" ? this.freeSlot() : 0;
    if (slot < 0) return new Response("room full\n", { status: 409 });

    const [client, server] = Object.values(new WebSocketPair());
    const attachment: Attachment = { role, slot, entry: request.headers.get("x-entry-colo") ?? "" };
    if (role === "desktop") {
      // Last desktop wins, so a half-open old socket cannot hold the room.
      for (const old of this.ctx.getWebSockets("desktop")) old.close(CLOSE_REPLACED, "replaced");
    }
    this.ctx.acceptWebSocket(server, role === "desktop" ? ["desktop"] : ["phone", slotTag(slot)]);
    server.serializeAttachment(attachment);
    return new Response(null, { status: 101, webSocket: client });
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    if (typeof message === "string") {
      if (message === "info") ws.send(this.info());
      else if (message === "colo") ws.send(await this.where(ws));
      else ws.close(CLOSE_TEXT_NOT_ACCEPTED, "binary frames only");
      return;
    }
    const from = ws.deserializeAttachment() as Attachment;
    if (from.role === "phone") this.toDesktop(from.slot, message);
    else this.toPhone(message);
  }

  /** Nothing is queued: with no desktop attached the frame is dropped. */
  private toDesktop(slot: number, frame: ArrayBuffer): void {
    const framed = new Uint8Array(frame.byteLength + SLOT_BYTES);
    new DataView(framed.buffer).setUint16(0, slot);
    framed.set(new Uint8Array(frame), SLOT_BYTES);
    for (const desktop of this.ctx.getWebSockets("desktop")) desktop.send(framed);
  }

  private toPhone(frame: ArrayBuffer): void {
    if (frame.byteLength < SLOT_BYTES) return;
    const slot = new DataView(frame).getUint16(0);
    const payload = frame.slice(SLOT_BYTES);
    for (const phone of this.ctx.getWebSockets(slotTag(slot))) phone.send(payload);
  }

  private freeSlot(): number {
    const used = new Set(
      this.ctx.getWebSockets("phone").map((ws) => (ws.deserializeAttachment() as Attachment).slot),
    );
    for (let slot = 0; slot < MAX_PHONES; slot++) if (!used.has(slot)) return slot;
    return -1;
  }

  /** Which start of this object answered: a new `boot` means it was evicted in between. */
  private info(): string {
    return JSON.stringify({
      boot: this.boot,
      uptime_ms: Date.now() - this.bootedAt,
      sockets: this.ctx.getWebSockets().length,
      version: this.env.RELAY_VERSION ?? "",
    });
  }

  /** The object's own colo, and the entry colo this socket came in through. */
  private async where(ws: WebSocket): Promise<string> {
    return JSON.stringify({
      colo: await (this.colo ??= ownColo()),
      entry: (ws.deserializeAttachment() as Attachment).entry,
    });
  }
}

// A script must have a default export to load. Nothing routes here: only the binding reaches rooms.
export default {
  fetch: () => new Response("not found\n", { status: 404 }),
} satisfies ExportedHandler;

function slotTag(slot: number): string {
  return `slot:${slot}`;
}

async function ownColo(): Promise<string> {
  try {
    const trace = await (await fetch("https://www.cloudflare.com/cdn-cgi/trace")).text();
    return /^colo=(\w+)$/m.exec(trace)?.[1] ?? "unknown";
  } catch {
    return "unknown";
  }
}
