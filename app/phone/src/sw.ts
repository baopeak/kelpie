// Keeps the app shell (page, scripts, styles, icon) so that the app opens on a bad network or with the computer
// off, and shows what it last saw. It never touches the socket or the pairing request, and never keeps
// anything the daemon says about sessions: that lives in the page's own cache and is marked as old.
// Registered only on secure origins (main.tsx). The build fills in the file list and a version that changes
// with the files.
interface FetchEvent extends Event {
  request: Request;
  respondWith(response: Promise<Response>): void;
}
interface ExtendableEvent extends Event {
  waitUntil(promise: Promise<unknown>): void;
}
interface Scope {
  addEventListener(type: "install" | "activate", listener: (e: ExtendableEvent) => void): void;
  addEventListener(type: "fetch", listener: (e: FetchEvent) => void): void;
  skipWaiting(): Promise<void>;
  clients: { claim(): Promise<void> };
  location: Location;
}

declare const __PRECACHE__: string[];
declare const __VERSION__: string;

export {};

const sw = self as unknown as Scope;
const CACHE = `kelpie-phone-${__VERSION__}`;
/** How long a page request waits for the computer before the kept shell is used instead. */
const NAVIGATION_WAIT_MS = 2500;

sw.addEventListener("install", (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(__PRECACHE__)).then(() => sw.skipWaiting()));
});

sw.addEventListener("activate", (e) => {
  e.waitUntil(
    caches
      .keys()
      .then((names) => Promise.all(names.filter((n) => n.startsWith("kelpie-phone-") && n !== CACHE).map((n) => caches.delete(n))))
      .then(() => sw.clients.claim()),
  );
});

const withTimeout = (p: Promise<Response>, ms: number) =>
  new Promise<Response>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("slow")), ms);
    p.then(
      (r) => (clearTimeout(timer), resolve(r)),
      (e) => (clearTimeout(timer), reject(e)),
    );
  });

sw.addEventListener("fetch", (e) => {
  const req = e.request;
  const url = new URL(req.url);
  if (req.method !== "GET" || url.origin !== sw.location.origin) return;
  // Not ours to keep: the reachability probe (it must see the network), the socket.
  if (url.pathname === "/link" || url.pathname === "/manifest.webmanifest") return;
  if (req.mode === "navigate") {
    e.respondWith(
      // A 5xx is Tailscale answering for a computer whose Kelpie does not: the kept shell says that better than its error page.
      withTimeout(fetch(req), NAVIGATION_WAIT_MS)
        .then((r) => {
          if (r.status >= 500) throw new Error("no answer");
          return r;
        })
        .catch(async () => (await caches.open(CACHE).then((c) => c.match("/"))) ?? fetch(req)),
    );
    return;
  }
  if (url.pathname.startsWith("/assets/") || __PRECACHE__.includes(url.pathname)) {
    e.respondWith(
      caches.open(CACHE).then(async (c) => {
        const hit = await c.match(req);
        if (hit) return hit;
        const fresh = await fetch(req);
        if (fresh.ok) void c.put(req, fresh.clone());
        return fresh;
      }),
    );
  }
});
