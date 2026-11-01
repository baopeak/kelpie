import { beforeEach, describe, expect, it } from "vitest";
import { PHONE_API } from "../../src/lib/attention";
import { canAct, deriveConn } from "./conn-state";
import { Link, linkCodec, PING_AFTER_MS, PROBE_MS, RETRY_WINDOW_MS, type Codec, type Env, type Hello, type LinkHandlers, type Outcome, type SocketLike } from "./link";
import { FIRST_HEADER, Initiator, keypair, MODE_RECONNECT } from "./noise";

/** Plain JSON, no handshake: what the link does is tested apart from kelpie-link (noise.test.ts, and the codec test below). */
const jsonCodec = (): Codec => ({
  open: () => null,
  ready: () => true,
  receive: (d) => [JSON.parse(d as string)],
  send: (m) => [JSON.stringify(m)],
});

class FakeSocket implements SocketLike {
  onopen: SocketLike["onopen"] = null;
  onmessage: SocketLike["onmessage"] = null;
  onclose: SocketLike["onclose"] = null;
  onerror: SocketLike["onerror"] = null;
  readyState = 0;
  sent: any[] = [];
  closed = false;
  send(data: Uint8Array | string) {
    this.sent.push(typeof data === "string" ? JSON.parse(data) : { binary: data });
  }
  close() {
    this.closed = true;
  }
  open() {
    this.readyState = 1;
    this.onopen?.({});
  }
  say(m: object) {
    this.onmessage?.({ data: JSON.stringify(m) });
  }
  die(code = 1006) {
    this.readyState = 3;
    this.onclose?.({ code });
  }
  types() {
    return this.sent.map((m) => m.type ?? "auth");
  }
}

function hello(over: Partial<Hello> = {}): Hello {
  return {
    v: 2, api: PHONE_API, daemon_epoch: "e1", rev: 1, device: { name: "Test", can_write: true }, sessions: [], projects: {},
    load: { level: 0, reason: [], since: 0 }, server_ts: 1000, hb: { idle_s: 20, pong_s: 8, dead_s: 45, legacy: false }, ...over,
  };
}

function rig(opts: { random?: number; token?: string | null; codec?: () => Codec | null } = {}) {
  let t = 1_000_000;
  const timers: { at: number; fn: () => void; id: number }[] = [];
  let nextId = 1;
  const sockets: FakeSocket[] = [];
  let online = true;
  let reach: "reachable" | "portal" | "unreachable" = "reachable";
  const env: Env = {
    socket: () => {
      const s = new FakeSocket();
      sockets.push(s);
      return s;
    },
    url: () => "ws://test/ws",
    now: () => t,
    setTimeout: (fn, ms) => {
      const id = nextId++;
      timers.push({ at: t + ms, fn, id });
      return id;
    },
    clearTimeout: (h) => {
      const i = timers.findIndex((x) => x.id === h);
      if (i >= 0) timers.splice(i, 1);
    },
    random: () => opts.random ?? 0.5,
    online: () => online,
    reach: async () => reach,
  };
  const log: string[] = [];
  const frames: unknown[] = [];
  const queues: { id: string }[] = [];
  let have: Record<string, number | null> = {};
  const handlers: LinkHandlers = {
    changed: () => {},
    hello: (h, changed) => log.push(`hello ${h.daemon_epoch}${changed ? " changed" : ""}`),
    sessions: () => log.push("sessions"),
    frame: (m) => frames.push(m),
    chat: () => {},
    chatResolved: () => {},
    chatQueue: (m) => void queues.push(m),
    chatResync: (id) => log.push(`resync ${id}`),
    locked: (lock) => log.push(`locked ${lock.state}`),
    unpaired: () => log.push("unpaired"),
    haveRev: (id) => have[id] ?? null,
    chatSince: () => null,
  };
  const link = new Link(env, opts.codec ?? (() => (opts.token === null ? null : jsonCodec())), handlers);
  const advance = (ms: number) => {
    const end = t + ms;
    for (;;) {
      timers.sort((a, b) => a.at - b.at);
      const next = timers[0];
      if (!next || next.at > end) break;
      timers.shift();
      t = Math.max(t, next.at);
      next.fn();
    }
    t = end;
  };
  const last = () => sockets[sockets.length - 1];
  /** Opens the newest socket and completes the handshake. */
  const connect = (h: Partial<Hello> = {}) => {
    last().open();
    last().say({ type: "hello", ...hello({ server_ts: t, ...h }) });
  };
  return { link, env, sockets, last, advance, connect, log, frames, queues, setOnline: (v: boolean) => (online = v), setReach: (r: typeof reach) => (reach = r), setHave: (h: typeof have) => (have = h), now: () => t };
}

