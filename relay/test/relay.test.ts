import { evictDurableObject, listDurableObjectIds, runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { env, exports } from "cloudflare:workers";
import { afterEach, describe, expect, it, vi } from "vitest";
import roomSource from "../src/room.ts?raw";
import workerSource from "../src/worker.ts?raw";
import { encodeBase64Url, hex } from "../src/encoding";
import { desktopStatement, MESSAGES_PER_SECOND, MONTHLY_BYTES, ROOM_TTL_MS, tokenSha256, verifyDesktop, type Room } from "../src/room";
import { ADMISSION_REFUSALS, makeTicket, type Env } from "../src/worker";
import vectors from "./vectors.json";

const relayEnv = env as unknown as Env;
const REGISTRATION_KEY = "test-only-registration-key";
const open: WebSocket[] = [];

interface Registered {
  room: string;
  ticket: string;
  keys: CryptoKeyPair;
}

function self(): Fetcher {
  return (exports as unknown as { default: Fetcher }).default;
}

function randomRoom(): string {
  return hex(crypto.getRandomValues(new Uint8Array(16)));
}

function token(): string {
  return encodeBase64Url(crypto.getRandomValues(new Uint8Array(32)));
}

async function tokenHash(t: string): Promise<string> {
  const bytes = Uint8Array.from(atob(t.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0));
  return hex(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)));
}

async function roomKeys(): Promise<{ keys: CryptoKeyPair; pk: string }> {
  const keys = (await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, ["sign", "verify"])) as CryptoKeyPair;
  const raw = new Uint8Array((await crypto.subtle.exportKey("raw", keys.publicKey)) as ArrayBuffer);
  return { keys, pk: encodeBase64Url(raw) };
}

function registerWith(body: unknown, headers: Record<string, string> = { authorization: `Bearer ${REGISTRATION_KEY}` }): Promise<Response> {
  return self().fetch("https://relay.test/v1/rooms", {
    method: "POST",
    headers: { "content-type": "application/json", "cf-connecting-ip": "203.0.113.7", ...headers },
    body: JSON.stringify(body),
  });
}

async function register(): Promise<Registered> {
  const { keys, pk } = await roomKeys();
  const response = await registerWith({ pk });
  expect(response.status).toBe(201);
  const { room, ticket } = (await response.json()) as { room: string; ticket: string };
  return { room, ticket, keys };
}

async function signature(r: Registered, ts: number): Promise<string> {
  const sig = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, r.keys.privateKey, new TextEncoder().encode(desktopStatement(r.room, ts)));
  return encodeBase64Url(new Uint8Array(sig));
}

/** Each signature a later time than the last, as a desktop's clock gives them. */
let lastTs = 0;

async function desktopQuery(r: Registered, ts = (lastTs = Math.max(Date.now(), lastTs + 1))): Promise<string> {
  return `t=${encodeURIComponent(r.ticket)}&ts=${ts}&sig=${await signature(r, ts)}`;
}

/** Each upgrade from an address of its own unless the test names one, so that refusals counted by address stay with the test that makes them. */
function upgrade(path: string, query: string, headers: Record<string, string> = {}): Promise<Response> {
  const from = `10.${crypto.getRandomValues(new Uint8Array(3)).join(".")}`;
  return self().fetch(`https://relay.test${path}${query ? `?${query}` : ""}`, { headers: { upgrade: "websocket", "cf-connecting-ip": from, ...headers } });
}

interface Socket {
  ws: WebSocket;
  next(): Promise<string | ArrayBuffer>;
  /** The next text message, parsed. */
  said(): Promise<Record<string, unknown>>;
  closed: Promise<number>;
}

/** Opens a socket and collects what arrives, so a test can wait for the next frame. */
function socket(response: Response): Socket {
  expect(response.status).toBe(101);
  const ws = response.webSocket!;
  const inbox: (string | ArrayBuffer)[] = [];
  const waiters: ((value: string | ArrayBuffer) => void)[] = [];
  ws.addEventListener("message", (event) => {
    const data = event.data as string | ArrayBuffer;
    const waiter = waiters.shift();
    if (waiter) waiter(data);
    else inbox.push(data);
  });
  const closed = new Promise<number>((resolve) => ws.addEventListener("close", (event) => resolve((event as CloseEvent).code)));
  (ws as unknown as { binaryType: string }).binaryType = "arraybuffer";
  ws.accept();
  open.push(ws);
  const next = () =>
    new Promise<string | ArrayBuffer>((resolve, reject) => {
      const queued = inbox.shift();
      if (queued !== undefined) return resolve(queued);
      const timer = setTimeout(() => reject(new Error("no frame within 2 s")), 2000);
      waiters.push((value) => {
        clearTimeout(timer);
        resolve(value);
      });
    });
  const said = async () => JSON.parse((await next()) as string) as Record<string, unknown>;
  return { ws, next, said, closed };
}

