// Keeps the app shell (page, scripts, styles, icon) so that the app opens on a bad network or with the computer
// off, and shows what it last saw. It never touches the socket or the pairing request, and never keeps
// anything the daemon says about sessions: that lives in the page's own cache and is marked as old.
// Registered only on secure origins (main.tsx). The build fills in the file list and a version that changes
// with the files.
//
// It also shows alerts where the system does not do it by itself (iOS 18.4 reads the same payload natively): one notification per
// `tag`, so a repeat takes the place of the last. An alert has no text from a session, only a fixed sentence and a link to open. This
// file stands alone in the build: a classic worker cannot import or export anything.
interface FetchEvent extends Event {
  request: Request;
  respondWith(response: Promise<Response>): void;
}
interface ExtendableEvent extends Event {
  waitUntil(promise: Promise<unknown>): void;
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
  e.waitUntil(sw.registration.showNotification(text(alert.title) ?? "Kelpie", { body: text(alert.body), tag: text(alert.tag), icon: "/icon-192.png", data: { navigate: alert.navigate } }));
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
