import { afterEach, describe, expect, it, vi } from "vitest";

const ORIGIN = "https://desk.tail1234.ts.net:8443";
const KEPT = "the kept shell";

/** The worker, loaded fresh on a scope of its own: what it does with one request. */
async function ask(request: { url: string; mode: string; method?: string }, network: () => Promise<Response>, paired = false) {
  vi.resetModules();
  const handlers: Record<string, (e: unknown) => void> = {};
  vi.stubGlobal("self", { addEventListener: (type: string, h: (e: unknown) => void) => (handlers[type] = h), location: { origin: ORIGIN }, skipWaiting: async () => {}, clients: { claim: async () => {} } });
  vi.stubGlobal("__PRECACHE__", ["/", "/icon.svg"]);
  vi.stubGlobal("__VERSION__", "test");
  const kept = (name: string) => async () => (name === "kelpie-shell-flags" ? (paired ? new Response("1") : undefined) : new Response(KEPT));
  vi.stubGlobal("caches", { open: async (name: string) => ({ match: kept(name), put: async () => {}, addAll: async () => {} }), keys: async () => [] });
  vi.stubGlobal("fetch", vi.fn(network));
  await import("./sw");
  let answered: Promise<Response> | null = null;
  handlers.fetch({ request: { method: "GET", ...request, url: ORIGIN + request.url }, respondWith: (p: Promise<Response>) => (answered = p), waitUntil: () => {} });
  return answered as Promise<Response> | null;
}

const page = { url: "/", mode: "navigate" };

/** The worker, loaded fresh, with what its alerts reach. */
async function worker(open: { url: string; focus: () => Promise<void>; postMessage: (m: unknown) => void }[] = []) {
  vi.resetModules();
  const handlers: Record<string, (e: unknown) => void> = {};
  const shown: { title: string; options: { body?: string; tag?: string; data?: { navigate?: unknown } } }[] = [];
  const opened: string[] = [];
  const kept: Record<string, Response> = {};
  vi.stubGlobal("self", {
    addEventListener: (type: string, h: (e: unknown) => void) => (handlers[type] = h),
    location: { origin: ORIGIN },
    skipWaiting: async () => {},
    clients: { claim: async () => {}, matchAll: async () => open, openWindow: async (u: string) => void opened.push(u) },
    registration: { showNotification: async (title: string, options: never) => void shown.push({ title, options }) },
  });
  vi.stubGlobal("__PRECACHE__", ["/"]);
  vi.stubGlobal("__VERSION__", "test");
  vi.stubGlobal("caches", { open: async (name: string) => ({ put: async (key: string, value: Response) => void (kept[name + key] = value) }), keys: async () => [] });
  await import("./sw");
  const fire = async (type: string, event: object) => {
    let done: Promise<unknown> = Promise.resolve();
    handlers[type]({ ...event, waitUntil: (p: Promise<unknown>) => (done = p) });
    await done;
  };
  return { fire, shown, opened, kept };
}

const alert = (notification: unknown) => ({ data: { json: () => ({ web_push: 8030, notification }) } });
const LINK = `${ORIGIN}/#s=abc&a=1`;

describe("an alert", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("shows the fixed sentence, under its tag, and keeps the link for the tap", async () => {
    const w = await worker();
    await w.fire("push", alert({ title: "Kelpie", body: "A session needs you", tag: "t1", navigate: LINK }));
    expect(w.shown).toEqual([{ title: "Kelpie", options: { body: "A session needs you", tag: "t1", icon: expect.stringMatching(/^data:image\/png;base64,/), data: { navigate: LINK } } }]);
  });

  it("shows it with no request to anything: the phone may not reach the computer", async () => {
    const w = await worker();
    const network = vi.fn(() => Promise.reject(new TypeError("no network")));
    vi.stubGlobal("fetch", network);
    await w.fire("push", alert({ title: "Kelpie", body: "A session needs you", tag: "t1", navigate: LINK }));
    expect(w.shown).toHaveLength(1);
    expect(network).not.toHaveBeenCalled();
  });

  it("shows something for a message it cannot read: a browser punishes a push with nothing shown", async () => {
    const w = await worker();
    await w.fire("push", { data: { json: () => { throw new Error("not json"); } } });
    await w.fire("push", { data: null });
    await w.fire("push", alert({ title: 7, body: {} }));
    expect(w.shown.map((s) => s.title)).toEqual(["Kelpie", "Kelpie", "Kelpie"]);
    expect(w.shown.every((s) => s.options.body === undefined)).toBe(true);
  });

  it("a tap opens the session when the app is not running", async () => {
    const w = await worker();
    let closed = false;
    await w.fire("notificationclick", { notification: { data: { navigate: LINK }, close: () => (closed = true) } });
    expect(closed).toBe(true);
    expect(w.opened).toEqual([LINK]);
  });

  it("a tap tells a running app what to open, and brings it forward", async () => {
    const told: unknown[] = [];
    let focused = 0;
    const w = await worker([{ url: `${ORIGIN}/`, focus: async () => void focused++, postMessage: (m) => told.push(m) }]);
    await w.fire("notificationclick", { notification: { data: { navigate: LINK }, close: () => {} } });
    expect(w.opened).toEqual([]);
    expect(told).toEqual([{ type: "kelpie-open", navigate: LINK }]);
    expect(focused).toBe(1);
  });

  it("a tap never leaves this origin, whatever the message said", async () => {
    for (const navigate of ["https://evil.example/#s=abc", "javascript:alert(1)", "not a url", 42, undefined]) {
      const w = await worker();
      await w.fire("notificationclick", { notification: { data: { navigate }, close: () => {} } });
      expect(w.opened).toEqual([`${ORIGIN}/`]);
    }
  });

  it("a subscription the browser changed is marked for the next open", async () => {
    const w = await worker();
    await w.fire("pushsubscriptionchange", {});
    expect(Object.keys(w.kept)).toEqual(["kelpie-push-flags/resubscribe"]);
  });
});