describe("while the computer holds the phone locked", () => {
  const LOCKED = { locked: true, lock: { state: "unlock", challenge: "c" } };

  it("a locked hello goes to the lock and nowhere else, and asks for no session", () => {
    const r = rig();
    r.link.start();
    r.link.follow("s1", "term");
    r.connect(LOCKED);
    expect(r.log).toEqual(["locked unlock"]);
    expect(r.link.ready).toBe(true);
    expect(r.last().types()).not.toContain("sub");
  });

  it("what is wanted waits for the hello that opens it, and picks up there", () => {
    const r = rig();
    r.link.start();
    r.link.follow("s1", "term");
    r.connect(LOCKED);
    r.link.follow("s2", "chat");
    expect(r.last().types()).toEqual(["auth"]);
    r.last().say({ type: "hello", ...hello() });
    expect(r.log).toEqual(["locked unlock", "hello e1"]);
    expect(r.last().types()).toEqual(expect.arrayContaining(["sub", "chat_sub"]));
  });

  it("being locked in a session stops what it asks for from then on, and leaving it again is a hello", () => {
    const r = rig();
    r.link.start();
    r.connect();
    r.last().say({ type: "locked", lock: { state: "unlock" } });
    expect(r.log).toEqual(["hello e1", "locked unlock"]);
    r.link.follow("s1", "term");
    expect(r.last().types()).not.toContain("sub");
    r.last().say({ type: "hello", ...hello() });
    expect(r.last().types()).toContain("sub");
  });

  it("a restart found while locked still counts as one, on the hello that opens it", () => {
    const r = rig();
    r.link.start();
    r.connect();
    r.last().die();
    r.advance(2000);
    r.connect({ ...LOCKED, daemon_epoch: "e2" });
    expect(r.log[r.log.length - 1]).toBe("locked unlock");
    r.last().say({ type: "hello", ...hello({ daemon_epoch: "e2" }) });
    expect(r.log[r.log.length - 1]).toBe("hello e2 changed");
  });

  it("a page asks, and hears: the lock's own requests carry no write id, and the reply brings the challenge", async () => {
    const r = rig();
    r.link.start();
    r.connect(LOCKED);
    const asked = r.link.lockAsk({ type: "lock_challenge" });
    const sent = r.last().sent[r.last().sent.length - 1];
    expect([sent.cid, sent.epoch]).toEqual([undefined, undefined]);
    r.last().say({ type: "ack", req: sent.req, lock: { state: "unlock", challenge: "new" } });
    expect(await asked).toMatchObject({ t: "ack", lock: { challenge: "new" } });
    r.link.lockNow();
    r.link.lockTouch();
    expect(r.last().types().slice(-2)).toEqual(["lock_now", "lock_touch"]);
  });
});

