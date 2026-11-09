// Closing Kelpie. The native side holds the close (src-tauri/src/quit.rs) and asks here: the session daemon stops too,
// unless Settings → Session Daemon → When Kelpie closes keeps it running in the background. When something still runs
// the person is asked first (components/close-dialog.tsx), unless they said not to. The stop is graceful: sessions come
// back interrupted and agents resume; nothing is waited for. Updates never come here. Tested in close-daemon.test.ts.
import { t } from "./i18n";

/** What still runs on the daemon: the names of live sessions, active file transfers, run tasks at work. */
export interface RunningWork {
  sessions: string[];
  transfers: number;
  runTasks: number;
}

export interface CloseSettings {
  daemonOnClose?: "stop" | "keep";
  askBeforeStop?: boolean;
}

export const hasWork = (w: RunningWork) => w.sessions.length > 0 || w.transfers > 0 || w.runTasks > 0;

export type CloseAction = "keep" | "stop" | "ask";

/** Keep the daemon, stop it, or ask first. A missing setting means stop; `work` null (the daemon cannot be asked)
 *  has nothing to name, so it stops without asking. */
export function closeAction(s: CloseSettings, work: RunningWork | null): CloseAction {
  if ((s.daemonOnClose ?? "stop") === "keep") return "keep";
  if (!work || !hasWork(work)) return "stop";
  return s.askBeforeStop === false ? "stop" : "ask";
}

const BUSY_TASK = new Set(["starting", "running"]);

/** From the daemon's lists: sessions as `{ alive, label }`, runs as the daemon sends them (`run.list`). */
export function runningWork(sessions: { alive: boolean; label: string }[], transfers: number, runs: { status: string; tasks: { status: string }[] }[]): RunningWork {
  return {
    sessions: sessions.filter((s) => s.alive).map((s) => s.label),
    transfers: Math.max(0, transfers),
    runTasks: runs.filter((r) => r.status === "running").reduce((n, r) => n + r.tasks.filter((x) => BUSY_TASK.has(x.status)).length, 0),
  };
}

/** The dialog's lines about what stops. */
export function workLines(w: RunningWork): string[] {
  const lines: string[] = [];
  if (w.sessions.length) lines.push(w.sessions.length === 1 ? t("1 session is running") : t("{n} sessions are running", { n: w.sessions.length }));
  if (w.runTasks) lines.push(w.runTasks === 1 ? t("1 run task is working") : t("{n} run tasks are working", { n: w.runTasks }));
  if (w.transfers) lines.push(w.transfers === 1 ? t("1 file transfer is copying") : t("{n} file transfers are copying", { n: w.transfers }));
  return lines;
}

export interface CloseDeps {
  /** The settings, or null while the app state is not loaded (the choice is unknown: the daemon is kept). */
  settings: () => CloseSettings | null;
  work: () => Promise<RunningWork | null>;
  /** Shows the question; null when the person cancels. */
  ask: (work: RunningWork) => Promise<{ dontAsk: boolean } | null>;
  dontAskAgain: () => void;
  /** Saves the app state now (a pending change would be lost with the exit). */
  save: () => Promise<void>;
  quit: (stopDaemon: boolean) => Promise<void>;
}

export type CloseOutcome = "kept" | "stopped" | "cancelled" | "busy";

/** One close at a time: a second request while one is being handled (the window's close button clicked again with the
 *  question up) changes nothing. */
export function createCloseFlow(deps: CloseDeps): () => Promise<CloseOutcome> {
  let busy = false;
  return async () => {
    if (busy) return "busy";
    busy = true;
    try {
      const s = deps.settings();
      if (!s || closeAction(s, null) === "keep") {
        await deps.save().catch(() => {});
        await deps.quit(false);
        return "kept";
      }
      const work = await deps.work();
      if (closeAction(s, work) === "ask") {
        const answer = await deps.ask(work!);
        if (!answer) return "cancelled";
        if (answer.dontAsk) deps.dontAskAgain();
      }
      await deps.save().catch(() => {});
      await deps.quit(true);
      return "stopped";
    } finally {
      busy = false;
    }
  };
}
