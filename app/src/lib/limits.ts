// A session stopped by the subscription's usage limit: when the limit lifts (the usage meter's
// reset time) and the daemon's "continue at reset" timer. The timer lives in the daemon, so it
// fires with the window closed; it types only into the same run of the same conversation, and
// only if nobody typed since (see crates/kelpied/src/scheduled_input.rs).
import { createEffect } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { store, sessionLabel } from "./store";
import { daemon, usageFetch } from "./ipc";
import { sessionHome } from "./accounts";
import { NOTIFY_API, describeCancelled } from "./notify-events";
import { errorText, t } from "./i18n";
import type { ProviderUsage, SessionInfo } from "./types";

/** Fire time (wall-clock ms) of each session's "continue" timer, by session id. */
export const [scheduled, setScheduled] = createStore<Record<string, number>>({});

/** A session the banner is for: a terminal agent the daemon says the usage limit stopped. */
export const isLimited = (s: SessionInfo | undefined): s is SessionInfo => !!s && !!s.limited && s.alive && !s.chat;

/** The usage windows of the account this session runs on. */
export async function usageFor(s: SessionInfo): Promise<ProviderUsage | null> {
  if (s.kind !== "claude" && s.kind !== "codex") return null;
  const home = sessionHome(s);
  const report = await usageFetch(s.kind === "claude" ? home : undefined, s.kind === "codex" ? home : undefined);
  return report[s.kind];
}

export async function scheduleContinue(s: SessionInfo, at: number, source: "meter" | "confirmed") {
  try {
    await daemon("session.schedule_continue", { handle: s.handle, at, source });
  } catch (e) {
    store.notify(errorText(e));
  }
}

export async function cancelContinue(s: SessionInfo) {
  await daemon("session.cancel_scheduled", { handle: s.handle }).catch((e) => store.notify(errorText(e)));
}

/** The timers the daemon has now replace the list (a merge would keep ones it no longer has), and the
 *  ones a daemon that stopped left behind each get an inbox note, once. */
export async function loadScheduled() {
  const r = await daemon<{ timers: { session: string; fire_at: number }[]; lost: { session: string }[] }>("session.scheduled").catch(() => null);
  if (!r) return;
  setScheduled(reconcile(Object.fromEntries(r.timers.map((i) => [i.session, i.fire_at])) as Record<string, number>));
  for (const l of r.lost) noteCancelled(l.session, "daemon");
}

function noteCancelled(session: string, reason: string) {
  const s = store.sessions[session];
  const note = describeCancelled(reason, s ? sessionLabel(s) : t("A session"));
  if (note) store.addInbox({ kind: note.kind, title: note.title, body: note.body, sessionId: s?.id, projectId: s?.project_id ?? undefined });
}

interface ScheduledEvent {
  session: string;
  at: number | null;
  result?: "sent" | "cancelled";
  reason?: string;
}

function onScheduled(p: ScheduledEvent) {
  setScheduled(p.session, p.at ?? undefined!);
  if (p.result === "sent") {
    const s = store.sessions[p.session];
    store.notify(t("Typed “continue” in {name}", { name: s ? sessionLabel(s) : "" }));
  } else if (p.result === "cancelled") {
    noteCancelled(p.session, p.reason ?? "");
  }
}

/** Keeps the timer list in step with the daemon (call once, from a component). */
export function initLimits() {
  window.addEventListener("kelpie:scheduled", (e) => onScheduled((e as CustomEvent<ScheduledEvent>).detail));
  createEffect(() => {
    if (store.connected() && store.daemonApi() >= NOTIFY_API) void loadScheduled();
  });
}
