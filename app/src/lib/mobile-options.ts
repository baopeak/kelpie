// What each option of Settings → Mobile can do now, and the page's summary, from what the daemon said (`mobile.status`, `notify.get`).
// Pure, so that every row is tested at every daemon level. The daemon enforces all of it; this only says so ahead of time:
// `presence` follows `LockOptions::weakened_by` and `mobile.set_tailscale`, and a change it does not mention can still be refused.
import { HOSTED_API, LOCK_API, RELAY_API, TAILSCALE_API, TELEGRAM_API } from "./attention";
import { t } from "./i18n";
import { NOTIFY_API } from "./notify-events";
import { pushSummary, type PushState, type TelegramMode } from "./push-config";
import { offersControl } from "./takeover-panes";
import type { MobilePageKind } from "./types";
import type { MobileDevice, MobilePermission, MobileStatus } from "../components/settings/mobile-settings";
import type { RelayStatus } from "../components/settings/mobile-relay-settings";

export const OPTION_IDS = ["mobile-access", "mobile-hosted", "mobile-tailscale", "mobile-relay", "mobile-webpush", "mobile-lock", "mobile-lock-idle", "mobile-lock-recheck", "mobile-lock-plain", "mobile-ntfy", "mobile-telegram", "mobile-lan"] as const;
export type OptionId = (typeof OPTION_IDS)[number];

/** What is known. `status` or `push` is null when the daemon could not say. */
export interface OptionFacts {
  api: number;
  status: MobileStatus | null;
  push: PushState | null;
}

export interface OptionState {
  on: boolean;
  /** Why it cannot be changed now, with the next step; null when it can. */
  disabled: string | null;
  /** What changing it from here will ask of the person; null when it asks nothing. */
  presence: string | null;
}

const needsRestart = () => t("The background service still running is older than this app, so this is not there yet. Restart Kelpie's background service to use it.");

/** Why a setting the daemon keeps with phone access cannot be changed now. `app`: only the verified Kelpie app may change it. */
function blocked(f: OptionFacts, level: number, present: boolean, app: boolean): string | null {
  const s = f.status;
  if (f.api < level || !s || !present) return needsRestart();
  if (s.store_error) return t("The phone access settings could not be read: reset phone access first.");
  if (!s.enabled) return t("Turn on phone access first.");
  if (app && !s.app) return t("This window is not the installed Kelpie app, so it cannot change this.");
  return null;
}

/** A channel the daemon sends through, phone access or not. */
function channel(f: OptionFacts, on: boolean): OptionState {
  const disabled = f.api < NOTIFY_API ? needsRestart() : f.push ? null : t("Kelpie could not read the notification settings. Check that its background service is running, then open this page again.");
  return { on, disabled, presence: null };
}

export function optionState(id: OptionId, f: OptionFacts): OptionState {
  const s = f.status;
  const lock = s?.lock;
  const lockBlocked = () => blocked(f, LOCK_API, !!lock, true);
  switch (id) {
    case "mobile-access":
      return { on: !!s?.enabled, disabled: null, presence: null };
    case "mobile-tailscale": {
      const on = !!s?.tailscale?.on;
      return { on, disabled: blocked(f, TAILSCALE_API, !!s?.tailscale, true), presence: on ? null : t("Turning this on asks you to verify first.") };
    }
    case "mobile-relay": {
      const on = !!s?.relay?.on;
      return { on, disabled: blocked(f, RELAY_API, !!s?.relay, true), presence: on ? null : t("Turning this on asks you to verify first.") };
    }
    // Pairing, not a setting: "on" while a phone can pair on the hosted app now. Starting a code needs no verification; approving the phone does.
    case "mobile-hosted": {
      const r = s?.relay;
      const waiting = () => (!r!.on ? t("Turn on the relay first: the phone app comes from it.") : r!.hosted!.available ? null : hostedLine(r!));
      return { on: hostedPairing(f), disabled: blocked(f, HOSTED_API, !!r?.hosted, false) ?? waiting(), presence: null };
    }
    // Off it leaves nothing that reaches this computer unless the relay or Tailscale does, and the daemon would stop phone access: not offered then.
    case "mobile-lan": {
      const on = s?.lan !== false;
      const alone = on && !s?.relay?.on && !s?.tailscale?.on;
      return { on, disabled: blocked(f, HOSTED_API, s?.lan !== undefined, false) ?? (alone ? t("Turn on the relay or Tailscale first: with this off, nothing else would reach this computer.") : null), presence: null };
    }
    // Each phone turns this on for itself: this computer only reports it, so nothing here changes it.
    case "mobile-webpush":
      return { on: (f.push?.webpush?.devices ?? 0) > 0, disabled: f.push && !f.push.webpush ? needsRestart() : null, presence: null };
    case "mobile-lock":
      return { on: !!lock?.on, disabled: lockBlocked(), presence: lock?.on ? t("Turning this off asks you to verify first.") : null };
    case "mobile-lock-idle":
      return { on: !!lock?.on, disabled: lockBlocked() ?? (lock?.on ? null : t("Turn on the app lock first.")), presence: t("Choosing a longer time asks you to verify first.") };
    case "mobile-lock-recheck":
      return { on: !!lock?.recheck, disabled: lockBlocked() ?? (lock?.on ? null : t("Turn on the app lock first.")), presence: lock?.recheck ? t("Turning this off asks you to verify first.") : null };
    // With the lock off nothing stands in such a page's way, so this would change nothing.
    case "mobile-lock-plain":
      return { on: !!lock?.plain_read, disabled: lockBlocked() ?? (lock?.on ? null : t("This counts only while the app lock is on. With the lock off such a page reads anyway: turn the lock on first.")), presence: lock?.plain_read ? null : t("Turning this on asks you to verify first.") };
    case "mobile-ntfy":
      return channel(f, !!f.push?.config.ntfy.enabled);
    case "mobile-telegram":
      return channel(f, !!f.push?.config.telegram.enabled);
  }
}