describe("Link handshake", () => {
  it("declares ack and hb in its first message, so it is never served as a legacy page", () => {
    const r = rig();
    r.link.start();
    r.last().open();
    expect(r.last().sent[0]).toEqual({ caps: ["ack", "hb"] });
  });

  it("says the window's secret first on every connection while it holds one, and not once it has let it go", () => {
    const r = rig();
    r.link.setResume("the-secret");
    r.link.start();
    r.last().open();
    expect(r.last().sent[0]).toEqual({ caps: ["ack", "hb"], resume: "the-secret" });
    r.link.setResume(null);
    r.last().close(1006);
    r.link.start();
    r.last().open();
    expect(r.last().sent[0]).toEqual({ caps: ["ack", "hb"] });
  });

  it("over kelpie-link: every connection opens with a new first message, so the daemon never takes a retry for a replay", async () => {
    const desktop = keypair();
    const me = keypair();
    const r = rig({ codec: () => linkCodec({ priv: me.priv, pub: me.pub, dpk: desktop.pub, deviceId: "d", name: "n", computer: "c", endpoints: [], addrsSeq: 0 }) });
    const firsts = new Set<string>();
    r.link.start();
    for (let attempt = 0; attempt < 4; attempt++) {
      r.last().open();
      firsts.add(Array.from(r.last().sent[0].binary as Uint8Array).join());
      const before = r.sockets.length;
      r.last().die();
      // The moment the retry opens its socket, not later: one left unopened is given up on in its turn.
      for (let waited = 0; r.sockets.length === before && waited < 120; waited++) {
        await Promise.resolve();
        r.advance(1000);
      }
    }
    expect(firsts.size).toBe(4);
  });

  it("over kelpie-link: the handshake goes first, nothing else before it, and a reply from another computer ends the connection", () => {
    const desktop = keypair();
    const me = keypair();
    const r = rig({ codec: () => linkCodec({ priv: me.priv, pub: me.pub, dpk: desktop.pub, deviceId: "d", name: "n", computer: "c", endpoints: [], addrsSeq: 0 }) });
    r.link.start();
    r.last().open();
    const first = r.last().sent[0].binary as Uint8Array;
    expect(first[1]).toBe(MODE_RECONNECT);
    expect(r.last().sent).toHaveLength(1);
    // A reply made by another key does not finish the handshake: the connection is dropped.
    const other = new Initiator(MODE_RECONNECT, keypair(), keypair().pub, null).first.slice(FIRST_HEADER, FIRST_HEADER + 48);
    r.last().onmessage?.({ data: other.buffer.slice(other.byteOffset, other.byteOffset + other.byteLength) });
    expect(r.last().closed).toBe(true);
    expect(r.link.ready).toBe(false);
    // Plain text is not kelpie-link either.
    r.advance(60_000);
    r.last().open();
    r.last().onmessage?.({ data: JSON.stringify({ type: "hello" }) });
    expect(r.last().closed).toBe(true);
  });

  it("does nothing without a pairing", () => {
    const r = rig({ token: null });
    r.link.start();
    expect(r.sockets).toHaveLength(0);
  });

  it("answers the daemon's heartbeat and flags an older daemon", () => {
    const r = rig();
    r.link.start();
    r.connect({ api: 16 });
    expect(r.link.facts.outdated).toBe(true);
    r.last().say({ type: "hb", n: 3, server_ts: 5, level: 0 });
    expect(r.last().sent.at(-1)).toEqual({ type: "hb", echo: true, n: 3 });
  });

  it("flags a daemon one level below sending from the phone: level 20 has no chat_send", () => {
    const r = rig();
    r.link.start();
    r.connect({ api: 20 });
    expect(r.link.facts.outdated).toBe(true);
  });

  it("follows what it wants after the hello, resuming a terminal from the frame it holds", () => {
    const r = rig();
    r.setHave({ t1: 7 });
    r.link.start();
    r.link.follow("t1", "term");
    r.link.follow("c1", "chat");
    expect(r.last().sent).toHaveLength(0);
    r.connect();
    const sent = r.last().sent;
    expect(sent.find((m) => m.type === "sub")).toMatchObject({ id: "t1", have_rev: 7, focus: true });
    expect(sent.find((m) => m.type === "chat_sub")).toMatchObject({ id: "c1", since_seq: null });
  });

  it("asks a chat for its log again when the daemon says it stopped feeding it", () => {
    const r = rig();
    r.link.start();
    r.link.follow("c1", "chat");
    r.connect();
    const before = r.last().sent.filter((m) => m.type === "chat_sub").length;
    r.last().say({ type: "chat_resync", id: "c1" });
    expect(r.last().sent.filter((m) => m.type === "chat_sub").length).toBe(before + 1);
    expect(r.log).toContain("resync c1");
  });
});

