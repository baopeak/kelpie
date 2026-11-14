// How this phone reaches its computer, in words a person can check: how the app was paired (from where the page came), the way the
// connection takes now, and why the computer's screen is not offered here. Pure: the page's facts come in as arguments, and the words
// through the caller's translator (i18n.ts), so that tests need no dictionary.

type Translate = (text: string, vars?: Record<string, string | number>) => string;

/** How this app came to be paired, from where its page is served: `hosted` the relay's own origin, `tailscale` the computer's Tailscale
 *  address, `https` the computer at another HTTPS name, `http` this network over plain HTTP, `local` this computer itself. */
export type PageMethod = "hosted" | "tailscale" | "https" | "http" | "local";

/** The way the connection takes now. */
export type LivePath = "relay" | "tailscale" | "network" | "address" | "computer";

/** A Tailscale address: 100.64.0.0/10, a `.ts.net` name, or Tailscale's IPv6 range. */
export function isTailscaleHost(host: string): boolean {
  const v4 = host.match(/^(\d+)\.(\d+)\.\d+\.\d+$/);
  return (!!v4 && +v4[1] === 100 && +v4[2] >= 64 && +v4[2] < 128) || host.endsWith(".ts.net") || /^\[?fd7a:115c:a1e0:/i.test(host);
}

/** This computer itself. */
export function isLocalHost(host: string): boolean {
  return host === "localhost" || host === "127.0.0.1" || host === "[::1]";
}

/** As the computer names the way a phone paired: Tailscale is the app Tailscale Serve publishes, always over HTTPS at the computer's `.ts.net` name. A page
 *  at a Tailscale address by number, or over plain HTTP, is the computer's own listener, which Kelpie on the computer lists as this network. */
export function pageMethod(hosted: boolean, page: { protocol: string; hostname: string }): PageMethod {
  if (hosted) return "hosted";
  const https = page.protocol === "https:";
  if (https && page.hostname.endsWith(".ts.net")) return "tailscale";
  if (isLocalHost(page.hostname)) return "local";
  return https ? "https" : "http";
}

/** The direct route is the page's own origin (`host`), which goes over Tailscale whenever it is a Tailscale address, whatever way the app paired; the
 *  relay is the relay, whatever the page. */
export function livePath(route: "direct" | "relay", method: PageMethod, host = ""): LivePath {
  if (route === "relay" || method === "hosted") return "relay";
  if (method === "tailscale" || isTailscaleHost(host)) return "tailscale";
  if (method === "local") return "computer";
  // Another HTTPS name may be reached from outside this network too: what is sure is that it is the computer's own address.
  if (method === "https") return "address";
  return "network";
}

export function methodText(method: PageMethod, t: Translate): string {
  switch (method) {
    case "hosted":
      return t("The hosted app, through the relay");
    case "tailscale":
      return t("Tailscale, with the app from this computer");
    case "https":
      return t("This computer's HTTPS address");
    case "http":
      return t("This network, over plain HTTP");
    case "local":
      return t("This computer (localhost)");
  }
}

export function pathText(path: LivePath, t: Translate): string {
  switch (path) {
    case "relay":
      return t("the relay");
    case "tailscale":
      return t("Tailscale");
    case "network":
      return t("this network");
    case "address":
      return t("this computer's address");
    case "computer":
      return t("this computer");
  }
}

/** The ways a phone pairs, as Kelpie on the computer offers them under Pair a phone. */
export type PairWay = "tailscale" | "relay" | "network";

/** The way this app paired: the relay's app, the computer's over Tailscale, or the computer's on this network (HTTPS, plain HTTP or the computer itself). */
export function pairWay(method: PageMethod): PairWay {
  return method === "hosted" ? "relay" : method === "tailscale" ? "tailscale" : "network";
}

/** Which app this is, the other ways to connect this phone in one line each, and how to choose one: for the screens before pairing and for Settings.
 *  The relay is named plainly: the phone cannot tell Kelpie's relay from one the person runs, which Kelpie on the computer names. */
export function methodLines(method: PageMethod, t: Translate): { way: PairWay; self: string; others: string[]; choose: string } {
  const way = pairWay(method);
  const self = way === "relay" ? t("This is the app from the relay.") : way === "tailscale" ? t("This is the app from your computer over Tailscale.") : t("This is the app on this network.");
  const lines: Record<PairWay, string> = {
    tailscale: t("Tailscale: the app from your computer, from anywhere, with Tailscale on this phone and on the computer."),
    relay: t("The relay: the app the relay hosts, from anywhere, with no Tailscale needed."),
    network: t("This network: the app from your computer, on the same Wi-Fi."),
  };
  const others = (["tailscale", "relay", "network"] as const).filter((w) => w !== way).map((w) => lines[w]);
  return { way, self, others, choose: t("To use another way, choose it in Kelpie on your computer → Settings → Mobile → Pair a phone. It is paired as an app of its own, and this one stays as it is.") };
}

/** What keeps the screen from this phone, the first that applies. */
export type ScreenBlock = "unknown" | "older" | "hosted" | "not_control" | "plain_page" | "relay" | "lock" | "reason";

/** What the computer says about the screen on this connection (`device.screen`), and the reasons a way to it may be refused (`paths`, `sc_start`): the
 *  words of each are `screenReasonText`. */
const SCREEN_REASONS = ["not_granted", "plain_page", "method_off", "no_direct_link", "not_on_relay", "screen_remote_off", "lock_off", "no_passkey", "locked", "needs_face_id", "relay_off", "relay_budget_spent", "hosted_app_no_tailscale"] as const;

/** The reason in words, with what would change it; null for a code this app does not know. */
export function screenReasonText(code: string, t: Translate): string | null {
  switch (code) {
    case "not_granted":
      return t("Choose \"May take control\" for this phone in Kelpie → Settings → Mobile.");
    case "plain_page":
      return t("This page did not come from your computer over HTTPS; seeing the screen needs the app from your computer over Tailscale.");
    case "method_off":
      return t("Turned off on the computer.");
    case "no_direct_link":
      return t("Tailscale is off on this phone or not reachable now.");
    case "not_on_relay":
      return t("This phone is not connected through the relay now.");
    case "screen_remote_off":
      return t("Turn on \"See and control the screen without Tailscale\" in Kelpie → Settings → Mobile.");
    case "lock_off":
      return t("The app lock is off on the computer: turn it on in Kelpie → Settings → Mobile.");
    case "no_passkey":
      return t("This phone has no app lock set up yet: set it up first.");
    case "locked":
      return t("Needs the app lock on and unlocked.");
    case "needs_face_id":
      return t("Face ID is asked for each time the screen starts through the relay.");
    case "relay_off":
      return t("The relay is off on the computer.");
    case "relay_budget_spent":
      return t("The screen data the relay may carry this month is used up.");
    case "hosted_app_no_tailscale":
      return t("This app came from the relay, so it can reach the computer only through the relay.");
    case "no_webrtc":
      return t("This browser cannot connect peer to peer (no WebRTC).");
    case "no_p2p_path":
      return t("A direct connection did not work from this network a moment ago.");
    case "no_p2p_here":
      return t("This computer cannot send its screen peer to peer.");
    case "turn_not_configured":
      return t("This relay offers no TURN.");
    case "turn_full":
      return t("The relay's TURN is used up for now. Try again later.");
    case "turn_budget_spent":
      return t("This computer's TURN allowance is used up (Kelpie → Settings → Mobile → Advanced).");
    case "turn_failed":
      return t("The relay did not give TURN credentials a moment ago.");
    default:
      return null;
  }
}

/** The computer's word `device.screen` is one this app knows. */
export const knownScreenReason = (code: string | null | undefined): code is string => code === "ok" || (!!code && (SCREEN_REASONS as readonly string[]).includes(code));

/** The screen may be started from here as the computer says: `ok`, or only Face ID is left, which is asked for at the start. */
export const screenReady = (code: string) => code === "ok" || code === "needs_face_id";

/** The screen's entry is offered: from a computer that says so (`device.screen`) by its word, otherwise by the permission, as before. */
export function screenOffered(f: { older: boolean; screen?: string | null; permission: "read" | "write" | "control" | undefined }): boolean {
  if (f.older) return false;
  return knownScreenReason(f.screen) ? screenReady(f.screen) : f.permission === "control";
}

/** What the computer says about control on this connection (`device.control`), as what keeps the screen from this phone: null for `ok`, for a computer that
 *  does not say, and for a reason this app does not know, so that what the phone works out applies. The lock's three reasons read as one. */
export function computerBlock(control: string | undefined): ScreenBlock | null {
  switch (control) {
    case "hosted_app":
      return "hosted";
    case "not_granted":
      return "not_control";
    case "plain_page":
      return "plain_page";
    case "via_relay":
      return "relay";
    case "lock_off":
    case "no_passkey":
    case "locked":
      return "lock";
    default:
      return null;
  }
}

/** On the hosted app, when the computer is also published over Tailscale (`ts` in its hello): where the screen is. Null otherwise. */
export function tailscaleHint(method: PageMethod, ts: boolean, t: Translate): string | null {
  return method === "hosted" && ts
    ? t("This computer also has Tailscale. To see its screen from this phone, pair the Tailscale app: Kelpie on your computer → Settings → Mobile → Pair a phone → Tailscale.")
    : null;
}

export interface ScreenFacts {
  /** What the computer says about the screen on this connection (`device.screen`, from API level 41): its word comes before everything else. */
  screen?: string | null;
  /** What the computer itself says stands in the way, where it says so: it comes before anything the phone works out. */
  computer?: ScreenBlock | null;
  /** The computer has said what it is (a hello came): until then nothing about it is known. */
  known: boolean;
  /** The computer's feature level is below the screen's. */
  older: boolean;
  hosted: boolean;
  permission: "read" | "write" | "control" | undefined;
  route: "direct" | "relay";
  /** The app lock is on and open here. */
  unlocked: boolean;
}

export function screenBlock(f: ScreenFacts): ScreenBlock | null {
  if (knownScreenReason(f.screen)) return screenReady(f.screen) ? (f.unlocked ? null : "lock") : "reason";
  if (f.computer) return f.computer;
  if (!f.known) return "unknown";
  if (f.older) return "older";
  if (f.hosted) return "hosted";
  // Before the permission: through the relay the computer caps every phone below control, so a phone that has it would be told to ask for it.
  if (f.route === "relay") return "relay";
  if (f.permission !== "control") return "not_control";
  if (!f.unlocked) return "lock";
  return null;
}

/** Null when the screen may be seen from here; otherwise the one sentence that says why not, and what would change it. */
export function screenWhy(f: ScreenFacts, t: Translate): string | null {
  switch (screenBlock(f)) {
    case null:
      return null;
    case "unknown":
      return t("Connect to your computer to see whether its screen can be shown here.");
    case "older":
      return t("Update Kelpie on your computer to see its screen from here.");
    case "hosted":
      return t("This app comes from the relay; seeing the screen needs the app from your computer over Tailscale.");
    case "not_control":
      return t("Choose \"May take control\" for this phone in Kelpie → Settings → Mobile.");
    case "plain_page":
      // Paired over plain HTTP, or this page is not a secure one: either way, the page did not come from the computer over HTTPS.
      return t("This page did not come from your computer over HTTPS; seeing the screen needs the app from your computer over Tailscale.");
    case "relay":
      return t("Seeing the screen needs Tailscale; this phone is on the relay now.");
    case "lock":
      return t("Needs the app lock on and unlocked.");
    case "reason":
      return screenReasonText(f.screen ?? "", t);
  }
}
