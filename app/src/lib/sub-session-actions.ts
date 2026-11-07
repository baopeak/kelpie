// What the user does with sub-sessions from the window: closing the finished ones, answering a request. Kept apart from
// `sub-sessions.ts` (pure) because it talks to the store and the daemon.
import { store, sessionLabel } from "./store";
import * as ipc from "./ipc";
import { t } from "./i18n";
import { confirmDialog } from "./confirm-dialog";
import { dispatchRequest, finishedChildren, setSubPolicy, showPostponed, toSubPolicy, type SubDecision } from "./sub-sessions";
import type { SessionInfo } from "./types";

/** The sub-sessions of `parent` that would be closed by *Close finished sub-sessions*. */
export const finishedOf = (parent: SessionInfo): SessionInfo[] => {
  const tree = store.sessionTree();
  return finishedChildren(tree.children.get(parent.id) ?? [], tree);
};

/** Closes the sub-sessions of `parent` that said they are done or exited, after a question that lists them. Their folders and
 *  worktrees stay: closing a session never removes one. Returns how many were closed. */
export async function closeFinishedSubSessions(parent: SessionInfo): Promise<number> {
  const finished = finishedOf(parent);
  if (!finished.length) {
    store.notify(t("No finished sub-sessions to close."));
    return 0;
  }
  const list = finished.map((c) => `• ${sessionLabel(c)}`).join("\n");
  if (!(await confirmDialog(t("Close these finished sub-sessions?") + `\n\n${list}\n\n` + t("Their folders and worktrees are kept.")))) return 0;
  let closed = 0;
  for (const child of finished) {
    // Read again: one may have started working, or been closed, while the question was open.
    const now = store.sessions[child.id];
    if (!now || !finishedChildrenNow(now)) continue;
    try {
      const loc = store.paneLocation(now.id);
      if (loc) await store.closePane(loc.projectId, loc.tabId, loc.paneId);
      else await ipc.daemon("session.close", { handle: now.handle });
      closed++;
    } catch (e) {
      store.notify(String(e));
    }
  }
  return closed;
}

const finishedChildrenNow = (s: SessionInfo) => finishedChildren([s], store.sessionTree()).length === 1;

/** Puts the keyboard on the first request card (showing the ones put off with "Later"). Does nothing without a request. */
export function focusFirstSubRequest() {
  showPostponed();
  // After the cards are drawn; a button is disabled for a moment after its card appears, so the card itself is the fallback.
  setTimeout(() => {
    const card = document.querySelector<HTMLElement>("[data-sub-request]");
    (card?.querySelector<HTMLElement>("[data-sub-primary]:not(:disabled)") ?? card)?.focus();
  }, 0);
}

/** Sends the user's answer to a request. A request that is gone (answered in another window, timed out) is dropped quietly.
 *  Resolves true when the card can go. */
export async function answerSubRequest(id: string, decision: SubDecision): Promise<boolean> {
  try {
    await ipc.subAnswer(id, decision);
  } catch (e) {
    const message = String(e);
    if (!/already answered|timed out|gone/i.test(message)) {
      store.notify(message.replace(/^Error: /, ""), undefined, 8000);
      return false;
    }
    store.notify(t("That request was already answered or has run out of time."));
  }
  dispatchRequest({ type: "done", id });
  // "Always" changed the policy: the settings page and the next request must see it.
  if (decision === "always") setSubPolicy(toSubPolicy(await ipc.subPolicyGet().catch(() => null)));
  return true;
}
