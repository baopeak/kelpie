// Which way the screen connects: the "Connect through" choice before a screen starts. The computer lists the ways (`paths` in its `screens` answer) with
// whether each can be used now and why not; the phone adds its own reason, names the relay, preselects the way chosen last for this pairing, and says
// whether starting through it needs the person's Face ID. Pure: the words come through the caller's translator.
import { screenReasonText } from "./conn-path";

type Translate = (text: string, vars?: Record<string, string | number>) => string;

export const VIA_IDS = ["auto", "tailscale", "relay"] as const;
/** `auto`: Tailscale while the link is direct, else the relay (the computer decides). */
export type ViaId = (typeof VIA_IDS)[number];

/** One way, as the computer lists it. `why` is a reason code. */
export interface PathInfo {
  id: ViaId;
  ok: boolean;
  why?: string;
}

/** The relay the computer uses: Kelpie's own, or the person's (`host`). */
export interface RelayInfo {
  ours: boolean;
  host: string;
}

export interface PathRow {
  id: ViaId;
  label: string;
  ok: boolean;
  /** Why it cannot be used now, in words; null when it can. */
  why: string | null;
  recommended: boolean;
}

export const isVia = (v: unknown): v is ViaId => typeof v === "string" && (VIA_IDS as readonly string[]).includes(v);

/** The relay by name: `Kelpie's relay`, or `Your relay (host)`. */
export function relayName(relay: RelayInfo | null | undefined, t: Translate): string {
  if (!relay) return t("The relay");
  return relay.ours ? t("Kelpie's relay") : t("Your relay ({host})", { host: relay.host });
}

/** The rows of the Connect through list, in the computer's order, those this app knows. On the hosted app Tailscale cannot be used, whatever the computer
 *  says: the app came from the relay, and that is the only way it reaches the computer. */
export function pathRows(paths: readonly PathInfo[] | null | undefined, relay: RelayInfo | null | undefined, f: { hosted: boolean }, t: Translate): PathRow[] {
  return (paths ?? []).filter((p) => isVia(p.id)).map((p) => {
    const hostedNoTailscale = f.hosted && p.id === "tailscale";
    const ok = p.ok && !hostedNoTailscale;
    const code = hostedNoTailscale ? "hosted_app_no_tailscale" : p.why;
    const label = p.id === "auto" ? t("Automatic") : p.id === "tailscale" ? t("Tailscale") : relayName(relay, t);
    return { id: p.id, label, ok, why: ok ? null : (code ? screenReasonText(code, t) : null) ?? t("Not available now."), recommended: p.id === "auto" };
  });
}

/** The way preselected: the one chosen last when it can be used now, else Automatic, else the first that can. */
export function chooseVia(rows: readonly PathRow[], remembered: ViaId | null): ViaId {
  const usable = rows.filter((r) => r.ok).map((r) => r.id);
  if (remembered && usable.includes(remembered)) return remembered;
  if (usable.includes("auto")) return "auto";
  return usable[0] ?? "auto";
}

/** Starting through the relay needs a Face ID of its own: when the way is the relay, or Automatic on a phone whose connection is the relay now. */
export function needsProof(via: ViaId, route: "direct" | "relay"): boolean {
  return via === "relay" || (via === "auto" && route === "relay");
}

/** Where the way chosen last is kept for a pairing. */
export const viaKey = (pairingId: string) => `kelpie.screen.via.${pairingId}`;

/** A kept way; null for anything this app does not know. */
export function readVia(raw: string | null): ViaId | null {
  return isVia(raw) ? raw : null;
}

/** The way the computer used, as the live view names it (null for one this app does not know, so that the phone's own path is shown instead). */
export function viaText(via: string | null | undefined, t: Translate): string | null {
  switch (via) {
    case "tailscale":
      return t("Tailscale");
    case "relay":
      return t("the relay");
    default:
      return null;
  }
}

/** The computer's words for a refusal of `sc_start`, where the phone says it better; else the computer's own. */
export function startRefusal(code: string, message: string, t: Translate): string {
  return code === "needs_face_id" ? t("Face ID did not reach your computer in time, or was already used. Try again.") : message;
}