async function desktop(r: Registered, allowed: string[] = []): Promise<Socket> {
  const d = socket(await upgrade("/v1/desktop", await desktopQuery(r)));
  await allow(d, allowed);
  // The room takes a socket's messages in order: once this is answered, the list is in.
  d.ws.send("info");
  await d.next();
  return d;
}

async function allow(d: Socket, tokens: string[]): Promise<void> {
  d.ws.send(JSON.stringify({ t: "allow", h: await Promise.all(tokens.map(tokenHash)) }));
}

function phone(r: Registered, query: string, headers: Record<string, string> = {}): Promise<Response> {
  return upgrade("/v1/phone", `t=${encodeURIComponent(r.ticket)}&${query}`, headers);
}

function stub(r: Registered) {
  return relayEnv.ROOM.get(relayEnv.ROOM.idFromName(r.room));
}

afterEach(() => {
  for (const ws of open.splice(0)) {
    try {
      ws.close(1000, "test over");
    } catch {
      /* closed by the test */
    }
  }
  vi.restoreAllMocks();
});

describe("the ticket is checked before any Durable Object call", () => {
  it.each([
    ["no ticket", ""],
    ["not a ticket", "t=hello"],
    ["forged signature", `t=${"a".repeat(32)}.${"A".repeat(43)}`],
  ])("refuses %s with no object created", async (_name, query) => {
    const before = (await listDurableObjectIds(relayEnv.ROOM)).length;
    expect((await upgrade("/v1/phone", query)).status).toBe(403);
    expect((await listDurableObjectIds(relayEnv.ROOM)).length).toBe(before);
  });

  it("refuses a ticket signed with another secret, or moved to another room", async () => {
    const before = (await listDurableObjectIds(relayEnv.ROOM)).length;
    expect((await upgrade("/v1/desktop", `t=${await makeTicket(randomRoom(), "some-other-secret")}`)).status).toBe(403);
    const mac = (await makeTicket(randomRoom(), relayEnv.TICKET_SECRET)).split(".")[1];
    expect((await upgrade("/v1/desktop", `t=${randomRoom()}.${mac}`)).status).toBe(403);
    expect((await listDurableObjectIds(relayEnv.ROOM)).length).toBe(before);
  });

  it("never asks the namespace for a stub when the ticket is bad", async () => {
    const lookup = vi.spyOn(relayEnv.ROOM, "idFromName");
    expect((await upgrade("/v1/phone", `t=${await makeTicket(randomRoom(), "wrong")}`)).status).toBe(403);
    expect(lookup).not.toHaveBeenCalled();
  });

  it("finds no room behind a genuine ticket that was never registered", async () => {
    expect((await upgrade("/v1/phone", `t=${await makeTicket(randomRoom(), relayEnv.TICKET_SECRET)}&k=${token()}`)).status).toBe(404);
  });
});

describe("refused admissions", () => {
  it("stop reaching the room once one address has had too many refused", async () => {
    const r = await register();
    const from = { "cf-connecting-ip": "198.51.100.77" };
    for (let i = 0; i < ADMISSION_REFUSALS; i++) expect((await phone(r, `k=${token()}`, from)).status).toBe(403);
    const lookup = vi.spyOn(relayEnv.ROOM, "idFromName");
    expect((await phone(r, `k=${token()}`, from)).status).toBe(429);
    expect(lookup).not.toHaveBeenCalled();
    // Another address is not held up by it.
    expect((await phone(r, `k=${token()}`, { "cf-connecting-ip": "198.51.100.78" })).status).toBe(403);
  });
});

