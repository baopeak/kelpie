import { evictDurableObject, listDurableObjectIds, runDurableObjectAlarm, runInDurableObject } from "cloudflare:test";
import { env, exports } from "cloudflare:workers";
import { afterEach, describe, expect, it, vi } from "vitest";
import registrarSource from "../src/registrar.ts?raw";
import roomSource from "../src/room.ts?raw";
import workerSource from "../src/worker.ts?raw";
import { encodeBase64Url, hex } from "../src/encoding";
import { TOTAL, utcDay, type Registrar } from "../src/registrar";
import {
  desktopStatement,
  MESSAGES_PER_SECOND,
  MONTHLY_BYTES,
  PROTOCOL_VERSION,
  ROOM_BURST_BYTES,
  ROOM_TTL_MS,
  tokenSha256,
  USAGE_FLUSH_MS,
  verifyDesktop,
  type Room,
} from "../src/room";
import worker, { ADMISSION_REFUSALS, makeTicket, network, ROOMS_FULL, ROOMS_NETWORK, ROOMS_RATE, Strikes, TOO_MANY_REFUSED, type Env } from "../src/worker";
import vectors from "./vectors.json";
import { cleanServers, coturnMint, revokeStatement, TURN_FULL, TURN_MINTS_PER_HOUR, TURN_NETWORK, TURN_NO_PHONES, TURN_NOT_CONFIGURED, TURN_NOT_OFFERED, TURN_ROOM_LIMIT, TURN_TTL_S, TURN_UPSTREAM, turnMode, turnStatement } from "../src/turn";

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
  /** The reason the socket was closed with. */
  reason: Promise<string>;
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
  const reason = new Promise<string>((resolve) => ws.addEventListener("close", (event) => resolve((event as CloseEvent).reason)));
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
  return { ws, next, said, closed, reason };
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

/** The relay as it is configured with registration open and no key. */
function openEnv(extra: Partial<Env> = {}): Env {
  return { ...relayEnv, REGISTRATION_KEY: undefined, OPEN_REGISTRATION: "1", ...extra };
}

/** A network of its own, so that one test's rooms never count against another's. */
function someNetwork(): string {
  return `100.${crypto.getRandomValues(new Uint8Array(3)).join(".")}`;
}

/** Asks the router to make a room, as a relay configured with `relay` would. */
async function registerIn(relay: Env, from: string, headers: Record<string, string> = {}): Promise<Response> {
  const { pk } = await roomKeys();
  const request = new Request("https://relay.test/v1/rooms", {
    method: "POST",
    headers: { "content-type": "application/json", "cf-connecting-ip": from, ...headers },
    body: JSON.stringify({ pk }),
  });
  return worker.fetch(request, relay);
}

function registrar(name = "registrar") {
  return relayEnv.REGISTRAR.get(relayEnv.REGISTRAR.idFromName(name));
}

async function registrarRows(name = "registrar"): Promise<{ day: string; key: string; count: number }[]> {
  return runInDurableObject(registrar(name), (_r: Registrar, state) =>
    state.storage.sql.exec<{ day: string; key: string; count: number }>("SELECT day, key, count FROM rooms").toArray(),
  );
}

async function todaysTotal(): Promise<number> {
  return (await registrarRows()).find((row) => row.day === utcDay(Date.now()) && row.key === TOTAL)?.count ?? 0;
}

