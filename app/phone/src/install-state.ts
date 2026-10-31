// What the first screen should be on a page that is not paired yet, and what a scanned or pasted code means. On a secure origin the
// app is installed first, because iOS keeps a Safari tab's storage apart from the installed app's: a pairing made in the tab would stay
// in the tab. Plain http:// cannot be installed and stays what it was: the link pairs that page.
import { decodePairPayload, type Endpoint } from "./noise";

export type Platform = "ios" | "android" | "other";

export function platformOf(userAgent: string, touchPoints: number): Platform {
  // An iPad says it is a Mac, and a Mac has no touch screen.
  if (/iPhone|iPad|iPod/.test(userAgent) || (/Macintosh/.test(userAgent) && touchPoints > 1)) return "ios";
  if (/Android/.test(userAgent)) return "android";
  return "other";
}

export interface PageFacts {
  https: boolean;
  /** At a Tailscale name. A person's own certificate on this network is HTTPS too, and pairing there worked before any of this. */
  tailnet: boolean;
  /** Opened from the Home Screen, not in a browser tab. */
  installed: boolean;
  paired: boolean;
  /** This page was opened from a pairing link. */
  hasCode: boolean;
  /** The person chose to go on in this browser. */
  stayInBrowser: boolean;
}

/** `install`: how to install. `scan`: scan or paste the pairing code. `pair`: what there was before: name this device, or how to pair. */
export type FirstScreen = "install" | "scan" | "pair";

export function firstScreen(f: PageFacts): FirstScreen {
  if (!f.https || f.paired) return "pair";
  if (!f.installed && !f.stayInBrowser && (f.tailnet || !f.hasCode)) return "install";
  return f.hasCode ? "pair" : "scan";
}

/** What becomes of a pairing that failed. The code is often spent or expired, and the installed app has no address bar to bring a new one: on a secure origin it goes
 *  back to the scanner, saying what went wrong. A plain http:// page stays, and a new scan reaches it through its address. */
export const afterFailedPairing = (https: boolean): "back_to_scanner" | "stay" => (https ? "back_to_scanner" : "stay");

export type CodeReading =
  | { ok: true; code: string }
  /** `not_code`: no pairing code here. `install_code`: this page's own address, with no code: the code of the first step. `other_address`: a code for another address: this page can only pair with where it came from. */
  | { ok: false; why: "not_code" | "install_code" | "other_address"; host?: string };

/** Reads what the camera saw or the person pasted: a whole pairing link or only its code. `here` is this page's `location.host`. */
export function readCode(text: string, here: string): CodeReading {
  const s = text.trim();
  let url: URL | null = null;
  try {
    url = new URL(s);
  } catch {
    /* not a whole link */
  }
  if (url) {
    const code = new URLSearchParams(url.hash.slice(1)).get("p");
    if (!code) return { ok: false, why: url.host === here ? "install_code" : "not_code" };
    if (!decodePairPayload(code)) return { ok: false, why: "not_code" };
    return url.host === here ? { ok: true, code } : { ok: false, why: "other_address", host: url.host };
  }
  const code = s.replace(/^#?p=/, "");
  return decodePairPayload(code) ? { ok: true, code } : { ok: false, why: "not_code" };
}

/** The computer's Tailscale origin when this page is somewhere else: it was renamed, or this is its plain address. Browsers keep a pairing per address, so going there means pairing again. */
export function movedTo(endpoints: readonly Endpoint[], here: string): string | null {
  const ts = endpoints.find((e) => e.t === "ts")?.a;
  return ts && ts !== here ? ts : null;
}
