// When a usage limit lifts. Pure, so it is tested without the app (limits.ts holds the state).
import type { ProviderUsage } from "./types";

/** When the limit lifts: the latest reset of the windows that are full and still ahead. Both a
 *  full 5 h window and a full weekly one have to reset before the agent can work. `null`: no
 *  window is full (the stop was something else), or one that is full does not say when. */
export function resetTime(usage: ProviderUsage | null | undefined, now = Date.now()): number | null {
  if (!usage) return null;
  const full = usage.windows.filter((w) => w.used_percent >= 100);
  if (!full.length || full.some((w) => !w.resets_at || w.resets_at <= now)) return null;
  return Math.max(...full.map((w) => w.resets_at!));
}

/** The next time the clock shows `hh:mm`, strictly after `now` (today, else tomorrow). */
export function nextClockTime(hh: number, mm: number, now = Date.now()): number {
  const d = new Date(now);
  d.setHours(hh, mm, 0, 0);
  if (d.getTime() <= now) d.setDate(d.getDate() + 1);
  return d.getTime();
}
