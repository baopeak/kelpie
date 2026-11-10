import { afterEach, describe, expect, it, vi } from "vitest";
import { ShellReload, watchWorker, type ReloadEnv } from "./shell-reload";

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

  it("leaves the socket, the reachability request and what the origin says it serves to the network", async () => {
    expect(await ask({ url: "/link", mode: "websocket" }, async () => new Response(""))).toBeNull();
    expect(await ask({ url: "/manifest.webmanifest?probe=1", mode: "cors" }, async () => new Response(""))).toBeNull();
    expect(await ask({ url: "/version.json", mode: "cors" }, async () => new Response("{}"))).toBeNull();
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
    const told: unknown[] = [];
    const page = { url: ORIGIN + "/", focus: async () => {}, postMessage: (m: unknown) => void told.push(m) };
    vi.stubGlobal("self", { addEventListener: (type: string, h: (e: unknown) => void) => (handlers[type] = h), location: { origin: ORIGIN }, skipWaiting: async () => {}, clients: { claim: async () => {}, matchAll: async () => [page] } });
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
    return { open, message, csp, told };
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

  it("tells open pages once the kept CSP changed, and only then, since a page keeps the CSP it loaded with", async () => {
    const w = await shellWorker(async () => new Response("<html>v1</html>", { headers: { "content-security-policy": "connect-src 'self' wss://relay.example" } }));
    await w.message({ type: "kelpie-refresh-shell" });
    expect(w.told).toEqual([{ type: "kelpie-shell-changed" }]);
    // Fetched again with the same headers: nothing changed, nobody is told, so a page that reloads for it cannot do so again and again.
    await w.message({ type: "kelpie-refresh-shell" });
    await w.open();
    expect(w.told).toHaveLength(1);
  });

  it("tells nobody when the page answered is another version's, or nothing answered", async () => {
    const other = await shellWorker(async () => new Response("<html>v2</html>", { headers: { "content-security-policy": "connect-src 'self' wss://relay.example" } }));
    await other.message({ type: "kelpie-refresh-shell" });
    expect(other.told).toEqual([]);
    const offline = await shellWorker(() => Promise.reject(new TypeError("offline")));
    await offline.message({ type: "kelpie-refresh-shell" });
    expect(offline.told).toEqual([]);
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
/** Cache storage as a browser keeps it: caches in the order they were made, each a map of paths. */
class Store {
  caches = new Map<string, Map<string, Response>>();
  private path = (k: string | { url: string }) => (typeof k === "string" ? k : new URL(k.url).pathname);
  open = async (name: string) => {
    let c = this.caches.get(name);
    if (!c) this.caches.set(name, (c = new Map()));
    const kept = c;
    return {
      match: async (k: string | { url: string }) => kept.get(this.path(k))?.clone(),
      put: async (k: string | { url: string }, v: Response) => void kept.set(this.path(k), v),
      addAll: async (paths: string[]) => paths.forEach((p) => kept.set(p, new Response(`${name}${p}`))),
      delete: async (k: string | { url: string }) => kept.delete(this.path(k)),
    };
  };
  keys = async () => [...this.caches.keys()];
  delete = async (name: string) => this.caches.delete(name);
  match = async (k: string | { url: string }) => {
    for (const c of this.caches.values()) {
      const hit = c.get(this.path(k));
      if (hit) return hit.clone();
    }
    return undefined;
  };
  versions = () => [...this.caches.keys()].filter((n) => n.startsWith("kelpie-phone-"));
}

/** One version's worker, installed and activated over `store`, as a browser does when a new version is deployed. */
async function deploy(store: Store, version: string, assets: string[], network: () => Promise<Response> = async () => new Response("from the network")) {
  vi.resetModules();
  const handlers: Record<string, (e: unknown) => void> = {};
  vi.stubGlobal("self", { addEventListener: (type: string, h: (e: unknown) => void) => (handlers[type] = h), location: { origin: ORIGIN }, skipWaiting: async () => {}, clients: { claim: async () => {} } });
  vi.stubGlobal("__PRECACHE__", ["/", ...assets]);
  vi.stubGlobal("__VERSION__", version);
  vi.stubGlobal("caches", store);
  vi.stubGlobal("fetch", vi.fn(network));
  await import("./sw");
  for (const type of ["install", "activate"]) {
    let done: Promise<unknown> = Promise.resolve();
    handlers[type]({ waitUntil: (p: Promise<unknown>) => (done = p) });
    await done;
  }
  const get = async (path: string) => {
    let answered: Promise<Response> | null = null;
    handlers.fetch({ request: { method: "GET", url: ORIGIN + path, mode: "cors" }, respondWith: (p: Promise<Response>) => (answered = p), waitUntil: () => {} });
    const got = answered as Promise<Response> | null;
    return got ? (await got).text() : null;
  };
  return { get };
}

describe("a new version deployed under an open page", () => {
  afterEach(() => vi.unstubAllGlobals());

  it("serves the previous version's chunks from its kept files, which the origin no longer has", async () => {
    const store = new Store();
    await deploy(store, "aaa", ["/assets/index-a.js", "/assets/chat-a.js"]);
    const b = await deploy(store, "bbb", ["/assets/index-b.js"], async () => new Response("gone", { status: 404 }));
    // A page of version A, still open, loads its chat chunk lazily.
    expect(await b.get("/assets/chat-a.js")).toBe("kelpie-phone-aaa/assets/chat-a.js");
    expect(await b.get("/assets/index-b.js")).toBe("kelpie-phone-bbb/assets/index-b.js");
    expect(fetch).not.toHaveBeenCalled();
  });

  it("keeps the previous version's files through the next activation and lets them go at the one after", async () => {
    const store = new Store();
    await deploy(store, "aaa", ["/assets/a.js"]);
    await deploy(store, "bbb", ["/assets/b.js"]);
    expect(store.versions()).toEqual(["kelpie-phone-aaa", "kelpie-phone-bbb"]);
    await deploy(store, "ccc", ["/assets/c.js"]);
    expect(store.versions()).toEqual(["kelpie-phone-bbb", "kelpie-phone-ccc"]);
  });

  it("goes by the order it wrote down, not by the names, which are hashes", async () => {
    const store = new Store();
    await deploy(store, "zzz", ["/assets/z.js"]);
    await deploy(store, "mmm", ["/assets/m.js"]);
    await deploy(store, "aaa", ["/assets/a.js"]);
    expect(store.versions().sort()).toEqual(["kelpie-phone-aaa", "kelpie-phone-mmm"]);
  });

  it("keeps the one version a worker from before this kept, the first time", async () => {
    const store = new Store();
    // What the earlier worker left: its own cache and no order.
    (await store.open("kelpie-phone-old")).put("/assets/old.js", new Response("old"));
    const now = await deploy(store, "new", ["/assets/new.js"]);
    expect(store.versions().sort()).toEqual(["kelpie-phone-new", "kelpie-phone-old"]);
    expect(await now.get("/assets/old.js")).toBe("old");
  });

  it("fetches an asset no kept version has, and keeps it with this version", async () => {
    const store = new Store();
    const w = await deploy(store, "aaa", []);
    expect(await w.get("/assets/late.js")).toBe("from the network");
    expect(await (await store.open("kelpie-phone-aaa")).match("/assets/late.js")).toBeDefined();
  });
});

describe("a page a new version's worker took over", () => {
  /** The page's reload record lives in its tab, as in a browser. */
  function page(controller: unknown) {
    let kept: string | null = null;
    let clock = 1_000_000;
    const env: ReloadEnv & { reloads: number } = { reloads: 0, now: () => clock, read: () => kept, write: (v) => void (kept = v), reload: () => void env.reloads++ };
    let changed: (() => void) | null = null;
    const reload = new ShellReload(env);
    watchWorker({ controller, addEventListener: (_type, listener) => void (changed = listener) }, reload);
    return { env, reload, takeOver: () => changed?.(), later: (ms: number) => void (clock += ms) };
  }

  it("reloads once, at the next foreground, not under the person", () => {
    const p = page({});
    p.takeOver();
    expect(p.env.reloads).toBe(0);
    p.reload.visible();
    expect(p.env.reloads).toBe(1);
    p.reload.visible();
    expect(p.env.reloads).toBe(1);
  });

  it("does not reload for the first worker taking a page that had none", () => {
    const p = page(null);
    p.takeOver();
    p.reload.visible();
    expect(p.env.reloads).toBe(0);
    // A later version's worker does.
    p.takeOver();
    p.reload.visible();
    expect(p.env.reloads).toBe(1);
  });

  it("reloads at most once a minute however often workers change", () => {
    const p = page({});
    p.takeOver();
    p.reload.visible();
    p.takeOver();
    p.reload.visible();
    expect(p.env.reloads).toBe(1);
    p.later(60_000);
    p.takeOver();
    p.reload.visible();
    expect(p.env.reloads).toBe(2);
  });
});