describe("Link reconnecting", () => {
  it("first retry is within 200 ms, then backs off with jitter up to 30 s", () => {
    const r = rig({ random: 0.999 });
    r.link.start();
    r.connect();
    r.advance(6000);
    r.last().die();
    expect(r.link.facts.attempt).toBe(1);
    expect(r.link.facts.nextAt! - r.now()).toBeLessThan(200);
    const delays: number[] = [];
    for (let i = 0; i < 8; i++) {
      r.advance(r.link.facts.nextAt! - r.now());
      r.last().die();
      delays.push(r.link.facts.nextAt! - r.now());
    }
    expect(delays[0]).toBeLessThan(1000);
    expect(delays[2]).toBeLessThan(4000);
    expect(delays[2]).toBeGreaterThan(2000);
    expect(Math.max(...delays)).toBeLessThanOrEqual(30_000);
    expect(r.link.facts.attempt).toBe(9);
  });

  it("a connection that lasted starts the failure count over", () => {
    const r = rig();
    r.link.start();
    r.connect();
    r.last().die();
    r.advance(1000);
    r.connect();
    r.advance(10_000);
    r.last().die();
    expect(r.link.facts.attempt).toBe(1);
  });

  it("is 'reconnecting' as soon as the socket closes, and 'live' again after the next hello without a reload", () => {
    const r = rig();
    r.link.start();
    r.connect();
    r.advance(6000);
    expect(deriveConn(r.link.facts, r.now()).kind).toBe("live");
    r.last().die();
    expect(deriveConn(r.link.facts, r.now())).toMatchObject({ kind: "reconnecting", attempt: 1 });
    r.advance(300);
    expect(r.sockets).toHaveLength(2);
    r.connect();
    expect(deriveConn(r.link.facts, r.now()).kind).toBe("live");
    expect(r.log.filter((l) => l.startsWith("hello"))).toEqual(["hello e1", "hello e1"]);
  });

  it("notices a restarted computer by its epoch", () => {
    const r = rig();
    r.link.start();
    r.connect();
    r.last().die();
    r.advance(300);
    r.connect({ daemon_epoch: "e2" });
    expect(r.log.at(-1)).toBe("hello e2 changed");
    expect(deriveConn(r.link.facts, r.now()).kind).toBe("restarted");
  });

  it.each([
    [4408, true],
    [4410, true],
    [4411, true],
    [1006, true],
    [4401, true],
    [4409, false],
  ])("close code %i in a session reconnects by itself: %s", (code, again) => {
    const r = rig();
    r.link.start();
    r.connect();
    r.last().die(code);
    r.advance(1500);
    expect(r.sockets.length).toBe(again ? 2 : 1);
    expect(r.log).not.toContain("unpaired");
  });

  it("only the computer's sealed word forgets the pairing, and nothing is tried after it", () => {
    const r = rig();
    r.link.start();
    r.connect();
    r.last().say({ type: "revoked" });
    expect(r.log).toContain("unpaired");
    expect(deriveConn(r.link.facts, r.now()).kind).toBe("unpaired");
    r.last().die(4401);
    r.advance(120_000);
    r.link.kick();
    expect(r.sockets).toHaveLength(1);
  });

  /** A connection whose handshake has not finished: `ready` once the computer answered. */
  const pendingCodec = (): Codec => {
    let ready = false;
    return {
      open: () => null,
      ready: () => ready,
      receive: (d) => {
        ready = true;
        return [JSON.parse(d as string)];
      },
      send: (m) => [JSON.stringify(m)],
    };
  };

  it.each([
    [4401, "unknown"],
    [4402, "unconfirmed"],
  ])("close code %i before a session keeps the key, says so, and asks again now and then", (code, why) => {
    const r = rig({ codec: pendingCodec });
    r.link.start();
    r.last().open();
    r.last().die(code);
    expect(r.log).not.toContain("unpaired");
    expect(deriveConn(r.link.facts, r.now())).toEqual({ kind: "not_paired_here", why });
    // Not a hammering: a failed attempt counts against the phone's address on the computer (10 a minute).
    r.advance(59_000);
    expect(r.sockets).toHaveLength(1);
    r.advance(2000);
    expect(r.sockets).toHaveLength(2);
    // The computer knows it after all: everything is as it was.
    r.last().open();
    r.last().say({ type: "hello", ...hello() });
    expect(deriveConn(r.link.facts, r.now()).kind).not.toBe("not_paired_here");
    expect(r.log).not.toContain("unpaired");
  });

  it("says this computer may have been reset after a few connections closed with no reply while it answers otherwise", async () => {
    const r = rig({ codec: pendingCodec });
    r.link.start();
    const closeWithNoReply = async () => {
      r.last().open();
      r.last().die(1006);
      await Promise.resolve();
      await Promise.resolve();
    };
    const nextConnection = () => {
      const count = r.sockets.length;
      while (r.sockets.length === count) r.advance(100);
    };
    await closeWithNoReply();
    nextConnection();
    await closeWithNoReply();
    expect(deriveConn(r.link.facts, r.now()).kind).toBe("reconnecting");
    nextConnection();
    await closeWithNoReply();
    expect(deriveConn(r.link.facts, r.now())).toEqual({ kind: "not_paired_here", why: "reset" });
    expect(r.log).not.toContain("unpaired");
    // Once a minute from now on, and the key stays through any number of them.
    const before = r.sockets.length;
    r.advance(59_000);
    expect(r.sockets).toHaveLength(before);
    r.advance(2000);
    expect(r.sockets).toHaveLength(before + 1);
    await closeWithNoReply();
    expect(deriveConn(r.link.facts, r.now())).toEqual({ kind: "not_paired_here", why: "reset" });
    // It answers again: all is as it was.
    r.advance(61_000);
    r.last().open();
    r.last().say({ type: "hello", ...hello() });
    expect(deriveConn(r.link.facts, r.now()).kind).not.toBe("not_paired_here");
    expect(r.log).not.toContain("unpaired");
  });

  it("a session whose hello never came is not a computer that does not answer", () => {
    const r = rig({ codec: pendingCodec });
    r.link.start();
    for (let i = 0; i < 5; i++) {
      r.last().open();
      r.last().say({ type: "nothing-yet" });
      const count = r.sockets.length;
      while (r.sockets.length === count) r.advance(500);
    }
    expect(deriveConn(r.link.facts, r.now()).kind).not.toBe("not_paired_here");
  });

  it("stops saying the computer was reset once it does not answer at all", async () => {
    const r = rig({ codec: pendingCodec });
    r.link.start();
    const close = async () => {
      r.last().open();
      r.last().die(1006);
      await Promise.resolve();
      await Promise.resolve();
      const count = r.sockets.length;
      while (r.sockets.length === count) r.advance(500);
    };
    for (let i = 0; i < 3; i++) await close();
    expect(deriveConn(r.link.facts, r.now())).toEqual({ kind: "not_paired_here", why: "reset" });
    r.setReach("unreachable");
    r.advance(11_000);
    for (let i = 0; i < 3; i++) await close();
    expect(deriveConn(r.link.facts, r.now()).kind).not.toBe("not_paired_here");
  });

  it("forgetting and pairing again starts the count over", async () => {
    const r = rig({ codec: pendingCodec });
    r.link.start();
    for (let i = 0; i < 3; i++) {
      r.last().open();
      r.last().die(1006);
      await Promise.resolve();
      await Promise.resolve();
      const count = r.sockets.length;
      while (r.sockets.length === count) r.advance(500);
    }
    r.link.stop();
    r.link.start();
    r.last().open();
    r.last().die(1006);
    expect(deriveConn(r.link.facts, r.now()).kind).not.toBe("not_paired_here");
  });

  it("does not say so when the server cannot be reached, or when the connection never opened, or for a stated close", async () => {
    for (const [reach, opened, code] of [["unreachable", true, 1006], ["reachable", false, 1006], ["reachable", true, 4503]] as const) {
      const r = rig({ codec: pendingCodec });
      r.setReach(reach);
      r.link.start();
      for (let i = 0; i < 6; i++) {
        if (opened) r.last().open();
        r.last().die(code);
        await Promise.resolve();
        await Promise.resolve();
        const count = r.sockets.length;
        while (r.sockets.length === count) r.advance(100);
      }
      expect(deriveConn(r.link.facts, r.now()).kind).not.toBe("not_paired_here");
    }
  });

  it("a page looked at asks again at once, and any other answer ends the 'not paired here'", () => {
    const r = rig({ codec: pendingCodec });
    r.link.start();
    r.last().open();
    r.last().die(4401);
    r.advance(3000);
    r.link.kick();
    expect(r.sockets).toHaveLength(2);
    r.last().die(1006);
    expect(deriveConn(r.link.facts, r.now()).kind).not.toBe("not_paired_here");
  });

  it("a replaced connection waits until the page is looked at", () => {
    const r = rig();
    r.link.start();
    r.connect();
    r.last().die(4409);
    r.advance(60_000);
    expect(r.sockets).toHaveLength(1);
    expect(deriveConn(r.link.facts, r.now()).kind).toBe("replaced");
    r.link.kick();
    expect(r.sockets).toHaveLength(2);
  });

  it("shows 'no network' and waits for the network instead of counting down", () => {
    const r = rig();
    r.link.start();
    r.connect();
    r.setOnline(false);
    r.link.setOnline(false);
    expect(deriveConn(r.link.facts, r.now()).kind).toBe("no_network");
    r.advance(120_000);
    expect(r.sockets).toHaveLength(1);
    r.setOnline(true);
    r.link.setOnline(true);
    expect(r.sockets).toHaveLength(2);
  });

  it("tells a captive portal and an unreachable computer apart from a plain reconnect", async () => {
    const r = rig();
    r.setReach("portal");
    r.link.start();
    r.connect();
    r.last().die();
    await Promise.resolve();
    expect(deriveConn(r.link.facts, r.now()).kind).toBe("sign_in");

    const q = rig();
    q.setReach("unreachable");
    q.link.start();
    q.connect();
    q.last().die();
    q.advance(500);
    q.last().die();
    await Promise.resolve();
    expect(deriveConn(q.link.facts, q.now())).toMatchObject({ kind: "computer_offline" });
  });

  it("a page that is shown again connects at once, but not more than once every 2 s", () => {
    const r = rig({ random: 0.999 });
    r.link.start();
    r.connect();
    r.last().die();
    r.advance(100);
    r.last().die();
    const n = r.sockets.length;
    r.advance(2500);
    const afterBackoff = r.sockets.length;
    expect(afterBackoff).toBeGreaterThanOrEqual(n);
    r.last().die();
    r.advance(2100);
    const before = r.sockets.length;
    r.link.kick();
    r.link.kick();
    expect(r.sockets.length).toBeLessThanOrEqual(before + 1);
  });
});

