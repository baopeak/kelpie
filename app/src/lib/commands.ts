// Command registry + keymap. One table drives keyboard shortcuts and the command palette.
//
// Defaults avoid stealing keys TUIs need: on Windows/Linux app commands use Ctrl+Shift
// (Ctrl+P/J/K/O/G… are used by Claude Code, Codex, OMP and readline); on macOS Cmd is
// never sent to the terminal, so plain Cmd chords are safe.
import { open } from "@tauri-apps/plugin-dialog";
import { store } from "./store";
import { GRID_PRESETS, allPanes, cyclePane, findPane, gridLabel, paneInDirection } from "./layout";
import { chordLabel, isMac } from "./platform";
import { allViews, peekView, setAppShortcutMatcher, setOverlayCheck } from "./terminal-host";
import { navBack, navForward } from "./nav-history";
import { addExistingWorktrees } from "./worktrees";
import { openFinish, openReviewWith, openTryAgents } from "./review-queue";
import { waitingSessions } from "./attention";
import { openWorktreeDialog } from "../components/worktree-dialog";
import { openRuns } from "../components/runs-page";
import { openHosts } from "../components/hosts-page";
import { openBrowser } from "./browser";
import { toggleDictation } from "./voice";
import type { SessionKind } from "./types";
import { msg, t } from "./i18n";
import { confirmDialog } from "./confirm-dialog";
import { focusedSession, newSpec } from "./quick-commands";
import { CHECKPOINT_API } from "./checkpoint-api";
import { toggleComposer } from "./composer-state";
import { conversationCommands, restartKeepingConversation, runConversationCommand } from "./agent-actions";

export interface Command {
  id: string;
  label: string;
  /** Chord per platform, e.g. "mod+shift+p". `mod` = Cmd on macOS, Ctrl elsewhere. */
  win?: string;
  mac?: string;
  run: () => void;
  when?: () => boolean;
}

const pid = () => store.state.activeProject;
const hasProject = () => !!pid();

function focusedPane() {
  const p = pid();
  const tab = p ? store.activeTab(p) : undefined;
  return p && tab ? { projectId: p, tab, paneId: tab.focus } : null;
}

/** The focused session when it is a running agent with conversation commands. */
function agentHere() {
  const s = focusedSession();
  const conv = s && conversationCommands(s);
  return s && conv ? { s, conv } : undefined;
}

function newSession(kind: SessionKind, where: "tab" | "right" | "down" = "tab") {
  const p = pid();
  if (p) void store.newSession(p, kind, { where });
}

function moveActiveTab(delta: 1 | -1) {
  const f = focusedPane();
  if (!f) return;
  const i = store.workspace(f.projectId).tabs.findIndex((x) => x.id === f.tab.id);
  store.moveTab(f.projectId, f.tab.id, i + delta);
}

export async function pickProjectFolder() {
  const dir = await open({ directory: true, multiple: false, title: t("Add project folder") });
  if (typeof dir !== "string") return;
  const id = store.addProject(dir);
  // A repository's other worktrees come along, under it.
  const p = store.state.projects.find((x) => x.id === id);
  if (p && !p.parentId) void addExistingWorktrees(p).catch(() => {});
}

function zoom(delta: number) {
  const size = delta === 0 ? 13 : Math.min(28, Math.max(8, store.state.settings.fontSize + delta));
  store.setSettings({ fontSize: size });
  for (const v of allViews()) v.applySettings(store.state.settings);
}

const FOCUS_LABEL = { left: msg("Focus Pane Left"), right: msg("Focus Pane Right"), up: msg("Focus Pane Up"), down: msg("Focus Pane Down") } as const;

