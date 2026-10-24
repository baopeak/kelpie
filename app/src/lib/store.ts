// Central app state. Fine-grained Solid stores: a status change on one of 20 sessions
// re-renders only the dots/cards bound to that session.
import { createRoot, createSignal } from "solid-js";
import { createStore, produce, reconcile } from "solid-js/store";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import * as ipc from "./ipc";
import { olderVersion } from "./version";
import { ANSWER_API, nextWaiting, setAnswering } from "./attention";
import { NOTIFY_API, describeNotify, type NotifyEvent } from "./notify-events";
import { t } from "./i18n";
import { allPanes, autoGrid, buildGrid, cellCount, equalize, findPane, fromTemplate, insertBeside, leaf, migrateLayout, moveGutter, movePane, removePane, replacePane, splitPane, swapPanes, templateLeaves, toTemplate, type GridSpec, type Side } from "./layout";
import { basename, isMac, normPath, uid } from "./platform";
import { projectColors } from "./theme";
import { disposeView, peekView, resetStreams, setInputFanout } from "./terminal-host";
import { CORE_AGENTS, isAgentKind, kindShortName, loadAgentPresets, presetFor } from "./agents";
import type { SettingsSection } from "./settings-catalog";
import { accountEnv, accountHomes, sessionHome } from "./accounts";
import { closeBrowser } from "./browser";
import { projectTarget, sessionTarget, targetEnv, targetRemote } from "./remote";
import { agentFolder, agentsAt, hostOf, isHostKeyError, layoutTarget, layoutTargetFor } from "./ssh-hosts";
import { confirmRootStart } from "./root-user";
import type { WorkStatus, AgentProfile, AppState, BranchInfo, InboxItem, LayoutNode, LayoutTemplate, Pane, Project, ProjectHost, RecentItem, ReviewNote, SessionInfo, SessionKind, SessionTarget, Settings, Tab, Workspace, WorkspaceMeta } from "./types";
import { confirmDialog } from "./confirm-dialog";

export const defaultSettings: Settings = {
  fontFamily: '"JetBrains Mono", "Cascadia Code", "Cascadia Mono", Menlo, Consolas, "Symbols Nerd Font Mono", monospace',
  fontSize: 13,
  lineHeight: 1.2,
  scrollback: 5000,
  cursorBlink: true,
  webgl: true,
  shellArgs: [],
  rightPanelWidth: 300,
  sidebarWidth: 232,
  sidebarCollapsed: false,
  notifications: true,
  shortcutHints: true,
  projectFilter: "all",
  projectSort: "manual",
  usageMeters: true,
  memoryMeter: true,
  costCap: 0,
  costCapEsc: false,
  quickCommands: [],
  theme: "dark",
  density: "comfortable",
  uiScale: 1,
  tips: true,
  tipsSeen: [],
  agentPermissions: "manual",
  hiddenAgents: [],
  hibernate: false,
  hibernateMinutes: 30,
  keepAwake: "off",
  prioritizeVisible: true,
  checkpoints: false,
  agentsScripting: true,
  notificationSound: false,
  updateChecks: true,
  terminalTheme: "default",
  wordWrap: false,
  aiAgent: "claude",
  autoWorktree: "ask",
  floatingTerminal: true,
  mascot: true,
  mascotMotion: "system",
  layouts: [],
  resumeSessions: "shown",
  accounts: [],
  activeAccounts: {},
  integrations: { linear: false, jira: null, gitlabHosts: [] },
  localhostInBrowser: true,
  sshConfig: "",
  voice: { model: null, language: "auto", device: null },
  language: "system",
  profiles: [
    { kind: "claude", label: "Claude Code", args: [] },
    { kind: "codex", label: "Codex", args: [] },
    { kind: "omp", label: "OMP", args: [] },
  ],
};

/** app-state.json format; older files are migrated (after a backup) on load. */
const STATE_VERSION = 3;
const emptyState = (): AppState => ({
  version: STATE_VERSION, projects: [], activeProject: null, workspaces: {}, settings: { ...defaultSettings }, recent: [], collapsed: {}, keymap: {}, approvedSetups: {},
  hibernated: [], inbox: [], reviewNotes: {}, floating: { sessions: [], active: null, open: false },
});
const MAX_RECENT = 40;
const MAX_INBOX = 200;

/** Same key the daemon uses for workspace notes (crates/kelpied/src/meta.rs `key`). */
export function metaKey(path: string): string {
  let k = path.trim().replace(/\\/g, "/");
  while (k.length > 1 && k.endsWith("/") && !k.endsWith(":/")) k = k.slice(0, -1);
  return isMac || !/^\//.test(k) ? k.toLowerCase() : k;
}

/** A short two-note chime for desktop notifications (no audio file to ship). */
function chime() {
  try {
    const ctx = new AudioContext();
    const t = ctx.currentTime;
    for (const [i, f] of [880, 1320].entries()) {
      const o = ctx.createOscillator();
      const g = ctx.createGain();
      o.frequency.value = f;
      g.gain.setValueAtTime(0.0001, t + i * 0.12);
      g.gain.exponentialRampToValueAtTime(0.12, t + i * 0.12 + 0.02);
      g.gain.exponentialRampToValueAtTime(0.0001, t + i * 0.12 + 0.35);
      o.connect(g).connect(ctx.destination);
      o.start(t + i * 0.12);
      o.stop(t + i * 0.12 + 0.4);
    }
    setTimeout(() => void ctx.close(), 1000);
  } catch {
    /* no audio device */
  }
}

export type Overlay = null | "cost" | "palette" | "quickopen" | "jump" | "dashboard" | "settings" | "newsession" | "board" | "worktree" | "runs" | "remote" | "hosts" | "update" | "answer" | "finish";

/** A pane of a tab (an empty pane a new session takes the place of). */
export type PaneTarget = { tabId: string; paneId: string };