describe("Link after a restart", () => {
  it("holds input until what is followed has been sent again", () => {
    const r = rig();
    r.link.start();
    r.link.follow("t1", "term");
    r.connect();
    r.last().die();
    r.advance(300);
    r.connect({ daemon_epoch: "e2" });
    expect(deriveConn(r.link.facts, r.now())).toEqual({ kind: "restarted", resyncing: true });
    expect(canAct(deriveConn(r.link.facts, r.now()))).toBe(false);
    r.last().say({ type: "screen", id: "t1", rev: 1, base: null, shift: 0, rows: [], height: 1, cursor: null, prompt: null, waiting_since: null, title: null, server_ts: r.now(), level: 0 });
    expect(canAct(deriveConn(r.link.facts, r.now()))).toBe(true);
  });

  it("does not hold input when nothing is followed, or when the first state never comes", () => {
    const r = rig();
    r.link.start();
    r.connect();
    r.last().die();
    r.advance(300);
    r.connect({ daemon_epoch: "e2" });
    expect(canAct(deriveConn(r.link.facts, r.now()))).toBe(true);

    const q = rig();
    q.link.start();
    q.link.follow("t1", "term");
    q.connect();
    q.last().die();
    q.advance(300);
    q.connect({ daemon_epoch: "e2" });
    expect(canAct(deriveConn(q.link.facts, q.now()))).toBe(false);
    q.advance(5100);
    expect(canAct(deriveConn(q.link.facts, q.now()))).toBe(true);
  });

  it("a restart that happened while the page was hidden holds input until the page has followed again", () => {
    const r = rig();
    r.link.start();
    r.link.follow("t1", "term");
    r.connect();
    r.link.setHidden(true);
    r.last().die();
    r.advance(300);
    r.connect({ daemon_epoch: "e2" });
    // Nothing was followed again yet (the page is hidden): the gate is armed all the same.
    expect(r.last().sent.some((m) => m.type === "sub")).toBe(false);
    expect(deriveConn(r.link.facts, r.now())).toEqual({ kind: "restarted", resyncing: true });
    expect(canAct(deriveConn(r.link.facts, r.now()))).toBe(false);
    // A long time hidden does not open it either.
    r.advance(60_000);
    expect(canAct(deriveConn(r.link.facts, r.now()))).toBe(false);
    r.link.setHidden(false);
    expect(r.last().sent.some((m) => m.type === "sub" && m.id === "t1")).toBe(true);
    expect(canAct(deriveConn(r.link.facts, r.now()))).toBe(false);
    r.last().say({ type: "screen", id: "t1", rev: 1, base: null, shift: 0, rows: [], height: 1, cursor: null, prompt: null, waiting_since: null, title: null, server_ts: r.now(), level: 0 });
    expect(canAct(deriveConn(r.link.facts, r.now()))).toBe(true);
  });

  it("hiding the page again inside the resync window does not let the gate open early", () => {
    const r = rig();
    r.link.start();
    r.link.follow("t1", "term");
    r.connect();
    r.last().die();
    r.advance(300);
    r.connect({ daemon_epoch: "e2" });
    expect(canAct(deriveConn(r.link.facts, r.now()))).toBe(false);
    r.advance(3000);
    r.link.setHidden(true);
    // The 5 s limit does not run while hidden.
    r.advance(30_000);
    expect(canAct(deriveConn(r.link.facts, r.now()))).toBe(false);
    r.link.setHidden(false);
    // Showing the page checks the connection at once; the computer answers.
    const ping = r.last().sent.filter((m) => m.type === "hb").at(-1);
    r.last().say({ type: "hb", echo: true, n: ping.n, server_ts: r.now() });
    r.advance(4000);
    expect(canAct(deriveConn(r.link.facts, r.now()))).toBe(false);
    r.advance(1500);
    expect(canAct(deriveConn(r.link.facts, r.now()))).toBe(true);
  });

  it("a session that is gone does not hold it", async () => {
    const r = rig();
    r.link.start();
    r.link.follow("t1", "term");
    r.connect();
    r.last().die();
    r.advance(300);
    r.connect({ daemon_epoch: "e2" });
    const sub = r.last().sent.find((m) => m.type === "sub");
    r.last().say({ type: "error", req: sub.req, code: "no_session", message: "gone" });
    await Promise.resolve();
    await Promise.resolve();
    expect(canAct(deriveConn(r.link.facts, r.now()))).toBe(true);
  });
});

