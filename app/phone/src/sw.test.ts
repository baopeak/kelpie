import { afterEach, describe, expect, it, vi } from "vitest";

const ORIGIN = "https://desk.tail1234.ts.net:8443";
const KEPT = "the kept shell";

/** The worker, loaded fresh on a scope of its own: what it does with one request. */
async function ask(request: { url: string; mode: string; method?: string }, network: () => Promise<Response>) {
  vi.resetModules();
  const handlers: Record<string, (e: unknown) => void> = {};
  vi.stubGlobal("self", { addEventListener: (type: string, h: (e: unknown) => void) => (handlers[type] = h), location: { origin: ORIGIN }, skipWaiting: async () => {}, clients: { claim: async () => {} } });
  vi.stubGlobal("__PRECACHE__", ["/", "/icon.svg"]);
  vi.stubGlobal("__VERSION__", "test");
  vi.stubGlobal("caches", { open: async () => ({ match: async () => new Response(KEPT), put: async () => {}, addAll: async () => {} }), keys: async () => [] });
  vi.stubGlobal("fetch", vi.fn(network));
  await import("./sw");
  let answered: Promise<Response> | null = null;
  handlers.fetch({ request: { method: "GET", ...request, url: ORIGIN + request.url }, respondWith: (p: Promise<Response>) => (answered = p) });
  return answered as Promise<Response> | null;
}

const page = { url: "/", mode: "navigate" };

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

  it("leaves the socket and the reachability request to the network", async () => {
    expect(await ask({ url: "/link", mode: "websocket" }, async () => new Response(""))).toBeNull();
    expect(await ask({ url: "/manifest.webmanifest?probe=1", mode: "cors" }, async () => new Response(""))).toBeNull();
    expect(await ask({ url: "/", mode: "navigate", method: "POST" }, async () => new Response(""))).toBeNull();
  });
});
