// Keeps the app shell (page, scripts, styles, icon) so that the app opens on a bad network or with the computer
// off, and shows what it last saw. It never touches the socket or the pairing request, and never keeps
// anything the daemon says about sessions: that lives in the page's own cache and is marked as old.
// Registered only on secure origins (main.tsx). The build fills in the file list and a version that changes
// with the files.
//
// It also shows alerts where the system does not do it by itself (iOS 18.4 reads the same payload natively): one notification per
// `tag`, so a repeat takes the place of the last. An alert has no text from a session, only a fixed sentence and a link to open. This
// file stands alone in the build: a classic worker cannot import or export anything.
interface ExtendableEvent extends Event {
  waitUntil(promise: Promise<unknown>): void;
}
interface FetchEvent extends ExtendableEvent {
  request: Request;
  respondWith(response: Promise<Response>): void;
}
interface MessageEvent extends ExtendableEvent {
  data: unknown;
}
interface PushEvent extends ExtendableEvent {
  data: { json(): unknown } | null;
}
interface ClickEvent extends ExtendableEvent {
  notification: { data?: { navigate?: unknown }; close(): void };
}
interface WindowClient {
  url: string;
  focus(): Promise<unknown>;
  postMessage(message: unknown): void;
}
interface Scope {
  addEventListener(type: "install" | "activate" | "pushsubscriptionchange", listener: (e: ExtendableEvent) => void): void;
  addEventListener(type: "fetch", listener: (e: FetchEvent) => void): void;
  addEventListener(type: "push", listener: (e: PushEvent) => void): void;
  addEventListener(type: "notificationclick", listener: (e: ClickEvent) => void): void;
  addEventListener(type: "message", listener: (e: MessageEvent) => void): void;
  skipWaiting(): Promise<void>;
  clients: { claim(): Promise<void>; matchAll(options: { type: "window"; includeUncontrolled: boolean }): Promise<WindowClient[]>; openWindow(url: string): Promise<unknown> };
  registration: { showNotification(title: string, options: { body?: string; tag?: string; icon?: string; data?: unknown }): Promise<void> };
  location: Location;
}

declare const __PRECACHE__: string[];
declare const __VERSION__: string;

export {};

const sw = self as unknown as Scope;
const CACHE = `kelpie-phone-${__VERSION__}`;
/** How long a page request waits for the computer before the kept shell is used instead. */
const NAVIGATION_WAIT_MS = 2500;
/** Where the worker leaves word for the page, which reads it the next time it opens (push.ts). */
const PUSH_FLAGS = "kelpie-push-flags";
const RESUBSCRIBE_MARK = "/resubscribe";
/** Where the page leaves word that this app is paired (pairing-store.ts): it then opens from the kept shell at once, which dials the relay when the
 *  computer's own address does not answer, instead of waiting for that address first. */
const SHELL_FLAGS = "kelpie-shell-flags";
const PAIRED_MARK = "/paired";
/** What the page sends when the relay changed: the kept shell is fetched again for the CSP that goes with it. */
const REFRESH_SHELL = "kelpie-refresh-shell";
/** What open pages are told once the kept shell's CSP changed: a page keeps the CSP it loaded with, so it loads again (shell-reload.ts). */
const SHELL_CHANGED = "kelpie-shell-changed";
const CSP = "content-security-policy";

/** The kept page, fetched again behind the one just served: kept when it is this version's own page, so that what changes is its headers
 *  (the CSP names the relay once it is on). A page of another version waits for its own worker, which keeps its own files with it. */
async function refreshShell(): Promise<void> {
  let changed = false;
  try {
    const cache = await caches.open(CACHE);
    const [kept, fresh] = await Promise.all([cache.match("/"), fetch("/", { cache: "no-store" })]);
    if (!kept || !fresh.ok || fresh.redirected) return;
    const [was, is] = await Promise.all([kept.clone().text(), fresh.clone().text()]);
    if (was !== is) return;
    changed = kept.headers.get(CSP) !== fresh.headers.get(CSP);
    await cache.put("/", fresh);
  } catch {
    /* Out of reach: the next open tries again. */
    return;
  }
  if (!changed) return;
  try {
    for (const page of await sw.clients.matchAll({ type: "window", includeUncontrolled: false })) page.postMessage({ type: SHELL_CHANGED });
  } catch {
    /* No page to tell: the next one opens with the new shell. */
  }
}