/** What Telegram carries, as the daemon holds it, and why approvals cannot be turned on from here now (null: they can). Null when there is
 *  nothing to choose: Telegram is off, or the daemon is older than the choice and sends what it always did. */
export function telegramChoice(f: OptionFacts): { mode: TelegramMode; approveBlocked: string | null } | null {
  const tg = f.push?.config.telegram;
  if (!tg?.enabled || f.api < TELEGRAM_API || !f.push?.telegram) return null;
  const approveBlocked = !f.status?.app
    ? t("This window is not the installed Kelpie app, so it cannot turn on approvals from Telegram.")
    : !f.push.secrets.telegram_token
      ? t("Telegram needs a bot token.")
      : null;
  return { mode: tg.mode ?? "alert", approveBlocked };
}

/** How a phone reaches this computer now. */
function reachLine(s: MobileStatus): string {
  if (!s.enabled) return t("Phones cannot reach this computer: phone access is off.");
  if (!s.running) return t("Phone access is on, but not running, so phones cannot reach this computer. See the message below.");
  const lines: string[] = [];
  if (s.tailscale?.on) lines.push(s.tailscale.origin ? t("Phones with Tailscale on reach this computer at {origin}, from anywhere.", { origin: s.tailscale.origin }) : t("Tailscale mode is on, but phones cannot use it right now: see its row."));
  const r = s.relay;
  if (r?.on && !r.connected) lines.push(t("The relay is on, but this computer is not connected to it right now: see its row."));
  else if (r?.on) {
    const hosted = relayServesApp(r) && r.hosted?.origin;
    if (hosted) lines.push(t("Phones with the hosted app from {origin} reach this computer through the relay, from anywhere.", { origin: hosted }));
    // The app installed over Tailscale: let in through the relay from its origin, and the only one there without the hosted app.
    if (!hosted || r.origin) lines.push(t("With Tailscale off, the installed app reaches this computer through the relay."));
  }
  if (s.urls.length) {
    if (s.tls) lines.push(t("Phones on this network connect over HTTPS, with your certificate."));
    else if (s.lock?.on && !s.lock.plain_read) lines.push(t("Phones on this network connect over plain HTTP, and see nothing: Allow reading over plain HTTP is off."));
    else if (s.lock?.on) lines.push(t("Phones on this network connect over plain HTTP, and can only read."));
    else lines.push(t("Phones on this network connect over plain HTTP, and can only read, unless the address is a Tailscale one."));
  }
  return lines.join(" ");
}