describe("Link liveness", () => {
  it("asks after 15 s of silence and throws the connection away when nothing answers in 3 s", () => {
    const r = rig();
    r.link.start();
    r.connect();
    const first = r.last();
    r.advance(PING_AFTER_MS + 2500);
    expect(first.sent.at(-1)).toMatchObject({ type: "hb" });
    expect(first.closed).toBe(false);
    r.advance(PROBE_MS + 100);
    expect(first.closed).toBe(true);
    expect(deriveConn(r.link.facts, r.now()).kind).toBe("reconnecting");
    r.advance(300);
    expect(r.sockets).toHaveLength(2);
  });

  it("an answer to the probe keeps the connection and measures the round trip", () => {
    const r = rig();
    r.link.start();
    r.connect();
    r.advance(PING_AFTER_MS + 2500);
    const ping = r.last().sent.at(-1);
    r.advance(1200);
    r.last().say({ type: "hb", echo: true, n: ping.n, server_ts: 1 });
    expect(r.link.facts.rttMs).toBeGreaterThanOrEqual(1200);
    r.advance(PROBE_MS + 100);
    expect(r.sockets).toHaveLength(1);
    expect(deriveConn(r.link.facts, r.now())).toMatchObject({ kind: "degraded", why: "slow" });
  });

  it("an unanswered probe makes the state 'degraded' before it gives up", () => {
    const r = rig();
    r.link.start();
    r.connect();
    r.advance(PING_AFTER_MS + 2100);
    r.advance(900);
    expect(deriveConn(r.link.facts, r.now())).toMatchObject({ kind: "degraded", why: "late" });
  });

  it("checks at once when the page is shown again after a quiet time", () => {
    const r = rig();
    r.link.start();
    r.connect();
    r.advance(5000);
    r.link.kick();
    expect(r.last().sent.at(-1)).toMatchObject({ type: "hb" });
  });

  it("a hidden page lets go of its subscriptions and takes them up again on return", () => {
    const r = rig();
    r.setHave({ t1: 4 });
    r.link.start();
    r.link.follow("t1", "term");
    r.connect();
    r.link.setHidden(true);
    expect(r.last().sent.at(-1)).toEqual({ type: "unsub", id: "t1" });
    const count = r.last().sent.length;
    r.advance(120_000);
    expect(r.last().sent.length).toBe(count);
    r.link.setHidden(false);
    expect(r.last().sent.some((m) => m.type === "sub" && m.have_rev === 4 && m !== r.last().sent[1])).toBe(true);
  });

  it("shows the computer's load as 'busy' and a late frame as 'slow'", () => {
    const r = rig();
    r.link.start();
    r.connect();
    r.last().say({ type: "load.status", level: 2, reason: ["cpu"], since: 1, server_ts: r.now() });
    expect(deriveConn(r.link.facts, r.now())).toMatchObject({ kind: "busy", level: 2, reasons: ["cpu"] });
  });
});

