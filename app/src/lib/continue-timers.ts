// The daemon's "continue at reset" timers, by session (see limits.ts, which keeps them in step with the daemon). Apart from
// limits.ts so that what a session shows at a glance (session-state.ts) can read them without the app's store.
import { createStore } from "solid-js/store";
import type { SessionInfo } from "./types";

/** Fire time (wall-clock ms) of each session's "continue" timer, by session id. */
export const [scheduled, setScheduled] = createStore<Record<string, number>>({});

/** When a session the usage limit stopped will continue by itself, if a timer is set for it. Such a session does not need
 *  the person: it is not shown as failed. */
export const continueAt = (s: SessionInfo): number | undefined => (s.alive && s.limited ? scheduled[s.id] : undefined);
