// What each option of Settings → Mobile can do now, and the page's summary, from what the daemon said (`mobile.status`, `notify.get`).
// Pure, so that every row is tested at every daemon level. The daemon enforces all of it; this only says so ahead of time:
// `presence` follows `LockOptions::weakened_by` and `mobile.set_tailscale`, and a change it does not mention can still be refused.
import { LOCK_API, TAILSCALE_API } from "./attention";
import { t } from "./i18n";
import { NOTIFY_API } from "./notify-events";
import { pushSummary, type PushState } from "./push-config";
import type { MobileStatus } from "../components/settings/mobile-settings";

export const OPTION_IDS = ["mobile-access", "mobile-tailscale", "mobile-webpush", "mobile-lock", "mobile-lock-idle", "mobile-lock-recheck", "mobile-lock-plain", "mobile-ntfy", "mobile-telegram"] as const;
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

/** How a phone reaches this computer now. */
function reachLine(s: MobileStatus): string {
  if (!s.enabled) return t("Phones cannot reach this computer: phone access is off.");
  if (!s.running) return t("Phone access is on, but not running, so phones cannot reach this computer. See the message below.");
  const lines: string[] = [];
  if (s.tailscale?.on) lines.push(s.tailscale.origin ? t("Phones with Tailscale on reach this computer at {origin}, from anywhere.", { origin: s.tailscale.origin }) : t("Tailscale mode is on, but phones cannot use it right now: see its row."));
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
  return s.lock.on ? t("The app lock is on: a phone paired at a name asks for Face ID, and locks after {n} min idle.", { n: s.lock.idle_s / 60 }) : t("The app lock is off: whoever holds a paired phone reaches what it may do.");
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
