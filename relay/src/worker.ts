// The relay's front door. A socket reaches a room only with a valid ticket, and the ticket is
// checked here so that forged traffic never costs a Durable Object call. Never serves HTML.

export interface Env {
  ROOM: DurableObjectNamespace;
  TICKET_SECRET: string;
}

// A ticket is `<room id, 32 hex>.<HMAC-SHA256 of the room id, base64url without padding>`.
const TICKET = /^([0-9a-f]{32})\.([A-Za-z0-9_-]{43})$/;
const SOCKET_PATHS = new Set(["/v1/desktop", "/v1/phone"]);

let cachedKey: { secret: string; key: Promise<CryptoKey> } | undefined;

function ticketKey(secret: string): Promise<CryptoKey> {
  if (cachedKey?.secret !== secret) {
    const key = crypto.subtle.importKey(
      "raw",
      new TextEncoder().encode(secret),
      { name: "HMAC", hash: "SHA-256" },
      false,
      ["verify"],
    );
    cachedKey = { secret, key };
  }
  return cachedKey.key;
}

function decodeBase64Url(text: string): Uint8Array {
  const binary = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(binary, (c) => c.charCodeAt(0));
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

function plain(status: number, body: string): Response {
  return new Response(body, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8", "cache-control": "no-store" },
  });
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const { pathname, searchParams } = new URL(request.url);
    if (pathname === "/v1/health") return plain(200, "ok\n");
    if (!SOCKET_PATHS.has(pathname)) return plain(404, "not found\n");
    if (request.headers.get("upgrade")?.toLowerCase() !== "websocket") {
      return plain(426, "websocket only\n");
    }
    if (!env.TICKET_SECRET) return plain(503, "relay not configured\n");

    const room = await checkTicket(searchParams.get("t"), env.TICKET_SECRET);
    if (room === null) return plain(403, "bad ticket\n");

    // No location hint: this first get() is what places a new room near whoever opened it.
    const stub = env.ROOM.get(env.ROOM.idFromName(room));
    const forwarded = new Request(request);
    forwarded.headers.set("x-entry-colo", String(request.cf?.colo ?? ""));
    return stub.fetch(forwarded);
  },
};