describe("registration", () => {
  it("makes a room for the registration key's holder", async () => {
    const r = await register();
    expect(r.room).toMatch(/^[0-9a-f]{32}$/);
    expect(r.ticket.startsWith(`${r.room}.`)).toBe(true);
    await runInDurableObject(stub(r), async (_room: Room, state) => {
      expect(await state.storage.get("room")).toBe(r.room);
      expect(typeof (await state.storage.get("pk"))).toBe("string");
      expect(await state.storage.getAlarm()).toBeGreaterThan(Date.now());
    });
  });

  it("refuses no key, a wrong key, a page and a bad room key", async () => {
    const { pk } = await roomKeys();
    const before = (await listDurableObjectIds(relayEnv.ROOM)).length;
    expect((await registerWith({ pk }, {})).status).toBe(401);
    expect((await registerWith({ pk }, { authorization: "Bearer not-the-key", "cf-connecting-ip": "198.51.100.1" })).status).toBe(401);
    expect((await registerWith({ pk }, { authorization: `Bearer ${REGISTRATION_KEY}`, origin: "https://evil.example" })).status).toBe(403);
    expect((await registerWith({ pk: "A".repeat(87) })).status).toBe(400);
    expect((await registerWith({ pk: 7 })).status).toBe(400);
    expect((await listDurableObjectIds(relayEnv.ROOM)).length).toBe(before);
  });

  it("stops listening to an address that keeps guessing", async () => {
    const { pk } = await roomKeys();
    const guess = { authorization: "Bearer guess", "cf-connecting-ip": "192.0.2.99" };
    for (let i = 0; i < 10; i++) expect((await registerWith({ pk }, guess)).status).toBe(401);
    expect((await registerWith({ pk }, guess)).status).toBe(429);
    expect((await registerWith({ pk }, { authorization: `Bearer ${REGISTRATION_KEY}`, "cf-connecting-ip": "192.0.2.99" })).status).toBe(429);
  });
});

describe("what the desktop and the relay share", () => {
  it("checks the shared vectors as the daemon makes them", async () => {
    expect(await verifyDesktop(vectors.room_key_public, vectors.room, vectors.ts, vectors.signature)).toBe(true);
    expect(await verifyDesktop(vectors.room_key_public, vectors.room, vectors.ts + 1, vectors.signature)).toBe(false);
    expect(desktopStatement(vectors.room, vectors.ts)).toBe(vectors.statement);
    expect(await tokenSha256(vectors.device_token)).toBe(vectors.device_token_sha256);
    expect(await tokenSha256(vectors.pair_token)).toBe(vectors.pair_token_sha256);
  });
});

describe("the desktop's admission", () => {
  it("takes a fresh signature with the room key", async () => {
    const r = await register();
    socket(await upgrade("/v1/desktop", await desktopQuery(r)));
  });

  it("refuses a signature by another key, an old or reused one, and any Origin", async () => {
    const r = await register();
    const other = { ...r, keys: (await roomKeys()).keys };
    expect((await upgrade("/v1/desktop", await desktopQuery(other))).status).toBe(403);
    expect((await upgrade("/v1/desktop", await desktopQuery(r, Date.now() - 10 * 60_000))).status).toBe(403);
    expect((await upgrade("/v1/desktop", `t=${encodeURIComponent(r.ticket)}&ts=${Date.now()}&sig=${"A".repeat(86)}`)).status).toBe(403);
    expect((await upgrade("/v1/desktop", `t=${encodeURIComponent(r.ticket)}`)).status).toBe(403);
    expect((await upgrade("/v1/desktop", await desktopQuery(r), { origin: "https://evil.example" })).status).toBe(403);
    const query = await desktopQuery(r);
    socket(await upgrade("/v1/desktop", query));
    expect((await upgrade("/v1/desktop", query)).status).toBe(403);
  });

  it("gives the room to the newest desktop", async () => {
    const r = await register();
    const old = await desktop(r);
    await desktop(r);
    expect(await old.closed).toBe(4001);
  });
});

