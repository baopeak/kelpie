// Switching a session between its terminal UI (TUI) and Kelpie's chat view.
//
// Neither Claude Code, Codex nor OMP lets a second client drive a running TUI's
// conversation, so a switch is a hand-off of the *conversation*, not of the process:
//   1. find the agent-side conversation id (hooks, transcript dir, or thread/list),
//   2. stop the old process cleanly (Ctrl+C ×2, then kill) so it flushes its transcript and
//      we never have two writers on one conversation,
//   3. start the other front-end resuming that id, in the same pane.
import { ChatController, type Adapter, type AdapterContext } from "./chat-model";
import { applyClaudeMessage, claudeArgs, createClaudeAdapter } from "./claude-adapter";
import { codexArgs, createCodexAdapter } from "./codex-adapter";
import { createAcpAdapter } from "./acp-adapter";
import { CORE_AGENTS, acpArgv } from "../lib/agents";
import { type AgentSettings, claudeSettingsFrom, withSettings } from "./agent-settings";
import { agentLatestSession, claudeTranscript, createSession, daemon, sendInput } from "../lib/ipc";
import { store, type PaneTarget } from "../lib/store";
import { disposeView } from "../lib/terminal-host";
import { uid } from "../lib/platform";
import { HOME_VAR, isAccountAgent, sessionHome } from "../lib/accounts";
import { t } from "../lib/i18n";
import type { Pane, Project, SessionInfo, SessionKind, Tab } from "../lib/types";
import { confirmDialog } from "../lib/confirm-dialog";

/** Claude Code and Codex have native chat protocols; every other agent with an ACP mode chats
 *  through the generic ACP adapter. */
export const supportsChat = (kind: string) => kind === "claude" || kind === "codex" || acpArgv(kind) !== null;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Panes with a switch in flight (double clicks must not start two front-ends). */
const switching = new Set<string>();

/** Another live session of this agent in the same folder makes "newest conversation" ambiguous. */
function ambiguous(s: SessionInfo): boolean {
  const norm = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
  return Object.values(store.sessions).some((o) => o.id !== s.id && o.alive && o.kind === s.kind && norm(o.cwd) === norm(s.cwd));
}

async function stopProcess(s: SessionInfo, polite: () => Promise<void>): Promise<boolean> {
  if (!s.alive) return true;
  await polite();
  if (await waitExit(s.id, 3000)) return true;
  await daemon("session.kill", { handle: s.handle }).catch(() => {});
  return waitExit(s.id, 3000);
}

async function waitExit(sessionId: string, ms: number): Promise<boolean> {
  const end = Date.now() + ms;
  while (Date.now() < end) {
    const s = store.sessions[sessionId];
    if (!s || !s.alive) return true;
    await sleep(100);
  }
  return false;
}

/** A session's own environment (profile variables and its account), when it has one. */
const ownEnv = (s: SessionInfo) => (Object.keys(s.env ?? {}).length ? s.env : undefined);

/** Environment of a new chat: an explicit account, a history item's home, else the active account. */
function chatEnv(kind: SessionKind, opts: { account?: string; home?: string | null }): Record<string, string> | undefined {
  const env = store.profileLaunch(kind, { account: opts.home === null ? "default" : opts.account }).env ?? {};
  if (opts.home && isAccountAgent(kind)) env[HOME_VAR[kind]] = opts.home;
  return Object.keys(env).length ? env : undefined;
}

/** `settings`: what the conversation ran with so far (Claude only: Codex's app-server and OMP
 *  restore the conversation's own settings themselves). */
function chatArgv(kind: SessionKind, resumeId: string | null, settings: AgentSettings = {}): string[] {
  const profile = store.profileLaunch(kind, { yolo: false }).args;
  if (kind === "claude") return claudeArgs(resumeId, withSettings(kind, profile, settings));
  if (kind === "codex") return codexArgs(profile);
  const acp = acpArgv(kind);
  if (!acp) throw new Error(`${kind} has no chat mode`);
  return acp;
}

/** Claude's mode, model and effort as its transcript last recorded them, else its own flags. */
async function claudeSettings(s: SessionInfo, conv: string | null): Promise<AgentSettings> {
  const lines = conv ? await claudeTranscript(conv, s.cwd, sessionHome(s)).catch(() => [] as string[]) : [];
  return claudeSettingsFrom(lines, s.args ?? []);
}

/** What chat view's pickers are set to. */
function chatSettings(ctrl: ChatController | undefined): AgentSettings {
  const value = (id: string) => ctrl?.state.settings.find((x) => x.id === id)?.value ?? null;
  return { model: value("model"), effort: value("effort"), mode: value("mode") };
}

