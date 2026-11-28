// What the session menu and the Priorities page do to a session's priority, with the person's answers the daemon asks for
// (a third boost) and its refusals shown as notices.
import { createSignal } from "solid-js";
import { store, sessionLabel } from "./store";
import { confirmDialog } from "./confirm-dialog";
import { t } from "./i18n";
import { applyPriorities, priorities, setSessionPriority, type BoostLength, type Reserve, type Tier } from "./priorities";
import type { SessionInfo } from "./types";

/** What the Priorities page opens on (a project or session id), set by the menus that open it. */
const [priorityFocus, setPriorityFocus] = createSignal<string | null>(null);
export { priorityFocus };

export function openPriorities(focus: string | null = null) {
  setPriorityFocus(focus);
  store.setOverlay("priorities");
}

const label = (id: string) => {
  const s = store.sessions[id];
  return s ? sessionLabel(s) : id;
};

/** Boosts `s` for `length`. With two boosted already, asks whether the one boosted first should end instead. */
export async function boostSession(s: SessionInfo, length: BoostLength) {
  try {
    applyPriorities(await setSessionPriority(s.id, { boost: length }));
  } catch (e) {
    const boosted = (priorities()?.boosted ?? []).filter((id) => id !== s.id);
    if (!/boosted already/.test(String(e)) || boosted.length < 2) return store.notify(String(e));
    const oldest = boosted[0];
    const ask = t("Two sessions are boosted already: {a} and {b}. End the boost of {a} and boost {name} instead?", { a: label(oldest), b: label(boosted[1]), name: sessionLabel(s) });
    if (!(await confirmDialog(ask))) return;
    await setSessionPriority(s.id, { boost: length, replace: oldest }).then(applyPriorities, (err) => store.notify(String(err)));
  }
}

export const endBoost = (s: SessionInfo) => setSessionPriority(s.id, { boost: "off" }).then(applyPriorities, (e) => store.notify(String(e)));

/** A session's own tier, or back to its project's (`project`). */
export const setSessionTier = (s: SessionInfo, tier: Tier | "project") => setSessionPriority(s.id, { tier }).then(applyPriorities, (e) => store.notify(String(e)));

export const setSessionReserve = (s: SessionInfo, reserve: Reserve | null) =>
  setSessionPriority(s.id, { reserve }).then(applyPriorities, (e) => store.notify(String(e)));