describe("the phone's admission", () => {
  it("lets in a token on the allow-list and nothing else", async () => {
    const r = await register();
    const k = token();
    const d = await desktop(r, [k]);
    expect((await phone(r, "")).status).toBe(403);
    expect((await phone(r, `k=${token()}`)).status).toBe(403);
    expect((await phone(r, "k=short")).status).toBe(403);
    socket(await phone(r, `k=${k}`));
    expect(await d.said()).toEqual({ t: "open", s: 0, via: "token" });
  });

  it("takes only the origin the desktop registered, or none", async () => {
    const r = await register();
    const k = token();
    const d = await desktop(r, [k]);
    expect((await phone(r, `k=${k}`, { origin: "https://desk.tail1234.ts.net:8443" })).status).toBe(403);
    d.ws.send(JSON.stringify({ t: "origin", o: "https://desk.tail1234.ts.net:8443" }));
    await expect.poll(async () => runInDurableObject(stub(r), async (_room: Room, state) => state.storage.get("origin"))).toBe("https://desk.tail1234.ts.net:8443");
    expect((await phone(r, `k=${k}`, { origin: "https://evil.example" })).status).toBe(403);
    socket(await phone(r, `k=${k}`, { origin: "https://desk.tail1234.ts.net:8443" }));
    socket(await phone(r, `k=${k}`));
  });

  it("lets a pairing token in once, and not once it expired", async () => {
    const r = await register();
    const d = await desktop(r);
    const p = token();
    d.ws.send(JSON.stringify({ t: "pair", h: await tokenHash(p), ttl: 120 }));
    await expect.poll(async () => runInDurableObject(stub(r), async (_room: Room, state) => ((await state.storage.get<unknown[]>("pair")) ?? []).length)).toBe(1);
    socket(await phone(r, `p=${p}`));
    expect(await d.said()).toEqual({ t: "open", s: 0, via: "pair" });
    expect((await phone(r, `p=${p}`)).status).toBe(403);

    const late = token();
    await runInDurableObject(stub(r), async (_room: Room, state) => {
      await state.storage.put("pair", [{ h: await tokenHash(late), until: Date.now() - 1 }]);
    });
    expect((await phone(r, `p=${late}`)).status).toBe(403);
  });

  it("closes a phone whose token left the allow-list, and only it", async () => {
    const r = await register();
    const [kept, revoked] = [token(), token()];
    const d = await desktop(r, [kept, revoked]);
    const a = socket(await phone(r, `k=${kept}`));
    const b = socket(await phone(r, `k=${revoked}`));
    await d.said();
    await d.said();
    await allow(d, [kept]);
    expect(await b.closed).toBe(4002);
    a.ws.send("ping");
    expect(await a.next()).toBe("pong");
    expect((await phone(r, `k=${revoked}`)).status).toBe(403);
  });

  it("applies what a returning desktop removed before it hears of any phone", async () => {
    const r = await register();
    const [kept, revoked] = [token(), token()];
    const first = await desktop(r, [kept, revoked]);
    first.ws.close(1000, "away");
    await first.closed;
    const a = socket(await phone(r, `k=${kept}`));
    const b = socket(await phone(r, `k=${revoked}`));
    expect(await a.said()).toEqual({ t: "desktop_offline" });
    expect(await b.said()).toEqual({ t: "desktop_offline" });

    const back = socket(await upgrade("/v1/desktop", await desktopQuery(r)));
    // Not ready: a phone's frame goes nowhere.
    a.ws.send(new Uint8Array([1, 2, 3]));
    await allow(back, [kept]);
    expect(await b.closed).toBe(4002);
    expect(await back.said()).toEqual({ t: "open", s: 0, via: "token" });
    expect(await a.said()).toEqual({ t: "desktop_online" });
    a.ws.send(new Uint8Array([9]));
    // The phone that was closed is reported gone too; what comes next is a's frame, and not the one sent before the list.
    expect(await back.said()).toEqual({ t: "close", s: 1 });
    expect(Array.from(new Uint8Array((await back.next()) as ArrayBuffer))).toEqual([0, 0, 9]);
  });

  it("gives a phone's new socket a slot of its own while the old one closes, so nothing late of the old reaches the new", async () => {
    const r = await register();
    const k = token();
    const d = await desktop(r, [k]);
    const old = socket(await phone(r, `k=${k}`));
    const first = (await d.said()).s as number;
    const fresh = socket(await phone(r, `k=${k}`));
    const opened = await d.said();
    expect(opened.t).toBe("open");
    const second = opened.s as number;
    expect(second).not.toBe(first);
    // At once, before the old socket is gone: the new one gets what is sent to its slot, and the old nothing.
    d.ws.send(Uint8Array.of(second >> 8, second & 255, 5, 6));
    expect(Array.from(new Uint8Array((await fresh.next()) as ArrayBuffer))).toEqual([5, 6]);
    expect(await old.closed).toBe(4001);
    expect(await d.said()).toEqual({ t: "close", s: first });
    // A kick of the old slot, as a late one would be, leaves the new socket alone.
    d.ws.send(JSON.stringify({ t: "kick", s: first, c: 4409 }));
    d.ws.send(Uint8Array.of(second >> 8, second & 255, 7));
    expect(Array.from(new Uint8Array((await fresh.next()) as ArrayBuffer))).toEqual([7]);
  });

  it("replaces a phone's older socket with the same token", async () => {
    const r = await register();
    const k = token();
    await desktop(r, [k]);
    const old = socket(await phone(r, `k=${k}`));
    socket(await phone(r, `k=${k}`));
    expect(await old.closed).toBe(4001);
  });

  it("refuses a ninth phone and lets one in once a place is free", async () => {
    const r = await register();
    const tokens = Array.from({ length: 9 }, token);
    await desktop(r, tokens);
    const phones = [];
    for (const k of tokens.slice(0, 8)) phones.push(socket(await phone(r, `k=${k}`)));
    expect((await phone(r, `k=${tokens[8]}`)).status).toBe(409);
    phones[3].ws.close(1000, "done");
    await expect.poll(async () => (await phone(r, `k=${tokens[8]}`)).status).toBe(101);
  });
});