const controllers = new Map<string, ChatController>();

/** One controller per chat session, kept across pane remounts (project switches). */
export function getController(s: SessionInfo): ChatController {
  let c = controllers.get(s.id);
  if (c) return c;
  const make = (ctx: AdapterContext): Adapter =>
    s.kind === "claude" ? createClaudeAdapter(ctx) : s.kind === "codex" ? createCodexAdapter(ctx) : createAcpAdapter(ctx);
  c = new ChatController(s.id, s.cwd, s.agent_session_id, make);
  if (s.kind === "claude" && s.agent_session_id) {
    const resumeId = s.agent_session_id;
    const createdAt = s.created_at;
    // History from before chat mode comes from Claude's own transcript; turns made in chat
    // mode are replayed from the daemon log, so only take lines older than this process.
    c.loadHistory = async (ctx) => {
      const lines = await claudeTranscript(resumeId, s.cwd, sessionHome(s)).catch(() => []);
      c!.seed(claudeSettingsFrom(lines));
      const live = { msgId: null as string | null };
      for (const line of lines) {
        let m: any;
        try { m = JSON.parse(line); } catch { continue; }
        if (m.timestamp && Date.parse(m.timestamp) >= createdAt) break;
        if (m.type === "user" || m.type === "assistant") applyClaudeMessage(ctx, m, live);
      }
      if (ctx.state.items.length) ctx.upsert({ kind: "system", id: "history-mark", text: t("↑ earlier in this conversation · continued in chat view ↓"), tone: "info" });
    };
  }
  controllers.set(s.id, c);
  return c;
}

export function disposeController(sessionId: string) {
  controllers.get(sessionId)?.detach();
  controllers.delete(sessionId);
}

/** Starts a conversation directly in chat mode: a new one, or `resume` (an agent conversation
 *  id) in that conversation's own folder `cwd`. */
export async function newChat(projectId: string, kind: SessionKind, where: "tab" | "right" | "down" = "tab", opts: { resume?: string; cwd?: string; account?: string; home?: string | null; into?: PaneTarget } = {}) {
  const project = store.state.projects.find((p) => p.id === projectId);
  if (!project) return;
  try {
    const resume = opts.resume ?? null;
    const info = await createSession({
      kind, cwd: opts.cwd ?? project.path, project_id: projectId,
      agent_session_id: resume ?? undefined,
      chat_argv: chatArgv(kind, kind === "claude" ? resume : null),
      env: chatEnv(kind, opts),
    });
    store.setSessions(info.id, info);
    store.placePane(projectId, { id: uid(), kind: "chat", sessionId: info.id }, where, opts.into);
    return info;
  } catch (e) {
    store.notify(t("Could not start {agent} chat: {error}", { agent: kind, error: String(e) }));
  }
}

export async function switchToChat(project: Project, tab: Tab, pane: Pane) {
  if (switching.has(pane.id)) return;
  switching.add(pane.id);
  try {
    await doSwitchToChat(project, tab, pane);
  } finally {
    switching.delete(pane.id);
  }
}

async function doSwitchToChat(project: Project, tab: Tab, pane: Pane) {
  const s = pane.sessionId ? store.sessions[pane.sessionId] : undefined;
  if (!s || !supportsChat(s.kind)) return;
  if (s.alive && s.status === "working" && !(await confirmDialog(t("The agent is working. Interrupt it and continue in chat view?")))) return;
  let conv = s.agent_session_id;
  if (!conv && ambiguous(s)) {
    store.notify(t("This {agent} session has not reported its conversation id yet and other {agent} sessions run in this folder — finish one turn first, then switch.", { agent: s.kind }));
    return;
  }
  if (!conv && (s.kind === "claude" || s.kind === "omp")) conv = (await agentLatestSession(s.kind, s.cwd, sessionHome(s)).catch(() => null))?.id ?? null;
  if (s.kind !== "codex" && !conv) {
    store.notify(t("No conversation to continue yet — starting a new chat."));
  }
  store.notify(t("Switching {agent} to chat view…", { agent: s.kind }));
  const stopped = await stopProcess(s, async () => {
    await sendInput(s.handle, "\x03");
    await sleep(300);
    await sendInput(s.handle, "\x03");
  });
  if (!stopped) {
    store.notify(t("The terminal session did not stop; not starting a second writer."));
    return;
  }
  try {
    // Read after the TUI stopped: a mode switched just before is in the transcript by now.
    const settings = s.kind === "claude" ? await claudeSettings(s, conv) : {};
    const info = await createSession({
      kind: s.kind, cwd: s.cwd, project_id: s.project_id ?? project.id, name: s.name ?? undefined,
      // Codex without a known thread id resumes the newest thread of this folder.
      agent_session_id: conv ?? (s.kind === "codex" ? "latest" : undefined),
      chat_argv: chatArgv(s.kind, s.kind === "claude" ? conv : null, settings),
      env: ownEnv(s),
    });
    store.setSessions(info.id, info);
    store.replacePaneIn(project.id, tab.id, pane.id, { id: pane.id, kind: "chat", sessionId: info.id });
    disposeView(s.id);
    await daemon("session.close", { handle: s.handle }).catch(() => {});
  } catch (e) {
    store.notify(t("Chat mode failed: {error}", { error: String(e) }));
  }
}

