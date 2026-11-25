// The daemon's "continue at reset" timers, by session (see limits.ts, which keeps them in step with the daemon). Apart from
// limits.ts so that what a session shows at a glance (session-state.ts) can read them without the app's store.
import { createStore } from "solid-js/store";
import { formatTime } from "./i18n";
import type { SessionInfo } from "./types";

/** Fire time (wall-clock ms) of each session's "continue" timer, by session id. */
export const [scheduled, setScheduled] = createStore<Record<string, number>>({});

/** When a session the usage limit stopped will continue by itself, if a timer is set for it. Such a session does not need
 *  the person: it is not shown as failed. */
export const continueAt = (s: SessionInfo): number | undefined => (s.alive && s.limited ? scheduled[s.id] : undefined);

/** When a timer fires, as the sidebar and the banner say it: the time, with the day when it is not today (a weekly limit). */
export function continueTime(at: number, now = Date.now()): string {
  return formatTime(at, sameDay(at, now) ? undefined : { weekday: "short", hour: "2-digit", minute: "2-digit" });
}

/** The same time as short as the sidebar row can show it next to the session's name: 24-hour digits, so that dropping AM/PM
 *  leaves no doubt ("15:20", "0:05"), with the day when it is not today ("Fri 0:05"; Vietnamese puts the day last). The row's
 *  tooltip says it in full (`continueTime`). */
export function continueTimeShort(at: number, now = Date.now()): string {
  const time: Intl.DateTimeFormatOptions = { hour: "numeric", minute: "2-digit", hourCycle: "h23" };
  return formatTime(at, sameDay(at, now) ? time : { ...time, weekday: "short" });
}

const sameDay = (a: number, b: number) => new Date(a).toDateString() === new Date(b).toDateString();