describe("what the router serves", () => {
  it("answers health without a Durable Object", async () => {
    const before = (await listDurableObjectIds(relayEnv.ROOM)).length;
    const response = await self().fetch("https://relay.test/v1/health");
    expect(response.status).toBe(200);
    expect(await response.text()).toBe("ok\n");
    expect((await listDurableObjectIds(relayEnv.ROOM)).length).toBe(before);
  });

  it("serves no HTML on any path", async () => {
    for (const [path, status, method] of [
      ["/", 404, "GET"],
      ["/index.html", 404, "GET"],
      ["/v1/phone", 426, "GET"],
      ["/v1/desktop", 426, "GET"],
      ["/v1/rooms", 405, "GET"],
    ] as const) {
      const response = await self().fetch(`https://relay.test${path}`, { method });
      expect(response.status).toBe(status);
      expect(response.headers.get("content-type")).toMatch(/^text\/plain/);
      expect(await response.text()).not.toMatch(/</);
    }
  });
});

describe("forwarding", () => {
  it("carries a phone's bytes to the desktop with its slot, and back to that phone only", async () => {
    const r = await register();
    const [k1, k2] = [token(), token()];
    const d = await desktop(r, [k1, k2]);
    const first = socket(await phone(r, `k=${k1}`));
    const second = socket(await phone(r, `k=${k2}`));
    await d.said();
    await d.said();
    const payload = crypto.getRandomValues(new Uint8Array(1024));

    second.ws.send(payload);
    const framed = new Uint8Array((await d.next()) as ArrayBuffer);
    expect(Array.from(framed.subarray(0, 2))).toEqual([0, 1]);
    expect(hex(framed.subarray(2))).toBe(hex(payload));

    d.ws.send(framed);
    expect(hex(new Uint8Array((await second.next()) as ArrayBuffer))).toBe(hex(payload));

    first.ws.send("ping");
    expect(await first.next()).toBe("pong");
  });

  it("tells each side when the other goes", async () => {
    const r = await register();
    const k = token();
    const d = await desktop(r, [k]);
    const p = socket(await phone(r, `k=${k}`));
    expect(await d.said()).toEqual({ t: "open", s: 0, via: "token" });
    p.ws.close(1000, "bye");
    expect(await d.said()).toEqual({ t: "close", s: 0 });

    const again = socket(await phone(r, `k=${k}`));
    await d.said();
    d.ws.close(1000, "bye");
    expect(await again.said()).toEqual({ t: "desktop_offline" });
  });

  it("lets the desktop close one phone with its own code", async () => {
    const r = await register();
    const k = token();
    const d = await desktop(r, [k]);
    const p = socket(await phone(r, `k=${k}`));
    await d.said();
    d.ws.send(JSON.stringify({ t: "kick", s: 0, c: 4401, r: "not paired" }));
    expect(await p.closed).toBe(4401);
  });

  it("closes a frame over 64 KiB, unknown text from a phone, and garbage from the desktop", async () => {
    const r = await register();
    const [k1, k2] = [token(), token()];
    const d = await desktop(r, [k1, k2]);
    const big = socket(await phone(r, `k=${k1}`));
    big.ws.send(new Uint8Array(64 * 1024 + 1));
    expect(await big.closed).toBe(1009);
    const chatty = socket(await phone(r, `k=${k2}`));
    chatty.ws.send("hello");
    expect(await chatty.closed).toBe(4003);
    d.ws.send("{not json");
    expect(await d.closed).toBe(4003);
  });

  it("closes a socket that sends too many messages in a second", async () => {
    const r = await register();
    const k = token();
    await desktop(r, [k]);
    const p = socket(await phone(r, `k=${k}`));
    // This machine may not send 200 messages in a second, so the second's count is set as if it had; not near the second's end.
    while (Date.now() % 1000 > 700) await new Promise((resolve) => setTimeout(resolve, 50));
    await runInDurableObject(stub(r), (instance: Room) => {
      (instance as unknown as { rates: Map<string, { second: number; count: number }> }).rates.set("slot:0", { second: Math.floor(Date.now() / 1000), count: MESSAGES_PER_SECOND });
    });
    p.ws.send(new Uint8Array([1]));
    expect(await p.closed).toBe(4008);
  });

  it("closes the phone the desktop sends too much, never the desktop and the room with it", async () => {
    const r = await register();
    const [k1, k2] = [token(), token()];
    const d = await desktop(r, [k1, k2]);
    const busy = socket(await phone(r, `k=${k1}`));
    const other = socket(await phone(r, `k=${k2}`));
    const [a, b] = [(await d.said()).s as number, (await d.said()).s as number];
    while (Date.now() % 1000 > 700) await new Promise((resolve) => setTimeout(resolve, 50));
    await runInDurableObject(stub(r), (instance: Room) => {
      (instance as unknown as { rates: Map<string, { second: number; count: number }> }).rates.set(`to:${a}`, { second: Math.floor(Date.now() / 1000), count: MESSAGES_PER_SECOND });
    });
    d.ws.send(Uint8Array.of(a >> 8, a & 255, 1));
    expect(await busy.closed).toBe(4008);
    d.ws.send(Uint8Array.of(b >> 8, b & 255, 2));
    expect(Array.from(new Uint8Array((await other.next()) as ArrayBuffer))).toEqual([2]);
    await runInDurableObject(stub(r), (instance: Room) => {
      (instance as unknown as { bucket: { tokens: number; at: number } }).bucket = { tokens: 1000, at: Date.now() + 10_000 };
    });
    d.ws.send(new Uint8Array(64 * 1024).fill(0).map((_, i) => (i === 0 ? b >> 8 : i === 1 ? b & 255 : 0)));
    expect(await other.closed).toBe(4009);
    // The desktop is still in the room: it hears both phones go, then answers.
    d.ws.send("info");
    const said = [await d.said(), await d.said(), await d.said()];
    expect(said.filter((m) => m.t === "close").map((m) => m.s).sort()).toEqual([a, b].sort());
    expect(said.some((m) => "sockets" in m)).toBe(true);
  });

  it("closes a socket that pushes the room past its burst", async () => {
    const r = await register();
    const k = token();
    await desktop(r, [k]);
    const p = socket(await phone(r, `k=${k}`));
    // The burst is spent; it refills at 4 MB a second, far more than this test's next second could use.
    await runInDurableObject(stub(r), (instance: Room) => {
      (instance as unknown as { bucket: { tokens: number; at: number } }).bucket = { tokens: 1000, at: Date.now() + 10_000 };
    });
    p.ws.send(new Uint8Array(64 * 1024));
    expect(await p.closed).toBe(4009);
  });

  it("stops a room at its monthly cap, and keeps counting across evictions", async () => {
    const r = await register();
    const k = token();
    await desktop(r, [k]);
    const p = socket(await phone(r, `k=${k}`));
    await runInDurableObject(stub(r), async (_room: Room, state) => {
      await state.storage.put("usage", { month: new Date().toISOString().slice(0, 7), bytes: MONTHLY_BYTES - 10 });
    });
    await evictDurableObject(stub(r));
    p.ws.send(new Uint8Array(100));
    expect(await p.closed).toBe(4010);
    expect((await phone(r, `k=${k}`)).status).toBe(429);
  });
});

