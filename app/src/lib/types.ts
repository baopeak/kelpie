import type { SavedRegistryAgent } from "./agent-setup";
/** Mirrors `SessionInfo` from kelpied (crates/kelpied/src/session.rs). */
export type SessionStatus = "idle" | "working" | "waiting" | "exited";
/** shell | custom | claude | codex | omp, or a preset agent id from `agent.presets` (gemini, cursor…). */
export type SessionKind = string;

export interface SessionInfo {
  /** Wall-clock ms since it has been waiting on the user (daemon API 7+). */
  waiting_since?: number;
  handle: number;
  id: string;
  project_id: string | null;
  name: string | null;
  kind: SessionKind;
  cwd: string;
  args: string[];
  env: Record<string, string>;
  agent_session_id: string | null;
  model: string | null;
  created_at: number;
  title: string | null;
  status: SessionStatus;
  unread: boolean;
  alive: boolean;
  exit_code: number | null;
  /** The last agent turn errored, or the process exited with a failure code on its own (absent from older daemons). */
  failed?: boolean;
  /** The last turn ended on the subscription's usage limit (Claude Code reports it; daemon API 12+). */
  limited?: boolean;
  pid: number | null;
  cols: number;
  rows: number;
  seq: number;
  chat: boolean;
  /** Wall-clock ms of the last output/input (0 = never). */
  last_activity: number;
  /** Was running when the daemon stopped (reboot, update, crash); not restarted since. */
  interrupted: boolean;
  /** Runs on another machine over SSH. */
  remote?: { dest: string; port?: number; config?: string; tmux: boolean } | null;
}

export type WorkStatus = "todo" | "progress" | "review" | "done";

export interface Project {
  id: string;
  name: string;
  path: string;
  color: string;
  /** Set for a git worktree shown under its repository's project. */
  parentId?: string;
  branch?: string;
  /** The branch this worktree was created from: what Finish merges into (unknown for worktrees added later). */
  baseBranch?: string;
  /** Workspace board column (default todo) and pin. */
  status?: WorkStatus;
  pinned?: boolean;
  /** A folder on another machine, reached over SSH (`path` is then its `ssh://` root). */
  remote?: { dest: string; port?: number; path: string; tmux: boolean; agents?: string[] };
  /** Saved SSH hosts linked to this project, each with the folder its panes start in there
   *  (the hosts themselves live in the SSH vault, shared by every project). */
  hosts?: ProjectHost[];
}

export interface ProjectHost {
  hostId: string;
  /** Folder on the host (empty: the host's start folder, else its login folder). */
  path: string;
}

/** Where a session runs when not on this computer: an ssh destination (a saved host's alias or
 *  `user@host`), the folder there, and whether it lives in the host's tmux. */
export interface SessionTarget {
  dest: string;
  port?: number;
  path: string;
  tmux: boolean;
}

export interface Worktree {
  path: string;
  head: string | null;
  branch: string | null;
  bare: boolean;
  detached: boolean;
  prunable: boolean;
}

export type PaneKind = "terminal" | "chat" | "file" | "diff" | "browser" | "empty";

export interface Pane {
  id: string;
  kind: PaneKind;
  /** Daemon session id (stable across daemon restarts), for terminal/chat panes. */
  sessionId?: string;
  /** Project-relative path for file/diff panes. */
  path?: string;
  staged?: boolean;
  /** A file or diff pane showing a turn of an agent session instead of the folder: the snapshot
   *  at the end of turn `n` (a diff compares it with the one before). */
  rev?: { session: string; n: number; prefix?: string };
  /** A diff pane comparing the folder with a branch (a review): the one it forked from. */
  base?: string;
  line?: number;
  /** Browser panes: the page shown. */
  url?: string;
  /** Empty panes: the host their launcher starts on (copied from the pane that was split). */
  target?: SessionTarget;
}

export interface LeafNode {
  type: "leaf";
  id: string;
  pane: Pane;
}

export interface SplitNode {
  type: "split";
  id: string;
  /** row = side by side, col = stacked */
  dir: "row" | "col";
  /** Two or more, never a split of the same direction (see lib/layout.ts). */
  children: LayoutNode[];
  /** Share of each child, summing to 1. */
  sizes: number[];
}

export type LayoutNode = LeafNode | SplitNode;

/** A saved layout: its shape and what each pane runs (null = an empty pane) and where (no
 *  target = the project's own machine). */
export type LayoutTemplate =
  | { type: "leaf"; kind: SessionKind | null; target?: SessionTarget }
  | { type: "split"; dir: "row" | "col"; sizes: number[]; children: LayoutTemplate[] };

