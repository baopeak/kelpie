// Conversation upkeep for a running agent: compact or clear it with the agent's own slash
// commands (typed into its TUI, or sent through chat view), or restart its process on the same
// conversation. A long-lived TUI process grows heavy; a fresh one renders a small tree again.
import { store, sessionLabel } from "./store";
import { daemon, redact } from "./ipc";
import { kindInfo } from "./agents";
import { sessionTarget } from "./remote";
import { typeInto } from "./quick-commands";
import { confirmDialog } from "./confirm-dialog";
import { getController } from "../chat/mode-switch";
import { t } from "./i18n";
import type { SessionInfo } from "./types";

/** The agents' own commands (verified against each CLI's command list). */
const COMMANDS: Record<string, { compact: string; clear?: string }> = {
  claude: { compact: "/compact", clear: "/clear" },
  codex: { compact: "/compact", clear: "/new" },
  omp: { compact: "/compact" },
};

export const conversationCommands = (s: SessionInfo) => (s.alive ? COMMANDS[s.kind] : undefined);

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function okToInterrupt(s: SessionInfo): Promise<boolean> {
  return s.status !== "working" || confirmDialog(t("{name} is working. Continue anyway?", { name: sessionLabel(s) }));
}

export async function runConversationCommand(s: SessionInfo, which: "compact" | "clear") {
  const cmd = COMMANDS[s.kind]?.[which];
  if (!cmd || !(await okToInterrupt(s))) return;
  if (s.chat) {
    // A chat not shown since the app started has no stream yet: show it, then send.
    const ctrl = getController(s);
    if (!ctrl.state.ready) store.revealSession(s.id);
    const end = Date.now() + 5000;
    while (!ctrl.state.ready && Date.now() < end) await sleep(100);
    if (!ctrl.state.ready) return store.notify(t("{name} is not ready yet.", { name: sessionLabel(s) }));
    ctrl.send(cmd);
  } else await typeInto(s, cmd, true, 250);
}

/** The last lines of a terminal session as plain text (scrollback included), capped in size. */
export async function sessionTranscript(s: SessionInfo, lines = 400, maxChars = 12000): Promise<string> {
  const r = await daemon<{ text: string }>("session.read", { handle: s.handle, lines });
  const text = r.text.trimEnd();
  return text.length > maxChars ? "…\n" + text.slice(text.length - maxChars) : text;
}

/** Starts another agent seeded with this session's recent transcript (this one keeps running). */
export async function handOffSession(s: SessionInfo, kind: string) {
  if (!s.project_id) return store.notify(t("Hand-off needs a project session"));
  const text = await sessionTranscript(s).then(redact).catch(() => "");
  const prompt = `You are taking over a task from another coding agent (${kindInfo(s.kind).label}) working in this folder. Below is the end of its terminal session. Work out what was being done and what is left, then continue.\n\n----- transcript -----\n${text}\n----- end -----`;
  // On the same machine: an agent on a host hands off to one there, next to it.
  await store.newSession(s.project_id, kind, { prompt, where: "right", target: sessionTarget(s) });
}

/** Stops the agent and starts it again on the same conversation, in the same pane. */
export async function restartKeepingConversation(s: SessionInfo) {
  if (!s.alive || !store.resumable(s) || !(await okToInterrupt(s))) return;
  await daemon("session.kill", { handle: s.handle }).catch(() => {});
  const end = Date.now() + 5000;
  while (store.sessions[s.id]?.alive && Date.now() < end) await sleep(100);
  if (store.sessions[s.id]?.alive) return store.notify(t("{name} did not stop.", { name: sessionLabel(s) }));
  await store.resumeSession(s.id);
}
