// Sending a prepared prompt (review notes, a hand-off, a failed commit) to an agent: one of
// the project's running agents, or a new session that starts with it.
import { store, sessionLabel } from "./store";
import { t } from "./i18n";
import { focusedSession, typeInto } from "./quick-commands";
import { openComposer } from "./composer-state";
import type { Selection } from "./mentions";
import { redact } from "./ipc";
import { getController } from "../chat/mode-switch";
import { isAgentKind, kindInfo, launchableAgents } from "./agents";
import { openContextMenu, type MenuItem } from "../components/context-menu";
import type { SessionInfo } from "./types";

/** Running agent sessions of a project, most recently active first. */
export function agentSessions(projectId: string): SessionInfo[] {
  return Object.values(store.sessions)
    .filter((s) => s.project_id === projectId && s.alive && isAgentKind(s.kind))
    .sort((a, b) => b.last_activity - a.last_activity);
}

/** `submit` false leaves the text at the prompt for the user to read and send (terminals only). */
export async function sendToSession(s: SessionInfo, text: string, submit = true) {
  if (s.chat) await getController(s).send(text);
  else await typeInto(s, text, submit, 250);
  store.revealSession(s.id);
}

/** Menu at the pointer: running agents first, then "New … session" for each launchable agent.
 *  Credentials in `text` (a diff line, an issue body) are masked before it reaches the agent. */
export function chooseAgentAndSend(e: MouseEvent, projectId: string, text: string, onSent?: () => void) {
  const running: MenuItem[] = agentSessions(projectId).map((s) => ({
    label: t("Send to {name}", { name: sessionLabel(s) }),
    run: () => void redact(text).then((safe) => sendToSession(s, safe)).then(onSent),
  }));
  const fresh: MenuItem[] = launchableAgents(store.state.settings.hiddenAgents).map((kind) => ({
    label: t("New {agent} session", { agent: kindInfo(kind).label }),
    run: () => void redact(text).then((safe) => store.newSession(projectId, kind, { prompt: safe })).then((s) => s && onSent?.()),
  }));
  openContextMenu(e, running.length ? [...running, null, ...fresh] : fresh);
}

/** Menu at the pointer to pick the session whose composer takes `selection` (as `@selection`, so
 *  the user adds the instruction before sending): a running agent, or a new session. */
export function sendSelectionToAgent(e: MouseEvent, projectId: string, selection: Selection) {
  const draft = "@selection ";
  const running: MenuItem[] = agentSessions(projectId).map((s) => ({
    label: t("Send to {name}", { name: sessionLabel(s) }),
    run: () => openComposer(s.id, { draft, selection }),
  }));
  const fresh: MenuItem[] = launchableAgents(store.state.settings.hiddenAgents).map((kind) => ({
    label: t("New {agent} session", { agent: kindInfo(kind).label }),
    run: () => void store.newSession(projectId, kind).then((s) => s && openComposer(s.id, { draft, selection })),
  }));
  openContextMenu(e, running.length ? [...running, null, ...fresh] : fresh);
}

/** Opens the composer of the focused session (else the project's latest agent) with `draft`. */
export function composeWith(projectId: string, draft: string) {
  const focused = focusedSession();
  const target = focused?.alive && focused.project_id === projectId ? focused : agentSessions(projectId)[0];
  if (!target) return store.notify(t("Focus or start a session to compose a prompt in"));
  openComposer(target.id, { draft });
}