export const commands: Command[] = [
  { id: "palette", label: msg("Search Everything"), win: "mod+shift+p", mac: "mod+shift+p", run: () => store.setOverlay("palette") },
  { id: "quickopen", label: msg("Go to File…"), win: "mod+shift+o", mac: "mod+p", run: () => store.setOverlay("quickopen"), when: hasProject },
  { id: "jump", label: msg("Jump to Session / Project…"), win: "mod+shift+k", mac: "mod+k", run: () => store.setOverlay("jump") },
  { id: "attention.next", label: msg("Next Session Waiting for You"), win: "mod+shift+.", mac: "mod+shift+.", run: () => store.revealNextWaiting() },
  { id: "attention.answer", label: msg("Answer Waiting Session…"), when: () => Object.values(store.sessions).some((s) => s.alive && s.status === "waiting"), run: () => { const s = waitingSessions(Object.values(store.sessions))[0]; if (s) store.openAnswer(s.id); } },
  { id: "dashboard", label: msg("Toggle Agent Dashboard"), win: "mod+shift+m", mac: "mod+shift+m", run: () => store.setOverlay(store.overlay() === "dashboard" ? null : "dashboard") },
  { id: "project.add", label: msg("Add Project Folder…"), run: () => void pickProjectFolder() },
  { id: "project.addRemote", label: msg("Add Remote Project (SSH)…"), run: () => store.setOverlay("remote") },
  { id: "board", label: msg("Toggle Workspace Board"), win: "mod+shift+j", mac: "mod+shift+j", run: () => store.setOverlay(store.overlay() === "board" ? null : "board") },
  { id: "new.shell", label: msg("New Terminal"), win: "mod+shift+t", mac: "mod+t", run: () => newSession("shell"), when: hasProject },
  { id: "panel.history", label: msg("Agent Session History"), run: () => store.showRightPanel("history"), when: hasProject },
  { id: "panel.issues", label: msg("Issues (Linear, Jira, GitLab)"), run: () => store.showRightPanel("issues"), when: hasProject },
  { id: "panel.turns", label: msg("Agent Turns and Undo"), run: () => store.showRightPanel("turns"), when: () => hasProject() && store.daemonApi() >= CHECKPOINT_API },
  // Always bound: with no live session the chord must not fall through to the webview's DevTools shortcut.
  { id: "composer.open", label: msg("Compose Prompt…"), win: "mod+shift+i", mac: "mod+shift+i", run: () => { const s = focusedSession(); if (s?.alive) toggleComposer(s.id); else store.notify(t("Focus a running session to compose a prompt in")); } },
  { id: "browser.open", label: msg("Open Browser"), win: "mod+shift+u", mac: "mod+shift+u", run: () => openBrowser(), when: hasProject },
  { id: "browser.split", label: msg("Open Browser to the Right"), run: () => openBrowser("", "right"), when: hasProject },
  { id: "new.menu", label: msg("New Session…"), win: "mod+shift+n", mac: "mod+shift+n", run: () => store.setOverlay("newsession"), when: hasProject },
  { id: "new.claude", label: msg("New Claude Code Session"), run: () => newSession("claude"), when: hasProject },
  { id: "new.codex", label: msg("New Codex Session"), run: () => newSession("codex"), when: hasProject },
  { id: "new.omp", label: msg("New OMP Session"), run: () => newSession("omp"), when: hasProject },
  { id: "agent.compact", label: msg("Agent: Compact Conversation"), when: () => !!agentHere(), run: () => void runConversationCommand(agentHere()!.s, "compact") },
  { id: "agent.clear", label: msg("Agent: Clear Conversation"), when: () => !!agentHere()?.conv.clear, run: () => void runConversationCommand(agentHere()!.s, "clear") },
  { id: "agent.restart", label: msg("Agent: Restart, Keeping the Conversation"), when: () => { const s = focusedSession(); return !!s?.alive && store.resumable(s); }, run: () => void restartKeepingConversation(focusedSession()!) },
  // Splits open an empty pane that asks which CLI to start (agent, terminal, or the full dialog).
  { id: "split.right", label: msg("Split Right…"), win: "mod+shift+d", mac: "mod+d", run: () => store.splitPicker(pid()!, "right"), when: hasProject },
  { id: "split.down", label: msg("Split Down…"), win: "mod+shift+s", mac: "mod+shift+d", run: () => store.splitPicker(pid()!, "down"), when: hasProject },
  {
    id: "pane.close", label: msg("Close Pane"), win: "mod+shift+w", mac: "mod+w", when: hasProject,
    run: async () => {
      const f = focusedPane();
      if (!f) return;
      const pane = f.tab && findInTab(f.tab, f.paneId);
      const s = pane?.sessionId ? store.sessions[pane.sessionId] : undefined;
      if (s?.alive && s.kind !== "shell" && !(await confirmDialog(t("Stop {kind} and close this pane?", { kind: s.kind })))) return;
      void store.closePane(f.projectId, f.tab.id, f.paneId);
    },
  },
  {
    id: "pane.next", label: msg("Focus Next Pane"), win: "alt+]", mac: "mod+]", when: hasProject,
    run: () => { const f = focusedPane(); if (f) store.focusPane(f.projectId, f.tab.id, cyclePane(f.tab.root, f.paneId, 1)); },
  },
  {
    id: "pane.prev", label: msg("Focus Previous Pane"), win: "alt+[", mac: "mod+[", when: hasProject,
    run: () => { const f = focusedPane(); if (f) store.focusPane(f.projectId, f.tab.id, cyclePane(f.tab.root, f.paneId, -1)); },
  },
  ...(["left", "right", "up", "down"] as const).map((dir): Command => ({
    id: `pane.${dir}`, label: FOCUS_LABEL[dir], win: `mod+alt+arrow${dir}`, mac: `ctrl+alt+arrow${dir}`, when: hasProject,
    run: () => {
      const f = focusedPane();
      if (!f) return;
      const rects = [...document.querySelectorAll<HTMLElement>(`[data-tab="${f.tab.id}"][data-pane]`)].map((el) => ({ id: el.dataset.pane!, r: el.getBoundingClientRect() }));
      const next = paneInDirection(rects, f.paneId, dir);
      if (next) store.focusPane(f.projectId, f.tab.id, next);
    },
  })),
  { id: "layout.menu", label: msg("Layout…"), win: "mod+shift+l", mac: "mod+shift+l", when: () => hasProject() && !!focusedPane(), run: () => document.querySelector<HTMLElement>(".layout-btn")?.click() },
  { id: "layout.equalize", label: msg("Layout: Make All Panes Equal"), when: () => !!focusedPane(), run: () => { const f = focusedPane(); if (f) store.equalizeTab(f.projectId, f.tab.id); } },
  { id: "layout.broadcast", label: msg("Layout: Toggle Broadcast Input to All Terminals in Tab"), when: () => !!focusedPane(), run: () => { const f = focusedPane(); if (f) store.setBroadcast(f.projectId, f.tab.id, !f.tab.broadcast); } },
  ...GRID_PRESETS.map((spec): Command => ({
    id: `layout.grid.${spec.mode}.${spec.counts.join("-")}`, get label() { return t("Layout: Arrange as {grid}", { grid: gridLabel(spec) }); }, when: () => !!focusedPane(),
    run: () => { const f = focusedPane(); if (f) store.arrangeTab(f.projectId, f.tab.id, spec); },
  })),
  { id: "tab.next", label: msg("Next Tab"), win: "ctrl+tab", mac: "ctrl+tab", run: () => store.cycleTab(1) },
  { id: "tab.prev", label: msg("Previous Tab"), win: "ctrl+shift+tab", mac: "ctrl+shift+tab", run: () => store.cycleTab(-1) },
  { id: "tab.moveLeft", label: msg("Move Tab Left"), when: () => !!focusedPane(), run: () => moveActiveTab(-1) },
  { id: "tab.moveRight", label: msg("Move Tab Right"), when: () => !!focusedPane(), run: () => moveActiveTab(1) },
  { id: "layout.tile", label: msg("Tile All (this tab and one-CLI tabs)"), when: () => { const f = focusedPane(); return !!f && store.canTile(f.projectId, f.tab.id); }, run: () => { const f = focusedPane(); if (f) store.tileTab(f.projectId, f.tab.id); } },
  { id: "layout.spread", label: msg("Spread Panes into Tabs"), when: () => { const f = focusedPane(); return !!f && store.canSpread(f.projectId, f.tab.id); }, run: () => { const f = focusedPane(); if (f) store.spreadTab(f.projectId, f.tab.id); } },
  { id: "pane.toNewTab", label: msg("Move Pane to New Tab"), when: () => { const f = focusedPane(); return !!f && allPanes(f.tab.root).length > 1; }, run: () => { const f = focusedPane(); if (f) store.movePaneToTab(f.projectId, f.tab.id, f.paneId, null); } },
  { id: "panel.files", label: msg("Toggle Explorer"), win: "mod+shift+e", mac: "mod+shift+e", run: () => store.setRightPanel("files"), when: hasProject },
  { id: "panel.git", label: msg("Toggle Source Control"), win: "mod+shift+g", mac: "mod+shift+g", run: () => store.setRightPanel("git"), when: hasProject },
  { id: "panel.search", label: msg("Search in Files"), win: "mod+shift+h", mac: "mod+shift+f", run: () => store.showRightPanel("search"), when: hasProject },
  {
    id: "pane.zoom", label: msg("Toggle Maximize Pane"), win: "mod+shift+enter", mac: "mod+shift+enter", when: hasProject,
    run: () => { const f = focusedPane(); if (f) store.toggleZoom(f.tab.id, f.paneId); },
  },
  { id: "sidebar", label: msg("Toggle Sidebar"), win: "mod+shift+b", mac: "mod+b", run: () => store.setSettings({ sidebarCollapsed: !store.state.settings.sidebarCollapsed }) },
  {
    id: "find", label: msg("Find in Terminal"), win: "mod+shift+f", mac: "mod+f",
    run: () => window.dispatchEvent(new CustomEvent("kelpie:find")),
    // Pages and dialogs have their own find (e.g. Settings search).
    when: () => store.overlay() === null,
  },
  { id: "settings", label: msg("Settings"), win: "mod+,", mac: "mod+,", run: () => store.openSettings() },
  { id: "voice.toggle", label: msg("Dictate (start / stop voice input)"), win: "mod+shift+space", mac: "mod+shift+space", run: toggleDictation },
  { id: "floating.toggle", label: msg("Toggle Floating Terminal"), win: "mod+shift+`", mac: "mod+alt+a", run: () => store.toggleFloating(), when: () => store.state.settings.floatingTerminal },
  { id: "inbox", label: msg("Show Activity & Notifications"), run: () => window.dispatchEvent(new CustomEvent("kelpie:inbox")) },
  {
    id: "editor.wordWrap", label: msg("Toggle Word Wrap"), win: "alt+z", mac: "alt+z",
    run: () => store.setSettings({ wordWrap: !store.state.settings.wordWrap }),
    // Only with an editor focused: terminals keep Alt+Z.
    when: () => { const f = focusedPane(); return !!f && findInTab(f.tab, f.paneId)?.kind === "file"; },
  },
  {
    id: "tabs.closeEditors", label: msg("Close All Editor Tabs"), win: "mod+alt+w", mac: "mod+alt+w", when: hasProject,
    run: () => {
      const p = pid();
      if (!p) return;
      for (const tab of [...store.workspace(p).tabs]) {
        for (const pane of allPanes(tab.root)) if (pane.kind === "file" || pane.kind === "diff") void store.closePane(p, tab.id, pane.id);
      }
    },
  },
  { id: "panel.plan", label: msg("Plan (specs and tasks)"), run: () => store.showRightPanel("plan"), when: hasProject },
  { id: "spec.new", label: msg("New Spec…"), run: newSpec, when: hasProject },
  { id: "panel.review", label: msg("Review Queue"), run: () => store.showRightPanel("review"), when: () => { const a = store.activeProject(); return !!a && !a.remote && !!store.projectInfo[a.parentId ?? a.id]?.is_repo; } },
  { id: "worktree.finish", label: msg("Finish Worktree…"), when: () => { const a = store.activeProject(); return !!a?.parentId && !a.remote; }, run: () => openFinish(store.activeProject()!.id) },
  { id: "worktree.review", label: msg("Review Worktree with…"), when: () => { const a = store.activeProject(); return !!a?.parentId && !a.remote; }, run: () => openReviewWith(store.activeProject()!.id) },
  { id: "runs.try", label: msg("Try with Several Agents…"), when: () => { const a = store.activeProject(); const repo = a?.parentId ?? a?.id; return !!repo && !a?.remote && !!store.projectInfo[repo]?.is_repo; }, run: () => openTryAgents(store.activeProject()!.id) },
  { id: "worktree.new", label: msg("New Worktree…"), when: () => { const a = store.activeProject(); const repo = a?.parentId ?? a?.id; return !!repo && !a?.remote && !!store.projectInfo[repo]?.is_repo; }, run: () => { const a = store.activeProject()!; openWorktreeDialog(a.parentId ?? a.id); } },
  { id: "project.sleep", label: msg("Sleep Project (stop agents, resume on open)"), when: hasProject, run: () => void store.sleepProject(pid()!) },
  { id: "automations", label: msg("Scheduled Automations…"), run: () => store.openSettings("automations") },
  { id: "accounts", label: msg("Claude / Codex Accounts…"), run: () => store.openSettings("accounts") },
  { id: "agent-setup", label: msg("Agent Setup (MCP, instructions, skills)…"), run: () => store.openSettings("agent-setup") },
  { id: "usage.cost", label: msg("Usage & Cost…"), run: () => store.setOverlay("cost") },
  { id: "runs", label: msg("Toggle Runs (Orchestration)"), win: "mod+shift+r", mac: "mod+shift+r", run: () => store.setOverlay(store.overlay() === "runs" ? null : "runs") },
  { id: "runs.new", label: msg("New Run…"), run: () => openRuns(true) },
  { id: "hosts", label: msg("SSH Hosts…"), win: "mod+alt+h", mac: "mod+alt+h", run: () => (store.overlay() === "hosts" ? store.setOverlay(null) : openHosts()) },
  { id: "hosts.keys", label: msg("SSH Keys…"), run: () => openHosts("keys") },
  { id: "hosts.forwards", label: msg("SSH Port Forwarding…"), run: () => openHosts("forwards") },
  { id: "mobile", label: msg("Mobile Access (pair a phone)…"), run: () => store.openSettings("mobile") },
  // Alt+Shift+Arrow, not Alt+Arrow: shells use Alt+Arrow for word movement.
  { id: "nav.back", label: msg("Go Back"), win: "shift+alt+arrowleft", mac: "mod+alt+arrowleft", run: navBack, when: () => hasProject() && store.overlay() === null },
  { id: "nav.forward", label: msg("Go Forward"), win: "shift+alt+arrowright", mac: "mod+alt+arrowright", run: navForward, when: () => hasProject() && store.overlay() === null },
  { id: "zoom.in", label: msg("Increase Font Size"), win: "mod+=", mac: "mod+=", run: () => zoom(1) },
  { id: "zoom.out", label: msg("Decrease Font Size"), win: "mod+-", mac: "mod+-", run: () => zoom(-1) },
  { id: "zoom.reset", label: msg("Reset Font Size"), win: "mod+0", mac: "mod+0", run: () => zoom(0) },
  // Long agent conversations: jump between the prompts you sent (Cmd+Up/Down as in iTerm2).
  { id: "terminal.prevPrompt", label: msg("Terminal: Previous Prompt"), win: "mod+shift+arrowup", mac: "mod+arrowup", when: hasProject, run: () => focusedView()?.jumpPrompt(-1) },
  { id: "terminal.nextPrompt", label: msg("Terminal: Next Prompt"), win: "mod+shift+arrowdown", mac: "mod+arrowdown", when: hasProject, run: () => focusedView()?.jumpPrompt(1) },
  { id: "terminal.bottom", label: msg("Terminal: Scroll to Bottom"), win: "mod+shift+end", mac: "mod+end", when: hasProject, run: () => focusedView()?.scrollToBottom() },
  {
    id: "terminal.clear", label: msg("Clear Terminal Buffer"), when: hasProject,
    run: () => {
      const f = focusedPane();
      const pane = f && findInTab(f.tab, f.paneId);
      if (pane?.sessionId) peekView(pane.sessionId)?.term.clear();
    },
  },
];