function lockLine(s: MobileStatus): string {
  if (!s.lock) return t("This background service has no app lock yet. Restart it to get one.");
  if (s.lock.on) return t("The app lock is on: a phone paired at a name asks for Face ID, and locks after {n} min idle.", { n: s.lock.idle_s / 60 });
  // The daemon locks phones on the hosted app whatever the switch says.
  return s.devices.some((d) => d.page_kind === "hosted")
    ? t("The app lock is off: whoever holds a phone paired over Tailscale or this network reaches what it may do. Phones on the hosted app still ask for Face ID.")
    : t("The app lock is off: whoever holds a paired phone reaches what it may do.");
}

function phonesLine(s: MobileStatus): string {
  const n = s.devices.length + (s.legacy ?? []).length;
  return n === 0 ? t("No phone is paired.") : n === 1 ? t("1 phone is paired.") : t("{n} phones are paired.", { n });
}

/** The top of the page: how a phone gets here, whether the lock and alerts are on, and how many phones there are. */
export function mobileSummary(s: MobileStatus, push: PushState | null) {
  return {
    reach: reachLine(s),
    lock: lockLine(s),
    alerts: push ? pushSummary(push.config, push.secrets, push.webpush?.devices ?? 0) : null,
    phones: phonesLine(s),
  };
}

/** Whether removing a phone offers a new room at the relay: the removed phone still knows the room's ticket, though the relay no longer lets it in.
 *  Only while a new room needs nothing typed: this computer keeps the registration key, or the daemon makes rooms without one at a relay of the
 *  second version. Otherwise the person would verify only to be asked for the key. */
export function offerNewRoom(s: MobileStatus | null | undefined, api: number): boolean {
  return !!s?.relay?.on && !!s.relay.room && roomNeedsNothing(s.relay, api);
}

/** A new room at this relay needs no registration key typed. */
export function roomNeedsNothing(r: RelayStatus, api: number): boolean {
  return r.operator_key || (api >= HOSTED_API && (r.version ?? 0) >= 2);
}

// ---------- the phone app the relay hosts ----------

/** The daemon's reason while it has not looked at the relay's phone app yet: it is looking now, and says so when done (`mobile.updated`). */
const NOT_CHECKED = "not checked yet";

/** A new setup, as the daemon says (`new_store`): phone access never on, no phone, no relay, nothing chosen. Turning phone access on then sets up the
 *  hosted app over the relay. Nothing else makes a setup new: an existing one must never be moved. */
export function newSetup(f: OptionFacts): boolean {
  return f.api >= HOSTED_API && f.status?.new_store === true;
}

/** The calls turning phone access on makes. `secure` is `mobileSecure`: the person verifies first. */
export interface AccessCalls {
  daemon: (method: string, params: Record<string, unknown>) => Promise<unknown>;
  secure: (method: "mobile.set_relay", params: Record<string, unknown>) => Promise<unknown>;
  status: () => Promise<MobileStatus | null>;
}

/** Turns phone access on. A new setup: no listener on this network, then the default relay (one verification, no key), and "hosted" for the pairing guide
 *  to open. Otherwise exactly what the switch always did. Throws what stopped it. */
export async function turnOnAccess(f: OptionFacts, call: AccessCalls): Promise<"hosted" | "plain"> {
  if (!newSetup(f)) {
    await call.daemon("mobile.enable", {});
    return "plain";
  }
  try {
    await call.daemon("mobile.enable", { lan: false });
  } catch (e) {
    // Until the relay is on nothing reaches this computer, and the daemon says so; phone access is on all the same, which the relay needs.
    if (!(await call.status().catch(() => null))?.enabled) throw e;
  }
  await call.secure("mobile.set_relay", { on: true });
  return "hosted";
}

/** Pairing on the hosted app is offered: phone access is on, the relay is on and the daemon found the app serving. */
export function hostedPairing(f: OptionFacts): boolean {
  const s = f.status;
  return f.api >= HOSTED_API && !!s?.enabled && !s.store_error && !!s.relay?.on && !!s.relay.hosted?.available;
}

/** Pairing a browser on this network is offered only while this network's listeners are on (always, from a daemon without the choice). */
export function plainPairing(f: OptionFacts): boolean {
  return f.status?.lan !== false;
}

/** The relay row's line about the phone app: where and which version, still being checked, or not served. Null from a daemon without it. */
export function hostedLine(r: RelayStatus): string | null {
  const h = r.hosted;
  if (!h) return null;
  if (h.available && h.origin) return h.version ? t("Phone app at {origin}, version {version}.", { origin: h.origin, version: h.version }) : t("Phone app at {origin}.", { origin: h.origin });
  if (h.reason === NOT_CHECKED) return t("Kelpie is checking whether this relay serves the phone app.");
  return t("This relay does not serve the phone app: pair phones over Tailscale.");
}