describe("opening the app", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("takes the page from the computer when it answers", async () => {
    const r = await ask(page, async () => new Response("fresh"));
    expect(await (await r)!.text()).toBe("fresh");
  });

  it("uses the kept shell when Tailscale answers that nothing is behind it", async () => {
    for (const status of [500, 502, 503, 504]) {
      const r = await ask(page, async () => new Response("bad gateway", { status }));
      const got = (await r)!;
      expect(got.status).toBe(200);
      expect(await got.text()).toBe(KEPT);
    }
  });

  it("uses the kept shell with no network at all", async () => {
    const r = await ask(page, () => Promise.reject(new TypeError("failed to fetch")));
    expect(await (await r)!.text()).toBe(KEPT);
  });

  it("opens a paired app from its kept shell at once, with no wait for the computer's address", async () => {
    // A computer that never answers: the page is there all the same, and the network is asked only behind it.
    const r = await ask(page, () => new Promise<Response>(() => {}), true);
    expect(await (await r)!.text()).toBe(KEPT);
  });

  it("leaves the socket and the reachability request to the network", async () => {
    expect(await ask({ url: "/link", mode: "websocket" }, async () => new Response(""))).toBeNull();
    expect(await ask({ url: "/manifest.webmanifest?probe=1", mode: "cors" }, async () => new Response(""))).toBeNull();
    expect(await ask({ url: "/", mode: "navigate", method: "POST" }, async () => new Response(""))).toBeNull();
  });
});

describe("the kept shell, fetched again behind it", () => {
  afterEach(() => vi.unstubAllGlobals());

  /** The worker with a kept `/` (`body`, `csp`), paired, and a network that answers `fresh`. What it keeps, and what it waits on, are given back. */
  async function shellWorker(fresh: () => Promise<Response>) {
    vi.resetModules();
    const handlers: Record<string, (e: unknown) => void> = {};
    const kept = new Map<string, Response>([["kelpie-phone-test/", new Response("<html>v1</html>", { headers: { "content-security-policy": "connect-src 'self'" } })], ["kelpie-shell-flags/paired", new Response("1")]]);
    vi.stubGlobal("self", { addEventListener: (type: string, h: (e: unknown) => void) => (handlers[type] = h), location: { origin: ORIGIN }, skipWaiting: async () => {}, clients: { claim: async () => {} } });
    vi.stubGlobal("__PRECACHE__", ["/"]);
    vi.stubGlobal("__VERSION__", "test");
    vi.stubGlobal("caches", {
      open: async (name: string) => ({
        match: async (key: string) => kept.get(name + key)?.clone(),
        put: async (key: string, value: Response) => void kept.set(name + key, value),
        addAll: async () => {},
      }),
      keys: async () => [],
    });
    vi.stubGlobal("fetch", vi.fn(fresh));
    await import("./sw");
    const waits: Promise<unknown>[] = [];
    const open = async () => {
      let answered: Promise<Response> | null = null;
      handlers.fetch({ request: { method: "GET", url: ORIGIN + "/", mode: "navigate" }, respondWith: (p: Promise<Response>) => (answered = p), waitUntil: (p: Promise<unknown>) => waits.push(p) });
      const page = await answered!;
      await Promise.all(waits);
      return page;
    };
    const message = async (data: unknown) => {
      handlers.message({ data, waitUntil: (p: Promise<unknown>) => waits.push(p) });
      await Promise.all(waits);
    };
    const csp = () => kept.get("kelpie-phone-test/")?.headers.get("content-security-policy");
    return { open, message, csp };
  }

  it("keeps the page's new headers, so a relay turned on after install is allowed the next time the app opens", async () => {
    const w = await shellWorker(async () => new Response("<html>v1</html>", { headers: { "content-security-policy": "connect-src 'self' https://relay.example wss://relay.example" } }));
    const first = await w.open();
    expect(first.headers.get("content-security-policy")).toBe("connect-src 'self'");
    expect(w.csp()).toContain("wss://relay.example");
    expect((await w.open()).headers.get("content-security-policy")).toContain("wss://relay.example");
  });

  it("refreshes when the page says the relay changed", async () => {
    const w = await shellWorker(async () => new Response("<html>v1</html>", { headers: { "content-security-policy": "connect-src 'self' https://relay.example wss://relay.example" } }));
    await w.message({ type: "kelpie-refresh-shell" });
    expect(w.csp()).toContain("wss://relay.example");
  });

  it("leaves the kept page alone when another version answers, or nothing does", async () => {
    const other = await shellWorker(async () => new Response("<html>v2</html>", { headers: { "content-security-policy": "connect-src 'self' wss://relay.example" } }));
    await other.open();
    expect(other.csp()).toBe("connect-src 'self'");
    const offline = await shellWorker(() => Promise.reject(new TypeError("offline")));
    expect(await (await offline.open()).text()).toBe("<html>v1</html>");
    expect(offline.csp()).toBe("connect-src 'self'");
  });
});