describe("the room's life", () => {
  it("is dropped by its desktop, with every socket and everything stored", async () => {
    const r = await register();
    const k = token();
    const d = await desktop(r, [k]);
    const p = socket(await phone(r, `k=${k}`));
    d.ws.send(JSON.stringify({ t: "drop" }));
    expect(await p.closed).toBe(4004);
    expect(await d.closed).toBe(4004);
    await runInDurableObject(stub(r), async (_room: Room, state) => {
      expect(await state.storage.list()).toHaveProperty("size", 0);
      expect(await state.storage.getAlarm()).toBeNull();
    });
    expect((await phone(r, `k=${k}`)).status).toBe(404);
  });

  it("expires when unused for its lifetime, and the alarm does not come back", async () => {
    const r = await register();
    await runInDurableObject(stub(r), async (_room: Room, state) => {
      await state.storage.put("lastUsed", Date.now() - ROOM_TTL_MS - 1);
    });
    expect(await runDurableObjectAlarm(stub(r))).toBe(true);
    await runInDurableObject(stub(r), async (_room: Room, state) => {
      expect(await state.storage.list()).toHaveProperty("size", 0);
      expect(await state.storage.getAlarm()).toBeNull();
    });
    expect((await upgrade("/v1/desktop", await desktopQuery(r))).status).toBe(404);
  });

  it("is kept while its desktop stays connected, however long ago it signed in", async () => {
    const r = await register();
    await desktop(r);
    await runInDurableObject(stub(r), async (_room: Room, state) => {
      await state.storage.put("lastUsed", Date.now() - ROOM_TTL_MS - 1);
    });
    expect(await runDurableObjectAlarm(stub(r))).toBe(true);
    await runInDurableObject(stub(r), async (_room: Room, state) => {
      expect(await state.storage.get("pk")).toBeTypeOf("string");
    });
  });

  it("is kept when it was used since", async () => {
    const r = await register();
    await desktop(r);
    expect(await runDurableObjectAlarm(stub(r))).toBe(true);
    await runInDurableObject(stub(r), async (_room: Room, state) => {
      expect(await state.storage.get("pk")).toBeTypeOf("string");
      expect(await state.storage.getAlarm()).toBeGreaterThan(Date.now() + ROOM_TTL_MS - 60_000);
    });
  });
});

