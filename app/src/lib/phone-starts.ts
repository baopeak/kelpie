// Sessions phones started while no window was open. The daemon tells an open window at once (`mobile.session_started`); a closed one hears
// nothing, so when the window connects it reads the phones' audit log (`mobile.audit`, the list Settings → Mobile shows) and puts each start
// it has not shown yet in Activity, once. What it has seen is remembered as the newest time (`AppState.phoneStartsSeen`).

/** One line of the phones' audit log, as `mobile.audit` gives it. */
export interface AuditEntry {
  at: number;
  device: string;
  action: string;
  summary: string;
}

/** A start the window missed: when, which phone (its id), the session (when the line names it), and what was started, in the log's words. */
export interface MissedStart {
  at: number;
  device: string;
  sessionId?: string;
  what: string;
}

/** The daemon ends a start's line with the session it started: "Claude Code in Shop (session 1a2b…)". */
const SESSION = / \(session ([^\s()]+)\)$/;

/** The phone starts (`session_new`) in `entries` that are newer than `seen`, oldest first, without those whose session Activity already
 *  shows (`shown`, from the open window's own notice), and the newest time to remember. The first time (`seen` undefined) shows none of
 *  them: they are from before this was remembered. */
export function missedPhoneStarts(entries: AuditEntry[], seen: number | undefined, shown: ReadonlySet<string>): { starts: MissedStart[]; newest: number } {
  const started = entries.filter((e) => e.action === "session_new" && typeof e.at === "number");
  const newest = Math.max(seen ?? 0, ...started.map((e) => e.at));
  if (seen === undefined) return { starts: [], newest };
  const starts = started
    .filter((e) => e.at > seen)
    .map((e) => {
      const m = SESSION.exec(e.summary);
      return { at: e.at, device: e.device, sessionId: m?.[1], what: m ? e.summary.slice(0, m.index) : e.summary };
    })
    .filter((s) => !s.sessionId || !shown.has(s.sessionId))
    .sort((a, b) => a.at - b.at);
  return { starts, newest };
}
