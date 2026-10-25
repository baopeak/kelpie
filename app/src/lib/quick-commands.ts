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

/** The quick command whose text is the default "Review with…" prompt (edit it in Settings → Quick commands). */
export const REVIEW_PROMPT_LABEL = "Review prompt";

export const DEFAULT_REVIEW_PROMPT =
  "You are reviewing changes another agent made in this worktree; you did not write them. Read the diff below like a careful maintainer: look for bugs, missed edge cases, security problems, unrelated edits and missing tests. Do not change any files. Reply with a short verdict and the problems that matter most, each naming a file and line.";

/** The review prompt for a project: the user's quick command labelled "Review prompt", else the built-in text. */
export function reviewPrompt(projectId: string | null): string {
  const own = quickCommandsFor(projectId).find((c) => c.label.trim().toLowerCase() === REVIEW_PROMPT_LABEL.toLowerCase() && c.text.trim());
  return own ? own.text : DEFAULT_REVIEW_PROMPT;
}

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