describe("hibernation", () => {
  it("answers keepalives without running the object's message handler", async () => {
    const r = await register();
    const k = token();
    await desktop(r, [k]);
    const p = socket(await phone(r, `k=${k}`));
    const handled = await runInDurableObject(stub(r), (instance: Room) => vi.spyOn(instance, "webSocketMessage"));

    p.ws.send("ping");
    expect(await p.next()).toBe("pong");

    expect(handled).not.toHaveBeenCalled();
    await runInDurableObject(stub(r), (_instance: Room, state) => {
      const pair = state.getWebSocketAutoResponse();
      expect(pair?.request).toBe("ping");
      expect(pair?.response).toBe("pong");
    });
  });

  it("restarts the object when evicted, with sockets, slots, readiness and forwarding intact", async () => {
    const r = await register();
    const k = token();
    const d = await desktop(r, [k]);
    const p = socket(await phone(r, `k=${k}`));
    await d.said();
    d.ws.send("info");
    const before = JSON.parse((await d.next()) as string);
    expect(before).toMatchObject({ sockets: 2, version: "test" });

    await evictDurableObject(stub(r));

    p.ws.send("ping");
    expect(await p.next()).toBe("pong");
    p.ws.send(new Uint8Array([7, 7, 7]));
    expect(Array.from(new Uint8Array((await d.next()) as ArrayBuffer))).toEqual([0, 0, 7, 7, 7]);
    d.ws.send("info");
    const after = JSON.parse((await d.next()) as string);
    expect(after.boot).not.toBe(before.boot);
    expect(after.sockets).toBe(2);
  });

  it("keeps timers, plain accept and looping alarms out of the source", () => {
    for (const source of [roomSource, workerSource]) {
      const code = source.replace(/\/\*[\s\S]*?\*\/|\/\/.*$/gm, "");
      expect(code).not.toMatch(/\bsetTimeout\b|\bsetInterval\b/);
      expect(code).not.toMatch(/\.accept\(\s*\)/);
      expect(code).not.toMatch(/\.addEventListener\b/);
      expect(code).not.toMatch(/console\./);
    }
    expect(workerSource).not.toMatch(/setAlarm/);
    // The one alarm is the room's expiry: set when the room is made or used, and by the alarm only to a later expiry.
    expect(roomSource.match(/setAlarm\(/g)).toHaveLength(3);
    expect(roomSource).toMatch(/setAlarm\(lastUsed \+ ROOM_TTL_MS\)/);
    expect(roomSource).toMatch(/acceptWebSocket\(/);
    expect(roomSource).toMatch(/setWebSocketAutoResponse\(/);
  });
});
