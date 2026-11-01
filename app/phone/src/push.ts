// Alerts on this phone: the browser's push subscription and what the computer knows of it. The computer sends each alert itself
// (crates/kelpied/src/notify/web_push.rs), so all this keeps straight is that the two agree: looked at every time the app opens, since
// a browser may end or renew a subscription whenever it likes. Asking for the permission takes a tap; renewing once it is given does not.
import type { Outcome } from "./link";

export type AlertsState =
  /** A page that could have been swapped on the way: no browser gives it push, and neither would the computer. */
  | "plain_http"
  /** iOS gives push only to the installed app. */
  | "install_first"
  | "no_support"
  | "off"
  | "asking"
  | "on"
  /** The person said no, which only the phone's settings can undo. */
  | "blocked"
  | "failed";

export interface AlertsFacts {
  secure: boolean;
  supported: boolean;
  ios: boolean;
  installed: boolean;
  permission: "default" | "granted" | "denied";
}

export interface HeldSubscription {
  endpoint: string;
  p256dh: string;
  auth: string;
  /** The key it was made under, base64url. */
  key: string | null;
  unsubscribe(): Promise<unknown>;
}

export interface AlertsEnv {
  facts(): AlertsFacts;
  requestPermission(): Promise<AlertsFacts["permission"]>;
  current(): Promise<HeldSubscription | null>;
  subscribe(key: string): Promise<HeldSubscription>;
  /** The browser told the service worker the subscription changed, while the app was closed. */
  marked(): Promise<boolean>;
  clearMark(): Promise<void>;
  /** The person turned alerts on on this device. */
  wanted(): boolean;
  setWanted(on: boolean): void;
  lang(): string;
  ask(msg: Record<string, unknown>): Promise<Outcome>;
}

/** What the computer said in its `hello`. */
export interface PushFacts {
  vapid: string | null;
  subscribed: boolean;
}

/** Why this page cannot have alerts at all, if it cannot. On an iPhone's browser tab push is simply missing, so that comes before "no support". */
export function alertsUnavailable(f: AlertsFacts): "plain_http" | "install_first" | "no_support" | null {
  if (!f.secure) return "plain_http";
  if (f.ios && !f.installed) return "install_first";
  return f.supported ? null : "no_support";
}

const errorOf = (o: Outcome): string | null => (o.t === "ack" ? null : o.t === "error" ? o.message : o.t);

export class Alerts {
  state: AlertsState = "off";
  /** What went wrong, for the person. */
  error: string | null = null;
  private busy = false;

  constructor(private env: AlertsEnv, private changed: () => void) {}

  private set(state: AlertsState, error: string | null = null) {
    this.state = state;
    this.error = error;
    this.changed();
  }

  private async register(sub: HeldSubscription): Promise<string | null> {
    return errorOf(await this.env.ask({ type: "push_sub", endpoint: sub.endpoint, p256dh: sub.p256dh, auth: sub.auth, lang: this.env.lang() }));
  }

  /** A new subscription with this browser and the computer. */
  private async renew(key: string): Promise<string | null> {
    const sub = await this.env.subscribe(key);
    const refused = await this.register(sub);
    if (refused) await sub.unsubscribe().catch(() => {});
    return refused;
  }

  /** The tap on "Turn on alerts": the browser only asks for its permission from one. */
  async turnOn() {
    const none = alertsUnavailable(this.env.facts());
    if (none) return this.set(none);
    this.set("asking");
    try {
      const permission = await this.env.requestPermission();
      if (permission !== "granted") return this.set(permission === "denied" ? "blocked" : "off");
      const info = await this.env.ask({ type: "push_info" });
      const failed = errorOf(info);
      if (failed || info.t !== "ack" || !info.vapid) return this.set("failed", failed ?? "no key");
      const held = await this.env.current();
      // A browser refuses a second subscription under another key: one from before a reset of the computer goes first.
      if (held && held.key !== info.vapid) await held.unsubscribe().catch(() => {});
      const refused = await (held && held.key === info.vapid ? this.register(held) : this.renew(info.vapid));
      if (refused) return this.set("failed", refused);
      this.env.setWanted(true);
      await this.env.clearMark();
      this.set("on");
    } catch (e) {
      this.set("failed", e instanceof Error ? e.message : String(e));
    }
  }

  async turnOff() {
    this.env.setWanted(false);
    const held = await this.env.current().catch(() => null);
    await held?.unsubscribe().catch(() => {});
    const failed = errorOf(await this.env.ask({ type: "push_unsub" }));
    this.set("off", failed);
  }

  /** What the computer makes of a test alert: null when its push service took it. */
  async test(): Promise<string | null> {
    return errorOf(await this.env.ask({ type: "push_test" }));
  }