/** Why the relay's phone app cannot be used, as the daemon says it; null when it can or is still being checked. */
export function hostedReason(r: RelayStatus): string | null {
  const h = r.hosted;
  return h && !h.available && h.reason !== NOT_CHECKED ? h.reason : null;
}

/** The relay serves the phone app phones use: it was found serving, or phones are paired on it. */
export function relayServesApp(r: RelayStatus): boolean {
  return !!r.hosted?.available || (r.hosted_devices ?? 0) > 0;
}

/** What the hosted app trusts, in the words Settings shows under its row. Kelpie's own relay names Cloudflare and Kelpie's release deploy; another relay
 *  names whoever runs it, whose app no Kelpie release describes. */
export function hostedTrust(r: RelayStatus): string | null {
  const origin = r.hosted?.origin;
  if (!origin) return null;
  const host = origin.replace(/^https:\/\//, "");
  return r.url === r.default_url
    ? t("The phone app comes from Kelpie's relay at {host}, on Cloudflare. Whoever controls that Cloudflare account or Kelpie's release deploy could send your phone different code that reads your sessions and types into them while the app is unlocked. Each release publishes the app's fingerprint so anyone can check what the relay serves; nobody can check what one phone received. For a phone app served only by this computer, use Tailscale mode.", { host })
    : t("The phone app comes from the relay at {host}, not from Kelpie's. Whoever runs that relay or can deploy to it could send your phone different code that reads your sessions and types into them while the app is unlocked. The fingerprints Kelpie publishes describe the app on Kelpie's relay, not this one. For a phone app served only by this computer, use Tailscale mode.", { host });
}

/** Said before the relay moves to another address while phones use the app at the present one: it is their only way to this computer. */
export function movingRelayWarning(r: RelayStatus | undefined, url: string): string | null {
  const n = r?.hosted_devices ?? 0;
  const origin = r?.hosted?.origin;
  const same = (a: string) => a.trim().replace(/\/+$/, "");
  if (!r || n === 0 || !origin || same(url) === same(r.url)) return null;
  return n === 1
    ? t("1 phone using the app at {origin} loses its way to this computer and must be paired again.", { origin })
    : t("{n} phones using the app at {origin} lose their way to this computer and must be paired again.", { n, origin });
}

/** The relay answered that a room there needs a registration key, or did not take the one given: the key field is shown then. */
export function asksForKey(error: unknown): boolean {
  return /needs a registration key|did not take that registration key/.test(String(error));
}

/** A device's page, in a few words: the badge in the device list and the line in the approval. */
export function pageKindLabel(kind: MobilePageKind | null | undefined): string | null {
  switch (kind) {
    case "hosted":
      return t("hosted app");
    case "ts":
      return t("Tailscale");
    case "own":
      return t("this computer's HTTPS");
    case "plain":
      return t("this network");
    default:
      return null;
  }
}

/** What a paired device may be given in its list. Control never for one on the hosted app: the daemon refuses it, whatever its lock. */
export function permissionChoices(d: MobileDevice, api: number): MobilePermission[] {
  return d.page_kind !== "hosted" && offersControl(d, api) ? ["read", "write", "control"] : ["read", "write"];
}

// ---------- choosing how a phone pairs ----------

/** The ways a phone pairs, as Pair a phone offers them: the app from this computer over Tailscale, the app the relay hosts, or a browser on this
 *  network. Each pairing is a device record of its own (the daemon's security model); "the same phone" is only how this computer lists them. */
export type PairMethodId = "tailscale" | "relay" | "network";

export interface PairMethod {
  id: PairMethodId;
  label: string;
  /** What it needs, what it allows and what it trusts: one line each. */
  needs: string;
  allows: string;
  trusts: string;
  /** It can pair a phone now. */
  available: boolean;
  /** Why it cannot now; null when it can. */
  why: string | null;
  /** The row that turns on what it needs; null when there is nothing to turn on here. */
  fix: { label: string; row: OptionId } | null;
  recommended: boolean;
}

/** What a phone paired this way may do with this computer's screen and browser panes. The one place this sentence lives: when the screen's policy
 *  changes, it changes here. */
export function screenPolicyLine(method: PairMethodId): string {
  switch (method) {
    case "tailscale":
      return t("The screen and browser panes too, once you allow control for this phone and its app lock is on.");
    case "relay":
      return t("Never the screen or browser panes: those need the app from this computer, over Tailscale or HTTPS.");
    case "network":
      return t("Over HTTPS, the screen and browser panes too, once you allow control for this phone and its app lock is on; never over plain HTTP.");
  }
}

/** The relay by name: Kelpie's, or the person's own at its host. */
export function relayLabel(r: RelayStatus | undefined): string {
  if (!r || r.url === r.default_url) return t("Kelpie's relay");
  return t("Your relay at {host}", { host: r.url.replace(/^\w+:\/\//, "").replace(/\/.*$/, "") });
}

type WayState = { why: string | null; fix: PairMethod["fix"] };
const READY: WayState = { why: null, fix: null };
const notNow = (why: string, fix: PairMethod["fix"] = null): WayState => ({ why, fix });
const notRunning = () => notNow(t("Phone access is on, but not running: see the message at the top of this page."));

function tailscaleWay(f: OptionFacts): WayState {
  const s = f.status;
  const gate = blocked(f, TAILSCALE_API, !!s?.tailscale, false);
  if (gate) return notNow(gate);
  const fix = { label: t("Go to Tailscale mode"), row: "mobile-tailscale" as const };
  if (!s!.tailscale!.on) return notNow(t("Tailscale mode is off."), fix);
  if (!s!.tailscale!.origin) return notNow(t("Tailscale mode is on, but phones cannot use it right now: see its row."), fix);
  return s!.running ? READY : notRunning();
}

function relayWay(f: OptionFacts): WayState {
  const r = f.status?.relay;
  const gate = blocked(f, HOSTED_API, !!r?.hosted, false);
  if (gate) return notNow(gate);
  const fix = { label: t("Go to the relay"), row: "mobile-relay" as const };
  if (!r!.on) return notNow(t("The relay is off."), fix);
  if (!r!.hosted!.available) return notNow(hostedLine(r!)!, r!.hosted!.reason === NOT_CHECKED ? null : fix);
  return READY;
}

function networkWay(f: OptionFacts): WayState {
  const gate = blocked(f, 0, true, false);
  if (gate) return notNow(gate);
  if (!plainPairing(f)) return notNow(t("This network's listeners are off."), { label: t("Go to Pair browsers on this network"), row: "mobile-lan" });
  return f.status!.running ? READY : notRunning();
}

/** The ways Pair a phone offers, in its order, each with what it needs, allows and trusts, whether it can pair now (and if not, why and where to fix it),
 *  and the one recommended: Tailscale while phones can use it, else the relay's app while it is served, else this network. */
export function pairMethods(f: OptionFacts): PairMethod[] {
  const s = f.status;
  const r = s?.relay;
  const reading = t("Reading, typing and approvals, from anywhere.");
  const ways: Omit<PairMethod, "recommended" | "available">[] = [
    {
      id: "tailscale",
      label: t("Tailscale"),
      needs: t("Tailscale on this computer and on the phone, with Tailscale mode on."),
      allows: `${reading} ${screenPolicyLine("tailscale")}`,
      trusts: t("This computer, which serves the app, and Tailscale."),
      ...tailscaleWay(f),
    },
    {
      id: "relay",
      label: relayLabel(r),
      needs: t("The relay on, and serving the phone app."),
      allows: `${reading} ${screenPolicyLine("relay")}`,
      trusts: !r || r.url === r.default_url
        ? t("Whoever deploys the app on Kelpie's relay: see What this trusts, under Hosted phone app.")
        : t("Whoever runs that relay and deploys its app."),
      ...relayWay(f),
    },
    {
      id: "network",
      label: t("This network"),
      needs: t("The same Wi-Fi as this computer, with this network's listeners on."),
      allows: `${s?.tls ? t("Reading, typing and approvals, over HTTPS with your certificate.") : t("Reading only, over plain HTTP. With a certificate set, typing too.")} ${screenPolicyLine("network")}`,
      trusts: s?.tls ? t("This computer, which serves the app, and your certificate.") : t("Everyone on this network, not to swap the page: it comes over plain HTTP."),
      ...networkWay(f),
    },
  ];
  const usable = (id: PairMethodId) => !ways.find((w) => w.id === id)!.why;
  const recommended: PairMethodId = usable("tailscale") ? "tailscale" : usable("relay") ? "relay" : "network";
  return ways.map((w) => ({ ...w, available: !w.why, recommended: w.id === recommended }));
}

/** The way a record paired, from its page: null for one paired before pages were recorded. */
export function pairMethodOf(kind: MobilePageKind | null | undefined): PairMethodId | null {
  switch (kind) {
    case "hosted":
      return "relay";
    case "ts":
      return "tailscale";
    case "own":
    case "plain":
      return "network";
    default:
      return null;
  }
}

/** A record's way by name, for its line under a phone paired several ways. */
export function deviceMethodLabel(d: MobileDevice, r: RelayStatus | undefined): string {
  switch (d.page_kind) {
    case "hosted":
      return relayLabel(r);
    case "ts":
      return t("Tailscale");
    case "own":
      return t("This network, over HTTPS");
    case "plain":
      return t("This network");
    default:
      return d.kind === "app" ? t("Kelpie app") : t("Browser");
  }
}

/** How a record is connected now, as the daemon says. */
export function connectionLine(c: NonNullable<MobileDevice["connection"]>): string {
  switch (c.route) {
    case "relay":
      return t("connected now through the relay");
    case "tailscale":
      return t("connected now through Tailscale");
    case "lan":
      return t("connected now through this network");
    case "local":
      return t("connected now through this computer");
  }
}

/** A phone in the device list: the records joined in `phoneGroups` (`group` set) while two or more of them are still paired, else one record as before. */
export interface PhoneGroup {
  group: string | null;
  devices: MobileDevice[];
}

/** The device list as phones: the records of one group come together at the place of the first; every other record is a phone of its own. */
export function groupDevices(devices: MobileDevice[], groups: Record<string, string>): PhoneGroup[] {
  const size = new Map<string, number>();
  for (const d of devices) if (groups[d.id]) size.set(groups[d.id], (size.get(groups[d.id]) ?? 0) + 1);
  const shown = new Map<string, PhoneGroup>();
  const out: PhoneGroup[] = [];
  for (const d of devices) {
    const g = groups[d.id];
    if (!g || size.get(g)! < 2) {
      out.push({ group: null, devices: [d] });
      continue;
    }
    const known = shown.get(g);
    if (known) known.devices.push(d);
    else {
      const phone = { group: g, devices: [d] };
      shown.set(g, phone);
      out.push(phone);
    }
  }
  return out;
}

/** The record a pairing request most likely is the same phone as: one of the same name, paired another way. Only a suggestion the person sees. */
export function samePhoneSuggestion(r: { name: string; origin_kind?: MobilePageKind }, devices: MobileDevice[]): string | null {
  const name = r.name.trim().toLowerCase();
  const way = pairMethodOf(r.origin_kind);
  return devices.find((d) => d.state === "ok" && d.name.trim().toLowerCase() === name && pairMethodOf(d.page_kind) !== way)?.id ?? null;
}

/** `groups` with record `id` joined to the phone record `same` is: that phone's group, or a new one named after it. */
export function joinGroup(groups: Record<string, string>, id: string, same: string): Record<string, string> {
  const group = groups[same] ?? same;
  return { ...groups, [same]: group, [id]: group };
}

/** `groups` with only the records still paired; null when none is to go. */
export function prunePhoneGroups(groups: Record<string, string>, paired: string[]): Record<string, string> | null {
  const kept = Object.entries(groups).filter(([id]) => paired.includes(id));
  return kept.length === Object.keys(groups).length ? null : Object.fromEntries(kept);
}

/** The approval's two buttons: which one is the default, what the phone asks for as it can have it, and the line said with typing. A phone on the hosted
 *  app gets reading by default and the line on what typing from it means; control is never asked for there. */
export function approvalChoices(r: { want: MobilePermission; origin_kind?: MobilePageKind }): { choices: ("read" | "write")[]; primary: "read" | "write"; asks: MobilePermission; typing: string | null } {
  if (r.origin_kind === "hosted") {
    return { choices: ["read", "write"], primary: "read", asks: r.want === "read" ? "read" : "write", typing: t("Typing from the hosted app lets code served by Kelpie's relay type into your sessions while the phone is unlocked.") };
  }
  return { choices: ["read", "write"], primary: r.want === "read" ? "read" : "write", asks: r.want, typing: null };
}