/** The terminal of the focused pane, if it is one. */
function focusedView() {
  const f = focusedPane();
  const pane = f && findInTab(f.tab, f.paneId);
  return pane?.sessionId ? peekView(pane.sessionId) : undefined;
}

function findInTab(tab: { root: import("./types").LayoutNode }, paneId: string) {
  return findPane(tab.root, paneId);
}

// Project switching: Alt+1..9 (Windows) / Cmd+1..9 (macOS).
for (let i = 1; i <= 9; i++) {
  commands.push({
    id: `project.${i}`,
    get label() { return t("Switch to Project {n}", { n: i }); },
    win: `alt+${i}`,
    mac: `mod+${i}`,
    run: () => {
      const p = store.topProjects()[i - 1];
      if (p) store.switchProject(p.id);
    },
    when: () => store.topProjects().length >= i,
  });
}

export function eventChord(e: KeyboardEvent): string {
  const parts: string[] = [];
  const mod = isMac ? e.metaKey : e.ctrlKey;
  if (mod) parts.push("mod");
  if (isMac && e.ctrlKey) parts.push("ctrl");
  if (!isMac && e.metaKey) parts.push("meta");
  if (e.shiftKey) parts.push("shift");
  if (e.altKey) parts.push("alt");
  let key = e.key.toLowerCase();
  // Use the physical key for digits/punctuation so Shift/Alt layouts still match.
  if (/^Digit\d$/.test(e.code)) key = e.code.slice(5);
  else if (e.code === "BracketLeft") key = "[";
  else if (e.code === "BracketRight") key = "]";
  else if (e.code === "Equal") key = "=";
  else if (e.code === "Minus") key = "-";
  else if (e.code === "Comma") key = ",";
  else if (e.code === "Period") key = ".";
  else if (e.code === "Backquote") key = "`";
  else if (e.code === "Space") key = "space";
  else if (/^Key[A-Z]$/.test(e.code)) key = e.code.slice(3).toLowerCase();
  // Numpad keys by name: "+" itself would be read as a separator.
  else if (/^Numpad/.test(e.code)) key = e.code.toLowerCase();
  parts.push(key);
  return parts.join("+");
}

