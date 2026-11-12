// Starting an agent from the phone (`new_options`, `new_session`), as data: the request, what the computer's answers mean to the person, and
// the order the projects are offered in. The sheet (new-session.tsx) and the action (phone-state.ts) use these; the link only carries them.
import { t } from "./i18n";
import type { NewOptions, Outcome, SessionRow } from "./link";

/** What a `new_session` asks for. `cid` names this one start: a retry of it (a lost reply) gets the same session, never a second. */
export interface NewSessionRequest {
  project_id: string;
  kind: string;
  prompt?: string;
  cid: string;
}

/** The request for a start: the first prompt trimmed, and left out when there is none. */
export function newSessionRequest(projectId: string, kind: string, prompt: string, cid: string): NewSessionRequest {
  const first = prompt.trim();
  return { project_id: projectId, kind, ...(first ? { prompt: first } : {}), cid };
}

/** What the phone says while the computer has no launch catalogue (the daemon started and the app has not been opened since). */
export const notReadyText = () => t("Open Kelpie on your computer once so this phone can start sessions.");

/** Why something is not offered here, said once under the choices. */
export const notOfferedText = () => t("Projects on other computers (SSH) and chat-only agents cannot be started from the phone.");

export type OptionsResult = { ok: true; options: NewOptions } | { ok: false; message: string };

/** What the computer said to `new_options`, as the sheet shows it. */
export function optionsOutcome(o: Outcome): OptionsResult {
  switch (o.t) {
    case "ack":
      if (o.ready === false || !o.projects || !o.agents) return { ok: false, message: notReadyText() };
      return { ok: true, options: { ready: true, projects: o.projects, agents: o.agents } };
    case "error":
      return { ok: false, message: refusal(o.code, o.message) };
    case "offline":
      return { ok: false, message: t("You are offline. Try again when the connection is back.") };
    default:
      return { ok: false, message: t("No answer from your computer. Try again.") };
  }
}

export type StartResult = { ok: true; id: string } | { ok: false; message: string };

/** What the computer said to `new_session`: the new session's id, or why not in the person's words. */
export function startOutcome(o: Outcome): StartResult {
  switch (o.t) {
    case "ack":
      return o.id ? { ok: true, id: o.id } : { ok: false, message: t("No answer from your computer. Look in the list before you try again: it may have started.") };
    case "error":
      return { ok: false, message: refusal(o.code, o.message) };
    case "offline":
      return { ok: false, message: t("You are offline. Start it when the connection is back.") };
    default:
      return { ok: false, message: t("No answer from your computer. Look in the list before you try again: it may have started.") };
  }
}

/** A refusal in plain words. The computer's own (English) message stands for a code this phone does not know. */
function refusal(code: string, message: string): string {
  switch (code) {
    case "not_ready":
      return notReadyText();
    case "busy":
      return t("Your computer is too busy to start a session now. Try again in a minute.");
    case "rate_limited":
      return t("Starting sessions too fast: wait a few seconds and try again.");
    case "not_installed":
      return t("That agent is not installed on your computer.");
    case "no_project":
      return t("That project cannot be started from the phone: it is gone from your computer, or it runs over SSH.");
    case "no_agent":
      return t("That agent cannot be started from the phone: it is hidden on your computer, or chat-only.");
    case "no_folder":
      return t("That project's folder is not on your computer any more.");
    case "too_long":
      return t("The first prompt is too long.");
    case "read_only":
      return t("This phone may read but not type, so it cannot start sessions.");
    case "epoch":
      return t("Kelpie restarted on your computer. Try again.");
    default:
      return message;
  }
}

/** The projects as the sheet offers them: those with sessions first, the most recently active first, then the rest in the computer's order. */
export function projectChoices(projects: { id: string; name: string }[], sessions: Pick<SessionRow, "project_id" | "last_activity">[]): { id: string; name: string }[] {
  const latest = new Map<string, number>();
  for (const s of sessions) if (s.project_id) latest.set(s.project_id, Math.max(latest.get(s.project_id) ?? 0, s.last_activity || 1));
  const at = (id: string) => latest.get(id) ?? 0;
  return projects.map((p, i) => ({ p, i })).sort((a, b) => at(b.p.id) - at(a.p.id) || a.i - b.i).map(({ p }) => p);
}