describe("Link requests: nothing is queued while offline", () => {
  let r: ReturnType<typeof rig>;
  beforeEach(() => {
    r = rig();
    r.link.start();
  });

  it("refuses at once, without sending, when the connection is not up", async () => {
    expect(await r.link.input("t1", "hi")).toEqual({ t: "offline" });
    r.connect();
    r.last().die();
    expect(await r.link.key("t1", "enter")).toEqual({ t: "offline" });
    r.advance(300);
    r.connect();
    expect(r.last().sent.filter((m) => m.type === "key" || m.type === "input")).toHaveLength(0);
  });

  it("carries the epoch on writes and matches replies by req", async () => {
    r.connect();
    const p = r.link.input("t1", "hi");
    const sent = r.last().sent.at(-1);
    expect(sent).toMatchObject({ type: "input", id: "t1", text: "hi", enter: true, epoch: "e1" });
    r.last().say({ type: "ack", req: sent.req, entered: false });
    expect(await p).toEqual({ t: "ack", entered: false, result: undefined });
    const q = r.link.key("t1", "x");
    const k = r.last().sent.at(-1);
    r.last().say({ type: "error", req: k.req, code: "bad_request", message: "unknown key" });
    expect(await q).toMatchObject({ t: "error", code: "bad_request" });
  });

  it("an answer lost to a drop is sent once more after the reconnect, and only once", async () => {
    r.connect();
    let out: Outcome | null = null;
    void r.link.answer("t1", 99, { text: "1" }, false).then((o) => (out = o));
    expect(r.last().sent.at(-1)).toMatchObject({ type: "answer", waiting_since: 99, text: "1", enter: false });
    r.last().die();
    await Promise.resolve();
    expect(out).toBeNull();
    r.advance(300);
    r.connect();
    const again = r.last().sent.filter((m) => m.type === "answer");
    expect(again).toHaveLength(1);
    // The repeat is recognised by the daemon as this device's own answer.
    r.last().say({ type: "error", req: again[0].req, code: "dup", reason: "answered", message: "x", result: "dup" });
    await Promise.resolve();
    expect(out).toMatchObject({ t: "error", result: "dup" });
    expect(r.sockets.flatMap((s) => s.sent).filter((m) => m.type === "answer")).toHaveLength(2);
  });

  it("a second loss is 'unknown' and never a third send", async () => {
    r.connect();
    let out: Outcome | null = null;
    void r.link.chatAnswer("c1", "r1", "{}").then((o) => (out = o));
    r.last().die();
    r.advance(300);
    r.connect();
    r.last().die();
    await Promise.resolve();
    await Promise.resolve();
    expect(out).toEqual({ t: "unknown" });
    r.advance(300);
    r.connect();
    expect(r.sockets.flatMap((s) => s.sent).filter((m) => m.type === "chat_answer")).toHaveLength(2);
  });

  it("an answer is not repeated after a minute, or into another run of the daemon", async () => {
    r.connect();
    let late: Outcome | null = null;
    void r.link.answer("t1", 5, { text: "1" }, false).then((o) => (late = o));
    r.last().die();
    r.advance(RETRY_WINDOW_MS + 1000);
    await Promise.resolve();
    expect(late).toEqual({ t: "unknown" });

    const q = rig();
    q.link.start();
    q.connect();
    let restarted: Outcome | null = null;
    void q.link.answer("t1", 5, { text: "1" }, false).then((o) => (restarted = o));
    q.last().die();
    q.advance(300);
    q.connect({ daemon_epoch: "e2" });
    await Promise.resolve();
    expect(restarted).toEqual({ t: "unknown" });
    expect(q.sockets.flatMap((s) => s.sent).filter((m) => m.type === "answer")).toHaveLength(1);
  });

  it("a chat message lost to a drop is sent once more under the same cid, and the daemon's dup is the answer", async () => {
    r.connect();
    let out: Outcome | null = null;
    void r.link.chatSend("c1", "cid-1", "hello").then((o) => (out = o));
    expect(r.last().sent.at(-1)).toMatchObject({ type: "chat_send", id: "c1", cid: "cid-1", text: "hello", epoch: "e1" });
    r.last().die();
    await Promise.resolve();
    expect(out).toBeNull();
    r.advance(300);
    r.connect();
    const again = r.last().sent.filter((m) => m.type === "chat_send");
    expect(again).toHaveLength(1);
    expect(again[0].cid).toBe("cid-1");
    r.last().say({ type: "ack", req: again[0].req, result: "dup", qid: "q4", state: "queued" });
    await Promise.resolve();
    expect(out).toMatchObject({ t: "ack", result: "dup", qid: "q4", state: "queued" });
    expect(r.sockets.flatMap((s) => s.sent).filter((m) => m.type === "chat_send")).toHaveLength(2);
  });

  it("a chat message is not sent again into another run of the daemon, or after a second loss", async () => {
    r.connect();
    let restarted: Outcome | null = null;
    void r.link.chatSend("c1", "cid-1", "hello").then((o) => (restarted = o));
    r.last().die();
    r.advance(300);
    r.connect({ daemon_epoch: "e2" });
    await Promise.resolve();
    expect(restarted).toEqual({ t: "unknown" });
    expect(r.sockets.flatMap((s) => s.sent).filter((m) => m.type === "chat_send")).toHaveLength(1);

    const q = rig();
    q.link.start();
    q.connect();
    let twice: Outcome | null = null;
    void q.link.chatSend("c1", "cid-2", "again").then((o) => (twice = o));
    q.last().die();
    q.advance(300);
    q.connect();
    q.last().die();
    await Promise.resolve();
    await Promise.resolve();
    expect(twice).toEqual({ t: "unknown" });
    q.advance(300);
    q.connect();
    expect(q.sockets.flatMap((s) => s.sent).filter((m) => m.type === "chat_send")).toHaveLength(2);
  });

  it("tells the app about the queue of a chat", () => {
    r.connect();
    r.last().say({ type: "chat_queue", id: "c1", items: [], paused: false, held: false, in_flight: false });
    expect(r.queues.map((m) => m.id)).toEqual(["c1"]);
  });

  it("typing is never retried: it is unknown, and the draft stays with the person", async () => {
    r.connect();
    let out: Outcome | null = null;
    void r.link.input("t1", "rm", false).then((o) => (out = o));
    r.last().die();
    await Promise.resolve();
    expect(out).toEqual({ t: "unknown" });
    r.advance(300);
    r.connect();
    expect(r.sockets.flatMap((s) => s.sent).filter((m) => m.type === "input")).toHaveLength(1);
  });
});