function createAppStore() {
  const [state, setState] = createStore<AppState>(emptyState());
  /** Daemon sessions keyed by stable session id. */
  const [sessions, setSessions] = createStore<Record<string, SessionInfo>>({});
  /** Branch / dirty summary per project (sidebar subtitle). */
  const [projectInfo, setProjectInfo] = createStore<Record<string, BranchInfo>>({});
  const [connected, setConnected] = createSignal(false);
  const [overlay, setOverlay] = createSignal<Overlay>(null);
  const [loaded, setLoaded] = createSignal(false);
  const [toast, setToast] = createSignal<Toast | null>(null);
  /** Per-tab zoomed pane (transient, not persisted). */
  const [zoomed, setZoomed] = createSignal<Record<string, string>>({});
  const [lastSwitchMs, setLastSwitchMs] = createSignal<number | null>(null);
  const [settingsSection, setSettingsSection] = createSignal<SettingsSection>("agents");
  /** Setting row to scroll to and highlight when the page opens (from search). */
  const [settingsItem, setSettingsItem] = createSignal<string | null>(null);
  /** Folder comments / statuses from the daemon, by `metaKey(path)`. */
  const [meta, setMeta] = createStore<Record<string, WorkspaceMeta>>({});
  /** Each project's folder as the daemon keys notes: canonical (long names, no symlinks). */
  const [canonKeys, setCanonKeys] = createStore<Record<string, string>>({});
  const canonKey = (p: Project) => canonKeys[p.id] ?? metaKey(p.path);
  function resolveCanonKey(p: Project) {
    void ipc.canonicalPath(p.path).then((c) => setCanonKeys(p.id, metaKey(c)), () => {});
  }

  // ---------- persistence ----------
  let saveTimer = 0;
  /** Set when saving could destroy data we could not back up or do not understand. */
  let persistBlocked = false;
  const persist = () => {
    if (!loaded() || persistBlocked) return;
    clearTimeout(saveTimer);
    saveTimer = window.setTimeout(() => void ipc.saveState(JSON.parse(JSON.stringify(state))).catch(() => {}), 400);
  };
  const update = (fn: (s: AppState) => void) => {
    setState(produce(fn));
    persist();
  };

  function notify(text: string, action?: Toast["action"], ms = 3500) {
    const t: Toast = { text, action };
    setToast(t);
    setTimeout(() => setToast((cur) => (cur === t ? null : cur)), ms);
  }

  function toggleZoom(tabId: string, paneId: string) {
    setZoomed((z) => {
      const next = { ...z };
      if (next[tabId] === paneId) delete next[tabId];
      else next[tabId] = paneId;
      return next;
    });
  }

  // ---------- recents & per-project git summary ----------
  function recordRecent(kind: RecentItem["kind"], id: string, projectId: string) {
    update((s) => {
      // A file's id is project-relative, so the project is part of its identity.
      const same = (r: RecentItem) => r.kind === kind && r.id === id && r.projectId === projectId;
      s.recent = [{ kind, id, projectId, at: Date.now() }, ...(s.recent ?? []).filter((r) => !same(r))].slice(0, MAX_RECENT);
    });
  }

  // Branch summaries: debounced and single-flight per project. Watcher events arrive every
  // ~150 ms while an agent writes files; each would otherwise queue a `git status`.
  const infoJobs = new Map<string, { timer: number; running: boolean; again: boolean }>();
  function refreshProjectInfo(projectId: string, delay = 300) {
    let job = infoJobs.get(projectId);
    if (!job) infoJobs.set(projectId, (job = { timer: 0, running: false, again: false }));
    clearTimeout(job.timer);
    job.timer = window.setTimeout(() => void runProjectInfo(projectId), delay);
  }
  async function runProjectInfo(projectId: string) {
    const job = infoJobs.get(projectId)!;
    if (job.running) {
      job.again = true;
      return;
    }
    const p = state.projects.find((x) => x.id === projectId);
    if (!p) return;
    job.running = true;
    try {
      const info = await ipc.gitBranchInfo(p.path).catch(() => null);
      if (info && state.projects.some((x) => x.id === projectId)) setProjectInfo(projectId, reconcile(info));
    } finally {
      job.running = false;
      if (job.again) {
        job.again = false;
        void runProjectInfo(projectId);
      }
    }
  }

  /** The connected daemon is from an older release: it keeps serving the sessions it runs (an
   *  update never stops them) and hands over to this release's daemon once none is running. */
  let olderDaemon = false;
  /** The older daemon lacks a security fix this release made: ask until the user switches. */
  const [daemonNeedsSwitch, setDaemonNeedsSwitch] = createSignal(false);
  /** The connected daemon's API level: features needing newer RPCs hide on an older daemon. */
  const [daemonApi, setDaemonApi] = createSignal(0);

  async function switchDaemon() {
    if (!(await confirmDialog(t("Switch to the new session daemon now? Running sessions stop for a moment; agents that can resume continue their conversation.")))) return;
    olderDaemon = false;
    setDaemonNeedsSwitch(false);
    void ipc.daemon("daemon.shutdown", { kill_sessions: false }).catch(() => {});
  }

  /** Updates keep the daemon — and every session — running; the updated app connects to it.
   *  An older daemon is replaced as soon as no session runs on it, or now if the user asks (it
   *  stops gracefully: sessions come back interrupted and agents resume their conversation; the
   *  app starts the new daemon on its own). */
  async function checkDaemonApi() {
    olderDaemon = false;
    setDaemonNeedsSwitch(false);
    const [info, app] = await Promise.all([ipc.daemon<{ api?: number; version?: string; security?: number }>("daemon.info", {}), ipc.platformInfo()]).catch(() => [null, null]);
    // A daemon that predates API levels says none: level 1.
    setDaemonApi(info ? info.api ?? 1 : 0);
    // Only an older daemon: a dev app and an installed Kelpie may share one, and must not keep
    // offering to restart each other's.
    if (!info || !app || ((info.api ?? 1) >= app.api && !(info.version && olderVersion(info.version, app.version)))) return;
    olderDaemon = true;
    handOverIfIdle();
    const fix = (info.security ?? 1) < app.security;
    setDaemonNeedsSwitch(fix);
    notify(
      fix
        ? t("The session daemon from Kelpie {version} lacks a security fix in this release. Switch when you can: running sessions stop for a moment and agents resume.", { version: info.version ?? "?" })
        : t("Your sessions keep running on the session daemon from Kelpie {version}; the new one takes over once they have all stopped.", { version: info.version ?? "?" }),
      { label: t("Switch now"), run: () => void switchDaemon() },
      15000,
    );
  }

  /** The quick-answer popover for session `id` (daemons that can answer safely); otherwise the
   *  session itself. */
  function openAnswer(id: string) {
    const s = sessions[id];
    if (s && !s.chat && daemonApi() >= ANSWER_API && s.waiting_since) {
      setAnswering(id);
      setOverlay("answer");
    } else revealSession(id);
  }

  /** Shows the session that has waited longest on the user, then the next one on each call. */
  function revealNextWaiting() {
    const next = nextWaiting(Object.values(sessions), lastWaitingShown);
    if (!next) return notify(t("No session is waiting for you"));
    lastWaitingShown = next.id;
    revealSession(next.id);
  }
  let lastWaitingShown: string | null = null;

  /** Replaces an older daemon once nothing runs on it. A session can be down for a moment
   *  (restarted to keep its conversation, switched between terminal and chat, resumed at start),
   *  so it waits for things to settle and asks the daemon itself rather than trusting this
   *  window's list; `if_idle` lets newer daemons refuse when something started meanwhile. */
  let handOverTimer: ReturnType<typeof setTimeout> | undefined;
  function handOverIfIdle() {
    if (!olderDaemon) return;
    clearTimeout(handOverTimer);
    handOverTimer = setTimeout(async () => {
      if (!olderDaemon) return;
      const list = await ipc.listSessions().catch(() => null);
      if (!list || list.some((s) => s.alive)) return;
      olderDaemon = false;
      void ipc.daemon("daemon.shutdown", { kill_sessions: false, if_idle: true }).catch(() => {});
    }, 10_000);
  }

  // ---------- sessions ----------
  const byHandle = (handle: number) => Object.values(sessions).find((s) => s.handle === handle);

  async function refreshSessions() {
    const list = await ipc.listSessions();
    const map: Record<string, SessionInfo> = {};
    for (const s of list) map[s.id] = s;
    setSessions(reconcile(map));
  }

  // ---------- inbox (notifications & activity) ----------
  function addInbox(item: Omit<InboxItem, "id" | "at" | "read">) {
    update((s) => {
      s.inbox = [{ ...item, id: uid(), at: Date.now(), read: false }, ...(s.inbox ?? [])].slice(0, MAX_INBOX);
    });
  }

  function markInbox(id: string | "all", read = true) {
    update((s) => {
      for (const i of s.inbox ?? []) if (id === "all" || i.id === id) i.read = read;
    });
  }

  function clearInbox() {
    update((s) => (s.inbox = []));
  }

  const unreadInbox = () => (state.inbox ?? []).filter((i) => !i.read).length;

  /** Desktop notification (plus the optional chime) when the window is not looking at it. */
  async function desktopNotify(title: string, body: string) {
    if (!state.settings.notifications) return;
    if (state.settings.notificationSound) chime();
    try {
      let ok = await isPermissionGranted();
      if (!ok) ok = (await requestPermission()) === "granted";
      if (ok) sendNotification({ title, body });
    } catch {
      /* notifications unavailable */
    }
  }

  /** A daemon from before API 12 does not classify status changes: this window does. Newer ones
   *  send `notify.event`, also while no window is open. */
  const legacyNotify = () => daemonApi() > 0 && daemonApi() < NOTIFY_API;

  async function maybeNotify(prev: SessionInfo | undefined, next: SessionInfo) {
    if (!prev) return;
    // A turn that ended (or a process that died) in an error, reported once when it happens.
    const failed = !!next.failed && !prev.failed;
    if (!failed) {
      if (prev.status !== "working") return;
      if (next.status !== "idle" && next.status !== "waiting") return;
    }
    const visible = document.hasFocus() && isSessionVisible(next.id);
    if (visible) return;
    const project = state.projects.find((p) => p.id === next.project_id);
    const title = `${sessionLabel(next)}${next.remote ? " @" + next.remote.dest : ""}${project ? " · " + project.name : ""}`;
    const kind = failed ? "error" : next.status === "waiting" ? "waiting" : "finished";
    const body = failed
      ? next.alive ? t("Stopped with an error") : t("Exited with code {code}", { code: next.exit_code ?? "" })
      : kind === "waiting" ? t("Needs your input") : t("Finished");
    addInbox({ kind, title, body, sessionId: next.id, projectId: next.project_id ?? undefined });
    await desktopNotify(title, body);
  }

  /** An event from the daemon's single source: the inbox entry and the desktop notification.
   *  What a focused window already shows never arrives (the daemon knows through `ui.focus`). */
  function onNotifyEvent(ev: NotifyEvent) {
    const s = ev.session ? sessions[ev.session] : undefined;
    const projectId = ev.project_id ?? s?.project_id ?? undefined;
    const project = state.projects.find((p) => p.id === projectId);
    const d = describeNotify(ev, { label: s ? sessionLabel(s) : undefined, host: s?.remote?.dest, projectName: project?.name });
    addInbox({ kind: d.kind, title: d.title, body: d.body, sessionId: ev.session ?? undefined, projectId });
    void desktopNotify(d.title, d.body);
  }

  function onDaemonEvent(msg: { event: string; params: any }) {
    const p = msg.params;
    switch (msg.event) {
      case "session.updated": {
        const prev = sessions[p.id];
        setSessions(p.id, reconcile(p as SessionInfo));
        if (legacyNotify()) void maybeNotify(prev, p);
        // Background projects are not watched: an agent finishing a turn may have committed
        // or switched branch, so refresh that project's subtitle.
        if (prev?.status === "working" && p.status !== "working" && p.project_id) refreshProjectInfo(p.project_id, 1000);
        if (prev?.alive && !p.alive) handOverIfIdle();
        break;
      }
      case "session.closed": {
        const s = byHandle(p.handle);
        if (s) {
          setSessions(produce((m) => delete m[s.id]));
          if (state.floating?.sessions.includes(s.id)) {
            update((st) => {
              st.floating.sessions = st.floating.sessions.filter((x) => x !== s.id);
              if (st.floating.active === s.id) st.floating.active = st.floating.sessions[0] ?? null;
            });
          }
          if (state.recent?.some((r) => r.kind === "session" && r.id === s.id)) update((st) => (st.recent = st.recent.filter((r) => !(r.kind === "session" && r.id === s.id))));
        }
        handOverIfIdle();
        break;
      }
      case "session.resync": {
        const s = byHandle(p.handle);
        if (s?.chat) {
          window.dispatchEvent(new CustomEvent("kelpie:chat-resync", { detail: s.id }));
          break;
        }
        const v = s && peekView(s.id);
        if (v && v.handle === p.handle) void v.attach(p.handle);
        break;
      }
      case "meta.updated": {
        const m = p as WorkspaceMeta;
        setMeta(metaKey(m.path), m.comment || m.status ? reconcile(m) : undefined!);
        // An agent moving its card (`kelpied worktree set --status`) moves it on the board.
        const project = state.projects.find((x) => canonKey(x) === metaKey(m.path));
        if (project && m.status && project.status !== m.status) update((s) => (s.projects.find((x) => x.id === project.id)!.status = m.status!));
        break;
      }
      case "automation.ran": {
        if (legacyNotify()) {
          const title = t("Automation · {name}", { name: p.name });
          const body = p.error ? t("Failed: {error}", { error: p.error }) : t("Started");
          addInbox({ kind: p.error ? "error" : "automation", title, body, sessionId: p.session_id ?? undefined, projectId: (p.session_id && sessions[p.session_id]?.project_id) || undefined });
          void desktopNotify(title, body);
        }
        window.dispatchEvent(new CustomEvent("kelpie:automations"));
        break;
      }
      case "notify.event":
        onNotifyEvent(p as NotifyEvent);
        break;
      case "scheduled.updated":
        // Read by lib/limits.ts (the store must not import it: it imports the store).
        window.dispatchEvent(new CustomEvent("kelpie:scheduled", { detail: p }));
        break;
      case "automation.changed":
        window.dispatchEvent(new CustomEvent("kelpie:automations"));
        break;
      case "session.turn":
        // Read by lib/session-usage.ts (the store must not import it: it imports the store).
        window.dispatchEvent(new CustomEvent("kelpie:session-turn", { detail: p }));
        break;
      case "checkpoint.updated":
        window.dispatchEvent(new CustomEvent("kelpie:checkpoint", { detail: p }));
        break;
      case "checkpoint.disabled":
        // Said once per repository (the daemon remembers it).
        notify(t("Turn snapshots are off for {repo}: they took over a second, twice in a row. Turn them on again in the Turns panel.", { repo: p.repo }));
        break;
      case "session.clipboard": {
        // OSC 52: only the terminal the user is looking at may set the clipboard, and we say so.
        const s = byHandle(p.handle);
        if (s && document.hasFocus() && isSessionVisible(s.id) && typeof p.text === "string") {
          void writeText(p.text);
          notify(t("Clipboard set by {name}", { name: sessionLabel(s) }));
        }
        break;
      }
    }
  }

  // ---------- projects & workspaces ----------
  const activeProject = () => state.projects.find((p) => p.id === state.activeProject) ?? null;
  const workspace = (projectId: string): Workspace => state.workspaces[projectId] ?? { tabs: [], activeTab: null, rightPanel: null };

  function ensureWorkspace(projectId: string) {
    if (!state.workspaces[projectId]) update((s) => (s.workspaces[projectId] = { tabs: [], activeTab: null, rightPanel: "files" }));
  }

  const samePath = (a: string, b: string) => normPath(a) === normPath(b);

  /** Adds a folder (or, with `parentId`, a worktree of that project) and returns its id. */
  function addProject(path: string, opts: { parentId?: string; branch?: string; baseBranch?: string; name?: string; open?: boolean; remote?: Project["remote"] } = {}): string {
    const existing = state.projects.find((p) => samePath(p.path, path));
    if (existing) {
      // Adding a remote project again takes what its host has now (tmux installed since, agents).
      const remote = opts.remote;
      if (remote && existing.remote) {
        update((s) => {
          const p = s.projects.find((x) => x.id === existing.id)!;
          p.remote = { ...p.remote!, tmux: remote.tmux, agents: remote.agents ?? p.remote!.agents };
        });
      }
      if (opts.open !== false) switchProject(existing.id);
      return existing.id;
    }
    const parent = opts.parentId ? state.projects.find((p) => p.id === opts.parentId) : undefined;
    const project: Project = {
      id: uid(),
      name: opts.name ?? basename(path),
      path,
      // Worktrees share their repository's colour.
      color: parent?.color ?? projectColors[topProjects().length % projectColors.length],
      ...(parent ? { parentId: parent.id, branch: opts.branch, ...(opts.baseBranch ? { baseBranch: opts.baseBranch } : {}) } : {}),
      ...(opts.remote ? { remote: opts.remote } : {}),
    };
    update((s) => s.projects.push(project));
    ensureWorkspace(project.id);
    resolveCanonKey(project);
    refreshProjectInfo(project.id, 0);
    if (opts.open !== false) switchProject(project.id);
    return project.id;
  }

  /** Removes a project from the sidebar (files and running sessions are not touched); its
   *  worktree children go with it. */
  function removeProject(id: string) {
    const ids = new Set([id, ...state.projects.filter((p) => p.parentId === id).map((p) => p.id)]);
    update((s) => {
      s.projects = s.projects.filter((p) => !ids.has(p.id));
      for (const x of ids) {
        delete s.workspaces[x];
        if (s.collapsed) delete s.collapsed[x];
      }
      s.recent = (s.recent ?? []).filter((r) => !ids.has(r.projectId) && !(r.kind === "project" && ids.has(r.id)));
      if (s.activeProject && ids.has(s.activeProject)) s.activeProject = s.projects.find((p) => !p.parentId)?.id ?? s.projects[0]?.id ?? null;
    });
    setProjectInfo(produce((m) => ids.forEach((x) => delete m[x])));
  }

  /** Projects shown at the top level of the sidebar (worktrees sit under their repository). */
  function topProjects(): Project[] {
    return state.projects.filter((p) => !p.parentId || !state.projects.some((x) => x.id === p.parentId));
  }

  const childrenOf = (id: string) => state.projects.filter((p) => p.parentId === id);

  function setProjectStatus(id: string, status: WorkStatus) {
    update((s) => {
      const p = s.projects.find((x) => x.id === id);
      if (p) p.status = status;
    });
    // Agents read it back with `kelpied worktree get`.
    const p = state.projects.find((x) => x.id === id);
    if (p) void ipc.daemon("meta.set", { path: p.path, status }).catch(() => {});
  }

  /** The folder's comment / status set by agents or the user (`meta.*`). */
  const metaFor = (project: Project): WorkspaceMeta | undefined => meta[canonKey(project)];

  async function setComment(project: Project, comment: string) {
    await ipc.daemon("meta.set", { path: project.path, comment }).catch((e) => notify(String(e)));
  }

  async function loadMeta() {
    const map = await ipc.daemon<Record<string, Omit<WorkspaceMeta, "path">>>("meta.list").catch(() => null);
    if (!map) return;
    const next: Record<string, WorkspaceMeta> = {};
    for (const [k, v] of Object.entries(map)) next[metaKey(k)] = { ...v, path: k };
    setMeta(reconcile(next));
  }

  // ---------- review notes (diff annotations) ----------
  const notesFor = (projectId: string): ReviewNote[] => state.reviewNotes?.[projectId] ?? [];

  function addNote(projectId: string, note: Omit<ReviewNote, "id" | "at">) {
    update((s) => {
      s.reviewNotes ??= {};
      (s.reviewNotes[projectId] ??= []).push({ ...note, id: uid(), at: Date.now() });
    });
  }

  function removeNotes(projectId: string, ids: string[] | "all") {
    update((s) => {
      const list = s.reviewNotes?.[projectId];
      if (!list) return;
      s.reviewNotes[projectId] = ids === "all" ? [] : list.filter((n) => !ids.includes(n.id));
    });
  }

  /** Remembers the `setup:` / `tabs:` / `copy:` / `link:` of a `.kelpie.yaml` (their canonical text) the user agreed to run for this repository. */
  function approveSetup(repoPath: string, key: string) {
    update((s) => {
      s.approvedSetups ??= {};
      s.approvedSetups[normPath(repoPath)] = key;
    });
  }

  /** Remembers the `check:` commands the user agreed to run for this repository (separately: approving checks never approves setup). */
  function approveChecks(repoPath: string, check: string[]) {
    update((s) => {
      s.approvedChecks ??= {};
      s.approvedChecks[normPath(repoPath)] = [...check];
    });
  }

  function togglePin(id: string) {
    update((s) => {
      const p = s.projects.find((x) => x.id === id);
      if (p) p.pinned = !p.pinned;
    });
  }

  /** Links a saved host to a project (or changes its folder there); the host stays in the vault. */
  function linkHost(projectId: string, link: ProjectHost) {
    update((s) => {
      const p = s.projects.find((x) => x.id === projectId);
      if (!p) return;
      p.hosts = [...(p.hosts ?? []).filter((h) => h.hostId !== link.hostId), link];
    });
  }

  function unlinkHost(projectId: string, hostId: string) {
    update((s) => {
      const p = s.projects.find((x) => x.id === projectId);
      if (p?.hosts) p.hosts = p.hosts.filter((h) => h.hostId !== hostId);
    });
  }

  function renameProject(id: string, name: string) {
    update((s) => {
      const p = s.projects.find((x) => x.id === id);
      if (p) p.name = name;
    });
  }

  /** Moves a project among its siblings (top-level projects, or worktrees of one repository). */
  function moveProject(id: string, delta: number) {
    update((s) => {
      const me = s.projects.find((p) => p.id === id);
      if (!me) return;
      // A worktree whose repository is gone sits at the top level: move it among those.
      const parentOf = (p: Project) => (p.parentId && s.projects.some((x) => x.id === p.parentId) ? p.parentId : null);
      const siblings = s.projects.filter((p) => parentOf(p) === parentOf(me));
      const other = siblings[siblings.indexOf(me) + delta];
      if (!other) return;
      const i = s.projects.indexOf(me);
      const j = s.projects.indexOf(other);
      s.projects[i] = other;
      s.projects[j] = me;
    });
  }

  function switchProject(id: string) {
    if (state.activeProject === id) return;
    const t0 = performance.now();
    setLastSwitchMs(null);
    ensureWorkspace(id);
    update((s) => (s.activeProject = id));
    recordRecent("project", id, id);
    void refreshProjectInfo(id);
    // Measure until the new project's panes have painted.
    requestAnimationFrame(() => requestAnimationFrame(() => setLastSwitchMs(Math.round(performance.now() - t0))));
  }

  // ---------- tabs & panes ----------
  const activeTab = (projectId: string): Tab | undefined => {
    const ws = state.workspaces[projectId];
    return ws?.tabs.find((t) => t.id === ws.activeTab) ?? ws?.tabs[0];
  };

  function addTab(projectId: string, pane: Pane) {
    ensureWorkspace(projectId);
    const tab: Tab = { id: uid(), root: leaf(pane), focus: pane.id };
    update((s) => {
      const ws = s.workspaces[projectId];
      ws.tabs.push(tab);
      ws.activeTab = tab.id;
    });
    return tab;
  }

  function setTabRoot(projectId: string, tabId: string, root: LayoutNode, focus?: string) {
    update((s) => {
      const tab = s.workspaces[projectId]?.tabs.find((t) => t.id === tabId);
      if (!tab) return;
      tab.root = root;
      if (focus) tab.focus = focus;
    });
  }

  function selectTab(projectId: string, tabId: string) {
    update((s) => (s.workspaces[projectId].activeTab = tabId));
  }

  function cycleTab(delta: 1 | -1) {
    const pid = state.activeProject;
    if (!pid) return;
    const ws = workspace(pid);
    if (!ws.tabs.length) return;
    const i = ws.tabs.findIndex((t) => t.id === ws.activeTab);
    selectTab(pid, ws.tabs[(i + delta + ws.tabs.length) % ws.tabs.length].id);
  }

  function focusPane(projectId: string, tabId: string, paneId: string) {
    // Focus must never point at a pane hidden by another pane's maximize.
    const z = zoomed()[tabId];
    if (z && z !== paneId) toggleZoom(tabId, z);
    const ws = state.workspaces[projectId];
    const tab = ws?.tabs.find((t) => t.id === tabId);
    if (!tab || (tab.focus === paneId && ws.activeTab === tabId)) return;
    update((s) => {
      const w = s.workspaces[projectId];
      w.activeTab = tabId;
      const t = w.tabs.find((x) => x.id === tabId);
      if (t) t.focus = paneId;
    });
  }

  /** Places a pane: in place of an empty pane (`into`, keeping its id), split the focused pane,
   *  or a new tab. */
  function placePane(projectId: string, pane: Pane, where: "tab" | "right" | "down" = "tab", into?: PaneTarget) {
    // Found by pane id (its tab may have been merged meanwhile), and only while still empty: a
    // second start into the same pane must not replace the first session.
    const target = into && workspace(projectId).tabs.find((t) => findPane(t.root, into.paneId)?.kind === "empty");
    if (target) return setTabRoot(projectId, target.id, replacePane(target.root, into!.paneId, { ...pane, id: into!.paneId }), into!.paneId);
    const tab = activeTab(projectId);
    if (where === "tab" || !tab) return addTab(projectId, pane);
    // Splitting while a pane is maximized would hide the new pane: restore the layout first.
    if (zoomed()[tab.id]) toggleZoom(tab.id, zoomed()[tab.id]);
    setTabRoot(projectId, tab.id, splitPane(tab.root, tab.focus, pane, where === "right" ? "row" : "col"), pane.id);
  }

  function paneLocation(sessionId: string): { projectId: string; tabId: string; paneId: string } | null {
    for (const [projectId, ws] of Object.entries(state.workspaces)) {
      for (const tab of ws.tabs) {
        const pane = allPanes(tab.root).find((p) => p.sessionId === sessionId);
        if (pane) return { projectId, tabId: tab.id, paneId: pane.id };
      }
    }
    return null;
  }

  function isSessionVisible(sessionId: string) {
    if (state.floating?.open && state.floating.active === sessionId) return true;
    const loc = paneLocation(sessionId);
    if (!loc || loc.projectId !== state.activeProject) return false;
    return activeTab(loc.projectId)?.id === loc.tabId;
  }

  /** `account`: an account id for Claude / Codex ("default" = the agent's own; absent = the
   *  active one); `env` adds to the profile's environment (e.g. a history item's account). */
  /** `target`: run on another machine over SSH instead of where the project is (a local project
   *  can hold panes on any host). */
  type StartOpts = { args?: string[]; name?: string; agentSessionId?: string; cwd?: string; prompt?: string; account?: string; env?: Record<string, string>; target?: SessionTarget };

  /** Starts a session for a project without placing it in a layout. */
  async function startSession(projectId: string, kind: SessionKind, opts: StartOpts = {}): Promise<SessionInfo | undefined> {
    const project = state.projects.find((p) => p.id === projectId);
    if (!project) return;
    const launch = profileLaunch(kind, { account: opts.account });
    const target = opts.target ?? projectTarget(project);
    // Saved passwords / passphrases are looked up by the session's own host, not the project's.
    const remoteEnv = target ? await targetEnv(target.dest) : {};
    const env = opts.env || Object.keys(remoteEnv).length ? { ...(launch.env ?? {}), ...remoteEnv, ...(opts.env ?? {}) } : launch.env;
    const remote = target ? targetRemote(target) : undefined;
    const args = opts.args ?? (kind === "shell" ? state.settings.shellArgs : launch.args);
    if (!(await confirmRootStart(kind, args, target, projectId))) return;
    try {
      const info = await ipc.createSession({
        kind,
        // Resuming must use the conversation's own folder: agents look transcripts up by cwd.
        // A remote session starts in its folder on the host.
        cwd: opts.cwd ?? (target ? target.path : project.path),
        remote,
        project_id: projectId,
        name: opts.name,
        args,
        env,
        agent_session_id: opts.agentSessionId,
        prompt: opts.prompt,
        cols: 120,
        rows: 32,
      });
      setSessions(info.id, info);
      return info;
    } catch (e) {
      notify(t("Could not start {kind}: {error}", { kind, error: String(e) }));
    }
  }

  /** Starts a session and shows it: in a new tab, split next to the focused pane, or in place
   *  of an empty pane (`into`). */
  async function newSession(projectId: string, kind: SessionKind, opts: StartOpts & { where?: "tab" | "right" | "down"; into?: PaneTarget } = {}) {
    const info = await startSession(projectId, kind, opts);
    if (!info) return;
    placePane(projectId, { id: uid(), kind: "terminal", sessionId: info.id }, opts.where ?? "tab", opts.into);
    return info;
  }

  // ---------- grids & saved layouts ----------
  const emptyPane = (): Pane => ({ id: uid(), kind: "empty" });

  /** Splits the focused pane (or opens a tab) with an empty pane whose launcher asks what to
   *  run in it: every "split" entry point goes through here. A split of a pane on a host offers
   *  that host first. */
  function splitPicker(projectId: string, where: "tab" | "right" | "down") {
    const tab = activeTab(projectId);
    const focused = where !== "tab" && tab ? findPane(tab.root, tab.focus) : undefined;
    const s = focused?.sessionId ? sessions[focused.sessionId] : undefined;
    const target = (s && sessionTarget(s)) ?? focused?.target;
    placePane(projectId, { ...emptyPane(), ...(target ? { target } : {}) }, where);
  }

  /** Rearranges a tab's panes into a grid (reading order). Cells left over become empty panes;
   *  panes that do not fit move to new tabs in the same arrangement. */
  function arrangeTab(projectId: string, tabId: string, spec: GridSpec) {
    const tab = workspace(projectId).tabs.find((t) => t.id === tabId);
    if (!tab) return;
    if (zoomed()[tab.id]) toggleZoom(tab.id, zoomed()[tab.id]);
    const n = cellCount(spec);
    // Empty panes are placeholders: drop them before filling the new grid.
    const panes = allPanes(tab.root).filter((p) => p.kind !== "empty");
    const keep = panes.slice(0, n);
    const rest = panes.slice(n);
    const root = buildGrid(spec, keep, emptyPane);
    setTabRoot(projectId, tabId, root, keep.some((p) => p.id === tab.focus) ? tab.focus : allPanes(root)[0].id);
    for (let i = 0; i < rest.length; i += n) {
      const chunk = rest.slice(i, i + n);
      const extra = addTab(projectId, chunk[0]);
      setTabRoot(projectId, extra.id, buildGrid(spec, chunk, emptyPane), chunk[0].id);
    }
    if (rest.length) {
      selectTab(projectId, tabId);
      notify(rest.length === 1 ? t("{n} pane did not fit and moved to another tab", { n: rest.length }) : t("{n} panes did not fit and moved to another tab", { n: rest.length }));
    }
  }

  /** Panes that hold something (empty launcher cells do not count). */
  const realPanes = (tab: Tab) => allPanes(tab.root).filter((p) => p.kind !== "empty");

  /** The tabs (other than `tabId`) that "Tile all" would pull into that tab: tabs holding one CLI
   *  (terminal or chat). Tabs with a split are deliberate layouts; file, diff and browser tabs are
   *  not CLIs. */
  function tileCandidates(projectId: string, tabId: string): Tab[] {
    return workspace(projectId).tabs.filter((t) => {
      if (t.id === tabId) return false;
      const panes = allPanes(t.root);
      return panes.length === 1 && !!panes[0].sessionId;
    });
  }

  /** Tile all would change something: gather another tab, or re-even this tab's own panes. */
  function canTile(projectId: string, tabId: string) {
    const tab = workspace(projectId).tabs.find((t) => t.id === tabId);
    return !!tab && (realPanes(tab).length > 1 || (realPanes(tab).length === 1 && tileCandidates(projectId, tabId).length > 0));
  }

  function canSpread(projectId: string, tabId: string) {
    const tab = workspace(projectId).tabs.find((t) => t.id === tabId);
    return !!tab && realPanes(tab).length > 1;
  }

  /** Tile all: this tab's panes, then every one-CLI tab in tab-bar order, in the most even grid.
   *  Tabs whose pane joined close; their sessions keep running. Broadcast goes off when other
   *  tabs join, so keystrokes never start reaching agents the user did not pick. */
  function tileTab(projectId: string, tabId: string) {
    const tab = workspace(projectId).tabs.find((t) => t.id === tabId);
    if (!tab || !canTile(projectId, tabId)) return;
    if (zoomed()[tab.id]) toggleZoom(tab.id, zoomed()[tab.id]);
    const cap = cellCount(autoGrid(Infinity));
    const own = realPanes(tab);
    // More panes than the biggest grid: the arrange path spills the rest into new tabs.
    if (own.length >= cap) return arrangeTab(projectId, tabId, autoGrid(cap));
    const others = tileCandidates(projectId, tabId);
    const joining = others.slice(0, cap - own.length);
    const panes = [...own, ...joining.map((t) => allPanes(t.root)[0])];
    const joined = new Set(joining.map((t) => t.id));
    const taken = new Set(panes.map((p) => p.id));
    const root = buildGrid(autoGrid(panes.length), panes, emptyPane);
    update((s) => {
      const w = s.workspaces[projectId];
      w.tabs = w.tabs.filter((t) => !joined.has(t.id));
      const t = w.tabs.find((x) => x.id === tabId)!;
      t.root = root;
      if (!taken.has(t.focus)) t.focus = panes[0].id;
      if (joined.size) t.broadcast = false;
      w.activeTab = tabId;
    });
    if (joined.size) notify(joined.size === 1 ? t("{n} tab joined this grid", { n: joined.size }) : t("{n} tabs joined this grid", { n: joined.size }));
    const left = others.length - joining.length;
    if (left) notify(left === 1 ? t("{n} tab did not fit and stays as it is", { n: left }) : t("{n} tabs did not fit and stay as they are", { n: left }));
  }

  /** Spread into tabs: every pane of this tab gets a tab of its own, right after it; the focused
   *  pane stays here. Empty panes (launchers) are dropped; the tab's name and broadcast described
   *  the group, so they go with it. */
  function spreadTab(projectId: string, tabId: string) {
    const tab = workspace(projectId).tabs.find((t) => t.id === tabId);
    if (!tab || !canSpread(projectId, tabId)) return;
    const panes = realPanes(tab);
    if (zoomed()[tab.id]) toggleZoom(tab.id, zoomed()[tab.id]);
    const keep = panes.find((p) => p.id === tab.focus) ?? panes[0];
    const rest: Tab[] = panes.filter((p) => p.id !== keep.id).map((p) => ({ id: uid(), root: leaf(p), focus: p.id }));
    update((s) => {
      const w = s.workspaces[projectId];
      const i = w.tabs.findIndex((x) => x.id === tabId);
      if (i < 0) return;
      w.tabs[i].root = leaf(keep);
      w.tabs[i].focus = keep.id;
      w.tabs[i].broadcast = false;
      delete w.tabs[i].name;
      w.tabs.splice(i + 1, 0, ...rest);
      w.activeTab = tabId;
    });
  }

  /** Opens a new tab with a grid; cell i runs `kinds[i]` (null = an empty pane with a
   *  launcher). `prompt` goes to every agent started. */
  async function launchGrid(projectId: string, spec: GridSpec, kinds: (SessionKind | null)[], opts: { prompt?: string; args?: string[]; account?: string; target?: SessionTarget } = {}) {
    const n = cellCount(spec);
    const wanted = Array.from({ length: n }, (_, i) => kinds[i] ?? null);
    const started = await Promise.all(wanted.map((k) => (k ? startSession(projectId, k, { args: opts.args, account: opts.account, target: opts.target, prompt: isAgentKind(k) || k === "shell" ? opts.prompt : undefined }) : Promise.resolve(undefined))));
    const panes = started.map((info): Pane => (info ? { id: uid(), kind: "terminal", sessionId: info.id } : { ...emptyPane(), ...(opts.target ? { target: opts.target } : {}) }));
    const tab = addTab(projectId, panes[0]);
    setTabRoot(projectId, tab.id, buildGrid(spec, panes, emptyPane), panes[0].id);
    return tab;
  }

  /** Saves the shape of a tab and what each pane runs (replacing a layout of the same name). */
  function saveLayout(projectId: string, tabId: string, name: string) {
    const tab = workspace(projectId).tabs.find((t) => t.id === tabId);
    if (!tab || !name.trim()) return;
    // Panes on a host other than the project's own remember it, so the layout reopens them there.
    const project = state.projects.find((p) => p.id === projectId);
    const own = project && projectTarget(project);
    const leafOf = (p: Pane) => {
      const s = p.sessionId ? sessions[p.sessionId] : undefined;
      const target = s ? sessionTarget(s) : p.target;
      return { kind: s?.kind ?? null, target: target && target.dest !== own?.dest ? layoutTarget(project, target) : undefined };
    };
    const template = toTemplate(tab.root, leafOf);
    update((s) => {
      s.settings.layouts ??= [];
      const same = s.settings.layouts.find((l) => l.name.toLowerCase() === name.trim().toLowerCase());
      if (same) same.template = template;
      else s.settings.layouts.push({ id: uid(), name: name.trim(), template });
    });
    notify(t("Layout “{name}” saved", { name: name.trim() }));
  }

  function deleteLayout(id: string) {
    update((s) => (s.settings.layouts = (s.settings.layouts ?? []).filter((l) => l.id !== id)));
  }

  function renameLayout(id: string, name: string) {
    update((s) => {
      const l = s.settings.layouts?.find((x) => x.id === id);
      if (l && name.trim()) l.name = name.trim();
    });
  }

  /** Opens a saved layout in a new tab, starting what each pane ran. */
  async function applyLayout(projectId: string, template: LayoutTemplate) {
    const leaves = templateLeaves(template);
    const project = state.projects.find((p) => p.id === projectId);
    // Panes on a host start in this project's folder there; a host that cannot be reached (a new
    // host key, offline), or agents on a host without that folder, leave an empty pane offering it.
    const noFolder = new Set<string>();
    const targets = await Promise.all(leaves.map(async (l) => {
      if (!l.kind || !l.target) return l.target;
      const host = hostOf(l.target);
      if (host && isAgentKind(l.kind) && !agentFolder(host, project, l.target.path)) {
        if (!noFolder.has(host.id)) notify(t("Type the folder of {project} on {host} first: agents do not start in the login folder", { project: project?.name ?? "", host: host.label }));
        noFolder.add(host.id);
        return null;
      }
      try {
        const tg = await layoutTargetFor(project, l.target);
        const busy = agentsAt(tg, projectId);
        if (busy.length) notify(t("{n} agent(s) of another project already run in this folder: {list}", { n: busy.length, list: `${tg.dest}:${tg.path}` }));
        return tg;
      } catch (e) {
        const name = hostOf(l.target)?.label ?? l.target.dest;
        notify(isHostKeyError(e) ? t("Check the fingerprint of {name} first", { name }) : t("Could not connect to {list}", { list: name }));
        return null;
      }
    }));
    const started = await Promise.all(leaves.map((l, i) => (l.kind && targets[i] !== null ? startSession(projectId, l.kind, { target: targets[i] }) : Promise.resolve(undefined))));
    let i = 0;
    const root = fromTemplate(template, () => {
      const target = leaves[i].target;
      const info = started[i++];
      return info ? { id: uid(), kind: "terminal", sessionId: info.id } : { ...emptyPane(), ...(target ? { target } : {}) };
    });
    const first = allPanes(root)[0];
    const tab = addTab(projectId, first);
    setTabRoot(projectId, tab.id, root, first.id);
    return tab;
  }

  function equalizeTab(projectId: string, tabId: string) {
    const tab = workspace(projectId).tabs.find((t) => t.id === tabId);
    if (tab) setTabRoot(projectId, tabId, equalize(tab.root));
  }

  /** Drag and drop of panes: "center" swaps the two panes, an edge moves the pane there. */
  function dropPane(projectId: string, tabId: string, paneId: string, targetId: string, side: "center" | "left" | "right" | "top" | "bottom") {
    const tab = workspace(projectId).tabs.find((t) => t.id === tabId);
    if (!tab || paneId === targetId) return;
    const root = side === "center" ? swapPanes(tab.root, paneId, targetId) : movePane(tab.root, paneId, targetId, side);
    setTabRoot(projectId, tabId, root, paneId);
  }

  /** Moves a tab to position `to` in the tab bar (clamped). */
  function moveTab(projectId: string, tabId: string, to: number) {
    update((s) => {
      const tabs = s.workspaces[projectId]?.tabs;
      const from = tabs?.findIndex((t) => t.id === tabId) ?? -1;
      if (!tabs || from < 0) return;
      const dest = Math.max(0, Math.min(tabs.length - 1, to));
      if (dest === from) return;
      const [tab] = tabs.splice(from, 1);
      tabs.splice(dest, 0, tab);
    });
  }

  /** Moves a pane out of its tab: split to the right of another tab's focused pane, or into a
   *  new tab (`toTabId` null). A tab left empty closes; the session keeps running. */
  function movePaneToTab(projectId: string, fromTabId: string, paneId: string, toTabId: string | null) {
    const tabs = workspace(projectId).tabs;
    const from = tabs.find((t) => t.id === fromTabId);
    const pane = from && findPane(from.root, paneId);
    if (!from || !pane || fromTabId === toTabId) return;
    const dest = toTabId ? tabs.find((t) => t.id === toTabId) : undefined;
    if (toTabId && !dest) return;
    // Alone in its tab, "to a new tab" would change nothing.
    if (!dest && allPanes(from.root).length === 1) return;
    if (zoomed()[fromTabId]) toggleZoom(fromTabId, zoomed()[fromTabId]);
    const rest = removePane(from.root, paneId);
    update((st) => {
      const w = st.workspaces[projectId];
      if (!rest) w.tabs = w.tabs.filter((t) => t.id !== fromTabId);
      else {
        const t = w.tabs.find((x) => x.id === fromTabId)!;
        t.root = rest;
        if (t.focus === paneId) t.focus = allPanes(rest)[0].id;
      }
    });
    if (!dest) {
      // The new tab sits right after the one the pane came from.
      const tab = addTab(projectId, pane);
      const at = workspace(projectId).tabs.findIndex((t) => t.id === fromTabId);
      if (at >= 0) moveTab(projectId, tab.id, at + 1);
      return;
    }
    if (zoomed()[dest.id]) toggleZoom(dest.id, zoomed()[dest.id]);
    // A stale focus would make the split a no-op and lose the pane.
    const anchor = findPane(dest.root, dest.focus) ? dest.focus : allPanes(dest.root).at(-1)!.id;
    setTabRoot(projectId, dest.id, splitPane(dest.root, anchor, pane, "row"), pane.id);
    selectTab(projectId, dest.id);
  }

  /** Merges a whole tab into another: its layout goes next to pane `targetId` on `side`. */
  function mergeTab(projectId: string, fromTabId: string, toTabId: string, targetId: string, side: Side) {
    const tabs = workspace(projectId).tabs;
    const from = tabs.find((t) => t.id === fromTabId);
    const dest = tabs.find((t) => t.id === toTabId);
    if (!from || !dest || fromTabId === toTabId || !findPane(dest.root, targetId)) return;
    for (const id of [fromTabId, toTabId]) if (zoomed()[id]) toggleZoom(id, zoomed()[id]);
    const focus = findPane(from.root, from.focus) ? from.focus : allPanes(from.root)[0].id;
    const root = insertBeside(dest.root, targetId, from.root, side);
    update((st) => {
      const w = st.workspaces[projectId];
      w.tabs = w.tabs.filter((t) => t.id !== fromTabId);
      const t = w.tabs.find((x) => x.id === toTabId)!;
      t.root = root;
      t.focus = focus;
      w.activeTab = toTabId;
    });
  }

  function setBroadcast(projectId: string, tabId: string, on: boolean) {
    update((s) => {
      const t = s.workspaces[projectId]?.tabs.find((x) => x.id === tabId);
      if (t) t.broadcast = on;
    });
  }

  function renameTab(projectId: string, tabId: string, name: string) {
    update((s) => {
      const t = s.workspaces[projectId]?.tabs.find((x) => x.id === tabId);
      if (t) t.name = name.trim() || undefined;
    });
  }

  /** What is typed into a terminal of a tab with broadcast on also goes to its other live
   *  terminals. */
  setInputFanout((sessionId) => {
    const loc = paneLocation(sessionId);
    if (!loc) return [];
    const tab = state.workspaces[loc.projectId]?.tabs.find((t) => t.id === loc.tabId);
    if (!tab?.broadcast) return [];
    return allPanes(tab.root)
      .filter((p) => p.kind === "terminal" && p.sessionId && p.sessionId !== sessionId)
      .map((p) => sessions[p.sessionId!])
      .filter((x): x is SessionInfo => !!x?.alive)
      .map((x) => x.handle);
  });

  /** Reveals a session: switches project/tab, or opens it in a new tab if not in any layout. */
  function revealSession(sessionId: string) {
    const loc = paneLocation(sessionId);
    recordRecent("session", sessionId, sessions[sessionId]?.project_id ?? loc?.projectId ?? state.activeProject ?? "");
    if (loc) {
      switchProject(loc.projectId);
      focusPane(loc.projectId, loc.tabId, loc.paneId);
      return;
    }
    const s = sessions[sessionId];
    const projectId = s?.project_id && state.projects.some((p) => p.id === s.project_id) ? s.project_id : state.activeProject;
    if (!projectId) return;
    switchProject(projectId);
    addTab(projectId, { id: uid(), kind: s?.chat ? "chat" : "terminal", sessionId });
  }

  function openFile(projectId: string, path: string, opts: { line?: number; diff?: boolean; staged?: boolean; rev?: Pane["rev"]; base?: string } = {}) {
    const kind = opts.diff ? "diff" : "file";
    if (!opts.diff) recordRecent("file", path, projectId);
    const ws = workspace(projectId);
    for (const tab of ws.tabs) {
      const pane = allPanes(tab.root).find((p) => p.kind === kind && p.path === path && !!p.staged === !!opts.staged && p.rev?.session === opts.rev?.session && p.rev?.n === opts.rev?.n && p.base === opts.base);
      if (pane) {
        if (opts.line) setTabRoot(projectId, tab.id, replacePane(tab.root, pane.id, { ...pane, line: opts.line }));
        focusPane(projectId, tab.id, pane.id);
        return;
      }
    }
    addTab(projectId, { id: uid(), kind, path, line: opts.line, staged: opts.staged, rev: opts.rev, base: opts.base });
  }

  /** Closes a pane. Terminal sessions are killed only when `kill` (default for exited ones). */
  async function closePane(projectId: string, tabId: string, paneId: string, kill = true) {
    const ws = state.workspaces[projectId];
    const tab = ws?.tabs.find((t) => t.id === tabId);
    if (!tab) return;
    const pane = findPane(tab.root, paneId);
    if (pane?.kind === "browser") closeBrowser(pane.id);
    if (pane?.sessionId && kill) {
      const s = sessions[pane.sessionId];
      disposeView(pane.sessionId);
      if (s) await ipc.daemon("session.close", { handle: s.handle }).catch(() => {});
    }
    const root = removePane(tab.root, paneId);
    update((st) => {
      const w = st.workspaces[projectId];
      if (!root) {
        w.tabs = w.tabs.filter((t) => t.id !== tabId);
        if (w.activeTab === tabId) w.activeTab = w.tabs[w.tabs.length - 1]?.id ?? null;
      } else {
        const t = w.tabs.find((x) => x.id === tabId)!;
        t.root = root;
        if (t.focus === paneId) t.focus = allPanes(root)[0].id;
      }
    });
  }

  /** Changes a pane's fields without moving focus (a page loading in a background pane). */
  function updatePane(projectId: string, tabId: string, paneId: string, patch: Partial<Pane>) {
    const tab = workspace(projectId).tabs.find((t) => t.id === tabId);
    const pane = tab && findPane(tab.root, paneId);
    if (tab && pane) setTabRoot(projectId, tabId, replacePane(tab.root, paneId, { ...pane, ...patch }));
  }

  function replacePaneIn(projectId: string, tabId: string, paneId: string, pane: Pane) {
    const tab = workspace(projectId).tabs.find((t) => t.id === tabId);
    if (tab) setTabRoot(projectId, tabId, replacePane(tab.root, paneId, pane), pane.id);
  }

  /** Moves the gutter after child `index` of a split to `pos` (0–1 along the split). */
  function resizeSplit(projectId: string, tabId: string, splitId: string, index: number, pos: number) {
    const tab = workspace(projectId).tabs.find((t) => t.id === tabId);
    if (tab) setTabRoot(projectId, tabId, moveGutter(tab.root, splitId, index, pos));
  }

  function setRightPanel(panel: Workspace["rightPanel"]) {
    const pid = state.activeProject;
    if (!pid) return;
    ensureWorkspace(pid);
    update((s) => {
      const ws = s.workspaces[pid];
      ws.rightPanel = ws.rightPanel === panel ? null : panel;
    });
  }

  function openSettings(section?: SettingsSection, item?: string) {
    if (section) setSettingsSection(section);
    setSettingsItem(item ?? null);
    setOverlay("settings");
  }

  function setCollapsed(projectId: string, folded: boolean) {
    update((s) => {
      s.collapsed ??= {};
      if (folded) s.collapsed[projectId] = true;
      else delete s.collapsed[projectId];
    });
  }

  async function markUnread(sessionId: string, unread: boolean) {
    const s = sessions[sessionId];
    if (s) await ipc.daemon("session.set_unread", { handle: s.handle, unread }).catch((e) => notify(String(e)));
  }

  async function renameSession(sessionId: string, name: string | null) {
    const s = sessions[sessionId];
    if (s) await ipc.daemon("session.update", { handle: s.handle, name }).catch((e) => notify(String(e)));
  }

  /** Shows a right-panel view without toggling it off when already shown. */
  function showRightPanel(panel: NonNullable<Workspace["rightPanel"]>) {
    const pid = state.activeProject;
    if (!pid) return;
    ensureWorkspace(pid);
    update((s) => (s.workspaces[pid].rightPanel = panel));
  }

  function setSettings(patch: Partial<Settings>) {
    update((s) => Object.assign(s.settings, patch));
  }

  /** Launch arguments and environment of an agent profile, permission mode included. */
  /** `yolo: false`: without the Yolo flags (chat view asks Allow / Deny itself). */
  function profileLaunch(kind: SessionKind, opts: { yolo?: boolean; account?: string | null } = {}): { args: string[]; env?: Record<string, string> } {
    const p = state.settings.profiles.find((x) => x.kind === kind);
    const args = [...(p?.args ?? [])];
    if (p?.permission && kind === "claude") args.push("--permission-mode", p.permission);
    if (p?.permission && kind === "codex") args.push("-c", `approval_policy=${p.permission}`);
    // Yolo applies to agents without an explicit per-agent permission choice.
    const yolo = presetFor(kind)?.yolo ?? [];
    if (opts.yolo !== false && state.settings.agentPermissions === "yolo" && !p?.permission && yolo.length && !yolo.every((a) => args.includes(a))) args.push(...yolo);
    const env = { ...(p?.env ?? {}), ...(accountEnv(kind, opts.account) ?? {}) };
    return { args, env: Object.keys(env).length ? env : undefined };
  }

  /** Edits an agent profile in place (rows bound to it stay mounted); preset agents get one
   *  on first edit. */
  function setProfile(kind: SessionKind, patch: Partial<AgentProfile>) {
    update((s) => {
      const p = s.settings.profiles.find((x) => x.kind === kind);
      if (p) Object.assign(p, patch);
      else s.settings.profiles.push({ kind, label: presetFor(kind)?.label ?? kind, args: [], ...patch });
    });
  }

  // ---------- hibernation & sleeping worktrees ----------
  /** Agents that can be stopped and later resumed into the same conversation. */
  function resumable(s: SessionInfo) {
    if (s.chat || !isAgentKind(s.kind)) return false;
    if (s.agent_session_id) return true;
    return !CORE_AGENTS.includes(s.kind) && !!presetFor(s.kind)?.resume_latest;
  }

  /** Hibernated while on screen (sleeping the open project): they wake only once the user has
   *  looked away and come back, not at once. */
  const sleptInView = new Set<string>();

  async function hibernateSessions(ids: string[]) {
    for (const id of ids) if (isSessionVisible(id)) sleptInView.add(id);
    const stopped: string[] = [];
    for (const id of ids) {
      const s = sessions[id];
      if (!s?.alive) continue;
      await ipc.daemon("session.kill", { handle: s.handle }).then(() => stopped.push(id), () => {});
    }
    if (stopped.length) update((st) => (st.hibernated = [...new Set([...(st.hibernated ?? []), ...stopped])]));
    return stopped.length;
  }

  /** Stops a worktree's running agents; they resume when the worktree is opened again. */
  async function sleepProject(projectId: string) {
    const ids = Object.values(sessions).filter((s) => s.project_id === projectId && s.alive && resumable(s)).map((s) => s.id);
    const n = await hibernateSessions(ids);
    notify(n === 0 ? t("No resumable agents running here") : n === 1 ? t("{n} agent stopped; it resumes when you open this worktree", { n }) : t("{n} agents stopped; they resume when you open this worktree", { n }));
  }

  /** Periodic pass: finished agents in background projects, idle for the configured time. */
  function hibernateIdle() {
    if (!state.settings.hibernate) return;
    const limit = Math.max(1, state.settings.hibernateMinutes ?? 30) * 60_000;
    const now = Date.now();
    const ids = Object.values(sessions)
      .filter((s) => s.alive && s.status === "idle" && resumable(s) && s.project_id !== state.activeProject && !isSessionVisible(s.id))
      .filter((s) => s.last_activity > 0 && now - s.last_activity >= limit)
      .map((s) => s.id);
    if (ids.length) void hibernateSessions(ids);
  }

  /** Resumes hibernated sessions that are on screen. */
  function wakeVisible() {
    const pid = state.activeProject;
    const tab = pid ? activeTab(pid) : undefined;
    if (!tab || !connected() || !(state.hibernated ?? []).length) return;
    const shown = new Set(allPanes(tab.root).map((p) => p.sessionId).filter((id): id is string => !!id && state.hibernated.includes(id)));
    if (state.floating?.open && state.floating.active && state.hibernated.includes(state.floating.active)) shown.add(state.floating.active);
    for (const id of [...sleptInView]) if (!shown.has(id)) sleptInView.delete(id);
    for (const id of shown) {
      const s = sessions[id];
      if (!s) {
        update((st) => (st.hibernated = st.hibernated.filter((x) => x !== id)));
        continue;
      }
      // Still stopping (re-checked when `alive` flips), or slept while being looked at.
      if (s.alive || sleptInView.has(id)) continue;
      update((st) => (st.hibernated = st.hibernated.filter((x) => x !== id)));
      void resumeSession(id);
    }
  }

  // ---------- resume (after hibernation or a daemon restart) ----------
  /** Set by chat/mode-switch.ts: starts a chat front-end resuming a stopped chat session. */
  let chatResumer: ((s: SessionInfo) => Promise<void>) | null = null;
  const setChatResumer = (fn: (s: SessionInfo) => Promise<void>) => (chatResumer = fn);
  const resuming = new Set<string>();

  /** The conversation a stopped Claude / Codex / OMP session was in when it never reported its
   *  id (Codex does so only after a turn): the newest transcript of that folder written since
   *  the session started that no other session owns. */
  async function findConversation(s: SessionInfo): Promise<string | null> {
    // Transcripts read here are this computer's: a session on a host resumes by re-attaching to
    // its tmux instead.
    if (!["claude", "codex", "omp"].includes(s.kind) || s.remote) return null;
    const [items, folder] = await Promise.all([ipc.agentHistoryList(false, accountHomes()).catch(() => []), ipc.canonicalPath(s.cwd).catch(() => s.cwd)]);
    const owned = new Set(Object.values(sessions).map((x) => x.agent_session_id).filter(Boolean));
    const home = sessionHome(s) ?? null;
    const hit = items
      .filter((h) => h.agent === s.kind && (h.home ?? null) === home && normPath(h.folder) === normPath(folder) && h.updated_at >= s.created_at - 5000 && !owned.has(h.id))
      .sort((a, b) => b.updated_at - a.updated_at)[0];
    return hit?.id ?? null;
  }

  /** Restarts a stopped session. Agents continue their conversation; chat sessions get a new
   *  chat front-end in the same pane. */
  async function resumeSession(id: string) {
    const s = sessions[id];
    if (!s || s.alive || resuming.has(id)) return;
    resuming.add(id);
    try {
      if (s.chat) {
        await chatResumer?.(s);
        return;
      }
      if (!s.agent_session_id && isAgentKind(s.kind)) {
        const conv = await findConversation(s);
        if (conv) await ipc.daemon("session.update", { handle: s.handle, agent_session_id: conv }).catch(() => {});
      }
      await ipc.daemon("session.restart", { handle: s.handle });
    } catch (e) {
      notify(t("Could not resume {name}: {error}", { name: sessionLabel(s), error: String(e) }));
    } finally {
      resuming.delete(id);
    }
  }

  /** Sessions the daemon restored after it stopped while they ran (reboot, update, crash):
   *  resume them when shown, all now, or leave them for the Resume button. */
  function resumeInterrupted() {
    const mode = state.settings.resumeSessions ?? "shown";
    const ids = Object.values(sessions).filter((s) => s.interrupted && !s.alive).map((s) => s.id);
    if (!ids.length || mode === "off") return;
    if (mode === "all") {
      // Staggered: twenty agents starting in the same instant fight over disk and CPU.
      ids.forEach((id, i) => setTimeout(() => void resumeSession(id), i * 400));
      notify(ids.length === 1 ? t("Resuming {n} session stopped by a restart", { n: ids.length }) : t("Resuming {n} sessions stopped by a restart", { n: ids.length }));
      return;
    }
    update((st) => (st.hibernated = [...new Set([...(st.hibernated ?? []), ...ids])]));
    wakeVisible();
  }

  // ---------- floating terminal ----------
  /** A floating-terminal session in the home folder (`opts` for a one-off command, e.g. an
   *  agent's sign-in). */
  async function newFloating(kind: SessionKind = "shell", opts: { args?: string[]; env?: Record<string, string>; name?: string } = {}) {
    const home = (await ipc.platformInfo().catch(() => null))?.home ?? ".";
    const launch = profileLaunch(kind);
    try {
      const env = opts.env ? { ...(launch.env ?? {}), ...opts.env } : launch.env;
      const info = await ipc.createSession({ kind, cwd: home, name: opts.name, args: opts.args ?? (kind === "shell" ? state.settings.shellArgs : launch.args), env, cols: 120, rows: 24 });
      setSessions(info.id, info);
      update((s) => {
        s.floating ??= { sessions: [], active: null, open: true };
        s.floating.sessions.push(info.id);
        s.floating.active = info.id;
        s.floating.open = true;
      });
    } catch (e) {
      notify(t("Could not start a terminal: {error}", { error: String(e) }));
    }
  }

  function toggleFloating(open?: boolean) {
    const next = open ?? !state.floating?.open;
    update((s) => {
      s.floating ??= { sessions: [], active: null, open: false };
      // Forget sessions closed elsewhere.
      s.floating.sessions = s.floating.sessions.filter((id) => sessions[id]);
      if (s.floating.active && !sessions[s.floating.active]) s.floating.active = s.floating.sessions[0] ?? null;
      s.floating.open = next;
    });
    if (next && !state.floating.sessions.length) void newFloating();
  }

  function selectFloating(id: string) {
    update((s) => (s.floating.active = id));
  }

  async function closeFloating(id: string) {
    const s = sessions[id];
    disposeView(id);
    if (s) await ipc.daemon("session.close", { handle: s.handle }).catch(() => {});
    update((st) => {
      st.floating.sessions = st.floating.sessions.filter((x) => x !== id);
      if (st.floating.active === id) st.floating.active = st.floating.sessions[st.floating.sessions.length - 1] ?? null;
      if (!st.floating.sessions.length) st.floating.open = false;
    });
  }

  /** Sets (chord) or clears ("") a shortcut override; `null` restores the default. */
  function setShortcut(commandId: string, chord: string | null) {
    update((s) => {
      s.keymap ??= {};
      if (chord === null) delete s.keymap[commandId];
      else s.keymap[commandId] = chord;
    });
  }

  /** Per-connection setup: feature check, agent table, folder notes, keep-awake mode. */
  async function onConnected() {
    // Before resuming interrupted agents: a hand-over decision must see them start.
    await checkDaemonApi();
    void loadAgentPresets();
    void loadMeta();
    void ipc.daemon("daemon.set_awake", { mode: state.settings.keepAwake ?? "off" }).catch(() => {});
    void ipc.daemon("daemon.set_priority", { enabled: state.settings.prioritizeVisible ?? true }).catch(() => {});
    void ipc.daemon("daemon.set_policy", { agents_control_others: state.settings.agentsScripting ?? true }).catch(() => {});
    resumeInterrupted();
  }

  // ---------- boot ----------
  async function init() {
    // Never let a default state silently replace a file we could not read or whose format we
    // do not own: keep a copy first, and do not save at all when that is impossible.
    let saved: any = null;
    let problem: "unreadable" | "unknown" | null = null;
    let problemDetail = "";
    try {
      saved = await ipc.loadState();
    } catch (e) {
      problem = "unreadable";
      problemDetail = String(e);
    }
    const version = saved && typeof saved === "object" ? saved.version : undefined;
    if (saved !== null && !problem && !(Number.isInteger(version) && version >= 1)) problem = "unknown";
    let migrate = false;
    if (problem) {
      const copy = await ipc.stateBackup("unreadable").catch(() => null);
      if (copy) notify(problem === "unreadable" ? t("Saved app state could not be read ({error}); a copy was kept at {path}. Starting fresh.", { error: problemDetail, path: copy }) : t("Saved app state has an unknown format; a copy was kept at {path}. Starting fresh.", { path: copy }), undefined, 12000);
      else {
        persistBlocked = true;
        notify(problem === "unreadable" ? t("Saved app state could not be read ({error}) and could not be backed up; changes in this session will not be saved.", { error: problemDetail }) : t("Saved app state has an unknown format and could not be backed up; changes in this session will not be saved."), undefined, 15000);
      }
      saved = null;
    } else if (saved && version !== STATE_VERSION) {
      const copy = await ipc.stateBackup(`v${version}`).catch(() => null);
      if (version > STATE_VERSION) {
        // Written by a newer Kelpie: use what we understand, never write it back in our format.
        persistBlocked = true;
        notify(t("App state was written by a newer version of Kelpie; changes in this session will not be saved."), undefined, 12000);
      } else if (!copy) {
        persistBlocked = true;
        notify(t("App state could not be backed up before updating its format; changes in this session will not be saved."), undefined, 12000);
      } else migrate = true;
    }
    if (saved) {
      // v1 → v2 only added optional fields; v2 → v3 turned binary splits into n-ary ones.
      for (const ws of Object.values<Workspace>(saved.workspaces ?? {})) for (const tab of ws.tabs ?? []) tab.root = migrateLayout(tab.root);
      setState(reconcile({ ...emptyState(), ...saved, version: STATE_VERSION, settings: { ...defaultSettings, ...saved.settings } }));
    }
    setLoaded(true);
    if (migrate) persist();
    setInterval(hibernateIdle, 60_000);
    // Sidebar subtitles; git calls are capped at 3 in flight by the backend.
    for (const p of state.projects) {
      refreshProjectInfo(p.id, 0);
      resolveCanonKey(p);
    }
    void ipc.onEvent<{ root: string; git: boolean }>("fs-changed", (e) => {
      if (!e.git) return;
      const p = state.projects.find((x) => x.path === e.root);
      if (p) refreshProjectInfo(p.id);
    });
    await ipc.onEvent("daemon-event", onDaemonEvent);
    await ipc.onEvent<{ connected: boolean; error?: string }>("daemon-status", async (s) => {
      if (!s.connected) {
        // Views must forget their stream before anything can re-attach.
        resetStreams();
        setConnected(false);
        if (s.error) notify(s.error);
        return;
      }
      resetStreams();
      // Fresh handles first, then flip `connected` so panes attach to the right sessions.
      await refreshSessions().catch(() => {});
      setConnected(true);
      void onConnected();
    });
    // The connection may predate our listener.
    try {
      await refreshSessions();
      setConnected(true);
      void onConnected();
    } catch {
      /* connection event will follow */
    }
  }

  return {
    state, sessions, connected, overlay, setOverlay, loaded, toast, notify, lastSwitchMs,
    init, activeProject, workspace, activeTab, addProject, removeProject, renameProject, moveProject, switchProject,
    addTab, selectTab, cycleTab, focusPane, placePane, newSession, startSession, revealSession, openFile, closePane,
    arrangeTab, tileCandidates, canTile, canSpread, tileTab, spreadTab, launchGrid, saveLayout, deleteLayout, renameLayout, applyLayout, equalizeTab, dropPane, setBroadcast, renameTab,
    splitPicker, moveTab, movePaneToTab, mergeTab,
    resumeSession, resumeInterrupted, setChatResumer,
    replacePaneIn, updatePane, resizeSplit, setRightPanel, showRightPanel, setSettings, zoomed, toggleZoom,
    topProjects, childrenOf, setProjectStatus, togglePin, approveSetup, approveChecks, linkHost, unlinkHost,
    projectInfo, refreshProjectInfo, recordRecent, setCollapsed, markUnread, renameSession, profileLaunch, setShortcut, setProfile,
    settingsSection, setSettingsSection, settingsItem, openSettings, paneLocation, isSessionVisible, refreshSessions, setSessions,
    addInbox, markInbox, clearInbox, unreadInbox, desktopNotify, meta, metaFor, setComment, loadMeta,
    daemonNeedsSwitch, switchDaemon, daemonApi, legacyNotify, openAnswer, revealNextWaiting,
    notesFor, addNote, removeNotes, resumable, sleepProject, hibernateIdle, hibernateSessions, wakeVisible,
    newFloating, toggleFloating, selectFloating, closeFloating,
  };
}

export interface Toast {
  text: string;
  /** `run` gets the click, for actions that open a menu at the button. */
  action?: { label: string; run: (e?: MouseEvent) => void };
}

export const store = createRoot(createAppStore);

export function sessionLabel(s: SessionInfo): string {
  if (s.name) return s.name;
  const title = s.title?.replace(/^[⠀-⣿✳✶✻✽·*◐◑◒◓⠂⠐]\s*/u, "").trim();
  const kind = kindShortName(s.kind);
  const redundant = title && [s.kind, kind.toLowerCase()].includes(title.toLowerCase());
  if (title && !redundant && title.length < 48 && !/^[A-Z]:\\|\.exe$/i.test(title)) return s.kind === "shell" ? title : `${kind} · ${title}`;
  return kind;
}