export interface SavedLayout {
  id: string;
  name: string;
  template: LayoutTemplate;
}

export interface Tab {
  id: string;
  root: LayoutNode;
  focus: string; // pane id
  /** Keystrokes typed in one terminal of this tab go to all of its terminals. */
  broadcast?: boolean;
  /** Tab label set by the user (otherwise the focused pane's title). */
  name?: string;
}

export type RightPanel = "files" | "git" | "search" | "history" | "issues" | "turns" | "review" | "plan" | "processes" | null;

export interface Workspace {
  tabs: Tab[];
  activeTab: string | null;
  rightPanel: RightPanel;
}

export interface AgentProfile {
  kind: SessionKind;
  label: string;
  args: string[];
  /** Extra environment for this agent's sessions. */
  env?: Record<string, string>;
  /** Claude `--permission-mode` / Codex `approval_policy`; empty = the agent's own default. */
  permission?: string;
}

export interface Settings {
  fontFamily: string;
  fontSize: number;
  lineHeight: number;
  scrollback: number;
  cursorBlink: boolean;
  webgl: boolean;
  shellArgs: string[];
  rightPanelWidth: number;
  sidebarWidth: number;
  sidebarCollapsed: boolean;
  notifications: boolean;
  /** Toast once per shortcut when Kelpie keeps a key from the focused terminal. */
  shortcutHints: boolean;
  projectFilter: "all" | "running" | "attention";
  projectSort: "manual" | "recent";
  profiles: AgentProfile[];
  /** Claude / Codex subscription usage in the status bar (reads the agents' own sign-in). */
  usageMeters: boolean;
  memoryMeter: boolean;
  /** Per-session cost cap in USD (0 = off): a session over it gets a red chip and an inbox entry. */
  costCap: number;
  /** Also press Esc in a session when it first passes the cap (only with an exact transcript match). */
  costCapEsc: boolean;
  quickCommands: QuickCommand[];
  theme: "dark" | "light";
  density: "comfortable" | "compact";
  /** Interface (not terminal) text scale, 0.9 – 1.2. */
  uiScale: number;
  /** First-run tips; `tipsSeen` holds the ids already dismissed. */
  tips: boolean;
  tipsSeen: string[];
  /** manual: each agent asks as it normally does; yolo: launch with its skip-permission flags. */
  agentPermissions: "manual" | "yolo";
  /** Agent kinds left out of launchers. */
  hiddenAgents: string[];
  /** Agents saved from the ACP registry (Settings → Agent setup → Find more agents); sent to the daemon with the plugin agents. */
  registryAgents?: SavedRegistryAgent[];
  /** Stop finished background agents after `hibernateMinutes` idle; resume when shown again. */
  hibernate: boolean;
  hibernateMinutes: number;
  /** Prevent idle sleep: never, while an agent works, or always. */
  keepAwake: "off" | "agents" | "always";
  /** Sessions not on screen run below normal CPU priority, so typing stays quick when the
   *  machine is busy (Windows). */
  prioritizeVisible: boolean;
  /** Snapshot an agent's working folder when each turn ends (git folders on this computer), for
   *  the Turns panel and undo. */
  checkpoints: boolean;
  /** Programs in a session (agents, scripts) may drive other sessions through `kelpied`
   *  (`terminal send`, `call session.create`…). Off: only their own session. */
  agentsScripting: boolean;
  /** A short chime with desktop notifications. */
  notificationSound: boolean;
  /** Look for a newer Kelpie on GitHub Releases (30 s after start, then every 6 hours). */
  updateChecks: boolean;
  /** A release the user chose to skip; newer ones are offered again. */
  skippedUpdate?: string;
  /** Terminal colour theme id (lib/terminal-themes.ts); "default" follows the app theme. */
  terminalTheme: string;
  /** Editor soft wrap. */
  wordWrap: boolean;
  /** Agent for AI actions (commit message, pull request text). */
  aiAgent: "claude" | "codex";
  /** New agent sessions in a git folder: offer an isolated worktree (ask, off unless ticked),
   *  start in one (always, ticked unless unticked) or never offer it. Branch named from the first prompt. */
  autoWorktree?: "ask" | "always" | "never";
  /** Global floating terminal (not tied to a project). */
  floatingTerminal: boolean;
  /** Kip, the mascot: in the sidebar footer (agent status) and on empty screens. */
  mascot: boolean;
  /** Kip's animations: always, as the OS's animation setting says, or never. */
  mascotMotion: "on" | "system" | "off";
  /** Layouts saved from a tab, offered in the layout menu and the launcher. */
  layouts: SavedLayout[];
  /** After the session daemon restarts (reboot, update, crash): resume sessions when they are
   *  shown, all at once, or leave them stopped until you restart them. */
  resumeSessions: "shown" | "all" | "off";
  /** Extra Claude Code / Codex sign-ins (lib/accounts.ts). */
  accounts: AgentAccount[];
  /** Account new sessions use, by agent; absent = the agent's default sign-in. */
  activeAccounts: Partial<Record<"claude" | "codex", string>>;
  /** `ssh -F` file for remote projects (empty: ssh's default, ~/.ssh/config). */
  sshConfig: string;
  /** Saved hosts logged in as root where starting Claude Code in Yolo mode (it then asks for
   *  permissions) no longer explains why first. */
  rootYoloOk?: string[];
  /** Interface language (lib/i18n.ts); "system" follows the OS. */
  language: "system" | "en" | "vi";
  /** Voice dictation (lib/voice.ts): speech model id, Whisper language ("auto" detects), microphone (null: the default). */
  voice: { model: string | null; language: string; device: string | null };
  /** Ctrl/Cmd+click on a localhost link in a terminal opens it in a browser pane. */
  localhostInBrowser: boolean;
  /** Issue trackers and GitLab (tokens are in the OS credential store, not here). */
  integrations: { linear: boolean; jira: { url: string; email: string } | null; gitlabHosts: string[] };
}

