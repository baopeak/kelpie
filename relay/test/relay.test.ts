import { evictDurableObject, listDurableObjectIds, runInDurableObject } from "cloudflare:test";
import { env, exports } from "cloudflare:workers";
import { afterEach, describe, expect, it, vi } from "vitest";
import roomSource from "../src/room.ts?raw";
import workerSource from "../src/worker.ts?raw";
import { Room } from "../src/room";
import type { Env } from "../src/worker";

const relayEnv = env as unknown as Env;
const open: WebSocket[] = [];

function hex(bytes: Uint8Array): string {
  return Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
}

function randomRoom(): string {
  return hex(crypto.getRandomValues(new Uint8Array(16)));
}

async function sign(room: string, secret = relayEnv.TICKET_SECRET): Promise<string> {
  const key = await crypto.subtle.importKey(
    "raw",
    new TextEncoder().encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const mac = await crypto.subtle.sign("HMAC", key, new TextEncoder().encode(room));
  const text = btoa(String.fromCharCode(...new Uint8Array(mac)));
  return `${room}.${text.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "")}`;
}

function request(path: string, ticket?: string, upgrade = true): Promise<Response> {
  const query = ticket === undefined ? "" : `?t=${encodeURIComponent(ticket)}`;
  return (exports as unknown as { default: Fetcher }).default.fetch(`https://relay.test${path}${query}`, {
    headers: upgrade ? { upgrade: "websocket" } : {},
  });
}

/** Opens a socket and collects what arrives, so a test can wait for the next frame. */
async function connect(role: "desktop" | "phone", room: string) {
  const response = await request(`/v1/${role}`, await sign(room));
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
  (ws as unknown as { binaryType: string }).binaryType = "arraybuffer";
  ws.accept();
  open.push(ws);
  const next = () =>
    new Promise<string | ArrayBuffer>((resolve, reject) => {
      const queued = inbox.shift();
      if (queued !== undefined) return resolve(queued);
      waiters.push(resolve);
      const timer = setTimeout(() => reject(new Error("no frame within 2 s")), 2000);
      waiters[waiters.length - 1] = (value) => {
        clearTimeout(timer);
        resolve(value);
      };
    });
  return { ws, next };
}

afterEach(() => {
  for (const ws of open.splice(0)) ws.close(1000, "test over");
  vi.restoreAllMocks();
});

describe("the ticket is checked before any Durable Object call", () => {
  it.each([
    ["no ticket", undefined],
    ["not a ticket", "hello"],
    ["forged signature", `${"a".repeat(32)}.${"A".repeat(43)}`],
  ])("refuses %s with no object created", async (_name, ticket) => {
    const before = (await listDurableObjectIds(relayEnv.ROOM)).length;
    const response = await request("/v1/phone", ticket);
    expect(response.status).toBe(403);
    expect((await listDurableObjectIds(relayEnv.ROOM)).length).toBe(before);
  });

  it("refuses a ticket signed with another secret, or moved to another room", async () => {
    const before = (await listDurableObjectIds(relayEnv.ROOM)).length;
    const wrongSecret = await sign(randomRoom(), "some-other-secret");
    expect((await request("/v1/desktop", wrongSecret)).status).toBe(403);

    const signature = (await sign(randomRoom())).split(".")[1];
    expect((await request("/v1/desktop", `${randomRoom()}.${signature}`)).status).toBe(403);
    expect((await listDurableObjectIds(relayEnv.ROOM)).length).toBe(before);
  });

  it("never asks the namespace for a stub when the ticket is bad", async () => {
    const lookup = vi.spyOn(relayEnv.ROOM, "idFromName");
    expect((await request("/v1/phone", await sign(randomRoom(), "wrong"))).status).toBe(403);
    expect(lookup).not.toHaveBeenCalled();
  });

  it("creates the room for a valid ticket", async () => {
    const before = (await listDurableObjectIds(relayEnv.ROOM)).length;
    await connect("desktop", randomRoom());
    expect((await listDurableObjectIds(relayEnv.ROOM)).length).toBe(before + 1);
  });
});

describe("what the router serves", () => {
  it("answers health without a Durable Object", async () => {
    const before = (await listDurableObjectIds(relayEnv.ROOM)).length;
    const response = await request("/v1/health", undefined, false);
    expect(response.status).toBe(200);
    expect(await response.text()).toBe("ok\n");
    expect((await listDurableObjectIds(relayEnv.ROOM)).length).toBe(before);
  });

  it("serves no HTML on any path", async () => {
    for (const [path, status] of [
      ["/", 404],
      ["/index.html", 404],
      ["/v1/phone", 426],
      ["/v1/desktop", 426],
    ] as const) {
      const response = await request(path, undefined, false);
      expect(response.status).toBe(status);
      expect(response.headers.get("content-type")).toMatch(/^text\/plain/);
      expect(await response.text()).not.toMatch(/</);
    }
  });
});

describe("forwarding", () => {
  it("carries a phone's bytes to the desktop with its slot, and back to that phone only", async () => {
    const room = randomRoom();
    const desktop = await connect("desktop", room);
    const first = await connect("phone", room);
    const second = await connect("phone", room);
    const payload = crypto.getRandomValues(new Uint8Array(1024));

    second.ws.send(payload);
    const framed = new Uint8Array((await desktop.next()) as ArrayBuffer);
    expect(Array.from(framed.subarray(0, 2))).toEqual([0, 1]);
    expect(hex(framed.subarray(2))).toBe(hex(payload));

    desktop.ws.send(framed);
    expect(hex(new Uint8Array((await second.next()) as ArrayBuffer))).toBe(hex(payload));

    first.ws.send("ping");
    expect(await first.next()).toBe("pong");
  });

  it("drops a phone's frame when no desktop is attached, and keeps the socket", async () => {
    const phone = await connect("phone", randomRoom());
    phone.ws.send(new Uint8Array(16));
    phone.ws.send("ping");
    expect(await phone.next()).toBe("pong");
  });

  it("refuses a ninth phone and reuses a slot once one is free", async () => {
    const room = randomRoom();
    const phones = [];
    for (let i = 0; i < 8; i++) phones.push(await connect("phone", room));
    expect((await request("/v1/phone", await sign(room))).status).toBe(409);

    phones[3].ws.close(1000, "done");
    await expect.poll(async () => (await request("/v1/phone", await sign(room))).status).toBe(101);
  });

  it("gives the room to the newest desktop", async () => {
    const room = randomRoom();
    const old = await connect("desktop", room);
    const closed = new Promise<number>((resolve) =>
      old.ws.addEventListener("close", (event) => resolve((event as CloseEvent).code)),
    );
    await connect("desktop", room);
    expect(await closed).toBe(4001);
  });

  it("closes a socket that sends unknown text", async () => {
    const phone = await connect("phone", randomRoom());
    const closed = new Promise<number>((resolve) =>
      phone.ws.addEventListener("close", (event) => resolve((event as CloseEvent).code)),
    );
    phone.ws.send("hello");
    expect(await closed).toBe(4003);
  });
});

describe("hibernation", () => {
  it("answers keepalives without running the object's message handler", async () => {
    const room = randomRoom();
    const phone = await connect("phone", room);
    const stub = relayEnv.ROOM.get(relayEnv.ROOM.idFromName(room));
    const handled = await runInDurableObject(stub, (instance: Room) =>
      vi.spyOn(instance, "webSocketMessage"),
    );

    phone.ws.send("ping");
    expect(await phone.next()).toBe("pong");

    expect(handled).not.toHaveBeenCalled();
    await runInDurableObject(stub, (_instance: Room, state) => {
      const pair = state.getWebSocketAutoResponse();
      expect(pair?.request).toBe("ping");
      expect(pair?.response).toBe("pong");
      expect(state.getWebSocketAutoResponseTimestamp(state.getWebSockets()[0])).toBeInstanceOf(Date);
    });
  });

  it("restarts the object when evicted, with sockets, slots and forwarding intact", async () => {
    const room = randomRoom();
    const desktop = await connect("desktop", room);
    const phone = await connect("phone", room);
    desktop.ws.send("colo");
    expect(JSON.parse((await desktop.next()) as string)).toMatchObject({ colo: "TST" });
    desktop.ws.send("info");
    const before = JSON.parse((await desktop.next()) as string);
    expect(before).toMatchObject({ sockets: 2, version: "test" });
    expect(before.id).toBe(relayEnv.ROOM.idFromName(room).toString());

    await evictDurableObject(relayEnv.ROOM.get(relayEnv.ROOM.idFromName(room)));

    phone.ws.send("ping");
    expect(await phone.next()).toBe("pong");
    phone.ws.send(new Uint8Array([7, 7, 7]));
    expect(Array.from(new Uint8Array((await desktop.next()) as ArrayBuffer))).toEqual([0, 0, 7, 7, 7]);
    desktop.ws.send("info");
    const after = JSON.parse((await desktop.next()) as string);
    expect(after.boot).not.toBe(before.boot);
    expect(after.sockets).toBe(2);
  });

  it("keeps timers, plain accept and alarms out of the source", () => {
    for (const source of [roomSource, workerSource]) {
      const code = source.replace(/\/\*[\s\S]*?\*\/|\/\/.*$/gm, "");
      expect(code).not.toMatch(/\bsetTimeout\b|\bsetInterval\b/);
      expect(code).not.toMatch(/\.accept\(\s*\)/);
      expect(code).not.toMatch(/\.addEventListener\b/);
      expect(code).not.toMatch(/\bsetAlarm\b|\balarm\s*\(/);
    }
    expect(roomSource).toMatch(/acceptWebSocket\(/);
    expect(roomSource).toMatch(/setWebSocketAutoResponse\(/);
  });
});