const MODIFIER_ORDER = ["mod", "ctrl", "meta", "shift", "alt"];

/** Canonical form for comparing chords: "ctrl" is `mod` off macOS, modifiers in a fixed order. */
export function normalizeChord(chord: string): string {
  const parts = chord.toLowerCase().split("+").filter(Boolean).map((p) => (!isMac && p === "ctrl" ? "mod" : p));
  const key = parts.pop() ?? "";
  const mods = [...new Set(parts)].sort((a, b) => MODIFIER_ORDER.indexOf(a) - MODIFIER_ORDER.indexOf(b));
  return [...mods, key].join("+");
}

export const defaultChordOf = (c: Command) => (isMac ? c.mac : c.win);

/** The user's override (from Settings → Shortcuts) or the default; "" means unbound. */
/** " (Ctrl+Shift+X)" for a command's shortcut, to append to a tooltip; "" without one. */
export function commandHint(id: string): string {
  const c = commands.find((x) => x.id === id);
  return c && chordOf(c) ? ` (${chordLabel(chordOf(c)!)})` : "";
}

export function chordOf(c: Command): string | undefined {
  const override = store.state.keymap?.[c.id];
  return override !== undefined ? override || undefined : defaultChordOf(c);
}

/** Commands bound to `chord`, optionally ignoring one command. */
export function commandsWithChord(chord: string, except?: string): Command[] {
  const n = normalizeChord(chord);
  return commands.filter((c) => c.id !== except && chordOf(c) && normalizeChord(chordOf(c)!) === n);
}

