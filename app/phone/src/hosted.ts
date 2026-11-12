// The app as served from the relay's own origin (`--mode hosted`, vite.phone.config.ts) rather than by the computer. The same app, with
// these differences: it connects through the relay alone (the page's origin is the relay, not the computer), pairs only through it,
// is installed first, and says what it trusts. `HOSTED` is fixed by the build, so the daemon's bundle drops every branch that needs it.
// The words come through the caller's translator (i18n.ts), so that the link and its tests need no dictionary.

type Translate = (text: string, vars?: Record<string, string | number>) => string;

/** Built for the relay's origin. False in the daemon's bundle and in tests. */
export const HOSTED: boolean = typeof __HOSTED__ === "boolean" && __HOSTED__;
/** The app's version, as the build was told. */
export const APP_VERSION: string = typeof __APP_VERSION__ === "string" ? __APP_VERSION__ : "dev";
/** The relay's own answer to "are you there", on the page's origin: what the hosted app probes instead of the computer. */
export const HEALTH_PATH = "/v1/health";
/** What the hosted origin says it serves (written by scripts/hosted/bundle_manifest.py; the worker never keeps it). */
export const VERSION_PATH = "/version.json";

/** Whether `url` (a relay's socket address, or a page) is on `host`. */
export function onHost(url: string, host: string): boolean {
  try {
    return new URL(url).host === host;
  } catch {
    return false;
  }
}

export interface ServedVersion {
  version: string;
  bundle: string;
}

/** What `/version.json` says now, from the network only; null when it cannot be read or is not the phone app's. A changed deploy can say anything here:
 *  this is for support and for telling versions apart, not proof of what this phone runs. */
export async function servedVersion(fetcher: (url: string, init: RequestInit) => Promise<Response> = (u, i) => fetch(u, i)): Promise<ServedVersion | null> {
  try {
    const r = await fetcher(VERSION_PATH, { cache: "no-store", redirect: "error" });
    if (!r.ok) return null;
    const v = (await r.json()) as { app?: unknown; version?: unknown; bundle_sha256?: unknown };
    if (v?.app !== "kelpie-phone" || typeof v.version !== "string" || typeof v.bundle_sha256 !== "string" || !/^[0-9a-f]{64}$/.test(v.bundle_sha256)) return null;
    return { version: v.version, bundle: v.bundle_sha256 };
  } catch {
    return null;
  }
}

/** What this app trusts, in plain words, for the sheet the menu opens (and that shows once after pairing). */
export function trustText(t: Translate): string[] {
  return [
    t("This app comes from Kelpie's relay on Cloudflare, not from your computer."),
    t("Whoever controls that Cloudflare account, or Kelpie's release deploy, could send this phone different code. That code could read and type in your sessions while the app is unlocked."),
    t("A phone that may type can also start agents in your projects with your computer's settings, bypass mode included. Changed code could do that too while the app is unlocked."),
    t("Each Kelpie release publishes this app's fingerprint and how to check it against what the relay serves."),
    t("This screen cannot prove what this phone runs: changed code could show anything here."),
    t("For an app served only by your computer, use Tailscale mode in Kelpie → Settings → Mobile."),
  ];
}

/** The question asked before this app, paired with one computer, pairs with another: one computer per app. */
export const replaceQuestion = (t: Translate, held: string, next: string) => t("This app is paired with {a}. Pair with {b} instead? {a} loses this phone.", { a: held, b: next });