sw.addEventListener("message", (e) => {
  if ((e.data as { type?: unknown } | null)?.type === REFRESH_SHELL) e.waitUntil(refreshShell());
});
/** The app's icon, 72 px, in this file. A worker's own requests do not pass through its `fetch` handler, so a path here would go to the computer, which the phone may not be able to reach: the alert would wait for it. */
const ALERT_ICON = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAEgAAABICAMAAABiM0N1AAAAYFBMVEWdVDPrkF4WGyhWNCnx5Nf0so9fVlZ4RTGUcV0oT1gXqpnwx62alpMcK0Hqc1lNPUC8hGBndIHBXC0dhH4TFyMqMULqdDv1qHD9yan+9ejtd0zQaTcsKDIjLUDlbTT9snVGl2y/AAAAIHRSTlP//////////////////////////////////////////3Fe9GAAAAL1SURBVHja7ZbpctsgEIABcciVrKStQRfI7/+W3QUJX+Cg2NMfne5MZFjBp71YQk5vEvIf9B90I1X11abUigRoNl+YScxcAmKzeU4ixsyszCJjVJ6j4HWRRfhFY7JxqvAtKQflSJ5TCFJmwsU2xbH4Zkp5nkq/oQLjkCBZjJ+gpioFDSIdUc8ZSkF2mYZBTMtjJIhZJjEM02KLQFBIsLqhDyTg0Aa+kSqj5FnzoGG4JyEH9SLldBJEAghJ1XXoAgdApBgUdtyQIgdiXQpSG6ihsTChEDE+AaQKQdUGgooh9sSsZScLZooNVO0DAWYmhMxQPLMfrKhykDVT08D6xdzJAl9omil5elIgLKR+esB41CTSZZTu2XM4tmmZkmWUBhHzVMjfB1VzdOPGpyBzVQxSYQftexpRE87CUJ2K0x84574/0w10PbPFZ+25RaVnzV7CQm9idJnZwuYf01P9unBmFXNQ2Pxj8ismZdwL5fyzXvK+ZW7adfXIo3mKybEla8iK7/4NJA9VNA9mllzM2wdiTJkF/IE/hbNdoOgaGMFsJ3wPEqJj7BTNm/cF25B6GKMMNXl22jKgCWURCOCtlC3HkVi8uhhUraAOOJyPLQNpcTSKbgVVpTctgrqOB58kg8x74V3nQaU3bfBt6Uck8ZFrzdchXXKe5Xo2tmfhd8OjrreRWDLJzzU2D6LeHD421jb4CzC6ZP+ZS4KY8knD3EsrbdvCA8M0+LQpVgyS0Hug+9CGc0i+4FxAAXDe0KCXSZB+FGyGXj610/qgrYUHDD9X9fkzsekRxOqN058lAwrAHDyYvOhrthN0vSGnz4KcjOt76W70kXStz4JgB+3PIBBVV6LPgiCssgaR7m69c0Gv3RfBdnHFEQ9qYrlD/VHr5yBHDEl+rEhuQWb+OL4OggneFu4NIIeXD3kDSB8/kHR8GXT88GFyNoj7Lmj1zLlDkF0pvMtaiPWPIAf7bdCa/d+vgbRWmzPBsxeCvQ32x1oT/Sb5h0F/ALdqkPw7JUSOAAAAAElFTkSuQmCC";

sw.addEventListener("install", (e) => {
  e.waitUntil(caches.open(CACHE).then((c) => c.addAll(__PRECACHE__)).then(() => sw.skipWaiting()));
});

/** The versions whose files are kept, oldest first: this one's and the one before it. A page of the previous version may still be open (or
 *  alive in the background) and ask for a chunk it loads lazily, which the origin no longer has; it reloads into this version at its next
 *  foreground (shell-reload.ts). The order is written down, since names are content hashes and say nothing of it. */
const VERSIONS = "kelpie-sw-versions";
const VERSIONS_KEY = "/order";
const KEEP_VERSIONS = 2;

async function keepRecent(): Promise<void> {
  const book = await caches.open(VERSIONS);
  let order: string[] = [];
  try {
    const parsed: unknown = await (await book.match(VERSIONS_KEY))?.json();
    if (Array.isArray(parsed)) order = parsed.filter((n): n is string => typeof n === "string");
  } catch {
    /* none written, or unreadable */
  }
  const names = (await caches.keys()).filter((n) => n.startsWith("kelpie-phone-"));
  // Written by none yet (the worker before this one): the other versions' caches are what came before. That worker kept one.
  if (!order.length) order = names.filter((n) => n !== CACHE);
  order = [...order.filter((n) => n !== CACHE && names.includes(n)), CACHE].slice(-KEEP_VERSIONS);
  await Promise.all(names.filter((n) => !order.includes(n)).map((n) => caches.delete(n)));
  await book.put(VERSIONS_KEY, new Response(JSON.stringify(order), { headers: { "content-type": "application/json" } }));
}