/** A Claude Code or Codex sign-in kept in its own config home. */
export interface AgentAccount {
  id: string;
  agent: "claude" | "codex";
  label: string;
  /** `CLAUDE_CONFIG_DIR` / `CODEX_HOME` for its sessions. */
  dir: string;
}

export interface QuickCommand {
  id: string;
  label: string;
  text: string;
  /** terminal: type into the focused shell (or a new one); agent: send as a prompt to the focused agent. */
  action: "terminal" | "agent";
  enter: boolean;
  /** null = every project. */
  projectId: string | null;
}

export interface HistoryItem {
  agent: "claude" | "codex" | "omp";
  id: string;
  /** Folder as the agent recorded it; resume launches here. */
  cwd: string;
  /** `cwd` in canonical form (long names), for matching projects. */
  folder: string;
  title: string | null;
  first_prompt: string | null;
  updated_at: number;
  /** null until the background count reaches this transcript. */
  turns: number | null;
  /** Config home of the account it belongs to (null = the agent's default). */
  home: string | null;
}

export interface UsageWindow {
  label: string;
  used_percent: number;
  resets_at: number | null;
}

export interface ProviderUsage {
  windows: UsageWindow[];
  error: string | null;
}

export interface UsageReport {
  claude: ProviderUsage;
  codex: ProviderUsage;
}

/** A conversation's numbers from its transcript (app core `session_usage`). */
export interface SessionUsage {
  context_used: number;
  /** Absent when the agent does not say (OMP) or the model has no known window. */
  context_window: number | null;
  tokens_in: number;
  tokens_out: number;
  /** Whole conversation; null when any of it has no price (unknown model, Codex). */
  cost: number | null;
  /** The agent's own number (OMP) instead of an estimate at API prices (Claude). */
  cost_exact: boolean;
  model: string | null;
  /** The totals cover the whole transcript (false while a background count runs). */
  totals: boolean;
}

/** One cell of the usage report: a local day of one agent, project and model. */
export interface UsageRow {
  day: number;
  agent: string;
  project: string;
  model: string;
  tokens_in: number;
  tokens_out: number;
  cost: number;
  unpriced: number;
}

export interface UsageTotals {
  rows: UsageRow[];
  files: number;
  pending: number;
}

export interface MemoryStats {
  app: number;
  daemon: number;
  sessions: number;
  session_processes: number;
}

/** Notification inbox / activity feed entry. */
export interface InboxItem {
  id: string;
  at: number;
  kind: "finished" | "waiting" | "automation" | "worktree" | "error" | "cost";
  title: string;
  body: string;
  sessionId?: string;
  projectId?: string;
  read: boolean;
}

/** A review note on a diff line, sent to an agent in a batch. */
export interface ReviewNote {
  id: string;
  path: string;
  /** 1-based line number on `side`. */
  line: number;
  side: "new" | "old";
  /** The line as it read when the note was made (context for the agent). */
  text: string;
  note: string;
  at: number;
  /** The agent that wrote it (`kelpied review add`); none for the user's own. */
  by?: string;
}

/** Folder comment / board status shared with agents through the daemon (`meta.*`). */
export interface WorkspaceMeta {
  path: string;
  comment: string | null;
  status: WorkStatus | null;
  updated_at: number;
}