  /** The look every open makes, with what the computer just said. Asks the person nothing: with no permission yet it only shows the button. */
  async recheck(push: PushFacts | null | undefined) {
    const f = this.env.facts();
    const none = alertsUnavailable(f);
    if (none) return this.set(none);
    if (this.busy || !push) return;
    this.busy = true;
    try {
      const held = await this.env.current();
      if (!this.env.wanted()) {
        await held?.unsubscribe().catch(() => {});
        if (push.subscribed) await this.env.ask({ type: "push_unsub" });
        return this.set(f.permission === "denied" ? "blocked" : "off");
      }
      if (f.permission !== "granted") return this.set(f.permission === "denied" ? "blocked" : "off");
      const marked = await this.env.marked();
      if (held && push.subscribed && !marked && held.key === push.vapid) {
        // Said again each time: the language, or an address the browser changed in silence. The computer skips what it already has.
        const refused = await this.register(held);
        return this.set(refused ? "failed" : "on", refused);
      }
      // The browser's subscription, or the computer's, is over: the push service said so, or the browser did.
      const key = push.vapid ?? (await this.keyFromComputer());
      if (!key) return this.set("off");
      await held?.unsubscribe().catch(() => {});
      const refused = await this.renew(key);
      if (refused) return this.set("failed", refused);
      await this.env.clearMark();
      this.set("on");
    } catch {
      // iOS may want a tap for this too: the button is there.
      this.set("off");
    } finally {
      this.busy = false;
    }
  }

  private async keyFromComputer(): Promise<string | null> {
    const info = await this.env.ask({ type: "push_info" });
    return info.t === "ack" ? info.vapid ?? null : null;
  }

  /** The pairing is over: nothing of it should reach this phone any more. */
  async forget() {
    this.env.setWanted(false);
    const held = await this.env.current().catch(() => null);
    await held?.unsubscribe().catch(() => {});
    this.set("off");
  }
}

// ---------- the browser ----------

const FLAGS = "kelpie-push-flags";
const MARK = "/resubscribe";
const WANTED = "kelpie-alerts-on";

const toB64 = (bytes: ArrayBuffer | null) => (bytes ? btoa(String.fromCharCode(...new Uint8Array(bytes))).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "") : "");

function fromB64(text: string): Uint8Array {
  const raw = atob(text.replace(/-/g, "+").replace(/_/g, "/"));
  return Uint8Array.from(raw, (c) => c.charCodeAt(0));
}

function held(sub: PushSubscription): HeldSubscription {
  return { endpoint: sub.endpoint, p256dh: toB64(sub.getKey("p256dh")), auth: toB64(sub.getKey("auth")), key: toB64(sub.options.applicationServerKey) || null, unsubscribe: () => sub.unsubscribe() };
}

/** The worker's registration, unless it never comes: one that failed to install leaves `ready` waiting for ever. */
async function registration(): Promise<ServiceWorkerRegistration> {
  const failed = new Promise<never>((_, reject) => setTimeout(() => reject(new Error("the app's service worker did not start")), 10_000));
  return Promise.race([navigator.serviceWorker.ready, failed]);
}

export function browserAlerts(env: Pick<AlertsEnv, "lang" | "ask">, page: { ios: boolean; installed: () => boolean; store: { get(k: string): string | null; set(k: string, v: string): void; del(k: string): void } }): AlertsEnv {
  return {
    ...env,
    facts: () => ({
      secure: window.isSecureContext,
      supported: "serviceWorker" in navigator && "PushManager" in window && "Notification" in window,
      ios: page.ios,
      installed: page.installed(),
      permission: "Notification" in window ? Notification.permission : "default",
    }),
    requestPermission: () => Notification.requestPermission(),
    current: async () => {
      const sub = await (await registration()).pushManager.getSubscription();
      return sub ? held(sub) : null;
    },
    subscribe: async (key) => held(await (await registration()).pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: fromB64(key) as BufferSource })),
    marked: async () => !!(await (await caches.open(FLAGS)).match(MARK)),
    clearMark: async () => void (await (await caches.open(FLAGS)).delete(MARK)),
    wanted: () => page.store.get(WANTED) === "1",
    setWanted: (on) => (on ? page.store.set(WANTED, "1") : page.store.del(WANTED)),
  };
}

/** A session's link as it comes from an alert: the session, and whether it was waiting on the person when the alert left. */
export function readAlertLink(hash: string): { session: string | null; waited: boolean } {
  const params = new URLSearchParams(hash.replace(/^#/, ""));
  return { session: params.get("s"), waited: params.get("a") === "1" };
}