sw.addEventListener("activate", (e) => {
  e.waitUntil(keepRecent().catch(() => {}).then(() => sw.clients.claim()));
});

const withTimeout = (p: Promise<Response>, ms: number) =>
  new Promise<Response>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("slow")), ms);
    p.then(
      (r) => (clearTimeout(timer), resolve(r)),
      (e) => (clearTimeout(timer), reject(e)),
    );
  });

/** What a tap opens: a page of this app, whatever the message says, since only this origin holds the pairing. */
function openTarget(navigate: unknown): string {
  const home = sw.location.origin + "/";
  if (typeof navigate !== "string") return home;
  try {
    return new URL(navigate).origin === sw.location.origin ? navigate : home;
  } catch {
    return home;
  }
}

sw.addEventListener("push", (e) => {
  let alert: { title?: unknown; body?: unknown; tag?: unknown; navigate?: unknown } = {};
  try {
    alert = (e.data?.json() as { notification?: typeof alert })?.notification ?? {};
  } catch {
    /* A browser must show something for every push: it shows the name alone. */
  }
  const text = (v: unknown) => (typeof v === "string" ? v : undefined);
  e.waitUntil(sw.registration.showNotification(text(alert.title) ?? "Kelpie", { body: text(alert.body), tag: text(alert.tag), icon: ALERT_ICON, data: { navigate: alert.navigate } }));
});

sw.addEventListener("notificationclick", (e) => {
  e.notification.close();
  const target = openTarget(e.notification.data?.navigate);
  e.waitUntil(
    sw.clients.matchAll({ type: "window", includeUncontrolled: true }).then(async (open) => {
      const mine = open.find((c) => new URL(c.url).origin === sw.location.origin);
      if (!mine) return sw.clients.openWindow(target);
      // A running app stays as it is and is told what to open (a fragment alone would not reload it anyway).
      mine.postMessage({ type: "kelpie-open", navigate: target });
      return mine.focus();
    }),
  );
});

// The browser ended or replaced the subscription, maybe with the app closed and the computer out of reach: the app renews it when it next opens.
sw.addEventListener("pushsubscriptionchange", (e) => {
  e.waitUntil(caches.open(PUSH_FLAGS).then((c) => c.put(RESUBSCRIBE_MARK, new Response("1"))));
});

sw.addEventListener("fetch", (e) => {
  const req = e.request;
  const url = new URL(req.url);
  if (req.method !== "GET" || url.origin !== sw.location.origin) return;
  // Not ours to keep: the reachability probe (it must see the network), the socket, what the origin says it serves.
  if (url.pathname === "/link" || url.pathname === "/manifest.webmanifest" || url.pathname === "/version.json") return;
  if (req.mode === "navigate") {
    e.respondWith(
      (async () => {
        const shell = await caches.open(CACHE).then((c) => c.match("/"));
        // A paired app opens at once from what it kept, and the page is fetched again behind it. A newer version arrives as a new worker.
        if (shell && (await caches.open(SHELL_FLAGS).then((c) => c.match(PAIRED_MARK)))) {
          e.waitUntil(refreshShell());
          return shell;
        }
        // A 5xx is Tailscale answering for a computer whose Kelpie does not: the kept shell says that better than its error page.
        return withTimeout(fetch(req), NAVIGATION_WAIT_MS)
          .then((r) => {
            if (r.status >= 500) throw new Error("no answer");
            return r;
          })
          .catch(async () => shell ?? fetch(req));
      })(),
    );
    return;
  }
  const asset = url.pathname.startsWith("/assets/");
  if (asset || __PRECACHE__.includes(url.pathname)) {
    e.respondWith(
      caches.open(CACHE).then(async (c) => {
        // An asset's name is its content's hash: whichever kept version has it has the right file, and a page of the previous version finds its own.
        const hit = asset ? await caches.match(req) : await c.match(req);
        if (hit) return hit;
        const fresh = await fetch(req);
        if (fresh.ok) void c.put(req, fresh.clone());
        return fresh;
      }),
    );
  }
});
