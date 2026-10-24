// Saved commands/prompts: typed into the focused shell (or a new one), or sent as a prompt to
// the focused agent session.
import { store } from "./store";
import { t } from "./i18n";
import { sendInput } from "./ipc";
import { allPanes } from "./layout";
import { peekView } from "./terminal-host";
import { pasteInput, typedText } from "./typed-text";
import { getController } from "../chat/mode-switch";
import { pluginQuickCommands } from "./plugins";
import type { QuickCommand, SessionInfo } from "./types";

/** Commands for a project: global ones, its own, ones whose project was removed, then plugins' ones. */
export function quickCommandsFor(projectId: string | null): QuickCommand[] {
  const known = new Set(store.state.projects.map((p) => p.id));
  const own = (store.state.settings.quickCommands ?? []).filter((c) => !c.projectId || c.projectId === projectId || !known.has(c.projectId));
  return [...own, ...pluginQuickCommands()];
}

export function focusedSession(): SessionInfo | undefined {
  const p = store.state.activeProject;
  const tab = p ? store.activeTab(p) : undefined;
  const pane = tab && allPanes(tab.root).find((x) => x.id === tab.focus);
  return pane?.sessionId ? store.sessions[pane.sessionId] : undefined;
}

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Delivers `text` like a real paste: xterm adds bracketed-paste markers only when the program
 *  asked for them and turns newlines into Enter keys, exactly as when the user pastes. */
export async function typeInto(s: SessionInfo, raw: string, enter: boolean, enterDelay: number) {
  const view = peekView(s.id);
  // A view between attachments (e.g. during a daemon reconnect) would drop the paste: it goes
  // straight to the session, still inside the paste markers the program asked for.
  if (view?.handle != null) view.paste(typedText(raw));
  else await sendInput(s.handle, pasteInput(raw, !!view?.term.modes.bracketedPasteMode));
  if (!enter) return;
  // Agent TUIs treat Enter that follows a burst of input as part of the paste (Codex: within
  // 120 ms) and insert a newline instead of submitting.
  await sleep(enterDelay);
  await sendInput(s.handle, "\r");
}

export async function runQuickCommand(c: QuickCommand) {
  const project = store.activeProject();
  if (!project) return store.notify(t("Open a project first"));
  const s = focusedSession();
  if (c.action === "agent") {
    if (!s?.alive || s.kind === "shell" || s.kind === "custom") return store.notify(t("Focus an agent session to send \"{name}\"", { name: c.label }));
    if (s.chat) return getController(s).send(c.text);
    return typeInto(s, c.text, c.enter, 250);
  }
  let target = s?.alive && s.kind === "shell" && !s.chat ? s : undefined;
  // No shell focused: open one. The PTY buffers input until the shell reads it.
  target ??= await store.newSession(project.id, "shell");
  if (target) await typeInto(target, c.text, c.enter, 60);
}