/** While the shortcut editor records a key, no command may run. */
let suspended = false;
export const suspendKeymap = (on: boolean) => (suspended = on);

function match(e: KeyboardEvent): Command | undefined {
  if (suspended) return undefined;
  const chord = normalizeChord(eventChord(e));
  return commands.find((c) => {
    const k = chordOf(c);
    return k && normalizeChord(k) === chord && (!c.when || c.when());
  });
}

/** Chords already explained this session (one toast per chord). */
const explained = new Set<string>();

function explainIntercept(cmd: Command) {
  const chord = chordOf(cmd);
  if (!chord || explained.has(cmd.id) || !store.state.settings.shortcutHints) return;
  explained.add(cmd.id);
  store.notify(t("{chord} → {command} (kept from the terminal)", { chord: chordLabel(chord), command: t(cmd.label) }), {
    label: t("Don't show again"),
    run: () => store.setSettings({ shortcutHints: false }),
  }, 5000);
}

export function installKeymap() {
  setOverlayCheck(() => store.overlay() !== null);
  setAppShortcutMatcher((e, fromTerminal) => {
    const cmd = match(e);
    if (cmd && fromTerminal) explainIntercept(cmd);
    return !!cmd;
  });
  window.addEventListener(
    "keydown",
    (e) => {
      if (e.isComposing) return;
      const cmd = match(e);
      if (!cmd) return;
      e.preventDefault();
      e.stopPropagation();
      // The terminal never sees this key; tell the user once why.
      if ((e.target as HTMLElement | null)?.closest?.(".xterm")) explainIntercept(cmd);
      cmd.run();
    },
    true,
  );
}