/** `command`: typed into the terminal UI once it is ready (a command chat view cannot run). */
export async function switchToTerminal(project: Project, tab: Tab, pane: Pane, command?: string) {
  if (switching.has(pane.id)) return;
  switching.add(pane.id);
  try {
    await doSwitchToTerminal(project, tab, pane, command);
  } finally {
    switching.delete(pane.id);
  }
}

async function doSwitchToTerminal(project: Project, tab: Tab, pane: Pane, command?: string) {
  const c = pane.sessionId ? store.sessions[pane.sessionId] : undefined;
  if (!c) return;
  const ctrl = controllers.get(c.id);
  if (ctrl?.state.busy && !(await confirmDialog(t("The agent is working. Interrupt it and continue in the terminal?")))) return;
  let conv = ctrl?.state.conversationId ?? c.agent_session_id;
  if (conv === "latest") conv = null;
  const stopped = await stopProcess(c, async () => {
    ctrl?.interrupt();
    await sleep(200);
    await daemon("chat.close_stdin", { handle: c.handle }).catch(() => {});
  });
  if (!stopped) {
    store.notify(t("The chat process did not stop; not starting a second writer."));
    return;
  }
  try {
    // The terminal front-end gets the profile's flags back (chat argv used its own form), with
    // what chat view's pickers are set to in place of the profile's own model / effort / mode.
    const picks = chatSettings(ctrl);
    const launch = store.profileLaunch(c.kind, picks.mode ? { yolo: false } : {});
    const args = withSettings(c.kind, launch.args, picks);
    // Only agents whose terminal UI is known to resume an ACP / chat conversation by its id get
    // it; for the others the id may mean nothing to the CLI, so their terminal starts fresh.
    const resumes = CORE_AGENTS.includes(c.kind);
    const info = await createSession({ kind: c.kind, cwd: c.cwd, project_id: c.project_id ?? project.id, name: c.name ?? undefined, agent_session_id: resumes ? conv ?? undefined : undefined, args, env: ownEnv(c) ?? launch.env, cols: 120, rows: 32, prompt: command });
    store.setSessions(info.id, info);
    store.replacePaneIn(project.id, tab.id, pane.id, { id: pane.id, kind: "terminal", sessionId: info.id });
    disposeController(c.id);
    await daemon("session.close", { handle: c.handle }).catch(() => {});
  } catch (e) {
    store.notify(t("Terminal mode failed: {error}", { error: String(e) }));
  }
}

/** A chat session stopped by a daemon restart (or hibernation): a new chat front-end resuming
 *  its conversation takes its place in the same pane, on the same account and environment. */
async function resumeChat(s: SessionInfo) {
  const conv = s.agent_session_id && s.agent_session_id !== "latest" ? s.agent_session_id : null;
  try {
    const settings = s.kind === "claude" ? await claudeSettings(s, conv) : {};
    const info = await createSession({
      kind: s.kind, cwd: s.cwd, project_id: s.project_id ?? undefined, name: s.name ?? undefined,
      agent_session_id: conv ?? (s.kind === "codex" ? "latest" : undefined),
      chat_argv: chatArgv(s.kind, s.kind === "claude" ? conv : null, settings),
      env: ownEnv(s) ?? store.profileLaunch(s.kind).env,
    });
    store.setSessions(info.id, info);
    const loc = store.paneLocation(s.id);
    if (loc) store.replacePaneIn(loc.projectId, loc.tabId, loc.paneId, { id: loc.paneId, kind: "chat", sessionId: info.id });
    disposeController(s.id);
    await daemon("session.close", { handle: s.handle }).catch(() => {});
  } catch (e) {
    store.notify(t("Could not resume the {agent} chat: {error}", { agent: s.kind, error: String(e) }));
  }
}

store.setChatResumer(resumeChat);
