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
  /** The app served from the relay's origin (hosted.ts): installed first, as at a Tailscale name. */
  hosted: boolean;
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
  const installFirst = f.hosted || f.tailnet;
  if (!f.installed && !f.stayInBrowser && (installFirst || !f.hasCode)) return "install";
  return f.hasCode ? "pair" : "scan";
}

/** What becomes of a pairing that failed, given what this page would show with no code. The code is often spent or expired, and the installed app has no address bar to
 *  bring a new one: where the scanner is what comes next it goes back there, saying what went wrong. Any other page stays where the message can be read, and a new scan
 *  reaches it through its address. */
export const afterFailedPairing = (withNoCode: FirstScreen): "back_to_scanner" | "stay" => (withNoCode === "scan" ? "back_to_scanner" : "stay");

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

/** What an `addrs` message makes of the endpoints a pairing holds (`held`, none before it is kept): `known` to judge a move by, and what to
 *  keep (`keep`, null when nothing changes). A whole one replaces them all once it is newer; one from a computer from before `endpoints` replaces
 *  the direct addresses only. A `partial` one, which a page the relay hosts and a locked phone hear, brings the relay alone: it replaces the relay,
 *  keeps the rest, and leaves the sequence where it was, so that the whole list, heard once unlocked under the same number, is still taken. The
 *  page the relay hosts (`hosted`) can use nothing but the relay, and keeps nothing else, whatever a computer from before told it. */
export function applyAddrs(
  held: { endpoints: readonly Endpoint[]; addrsSeq: number } | null,
  m: { endpoints?: Endpoint[]; list: string[]; seq: number; partial?: boolean },
  hosted = false,
): { known: Endpoint[]; keep: { endpoints: Endpoint[]; addrsSeq: number } | null } {
  if (m.partial || hosted) {
    const relay = (m.endpoints ?? []).filter((e) => e.t === "relay");
    // The relay goes where it was, so a list that did not change is not written again.
    const base = hosted ? [] : (held?.endpoints ?? []);
    const at = base.findIndex((e) => e.t === "relay");
    const rest = base.filter((e) => e.t !== "relay");
    const known = at < 0 ? [...rest, ...relay] : [...rest.slice(0, at), ...relay, ...rest.slice(at)];
    // The hosted page goes by the newest it hears, whatever its number: the computer counts again from nothing after a restart, and a new
    // room told then must not be missed.
    const changed = !!held && (hosted || m.seq >= held.addrsSeq) && JSON.stringify(known) !== JSON.stringify(held.endpoints);
    return { known, keep: changed ? { endpoints: known, addrsSeq: held.addrsSeq } : null };
  }
  const known = m.endpoints ?? m.list.map((a) => ({ t: "lan", a }));
  if (!held || m.seq <= held.addrsSeq) return { known, keep: null };
  const endpoints = m.endpoints ? known : [...known, ...held.endpoints.filter((e) => e.t !== "lan" && e.t !== "ts")];
  return { known, keep: { endpoints, addrsSeq: m.seq } };
}

/** On the hosted app: the host of the computer's relay when it is not this page's. The app works only through the relay that serves it, so the
 *  computer moved and this phone must be paired again (at that relay's app, when it serves one). */
export function relayMovedTo(endpoints: readonly Endpoint[], here: string): string | null {
  const relay = endpoints.find((e) => e.t === "relay")?.a;
  if (!relay) return null;
  try {
    const host = new URL(relay).host;
    return host && host !== here ? host : null;
  } catch {
    return null;
  }
}
