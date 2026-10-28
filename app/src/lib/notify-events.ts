// The daemon's `notify.event` (API 12): the one place that decides "this is worth telling the user
// about". The app words it for its inbox and desktop notification; the daemon sends the same
// event to the phone channels. Against an older daemon the app still classifies status changes
// itself (store.ts), because only the daemon can say it for a window that is closed.
import { t } from "./i18n";
import type { InboxItem } from "./types";

/** First daemon API level with `notify.*`, `ui.focus`, `session.schedule_continue` and `session.set_env_restart`. */
export const NOTIFY_API = 12;

/** An event as the daemon sends it (crates/kelpied/src/notify.rs `Raised::to_json`). */
export interface NotifyEvent {
  id: string;
  kind: "waiting" | "finished" | "error" | "automation" | "approval";
  session: string | null;
  project_id: string | null;
  /** The process is still running (an error with a live process is a failed turn). */
  alive: boolean;
  exit_code: number | null;
  /** The automation's name, or the run and task of an approval. */
  detail: string | null;
  run: string | null;
  task: string | null;
  error: string | null;
  at: number;
}

/** What the app knows around an event: the session's label and host, the project's name. */
export interface NotifyContext {
  /** `sessionLabel(s)` when the session is known. */
  label?: string;
  /** The host a remote session runs on. */
  host?: string;
  projectName?: string;
}

export interface Described {
  kind: InboxItem["kind"];
  title: string;
  body: string;
}

/** The inbox entry and desktop notification for an event, in the interface language. */
export function describeNotify(ev: NotifyEvent, ctx: NotifyContext): Described {
  if (ev.kind === "automation") {
    return {
      kind: ev.error ? "error" : "automation",
      title: t("Automation · {name}", { name: ev.detail ?? "" }),
      body: ev.error ? t("Failed: {error}", { error: ev.error }) : t("Started"),
    };
  }
  if (ev.kind === "approval") {
    return { kind: "waiting", title: t("Run · {name} · {task}", { name: ev.run ?? "", task: ev.task ?? "" }), body: t("Waiting for your approval to start") };
  }
  const title = `${ctx.label ?? ""}${ctx.host ? " @" + ctx.host : ""}${ctx.projectName ? " · " + ctx.projectName : ""}`;
  if (ev.kind === "error") {
    return { kind: "error", title, body: ev.alive ? t("Stopped with an error") : t("Exited with code {code}", { code: ev.exit_code ?? "" }) };
  }
  return ev.kind === "waiting" ? { kind: "waiting", title, body: t("Needs your input") } : { kind: "finished", title, body: t("Finished") };
}

/** Why a "continue at reset" timer was dropped, as the daemon reports it. */
export type CancelReason = "exited" | "restarted" | "conversation" | "not_limited" | "input" | "daemon" | "user";

/** The inbox note for a dropped timer; none when the user cancelled it themselves. */
export function describeCancelled(reason: CancelReason | string, label: string): Described | null {
  const body: Record<string, string> = {
    exited: t("the session was closed or stopped"),
    restarted: t("the session was restarted"),
    conversation: t("the session moved to another conversation"),
    not_limited: t("the session is no longer stopped by the usage limit"),
    input: t("you typed in the session since its last prompt was sent"),
    daemon: t("the session daemon restarted"),
  };
  const why = body[reason];
  if (!why) return null;
  return { kind: "waiting", title: t("{name}: “continue” was not sent", { name: label }), body: t("Cancelled because {reason}.", { reason: why }) };
}