export interface Automation {
  id: string;
  name: string;
  enabled: boolean;
  kind: string;
  prompt: string;
  cwd: string;
  project_id: string | null;
  args: string[];
  /** Session environment, incl. the account home (CLAUDE_CONFIG_DIR / CODEX_HOME). */
  env?: Record<string, string>;
  every: "hourly" | "daily" | "weekdays" | "weekly";
  /** HH:MM local time (hourly: the minute). */
  time: string;
  /** 0 = Sunday. */
  weekday: number;
  last_run: number;
  last_session: string | null;
  last_error: string | null;
  next_run: number;
  /** When the run finishes: "review" moves the folder's card to the Review column. */
  deliver?: "review" | null;
  /** Runs on an event instead of the clock (`every` / `time` / `weekday` are then unused). */
  trigger?: AutomationTrigger | null;
  /** agent: a new session; quick: a saved prompt typed into an idle session of the agent; shell: a program, no shell. */
  action?: "agent" | "quick" | "shell";
  shell?: { program: string; argv: string[]; approved_hash?: string | null } | null;
  quick_label?: string | null;
  /** Records what it would do and does nothing. */
  dry_run?: boolean;
  last_note?: string | null;
  cap_day?: string;
  cap_count?: number;
}

export type AutomationEventKind = "file" | "turn" | "commit" | "pr_checks_failed" | "finished";

export interface AutomationTrigger {
  event: AutomationEventKind;
  /** file: comma-separated patterns relative to the folder. */
  glob: string;
  daily_cap: number;
}

/** What the window tells the daemon (`automation.event`). */
export interface AutomationEvent {
  kind: "file" | "pr_checks_failed" | "finished";
  root: string;
  files?: string[];
  branch?: string;
  commit?: string;
  number?: number;
}

export interface AutomationState {
  paused: boolean;
  /** Why saving is refused (a damaged automations file, or a newer list found beside an older one). */
  error: string | null;
  /** What the user can do about it: dismiss the notice, or restore / keep the newer list. */
  fix?: "dismiss" | "restore" | null;
}

export interface RecentItem {
  kind: "session" | "project" | "file";
  /** Session id, project id, or project-relative path for files. */
  id: string;
  projectId: string;
  at: number;
}

export interface AppState {
  /** 2: projects may be worktrees (parentId/branch) with a board status.
   *  3: splits have any number of children (`children` / `sizes`). */
  version: 3;
  projects: Project[];
  activeProject: string | null;
  workspaces: Record<string, Workspace>;
  settings: Settings;
  recent: RecentItem[];
  /** Projects whose session list is folded in the sidebar. */
  collapsed: Record<string, boolean>;
  /** Shortcut overrides by command id: a chord like "mod+alt+p", or "" for none. */
  keymap: Record<string, string>;
  /** The setup, tabs, copy and link of `.kelpie.yaml` (canonical text) the user agreed to run, by repository path. */
  approvedSetups: Record<string, string>;
  /** The `check:` commands the user agreed to run, by repository path (a different list needs approval again). */
  approvedChecks?: Record<string, string[]>;
  /** Sessions Kelpie stopped to save memory (hibernation, sleeping worktrees): resumed when shown. */
  hibernated: string[];
  inbox: InboxItem[];
  /** Diff review notes by project id: only those of a daemon older than API 13 (it keeps them from there). */
  reviewNotes: Record<string, ReviewNote[]>;
  /** Prompts sent from the composer, newest first, by project id. */
  promptHistory?: Record<string, string[]>;
  /** Floating terminal: its sessions (tab order) and the one shown. */
  floating: { sessions: string[]; active: string | null; open: boolean };
}

export interface BranchInfo {
  is_repo: boolean;
  branch: string | null;
  dirty: boolean;
  ahead: number;
  behind: number;
}

export interface DirEntry {
  name: string;
  path: string;
  is_dir: boolean;
  is_symlink: boolean;
  size: number;
  ignored: boolean;
}

export interface FileContent {
  text: string | null;
  base64: string | null;
  size: number;
  mtime: number;
  truncated: boolean;
  binary: boolean;
}

export interface GitFile {
  path: string;
  orig_path: string | null;
  index: string;
  worktree: string;
}

export interface GitStatus {
  is_repo: boolean;
  branch: string | null;
  upstream: string | null;
  ahead: number;
  behind: number;
  files: GitFile[];
  truncated: boolean;
}

export interface AttachReply {
  mode: "delta" | "reset";
  raw: boolean;
  seq: number;
  cols: number;
  rows: number;
  data: string; // base64
}