/** A fake of the Rate Limiting binding, which the test pool does not give. */
function limiter(success: boolean) {
  return { limit: vi.fn(async (_options: RateLimitOptions) => ({ success })) };
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

  it("count phones and the desktop apart, and say that the refusal is the router's", async () => {
    const r = await register();
    const from = { "cf-connecting-ip": "198.51.100.90" };
    for (let i = 0; i < ADMISSION_REFUSALS; i++) expect((await phone(r, `k=${token()}`, from)).status).toBe(403);
    const refused = await phone(r, `k=${token()}`, from);
    expect(refused.status).toBe(429);
    expect(refused.headers.get("x-kelpie-reason")).toBe(TOO_MANY_REFUSED);
    // A removed phone retrying from the desktop's own network leaves the desktop its room.
    socket(await upgrade("/v1/desktop", await desktopQuery(r), from));
  });

  it("count an IPv6 network as one, whichever of its addresses is used", async () => {
    const r = await register();
    for (let i = 0; i < ADMISSION_REFUSALS; i++) {
      expect((await phone(r, `k=${token()}`, { "cf-connecting-ip": `2001:db8:5:6:${(i + 1).toString(16)}::1` })).status).toBe(403);
    }
    expect((await phone(r, `k=${token()}`, { "cf-connecting-ip": "2001:0db8:0005:0006:ffff:ffff:ffff:ffff" })).status).toBe(429);
    expect((await phone(r, `k=${token()}`, { "cf-connecting-ip": "2001:db8:5:7::1" })).status).toBe(403);
  });

  it("know an address's network", () => {
    expect(network("203.0.113.9")).toBe("203.0.113.9");
    expect(network("2001:db8:5:6:a:b:c:d")).toBe("2001:db8:5:6::/64");
    expect(network("2001:DB8:0005:6::1")).toBe("2001:db8:5:6::/64");
    expect(network("2001:db8::1")).toBe("2001:db8:0:0::/64");
    expect(network("::ffff:192.0.2.1")).toBe("192.0.2.1");
    expect(network("not an address")).toBe("not an address");
    expect(network("1::2::3")).toBe("1::2::3");
  });

  it("let go of what expired, then of the oldest, never of every count at once", () => {
    const strikes = new Strikes(1, 1000, 3);
    strikes.note("a", 0);
    strikes.note("b", 10);
    strikes.note("c", 20);
    strikes.note("d", 30);
    expect(strikes.size).toBe(3);
    expect(strikes.out("a", 40)).toBe(false);
    expect(["b", "c", "d"].every((k) => strikes.out(k, 40))).toBe(true);
    // Once b and c expired, a newcomer takes their place and d is kept.
    strikes.note("e", 1025);
    expect(strikes.size).toBe(2);
    expect(strikes.out("d", 1025)).toBe(true);
    expect(strikes.out("e", 1025)).toBe(true);
    expect(strikes.out("b", 1025) || strikes.out("c", 1025)).toBe(false);
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

describe("registration modes", () => {
  it("makes a room without a key when registration is open, and pays no heed to an Authorization", async () => {
    const made = await registerIn(openEnv(), someNetwork());
    expect(made.status).toBe(201);
    const { room, ticket } = (await made.json()) as { room: string; ticket: string };
    expect(ticket.startsWith(`${room}.`)).toBe(true);
    expect((await registerIn(openEnv(), someNetwork(), { authorization: "Bearer anything" })).status).toBe(201);
  });

  it("asks for the key whenever one is set, open or not", async () => {
    const keyed = { ...relayEnv, OPEN_REGISTRATION: "1" };
    expect((await registerIn(keyed, someNetwork())).status).toBe(401);
    expect((await registerIn(keyed, someNetwork(), { authorization: `Bearer ${REGISTRATION_KEY}` })).status).toBe(201);
  });

  it("makes no room when neither a key nor open registration is set", async () => {
    const before = (await listDurableObjectIds(relayEnv.ROOM)).length;
    for (const open of [undefined, "0", "true"]) {
      const closed = await registerIn(openEnv({ OPEN_REGISTRATION: open }), someNetwork());
      expect(closed.status).toBe(503);
      expect(await closed.text()).toBe("relay not configured\n");
    }
    expect((await listDurableObjectIds(relayEnv.ROOM)).length).toBe(before);
  });

  it("refuses a page in every mode", async () => {
    const page = { origin: "https://evil.example", authorization: `Bearer ${REGISTRATION_KEY}` };
    for (const relay of [relayEnv, openEnv(), openEnv({ OPEN_REGISTRATION: undefined })]) {
      expect((await registerIn(relay, someNetwork(), page)).status).toBe(403);
    }
  });
});

describe("limits on rooms made without a key", () => {
  it("refuses a network past its rooms for the day, and still admits another network", async () => {
    const relay = openEnv({ ROOMS_PER_NETWORK_DAY: "2" });
    const busy = someNetwork();
    expect((await registerIn(relay, busy)).status).toBe(201);
    expect((await registerIn(relay, busy)).status).toBe(201);
    const lookup = vi.spyOn(relayEnv.ROOM, "idFromName");
    const refused = await registerIn(relay, busy);
    expect(refused.status).toBe(429);
    expect(refused.headers.get("x-kelpie-reason")).toBe(ROOMS_NETWORK);
    expect(lookup).not.toHaveBeenCalled();
    expect((await registerIn(relay, someNetwork())).status).toBe(201);
  });

  it("refuses everyone once the relay made its rooms for the day", async () => {
    const relay = openEnv({ ROOMS_PER_DAY: String((await todaysTotal()) + 1) });
    expect((await registerIn(relay, someNetwork())).status).toBe(201);
    const lookup = vi.spyOn(relayEnv.ROOM, "idFromName");
    const refused = await registerIn(relay, someNetwork());
    expect(refused.status).toBe(503);
    expect(refused.headers.get("x-kelpie-reason")).toBe(ROOMS_FULL);
    expect(lookup).not.toHaveBeenCalled();
  });

  it("sheds a burst with the rate binding before the Registrar or a room is asked", async () => {
    const shed = limiter(false);
    const registrarLookup = vi.spyOn(relayEnv.REGISTRAR, "idFromName");
    const roomLookup = vi.spyOn(relayEnv.ROOM, "idFromName");
    const refused = await registerIn(openEnv({ ROOM_RATE: shed }), someNetwork());
    expect(refused.status).toBe(429);
    expect(refused.headers.get("x-kelpie-reason")).toBe(ROOMS_RATE);
    expect(registrarLookup).not.toHaveBeenCalled();
    expect(roomLookup).not.toHaveBeenCalled();
    // Keyed by the network's hash, never its address.
    expect(shed.limit.mock.calls[0][0].key).toMatch(/^[0-9a-f]{64}$/);
    expect((await registerIn(openEnv({ ROOM_RATE: limiter(true) }), someNetwork())).status).toBe(201);
  });

  it("leaves the key's holder out of the limits", async () => {
    const shed = limiter(false);
    const registrarLookup = vi.spyOn(relayEnv.REGISTRAR, "idFromName");
    const keyed = { ...relayEnv, ROOM_RATE: shed, ROOMS_PER_NETWORK_DAY: "1", ROOMS_PER_DAY: "1" };
    expect((await registerIn(keyed, someNetwork(), { authorization: `Bearer ${REGISTRATION_KEY}` })).status).toBe(201);
    expect(shed.limit).not.toHaveBeenCalled();
    expect(registrarLookup).not.toHaveBeenCalled();
  });

  it("stores hashes of networks, never an address", async () => {
    const from = someNetwork();
    expect((await registerIn(openEnv(), from)).status).toBe(201);
    const rows = await registrarRows();
    expect(rows.length).toBeGreaterThan(1);
    for (const row of rows) {
      expect(row.key === TOTAL || /^[0-9a-f]{64}$/.test(row.key)).toBe(true);
      expect(JSON.stringify(row)).not.toContain(from);
    }
    await runInDurableObject(registrar(`unit-${randomRoom()}`), (instance: Registrar) => {
      expect(() => instance.admit("203.0.113.7", Date.now(), 20, 1000)).toThrow("not a network key");
    });
  });
});

describe("the Registrar", () => {
  const net = (n: number) => n.toString(16).padStart(64, "0");
  const day = 24 * 3600 * 1000;

  it("counts per network and in all, per UTC day", async () => {
    const name = `unit-${randomRoom()}`;
    const r = registrar(name);
    const now = Date.now();
    expect(await r.admit(net(1), now, 2, 3)).toBe("ok");
    expect(await r.admit(net(1), now, 2, 3)).toBe("ok");
    expect(await r.admit(net(1), now, 2, 3)).toBe("network");
    expect(await r.admit(net(2), now, 2, 3)).toBe("ok");
    expect(await r.admit(net(3), now, 2, 3)).toBe("full");
    // A new day counts from nothing.
    expect(await r.admit(net(1), now + day, 2, 3)).toBe("ok");
    const rows = await registrarRows(name);
    expect(rows.find((row) => row.day === utcDay(now) && row.key === TOTAL)?.count).toBe(3);
    expect(rows.find((row) => row.day === utcDay(now) && row.key === net(1))?.count).toBe(2);
  });

  it("forgets the days before yesterday when it next counts", async () => {
    const name = `unit-${randomRoom()}`;
    const r = registrar(name);
    const now = Date.now();
    await r.admit(net(1), now - 2 * day, 20, 1000);
    await r.admit(net(1), now - day, 20, 1000);
    expect(new Set((await registrarRows(name)).map((row) => row.day)).size).toBe(2);
    await r.admit(net(1), now, 20, 1000);
    expect(new Set((await registrarRows(name)).map((row) => row.day))).toEqual(new Set([utcDay(now - day), utcDay(now)]));
    // A refusal writes nothing, so it forgets nothing either.
    await r.admit(net(1), now + 2 * day, 20, 0);
    expect(new Set((await registrarRows(name)).map((row) => row.day))).toEqual(new Set([utcDay(now - day), utcDay(now)]));
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

  it("says on its 101 which version of the protocol the room speaks", async () => {
    const r = await register();
    const answer = await upgrade("/v1/desktop", await desktopQuery(r));
    expect(answer.headers.get("x-kelpie-relay-version")).toBe("2");
    expect(PROTOCOL_VERSION).toBe("2");
    socket(answer);
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
    expect(await d.said()).toEqual({ t: "open", s: 0, via: "token", k: await tokenHash(k) });
  });

  it("takes only the origin the desktop registered, or none", async () => {
    const r = await register();
    const k = token();
    const d = await desktop(r, [k]);
    expect((await phone(r, `k=${k}`, { origin: "https://desk.tail1234.ts.net:8443" })).status).toBe(403);
    d.ws.send(JSON.stringify({ t: "origin", o: "https://desk.tail1234.ts.net:8443" }));
    await expect.poll(async () => runInDurableObject(stub(r), async (_room: Room, state) => state.storage.get("origins"))).toEqual(["https://desk.tail1234.ts.net:8443"]);
    expect((await phone(r, `k=${k}`, { origin: "https://evil.example" })).status).toBe(403);
    socket(await phone(r, `k=${k}`, { origin: "https://desk.tail1234.ts.net:8443" }));
    socket(await phone(r, `k=${k}`));
  });

  it("takes phones from any origin the desktop listed, and from no other", async () => {
    const r = await register();
    const tokens = [token(), token(), token(), token()];
    const d = await desktop(r, tokens);
    const [ts, hosted] = ["https://desk.tail1234.ts.net:8443", "https://relay.example.workers.dev"];
    d.ws.send(JSON.stringify({ t: "origins", o: [ts, hosted] }));
    await expect.poll(async () => runInDurableObject(stub(r), async (_room: Room, state) => state.storage.get("origins"))).toEqual([ts, hosted]);
    socket(await phone(r, `k=${tokens[0]}`, { origin: ts }));
    socket(await phone(r, `k=${tokens[1]}`, { origin: hosted }));
    socket(await phone(r, `k=${tokens[2]}`));
    expect((await phone(r, `k=${tokens[3]}`, { origin: "https://evil.example" })).status).toBe(403);
    expect((await phone(r, `k=${tokens[3]}`, { origin: "http://relay.example.workers.dev" })).status).toBe(403);
    // An empty list lets no page in, and an app without an Origin still.
    d.ws.send(JSON.stringify({ t: "origins", o: [] }));
    await expect.poll(async () => runInDurableObject(stub(r), async (_room: Room, state) => state.storage.get("origins"))).toEqual([]);
    expect((await phone(r, `k=${tokens[3]}`, { origin: hosted })).status).toBe(403);
    socket(await phone(r, `k=${tokens[3]}`));
  });

  it("reads a room stored with one origin as a list of it", async () => {
    const r = await register();
    const k = token();
    await desktop(r, [k]);
    const ts = "https://desk.tail1234.ts.net:8443";
    await runInDurableObject(stub(r), async (_room: Room, state) => {
      await state.storage.delete("origins");
      await state.storage.put("origin", ts);
    });
    socket(await phone(r, `k=${k}`, { origin: ts }));
    expect((await phone(r, `k=${k}`, { origin: "https://evil.example" })).status).toBe(403);
  });

  it.each([
    ["more than four", ["https://a.example", "https://b.example", "https://c.example", "https://d.example", "https://e.example"]],
    ["one over plain http", ["https://a.example", "http://b.example"]],
    ["one with a path", ["https://a.example/app"]],
    ["not a list", "https://a.example"],
  ])("closes a desktop that lists origins with %s", async (_name, o) => {
    const r = await register();
    const d = await desktop(r);
    d.ws.send(JSON.stringify({ t: "origins", o }));
    expect(await d.closed).toBe(4003);
    expect(await d.reason).toBe("origins");
  });

  it("tells the desktop the origin a phone came from, and none for an app without one", async () => {
    const r = await register();
    const [browser, app] = [token(), token()];
    const hosted = "https://relay.example.workers.dev";
    const d = await desktop(r, [browser, app]);
    d.ws.send(JSON.stringify({ t: "origins", o: [hosted] }));
    await expect.poll(async () => runInDurableObject(stub(r), async (_room: Room, state) => state.storage.get("origins"))).toEqual([hosted]);
    socket(await phone(r, `k=${browser}`, { origin: hosted }));
    expect(await d.said()).toEqual({ t: "open", s: 0, via: "token", k: await tokenHash(browser), o: hosted });
    socket(await phone(r, `k=${app}`));
    const opened = await d.said();
    expect(opened).toEqual({ t: "open", s: 1, via: "token", k: await tokenHash(app) });
    expect("o" in opened).toBe(false);

    // A desktop that comes back hears both again, each with what it came from.
    const back = socket(await upgrade("/v1/desktop", await desktopQuery(r)));
    await allow(back, [browser, app]);
    const heard = [await back.said(), await back.said()].sort((a, b) => Number(a.s) - Number(b.s));
    expect(heard).toEqual([
      { t: "open", s: 0, via: "token", k: await tokenHash(browser), o: hosted },
      { t: "open", s: 1, via: "token", k: await tokenHash(app) },
    ]);
  });

  it("lets a pairing token in once, and not once it expired", async () => {
    const r = await register();
    const d = await desktop(r);
    const p = token();
    d.ws.send(JSON.stringify({ t: "pair", h: await tokenHash(p), ttl: 120 }));
    await expect.poll(async () => runInDurableObject(stub(r), async (_room: Room, state) => ((await state.storage.get<unknown[]>("pair")) ?? []).length)).toBe(1);
    socket(await phone(r, `p=${p}`));
    expect(await d.said()).toEqual({ t: "open", s: 0, via: "pair", k: await tokenHash(p) });
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
    expect(await back.said()).toEqual({ t: "open", s: 0, via: "token", k: await tokenHash(kept) });
    expect(await a.said()).toEqual({ t: "desktop_online" });
    a.ws.send(new Uint8Array([9]));
    // The phone that was closed is reported gone too; what comes next is a's frame, and not the one sent before the list.
    expect(await back.said()).toMatchObject({ t: "close", s: 1 });
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
    expect(await d.said()).toMatchObject({ t: "close", s: first });
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

  it("gives phones that come in at the same moment slots of their own, also just after the room woke", async () => {
    const r = await register();
    const tokens = [token(), token(), token()];
    const d = await desktop(r, tokens);
    await evictDurableObject(stub(r));
    for (const answer of await Promise.all(tokens.map((k) => phone(r, `k=${k}`)))) socket(answer);
    const slots = [(await d.said()).s, (await d.said()).s, (await d.said()).s];
    expect(new Set(slots).size).toBe(3);
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

  it("marks every answer nosniff and uncached, never HTML, and repeats nothing of the request", async () => {
    const r = await register();
    const marker = `zq${hex(crypto.getRandomValues(new Uint8Array(6)))}`;
    const ticket = encodeURIComponent(r.ticket);
    const unregistered = encodeURIComponent(await makeTicket(randomRoom(), relayEnv.TICKET_SECRET));
    const ws = { upgrade: "websocket" };
    const asked: [string, RequestInit][] = [
      [`/${marker}`, {}],
      [`/v1/${marker}?${marker}=${marker}`, {}],
      [`/v1/health?${marker}=1`, {}],
      [`/v1/rooms?${marker}`, {}],
      [`/v1/rooms`, { method: "POST", body: JSON.stringify({ pk: marker }) }],
      [`/v1/rooms`, { method: "POST", headers: { authorization: `Bearer ${REGISTRATION_KEY}` }, body: JSON.stringify({ pk: marker }) }],
      [`/v1/rooms`, { method: "POST", headers: { authorization: `Bearer ${marker}` }, body: "{}" }],
      [`/v1/rooms`, { method: "POST", headers: { origin: `https://${marker}.example` }, body: "{}" }],
      [`/v1/phone?t=${marker}`, {}],
      [`/v1/phone?t=${marker}`, { headers: ws }],
      [`/v1/phone?t=${ticket}&k=${marker}`, { headers: ws }],
      [`/v1/phone?t=${ticket}&k=${token()}&${marker}=1`, { headers: { ...ws, origin: `https://${marker}.example` } }],
      [`/v1/phone?t=${unregistered}&k=${marker}`, { headers: ws }],
      [`/v1/desktop?t=${ticket}&ts=${marker}&sig=${marker}`, { headers: ws }],
      [`/v1/desktop?t=${ticket}`, { headers: { ...ws, origin: `https://${marker}.example` } }],
    ];
    for (const [path, init] of asked) {
      const response = await self().fetch(`https://relay.test${path}`, { ...init, headers: { "cf-connecting-ip": someNetwork(), ...init.headers } });
      expect(response.status, path).not.toBe(101);
      expect(response.headers.get("x-content-type-options"), path).toBe("nosniff");
      expect(response.headers.get("cache-control"), path).toBe("no-store");
      expect(response.headers.get("content-type"), path).not.toMatch(/html/);
      expect(await response.text(), path).not.toContain(marker);
    }
    const made = await registerWith({ pk: (await roomKeys()).pk });
    expect(made.headers.get("content-type")).toBe("application/json");
    expect(made.headers.get("x-content-type-options")).toBe("nosniff");
  });

  it("gives each Cloudflare script a main module of entrypoints only, which the runtime requires", async () => {
    const modules = [await import("../src/entry-router"), await import("../src/entry-room")];
    for (const exported of modules.flatMap((m) => Object.values(m))) {
      expect(typeof exported === "function" || (typeof exported === "object" && exported !== null)).toBe(true);
    }
    expect(Object.keys(modules[1]).sort()).toEqual(["Registrar", "Room", "default"]);
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
    expect(await d.said()).toEqual({ t: "open", s: 0, via: "token", k: await tokenHash(k) });
    // With the phone's code: the route that lost the phone's race says so, and is not counted against it.
    p.ws.close(4000, "route lost");
    expect(await d.said()).toEqual({ t: "close", s: 0, c: 4000 });

    const again = socket(await phone(r, `k=${k}`));
    await d.said();
    d.ws.close(1000, "bye");
    expect(await again.said()).toEqual({ t: "desktop_offline" });
  });

  it("pairs and relays for a desktop from before open rooms, which sends one origin and reads no headers", async () => {
    const r = await register();
    const ts = "https://desk.tail1234.ts.net:8443";
    const d = socket(await upgrade("/v1/desktop", await desktopQuery(r)));
    await allow(d, []);
    d.ws.send(JSON.stringify({ t: "origin", o: ts }));
    const p = token();
    d.ws.send(JSON.stringify({ t: "pair", h: await tokenHash(p), ttl: 120 }));
    await expect.poll(async () => runInDurableObject(stub(r), async (_room: Room, state) => ((await state.storage.get<unknown[]>("pair")) ?? []).length)).toBe(1);
    const paired = socket(await phone(r, `p=${p}`, { origin: ts }));
    const opened = await d.said();
    expect(opened).toMatchObject({ t: "open", via: "pair", k: await tokenHash(p) });
    paired.ws.send(new Uint8Array([1, 2]));
    expect(Array.from(new Uint8Array((await d.next()) as ArrayBuffer))).toEqual([0, opened.s as number, 1, 2]);

    // Paired: the desktop lists the device's token, and the phone comes back with it.
    const k = token();
    await allow(d, [k]);
    paired.ws.close(1000, "paired");
    expect(await d.said()).toMatchObject({ t: "close", s: opened.s });
    const back = socket(await phone(r, `k=${k}`, { origin: ts }));
    const reopened = await d.said();
    expect(reopened).toMatchObject({ t: "open", via: "token", k: await tokenHash(k) });
    d.ws.send(Uint8Array.of(0, reopened.s as number, 9));
    expect(Array.from(new Uint8Array((await back.next()) as ArrayBuffer))).toEqual([9]);
    expect((await phone(r, `k=${k}`, { origin: "https://evil.example" })).status).toBe(403);
  });

  it("drops a phone's frames while no desktop is ready, and counts nothing for them", async () => {
    const r = await register();
    const k = token();
    const first = await desktop(r, [k]);
    first.ws.close(1000, "away");
    await first.closed;
    const p = socket(await phone(r, `k=${k}`));
    expect(await p.said()).toEqual({ t: "desktop_offline" });
    for (let i = 0; i < 3; i++) p.ws.send(new Uint8Array(32 * 1024));
    // Text from a phone closes it, once the frames before it were taken.
    p.ws.send("done");
    expect(await p.closed).toBe(4003);
    await runInDurableObject(stub(r), (instance: Room) => {
      const room = instance as unknown as { usage?: { bytes: number }; bucket: { tokens: number } };
      expect(room.usage?.bytes ?? 0).toBe(0);
      expect(room.bucket.tokens).toBe(ROOM_BURST_BYTES);
    });
  });

  it("hears nothing more from a desktop that another took the room from", async () => {
    const r = await register();
    const k = token();
    await desktop(r, [k]);
    const fresh = await desktop(r, [k]);
    const p = socket(await phone(r, `k=${k}`));
    expect((await fresh.said()).t).toBe("open");
    // The old connection's last word, arriving late, as the room would get it from the socket it replaced.
    await runInDurableObject(stub(r), async (instance: Room, state) => {
      const late = {
        readyState: 1,
        deserializeAttachment: () => ({ role: "desktop", slot: 0, entry: "", ready: true, replaced: true }),
        serializeAttachment: vi.fn(),
        send: vi.fn(),
        close: vi.fn(),
      } as unknown as WebSocket;
      await instance.webSocketMessage(late, JSON.stringify({ t: "allow", h: [] }));
      await instance.webSocketMessage(late, JSON.stringify({ t: "kick", s: 0, c: 4401 }));
      await instance.webSocketMessage(late, JSON.stringify({ t: "drop" }));
      expect(await state.storage.get("allow")).toEqual([await tokenHash(k)]);
    });
    p.ws.send(new Uint8Array([3]));
    expect(Array.from(new Uint8Array((await fresh.next()) as ArrayBuffer)).slice(2)).toEqual([3]);
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

  it("writes the month's usage at most once a minute under steady traffic", async () => {
    const r = await register();
    await desktop(r);
    await runInDurableObject(stub(r), async (instance: Room, state) => {
      const room = instance as unknown as { count(bytes: number): Promise<boolean>; flushedAt: number };
      let now = Date.now();
      const clock = vi.spyOn(Date, "now").mockImplementation(() => now);
      try {
        room.flushedAt = now;
        let flushes = 0;
        // Three minutes of a frame a second, well under a megabyte.
        for (let i = 0; i < 180; i++) {
          now += 1000;
          const before = room.flushedAt;
          await room.count(1000);
          if (room.flushedAt !== before) flushes += 1;
        }
        expect(flushes).toBe(180_000 / USAGE_FLUSH_MS);
        expect(USAGE_FLUSH_MS).toBe(60_000);
        expect((await state.storage.get<{ bytes: number }>("usage"))?.bytes).toBe(180 * 1000);
      } finally {
        clock.mockRestore();
      }
    });
  });

  it("keeps timers, plain accept and looping alarms out of the source", () => {
    expect(registrarSource.replace(/\/\*[\s\S]*?\*\/|\/\/.*$/gm, "")).not.toMatch(/\bsetTimeout\b|\bsetInterval\b|setAlarm|\balarm\b|console\./);
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

// ---------- TURN credentials ----------

describe("TURN credentials for the room's desktop", () => {
  const COTURN = { TURN_SECRET: "test-only-turn-secret", TURN_URLS: "turn:turn.example.test:3478?transport=udp,turns:turn.example.test:443?transport=tcp,turn:turn.example.test:53?transport=udp" };

  async function signed(r: Registered, statement: string): Promise<string> {
    const sig = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, r.keys.privateKey, new TextEncoder().encode(statement));
    return encodeBase64Url(new Uint8Array(sig));
  }

  function nextTs(): number {
    return (lastTs = Math.max(Date.now(), lastTs + 1));
  }

  async function ask(relay: Env, r: Registered, path = "/v1/turn", body?: Record<string, unknown>, fetchFn?: typeof fetch, from = someNetwork()): Promise<Response> {
    const ts = nextTs();
    const payload = body ?? { ts, sig: await signed(r, turnStatement(r.room, ts)) };
    const request = new Request(`https://relay.test${path}?t=${encodeURIComponent(r.ticket)}`, {
      method: "POST",
      headers: { "content-type": "application/json", "cf-connecting-ip": from },
      body: JSON.stringify(payload),
    });
    return (worker.fetch as (q: Request, e: Env, c?: unknown, f?: typeof fetch) => Promise<Response>)(request, relay, undefined, fetchFn);
  }

  async function withPhone(): Promise<Registered> {
    const r = await register();
    const d = await desktop(r, [token()]);
    d.ws.close();
    return r;
  }

  it("answers not configured without TURN secrets, before it looks at anything else", async () => {
    const r = await register();
    const answer = await ask(relayEnv, r, "/v1/turn", { ts: 1, sig: "x" });
    expect([answer.status, answer.headers.get("x-kelpie-reason")]).toEqual([404, TURN_NOT_CONFIGURED]);
    expect(turnMode({})).toBeNull();
    expect(turnMode({ TURN_KEY_ID: "k", TURN_API_TOKEN: "t" })).toBe("cloudflare");
    expect(turnMode(COTURN)).toBe("coturn");
  });

  it("mints coturn credentials for a signed request of a room with a phone, with no port-53 server, and never twice for one signature", async () => {
    const r = await withPhone();
    const relay = { ...relayEnv, ...COTURN };
    const ts = nextTs();
    const body = { ts, sig: await signed(r, turnStatement(r.room, ts)) };
    const answer = await ask(relay, r, "/v1/turn", body);
    expect(answer.status).toBe(200);
    const minted = (await answer.json()) as { ice_servers: { urls: string[]; username: string; credential: string }[]; username: string; expires: number };
    expect(minted.ice_servers[0].urls).toEqual(["turn:turn.example.test:3478?transport=udp", "turns:turn.example.test:443?transport=tcp"]);
    expect(minted.username).toMatch(new RegExp(`^\\d+:${r.room}$`));
    expect(minted.expires - Date.now()).toBeGreaterThan((TURN_TTL_S - 5) * 1000);
    const again = await ask(relay, r, "/v1/turn", body);
    expect(again.status, "the same signature again").toBe(403);
  });

  it("refuses a bad signature, one over another statement, a stale time and a room with no phone allowed", async () => {
    const r = await register();
    const relay = { ...relayEnv, ...COTURN };
    const ts = nextTs();
    expect((await ask(relay, r, "/v1/turn", { ts, sig: await signed(r, desktopStatement(r.room, ts)) })).status, "a socket's signature").toBe(403);
    const old = Date.now() - 10 * 60 * 1000;
    expect((await ask(relay, r, "/v1/turn", { ts: old, sig: await signed(r, turnStatement(r.room, old)) })).status, "ten minutes old").toBe(403);
    const none = await ask(relay, r);
    expect([none.status, none.headers.get("x-kelpie-reason")]).toEqual([409, TURN_NO_PHONES]);
    const bogus = new Request("https://relay.test/v1/turn?t=00.bad", { method: "POST", body: "{}" });
    expect((await worker.fetch(bogus, relay)).status).toBe(403);
    const fromPage = new Request(`https://relay.test/v1/turn?t=${encodeURIComponent(r.ticket)}`, { method: "POST", headers: { origin: "https://evil.test" }, body: "{}" });
    expect((await worker.fetch(fromPage, relay)).status, "never from a page").toBe(403);
  });

  it("holds a room to its credentials an hour, and the relay to its credentials a day", async () => {
    const r = await withPhone();
    const relay = { ...relayEnv, ...COTURN };
    for (let i = 0; i < TURN_MINTS_PER_HOUR; i++) expect((await ask(relay, r)).status).toBe(200);
    const over = await ask(relay, r);
    expect([over.status, over.headers.get("x-kelpie-reason")]).toEqual([429, TURN_ROOM_LIMIT]);
    const minted = await runInDurableObject(registrar(), (_o, state) => state.storage.sql.exec<{ count: number }>("SELECT count FROM turn_mints WHERE day = ? AND key = ?", utcDay(Date.now()), TOTAL).toArray()[0]?.count ?? 0);
    const other = await withPhone();
    const capped = { ...relay, TURN_MINTS_PER_DAY: String(minted + 1) };
    expect((await ask(capped, other)).status).toBe(200);
    const full = await ask(capped, other);
    expect([full.status, full.headers.get("x-kelpie-reason")]).toEqual([503, TURN_FULL]);
  });

  it("checks the signature before anything about the room, and holds one network to its day", async () => {
    const relay = { ...openEnv(COTURN) };
    const made = await registerIn(relay, someNetwork());
    expect(made.status).toBe(201);
    const { room, ticket } = (await made.json()) as { room: string; ticket: string };
    // The room's key is not this test's: its desktop signs with another. Admission looks at the room first, so a signature is checked as well.
    const r = await withPhone();
    const open: Registered = { ...r, room, ticket };
    const refused = await ask(relay, open, "/v1/turn", { ts: nextTs(), sig: "x".repeat(86) });
    expect(refused.status, "a bad signature is still a bad signature").toBe(403);
    // A keyed room (this test's) on a relay that allows open rooms too: fine; then one network's day.
    const net = someNetwork();
    const perNetwork = { ...relayEnv, ...COTURN, TURN_MINTS_PER_NETWORK_DAY: "2" };
    expect((await ask(perNetwork, r, "/v1/turn", undefined, undefined, net)).status).toBe(200);
    expect((await ask(perNetwork, r, "/v1/turn", undefined, undefined, net)).status).toBe(200);
    const over = await ask(perNetwork, r, "/v1/turn", undefined, undefined, net);
    expect([over.status, over.headers.get("x-kelpie-reason")]).toEqual([429, TURN_NETWORK]);
    expect((await ask(perNetwork, r, "/v1/turn", undefined, undefined, someNetwork())).status, "another network").toBe(200);
  });

  it("an open room's desktop, signing right, hears TURN is not offered unless TURN_OPEN_ROOMS is set", async () => {
    const { keys, pk } = await roomKeys();
    const relay = { ...openEnv(COTURN) };
    const made = await worker.fetch(new Request("https://relay.test/v1/rooms", { method: "POST", headers: { "content-type": "application/json", "cf-connecting-ip": someNetwork() }, body: JSON.stringify({ pk }) }), relay);
    const { room, ticket } = (await made.json()) as { room: string; ticket: string };
    const r: Registered = { room, ticket, keys };
    const d = await desktop(r, [token()]);
    d.ws.close();
    const refused = await ask(relay, r);
    expect([refused.status, refused.headers.get("x-kelpie-reason")]).toEqual([404, TURN_NOT_OFFERED]);
    expect((await ask({ ...relay, TURN_OPEN_ROOMS: "1" }, r)).status).toBe(200);
  });

  it("asks Cloudflare with the key's token, and passes on only what a phone needs; an upstream failure is said as one", async () => {
    const r = await withPhone();
    const relay = { ...relayEnv, TURN_KEY_ID: "key-1", TURN_API_TOKEN: "test-only-api-token" };
    const calls: { url: string; auth: string | null; body: string }[] = [];
    const cloudflare = (status: number, body: unknown) =>
      (async (input: RequestInfo | URL, init?: RequestInit) => {
        calls.push({ url: String(input), auth: new Headers(init?.headers).get("authorization"), body: String(init?.body ?? "") });
        return new Response(status === 204 ? null : JSON.stringify(body), { status });
      }) as typeof fetch;
    const servers = { iceServers: [{ urls: ["stun:stun.cloudflare.com:3478", "stun:stun.cloudflare.com:53"] }, { urls: ["turn:turn.cloudflare.com:3478?transport=udp", "turn:turn.cloudflare.com:53?transport=udp", "turns:turn.cloudflare.com:443?transport=tcp"], username: "u-1", credential: "c-1" }] };
    const answer = await ask(relay, r, "/v1/turn", undefined, cloudflare(201, servers));
    expect(answer.status).toBe(200);
    const minted = (await answer.json()) as { ice_servers: { urls: string[] }[]; username: string };
    expect(minted.ice_servers.flatMap((s) => s.urls).some((u) => u.includes(":53"))).toBe(false);
    expect(minted.username).toBe("u-1");
    expect(calls[0]).toMatchObject({ url: "https://rtc.live.cloudflare.com/v1/turn/keys/key-1/credentials/generate-ice-servers", auth: "Bearer test-only-api-token" });
    expect(JSON.parse(calls[0].body)).toEqual({ ttl: TURN_TTL_S });
    for (const [status, body] of [[401, {}], [500, {}], [201, { iceServers: [] }]] as const) {
      const failed = await ask(relay, r, "/v1/turn", undefined, cloudflare(status, body));
      expect([failed.status, failed.headers.get("x-kelpie-reason")], `upstream ${status}`).toEqual([502, TURN_UPSTREAM]);
    }
    // A revoke is signed over the credential it ends.
    const ts = nextTs();
    const revoke = { ts, sig: await signed(r, revokeStatement(r.room, ts, "u-1")), username: "u-1" };
    const ended = await ask(relay, r, "/v1/turn/revoke", revoke, cloudflare(204, {}));
    expect(ended.status).toBe(204);
    expect(calls.at(-1)?.url).toBe("https://rtc.live.cloudflare.com/v1/turn/keys/key-1/credentials/u-1/revoke");
    const ts2 = nextTs();
    const otherName = await ask(relay, r, "/v1/turn/revoke", { ts: ts2, sig: await signed(r, revokeStatement(r.room, ts2, "u-1")), username: "u-2" }, cloudflare(204, {}));
    expect(otherName.status, "a signature over another credential").toBe(403);
  });

  it("matches the shared vectors: the statements the desktop signs and coturn's credential", async () => {
    expect(turnStatement(vectors.room, vectors.ts)).toBe(vectors.turn_statement);
    expect(revokeStatement(vectors.room, vectors.ts, vectors.revoke_username)).toBe(vectors.revoke_statement);
    const [expiry] = vectors.revoke_username.split(":");
    const out = await coturnMint({ TURN_SECRET: vectors.coturn_secret, TURN_URLS: "turn:x.test:3478" }, vectors.revoke_username.split(":")[1], Number(expiry) * 1000 - TURN_TTL_S * 1000);
    expect(out.ok && out.minted.ice_servers[0].credential).toBe(vectors.coturn_credential);
    expect(cleanServers([{ urls: "turn:a:53" }, { urls: ["javascript:x", "stun:b:3478"] }, null])).toEqual([{ urls: ["stun:b:3478"] }]);
  });
});