// Sessions phones started while no window was open. The daemon tells an open window at once (`mobile.session_started`); a closed one hears
// nothing, so when the window connects it reads the phones' starts back from their audit log (`mobile.audit`, the list Settings → Mobile
// shows) and puts each start it has not shown yet in Activity, once. What it has seen is remembered as the newest time
// (`AppState.phoneStartsSeen`).

/** One line of the phones' audit log, as `mobile.audit` gives it. */
export interface AuditEntry {
  at: number;
  device: string;
  action: string;
  summary: string;
}

/** What `mobile.audit` answers: the lines, newest first, and, when older lines were left out (past the limit, or rotated away), the time of
 *  the oldest line it covers (`from`). */
export interface AuditRead {
  entries: AuditEntry[];
  from?: number;
}

/** The most lines one `mobile.audit` gives. */
export const PHONE_STARTS_LIMIT = 2000;

/** What the window asks for: the starts only (every key a phone types has a line of its own, and would push them out), as many as the daemon
 *  gives. A daemon that does not know `action` gives every kind of line, which `missedPhoneStarts` sorts out. */
export const PHONE_STARTS_READ = { action: "session_new", limit: PHONE_STARTS_LIMIT } as const;

/** A start the window missed: when, which phone (its id), the session (when the line names it), and what was started, in the log's words. */
export interface MissedStart {
  at: number;
  device: string;
  sessionId?: string;
  what: string;
}

/** The daemon ends a start's line with the session it started: "Claude Code in Shop (session 1a2b…)". */
const SESSION = / \(session ([^\s()]+)\)$/;

/** The phone starts (`session_new`) in `read` that are newer than `seen`, oldest first, without those whose session Activity already shows
 *  (`shown`, from the open window's own notice), and the newest time to remember. The first time (`seen` undefined) shows none of them: they
 *  are from before this was remembered.
 *
 *  `cutOff` says starts newer than `seen` may be missing: the answer begins after `seen` (`from`; a daemon without it said nothing of what it
 *  left out, so a full answer is taken as cut at its oldest line). Said once: the time to remember moves past `from`. */
export function missedPhoneStarts(read: AuditRead, seen: number | undefined, shown: ReadonlySet<string>): { starts: MissedStart[]; newest: number; cutOff: boolean } {
  const lines = read.entries.filter((e) => typeof e.at === "number");
  const from = typeof read.from === "number" ? read.from : lines.length >= PHONE_STARTS_LIMIT ? Math.min(...lines.map((e) => e.at)) : undefined;
  const started = lines.filter((e) => e.action === "session_new");
  const newest = Math.max(seen ?? 0, from ?? 0, ...started.map((e) => e.at));
  if (seen === undefined) return { starts: [], newest, cutOff: false };
  const starts = started
    .filter((e) => e.at > seen)
    .map((e) => {
      const m = SESSION.exec(e.summary);
      return { at: e.at, device: e.device, sessionId: m?.[1], what: m ? e.summary.slice(0, m.index) : e.summary };
    })
    .filter((s) => !s.sessionId || !shown.has(s.sessionId))
    .sort((a, b) => a.at - b.at);
  return { starts, newest, cutOff: from !== undefined && from > seen };
}

/** What reading the missed starts back needs from the window. */
export interface PhoneStartsIo {
  /** The phones' starts (`mobile.audit` with `PHONE_STARTS_READ`); null when they could not be read. */
  audit: () => Promise<AuditRead | null>;
  /** The phones' names by id (`mobile.status`); asked only when there is a start to show. */
  names: () => Promise<(device: string) => string | undefined>;
  /** The sessions whose phone start Activity shows, as it is now. */
  shown: () => ReadonlySet<string>;
  /** Puts one start in Activity, with its phone's name when known. */
  add: (start: MissedStart, phone: string | undefined) => void;
  /** Puts the line in Activity that says some starts may not be listed. */
  cutOff: () => void;
  /** Remembers the newest time seen (never moving it back). */
  remember: (newest: number) => void;
}

/** Reads back the starts the window missed and puts them in Activity. `seen` is the remembered time as it was when the window connected,
 *  read before any wait: a phone's start heard live meanwhile moves the remembered time to now, and must not hide older starts this reads
 *  back. What Activity shows is read again just before the starts go in: one heard live meanwhile is not put in twice. */
export async function loadMissedPhoneStarts(seen: number | undefined, io: PhoneStartsIo): Promise<void> {
  const read = await io.audit();
  if (!read) return;
  const { starts, newest, cutOff } = missedPhoneStarts(read, seen, io.shown());
  if (starts.length) {
    const nameOf = await io.names();
    const shown = io.shown();
    for (const s of starts) if (!s.sessionId || !shown.has(s.sessionId)) io.add(s, nameOf(s.device));
  }
  if (cutOff) io.cutOff();
  io.remember(newest);
}
