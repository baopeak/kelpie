// How this phone reaches its computer, in words a person can check: how the app was paired (from where the page came), the way the
// connection takes now, and why the computer's screen is not offered here. Pure: the page's facts come in as arguments, and the words
// through the caller's translator (i18n.ts), so that tests need no dictionary.

type Translate = (text: string, vars?: Record<string, string | number>) => string;

/** How this app came to be paired, from where its page is served: `hosted` the relay's own origin, `tailscale` the computer's Tailscale
 *  address, `https` the computer at another HTTPS name, `http` this network over plain HTTP, `local` this computer itself. */
export type PageMethod = "hosted" | "tailscale" | "https" | "http" | "local";

/** The way the connection takes now. */
export type LivePath = "relay" | "tailscale" | "network" | "computer";

/** A Tailscale address: 100.64.0.0/10, a `.ts.net` name, or Tailscale's IPv6 range. */
export function isTailscaleHost(host: string): boolean {
  const v4 = host.match(/^(\d+)\.(\d+)\.\d+\.\d+$/);
  return (!!v4 && +v4[1] === 100 && +v4[2] >= 64 && +v4[2] < 128) || host.endsWith(".ts.net") || /^\[?fd7a:115c:a1e0:/i.test(host);
}

/** This computer itself. */
export function isLocalHost(host: string): boolean {
  return host === "localhost" || host === "127.0.0.1" || host === "[::1]";
}

export function pageMethod(hosted: boolean, page: { protocol: string; hostname: string }): PageMethod {
  if (hosted) return "hosted";
  if (isTailscaleHost(page.hostname)) return "tailscale";
  if (isLocalHost(page.hostname)) return "local";
  return page.protocol === "https:" ? "https" : "http";
}

/** The direct route is the page's own origin; the relay is the relay, whatever the page. */
export function livePath(route: "direct" | "relay", method: PageMethod): LivePath {
  if (route === "relay" || method === "hosted") return "relay";
  if (method === "tailscale") return "tailscale";
  if (method === "local") return "computer";
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
    case "computer":
      return t("this computer");
  }
}

/** What keeps the screen from this phone, the first that applies. */
export type ScreenBlock = "older" | "hosted" | "not_control" | "relay" | "lock";

export interface ScreenFacts {
  /** What the computer itself says stands in the way, where it says so: it comes before anything the phone works out. */
  computer?: ScreenBlock | null;
  /** The computer's feature level is below the screen's. */
  older: boolean;
  hosted: boolean;
  permission: "read" | "write" | "control" | undefined;
  route: "direct" | "relay";
  /** The app lock is on and open here. */
  unlocked: boolean;
}

export function screenBlock(f: ScreenFacts): ScreenBlock | null {
  if (f.computer) return f.computer;
  if (f.older) return "older";
  if (f.hosted) return "hosted";
  if (f.permission !== "control") return "not_control";
  if (f.route === "relay") return "relay";
  if (!f.unlocked) return "lock";
  return null;
}

/** Null when the screen may be seen from here; otherwise the one sentence that says why not, and what would change it. */
export function screenWhy(f: ScreenFacts, t: Translate): string | null {
  switch (screenBlock(f)) {
    case null:
      return null;
    case "older":
      return t("Update Kelpie on your computer to see its screen from here.");
    case "hosted":
      return t("This app comes from the relay; seeing the screen needs the app from your computer over Tailscale.");
    case "not_control":
      return t("Choose \"May take control\" for this phone in Kelpie → Settings → Mobile.");
    case "relay":
      return t("Seeing the screen needs Tailscale; this phone is on the relay now.");
    case "lock":
      return t("Needs the app lock on and unlocked.");
  }
}
