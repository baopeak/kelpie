// Project sidebar: every project with its sessions (status, task title, model, last activity),
// branch subtitle, filter/sort, and context menus. Each row binds only to its own session's
// fields, so status ticks from 20 agents re-render single rows, not the list.
import { For, Show, createMemo, createResource, createSignal } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import { pickProjectFolder, chordOf, commandHint, commands } from "../lib/commands";
import { chordLabel, isMac, relTimeShort } from "../lib/platform";
import { editorOpen, editorsList, gitBranchDelete, revealEntry } from "../lib/ipc";
import { pullRequests, prLabel } from "../lib/pull-requests";
import { switchToChat, switchToTerminal, supportsChat } from "../chat/mode-switch";
import { conversationCommands, restartKeepingConversation, runConversationCommand } from "../lib/agent-actions";
import { daemon } from "../lib/ipc";
import { Icon, KindGlyph } from "./icons";
import { ContextRing } from "./context-ring";
import { BrandMark, BrandWordmark, KipCompanion, kipShown } from "./brand";
import { findPane, readingOrder } from "../lib/layout";
import { sessionAccount } from "../lib/accounts";
import { hostLabel } from "../lib/remote";
import { canGoBack, canGoForward, navBack, navForward } from "../lib/nav-history";
import { addExistingWorktrees, checkRemoval, removeWorktree } from "../lib/worktrees";
import { openWorktreeDialog } from "./worktree-dialog";
import { openFinish, overlapsOf } from "../lib/review-queue";
import { openContextMenu, type MenuItem } from "./context-menu";
import type { Project, SessionInfo } from "../lib/types";
import { t, msg } from "../lib/i18n";
import { openHosts } from "./hosts-page";
import { confirmDialog } from "../lib/confirm-dialog";

export const STATUS_WORDS = { idle: msg("idle"), working: msg("working"), waiting: msg("waiting"), exited: msg("exited") } as const;

/** What a session shows at a glance; each state has its own shape so color is never the only cue. */
export type SessionState = "waiting" | "error" | "working" | "done" | "idle" | "exited";

export function sessionState(s: SessionInfo): SessionState {
  if (!s.alive) return s.failed ? "error" : "exited";
  if (s.status === "waiting") return "waiting";
  if (s.status === "working") return "working";
  if (s.failed) return "error";
  return s.unread ? "done" : "idle";
}

/** States that ask for the user: they get a glyph, a brighter row and (for the first two) a word. */
const GLYPH: Partial<Record<SessionState, string>> = { waiting: "alert", error: "close", done: "check" };
const LABEL: Partial<Record<SessionState, string>> = { waiting: msg("needs you"), error: msg("error") };

function stateTitle(s: SessionInfo) {
  switch (sessionState(s)) {
    case "waiting": return t("Needs you");
    case "error": return s.alive ? t("Last turn failed") : t("Exited with code {code}", { code: s.exit_code ?? "" });
    case "done": return t("Finished, unread");
    default: return t(STATUS_WORDS[s.alive ? s.status : "exited"]);
  }
}

export function StatusDot(props: { s: SessionInfo }) {
  const state = () => sessionState(props.s);
  return (
    <span class={`dot ${state()}`} title={stateTitle(props.s)}>
      <Show when={GLYPH[state()]}>{(g) => <Icon name={g()} size={8} />}</Show>
    </span>
  );
}

/** Row being renamed inline: `s:<session id>` or `p:<project id>`. */
const [renaming, setRenaming] = createSignal<string | null>(null);

function RenameInput(props: { value: string; onDone: (value: string | null) => void }) {
  let done = false;
  const finish = (v: string | null) => {
    if (done) return;
    done = true;
    setRenaming(null);
    props.onDone(v);
  };
  return (
    <input
      class="rename-input"
      value={props.value}
      ref={(el) => queueMicrotask(() => (el.focus(), el.select()))}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Enter") finish(e.currentTarget.value);
        if (e.key === "Escape") finish(null);
      }}
      onBlur={(e) => finish(e.currentTarget.value)}
    />
  );
}

// Store proxies are stable, so returning the previous array when the order is unchanged
// keeps <For> from remounting every row on each session update.
const sameList = <T,>(prev: T[] | undefined, next: T[]) => (prev && prev.length === next.length && prev.every((x, i) => x === next[i]) ? prev : next);

/** Ticks every 30 s so relative ages stay fresh without per-row timers. */
const [now, setNow] = createSignal(Date.now());
setInterval(() => setNow(Date.now()), 30_000);

export const shortModel = (m: string | null | undefined) =>
  m ? m.replace(/^(claude|anthropic)[-/]/i, "").replace(/^openai[-/]/i, "").replace(/\[.*\]$/, "") : "";

/** A project's sessions in the order their panes are seen — tab by tab, each read top to bottom
 *  and left to right — so a session switched to chat view (a new session in the same pane) keeps
 *  its place. Sessions in no pane follow, oldest first. */
function projectSessions(projectId: string): SessionInfo[] {
  const rank = new Map<string, number>();
  for (const tab of store.workspace(projectId).tabs) {
    for (const p of readingOrder(tab.root)) if (p.sessionId && !rank.has(p.sessionId)) rank.set(p.sessionId, rank.size);
  }
  const at = (s: SessionInfo) => rank.get(s.id) ?? Number.MAX_SAFE_INTEGER;
  return Object.values(store.sessions)
    .filter((s) => s.project_id === projectId)
    .sort((a, b) => at(a) - at(b) || a.created_at - b.created_at);
}

const lastActivity = (projectId: string) =>
  projectSessions(projectId).reduce((m, s) => Math.max(m, s.last_activity || s.created_at), 0);

async function inPane(s: SessionInfo, fn: typeof switchToChat) {
  store.revealSession(s.id);
  const loc = store.paneLocation(s.id);
  const project = loc && store.state.projects.find((p) => p.id === loc.projectId);
  const tab = loc && store.workspace(loc.projectId).tabs.find((t) => t.id === loc.tabId);
  if (!loc || !project || !tab) return;
  const pane = findPane(tab.root, loc.paneId);
  if (pane) await fn(project, tab, pane);
}

function sessionMenu(s: SessionInfo): MenuItem[] {
  const items: MenuItem[] = [
    { label: t("Rename…"), run: () => setRenaming(`s:${s.id}`) },
    s.unread ? { label: t("Mark as read"), run: () => void store.markUnread(s.id, false) } : { label: t("Mark as unread"), run: () => void store.markUnread(s.id, true) },
  ];
  if (!s.alive && !s.chat) items.push({ label: store.resumable(s) ? t("Resume") : t("Restart"), run: () => void daemon("session.restart", { handle: s.handle }).catch((e) => store.notify(String(e))) });
  // Chat view runs the agent on this computer: never for a session on a remote host.
  if (s.alive && supportsChat(s.kind) && !s.remote) {
    items.push(s.chat ? { label: t("Continue in terminal (TUI)"), run: () => void inPane(s, switchToTerminal) } : { label: t("Continue in chat view"), run: () => void inPane(s, switchToChat) });
  }
  const conv = conversationCommands(s);
  if (conv) {
    items.push(null, { label: t("Compact conversation"), run: () => void runConversationCommand(s, "compact") });
    if (conv.clear) items.push({ label: t("Clear conversation"), run: () => void runConversationCommand(s, "clear") });
    if (store.resumable(s)) items.push({ label: t("Restart, keeping the conversation"), run: () => void restartKeepingConversation(s) });
  }
  items.push(null, {
    label: s.alive ? t("Stop and close") : t("Close"), danger: true,
    run: async () => {
      const loc = store.paneLocation(s.id);
      if (s.alive && s.kind !== "shell" && !(await confirmDialog(t("Stop {name}?", { name: sessionLabel(s) })))) return;
      if (loc) void store.closePane(loc.projectId, loc.tabId, loc.paneId);
      else void daemon("session.close", { handle: s.handle }).catch((e) => store.notify(String(e)));
    },
  });
  return items;
}

/** One confirmation that states everything removal loses, asked before anything is stopped. */
async function confirmRemoveWorktree(p: Project) {
  let check;
  try {
    check = await checkRemoval(p);
  } catch (e) {
    return store.notify(t("Could not inspect the worktree: {error}", { error: String(e) }), undefined, 8000);
  }
  if (check.orphanCommits) {
    const n = check.orphanCommits;
    return store.notify(
      n === 1
        ? t("“{name}” is on a detached HEAD with {n} commit no branch contains. Create a branch for them first (git switch -c <name>), then remove the worktree.", { name: p.name, n })
        : t("“{name}” is on a detached HEAD with {n} commits no branch contains. Create a branch for them first (git switch -c <name>), then remove the worktree.", { name: p.name, n }),
      undefined, 15000,
    );
  }
  const running = projectSessions(p.id).filter((s) => s.alive).length;
  const text = [
    t("Remove worktree “{name}”?", { name: p.name }),
    p.path,
    running ? (running === 1 ? t("Its {n} running session will be stopped.", { n: running }) : t("Its {n} running sessions will be stopped.", { n: running })) : "",
    check.changes ? (check.changes === 1 ? t("{n} uncommitted or untracked file will be lost.", { n: check.changes }) : t("{n} uncommitted or untracked files will be lost.", { n: check.changes })) : "",
    t("Ignored files in the folder (e.g. .env, build output) are deleted too."),
    check.branch ? t("The branch {branch} is kept.", { branch: check.branch }) : "",
  ].filter(Boolean).join("\n\n");
  if (!(await confirmDialog(text))) return;
  const repo = store.state.projects.find((x) => x.id === p.parentId);
  try {
    const { leftover } = await removeWorktree(p, { force: check.changes > 0 });
    if (leftover) store.notify(t("Removed worktree {name}, but its folder could not be fully deleted (a program still uses it): {path}", { name: p.name, path: p.path }), undefined, 12000);
    else if (!check.branch || !repo) store.notify(t("Removed worktree {name}", { name: p.name }));
  } catch (e) {
    return store.notify(t("Could not remove worktree: {error}", { error: String(e).replace(/^Error: /, "") }), undefined, 8000);
  }
  if (check.branch && repo) {
    store.notify(t("Removed worktree {name}; branch {branch} kept.", { name: p.name, branch: check.branch }), { label: t("Delete branch"), run: () => void deleteBranch(repo.path, check.branch!) }, 12000);
  }
}

/** `git branch -d`; when git refuses (unmerged commits) ask before `-D`. */
async function deleteBranch(repoPath: string, branch: string) {
  try {
    await gitBranchDelete(repoPath, branch, false);
    store.notify(t("Deleted branch {branch}", { branch }));
  } catch (e) {
    if (!(await confirmDialog(t("{error}\n\nThe branch {branch} has commits that are not merged. Delete it anyway? Those commits are then only reachable from the reflog.", { error: String(e), branch })))) return;
    await gitBranchDelete(repoPath, branch, true).then(() => store.notify(t("Deleted branch {branch}", { branch })), (err) => store.notify(String(err)));
  }
}

/** Code editors installed on this machine (for "Open in ..."). */
const [editors] = createResource(() => editorsList().catch(() => []));

function projectMenu(p: Project): MenuItem[] {
  const folded = !!store.state.collapsed?.[p.id];
  const repo = !p.parentId && store.projectInfo[p.id]?.is_repo;
  const worktree = !!p.parentId;
  return [
    ...(repo
      ? [
          { label: t("New worktree…"), run: () => openWorktreeDialog(p.id) },
          {
            label: t("Add existing worktrees"),
            run: () => void addExistingWorktrees(p).then((n) => store.notify(n ? (n === 1 ? t("Added {n} worktree", { n }) : t("Added {n} worktrees", { n })) : t("No other worktrees"))).catch((e) => store.notify(String(e))),
          },
          null,
        ]
      : []),
    { label: t("Rename…"), run: () => setRenaming(`p:${p.id}`) },
    { label: store.metaFor(p)?.comment ? t("Edit comment…") : t("Add comment…"), run: () => setRenaming(`c:${p.id}`) },
    { label: folded ? t("Show sessions") : t("Hide sessions"), run: () => store.setCollapsed(p.id, !folded) },
    { label: isMac ? t("Reveal in Finder") : t("Reveal in Explorer"), run: () => void revealEntry(p.path, "") },
    ...(editors() ?? []).slice(0, 4).map((ed) => ({ label: t("Open in {editor}", { editor: ed.label }), run: () => void editorOpen(ed.id, p.path).catch((e) => store.notify(String(e))) })),
    ...(projectSessions(p.id).some((s) => s.alive && store.resumable(s))
      ? [{ label: t("Sleep (stop agents, resume on open)"), run: () => void store.sleepProject(p.id) }]
      : []),
    { label: t("Move up"), run: () => store.moveProject(p.id, -1) },
    { label: t("Move down"), run: () => store.moveProject(p.id, 1) },
    null,
    ...(worktree && !p.remote ? [{ label: t("Finish…"), run: () => openFinish(p.id) }] : []),
    ...(worktree ? [{ label: t("Remove worktree…"), danger: true, run: () => void confirmRemoveWorktree(p) }] : []),
    {
      label: t("Remove from Kelpie"), danger: true,
      run: async () => {
        const kids = store.childrenOf(p.id).length;
        const text = !kids
          ? t("Remove {name} from the sidebar? Files and running sessions are not touched.", { name: p.name })
          : kids === 1
            ? t("Remove {name} and its {n} worktree from the sidebar? Files and running sessions are not touched.", { name: p.name, n: kids })
            : t("Remove {name} and its {n} worktrees from the sidebar? Files and running sessions are not touched.", { name: p.name, n: kids });
        if (await confirmDialog(text)) store.removeProject(p.id);
      },
    },
  ];
}

function SessionRow(props: { s: SessionInfo }) {
  const state = () => sessionState(props.s);
  return (
    <div
      class={`session-row ${state()}${store.isSessionVisible(props.s.id) ? " visible" : ""}`}
      onClick={() => store.revealSession(props.s.id)}
      onContextMenu={(e) => openContextMenu(e, sessionMenu(props.s))}
      title={[props.s.title, props.s.model, props.s.cwd].filter(Boolean).join("\n")}
    >
      <StatusDot s={props.s} />
      <KindGlyph kind={props.s.kind} size={14} />
      <span class="session-text">
        <Show when={renaming() === `s:${props.s.id}`} fallback={<span class="name">{sessionLabel(props.s)}</span>}>
          <RenameInput value={props.s.name ?? sessionLabel(props.s)} onDone={(v) => v !== null && void store.renameSession(props.s.id, v.trim() || null)} />
        </Show>
        <Show when={props.s.model || sessionAccount(props.s)}>
          <span class="model">{[props.s.model && shortModel(props.s.model), sessionAccount(props.s)?.label].filter(Boolean).join(" · ")}</span>
        </Show>
      </span>
      <ContextRing s={props.s} />
      <Show when={LABEL[state()]} fallback={<span class="age">{relTimeShort(props.s.last_activity || props.s.created_at, now())}</span>}>
        {(label) => <span class={`age state-label ${state()}`}>{t(label())}</span>}
      </Show>
    </div>
  );
}

function ProjectRow(props: { project: Project }) {
  const index = createMemo(() => store.topProjects().indexOf(props.project));
  const worktree = () => !!props.project.parentId;
  const children = createMemo<Project[]>((prev) => sameList(prev, store.childrenOf(props.project.id)));
  const active = () => store.state.activeProject === props.project.id;
  const sessions = createMemo(() => projectSessions(props.project.id));
  const counts = createMemo(() => {
    let working = 0, waiting = 0, unread = 0, error = 0;
    for (const s of sessions()) {
      const state = sessionState(s);
      if (state === "waiting") waiting++;
      else if (state === "error") error++;
      else if (state === "working") working++;
      else if (state === "done") unread++;
    }
    return { working, waiting, unread, error };
  });
  const folded = () => !!store.state.collapsed?.[props.project.id];
  const info = () => store.projectInfo[props.project.id];
  const collapsedSidebar = () => store.state.settings.sidebarCollapsed;
  const switchChord = () => {
    const c = commands.find((x) => x.id === `project.${index() + 1}`);
    return c && chordOf(c) ? chordLabel(chordOf(c)!) : "";
  };

  return (
    <div class={`project-block${active() ? " active" : ""}`}>
      <div
        class={`project${active() ? " active" : ""}`}
        style={{ "--kc": props.project.color }}
        onClick={() => store.switchProject(props.project.id)}
        onContextMenu={(e) => openContextMenu(e, projectMenu(props.project))}
        title={`${props.project.path}${index() >= 0 && index() < 9 ? `  (${switchChord()})` : ""}`}
        data-project={props.project.id}
      >
        <Show when={!collapsedSidebar() && (sessions().length || children().length)}>
          <button class="chev" title={folded() ? t("Show sessions") : t("Hide sessions")} onClick={(e) => { e.stopPropagation(); store.setCollapsed(props.project.id, !folded()); }}>
            <Icon name={folded() ? "chevronRight" : "chevronDown"} size={11} />
          </button>
        </Show>
        <Show when={worktree()} fallback={<span class="project-swatch" style={{ "--kc": props.project.color }}>{props.project.name.slice(0, 1).toUpperCase()}</span>}>
          <span class="wt-glyph" style={{ color: props.project.color }}><Icon name="branch" size={13} /></span>
        </Show>
        <Show when={!collapsedSidebar()}>
          <span class="project-text">
            <Show when={renaming() === `p:${props.project.id}`} fallback={<span class="project-name">{props.project.name}</span>}>
              <RenameInput value={props.project.name} onDone={(v) => v?.trim() && store.renameProject(props.project.id, v.trim())} />
            </Show>
            <Show when={renaming() === `c:${props.project.id}`}>
              <RenameInput value={store.metaFor(props.project)?.comment ?? ""} onDone={(v) => v !== null && void store.setComment(props.project, v)} />
            </Show>
            <Show when={props.project.remote}>
              <span class="project-sub remote-host" title={props.project.remote!.tmux ? t("On {host} over SSH · sessions kept in tmux", { host: hostLabel(props.project) }) : t("On {host} over SSH", { host: hostLabel(props.project) })}>
                <Icon name="server" size={10} /> {hostLabel(props.project)}
              </span>
            </Show>
            <Show when={info()?.is_repo}>
              <span class="project-sub">
                <Show when={!worktree()}><Icon name="branch" size={10} /> {info()!.branch ?? t("detached")}</Show>
                <Show when={worktree() && info()!.branch !== props.project.name}>{info()!.branch ?? t("detached")}</Show>
                {info()!.dirty ? "*" : ""}
                <Show when={info()!.ahead || info()!.behind}> ↑{info()!.ahead}↓{info()!.behind}</Show>
                <Show when={children().length}><span class="primary-tag">{t("primary")}</span></Show>
                <Show when={overlapsOf(props.project.id).length}>
                  <span class="primary-tag overlap" title={overlapsOf(props.project.id).map((o) => `${o.name}: ${o.files.slice(0, 5).join(", ")}${o.files.length > 5 ? "…" : ""}`).join("\n")}>
                    {t("Also changed in {name}", { name: overlapsOf(props.project.id)[0].name })}{overlapsOf(props.project.id).length > 1 ? ` +${overlapsOf(props.project.id).length - 1}` : ""}
                  </span>
                </Show>
                <Show when={pullRequests.prs[props.project.id]}>
                  {(pr) => <span class={`primary-tag pr-${prLabel(pr())}`} title={t("{title} ({state}, checks {checks})", { title: pr().title, state: prLabel(pr()), checks: pr().checks })}>{pr().provider === "gitlab" ? "!" : "#"}{pr().number}</span>}
                </Show>
              </span>
            </Show>
            <Show when={store.metaFor(props.project)?.comment && renaming() !== `c:${props.project.id}`}>
              <span class="project-comment" title={store.metaFor(props.project)!.comment!}>{store.metaFor(props.project)!.comment}</span>
            </Show>
          </span>
          <span class="project-badges">
            <Show when={counts().waiting}><span class="badge waiting" title={t("Needs you")}>{counts().waiting}</span></Show>
            <Show when={counts().error}><span class="badge error" title={t("Failed")}>{counts().error}</span></Show>
            <Show when={counts().working}><span class="badge working" title={t("Working")}>{counts().working}</span></Show>
            <Show when={counts().unread}><span class="badge unread" title={t("Finished, unread")}>{counts().unread}</span></Show>
          </span>
        </Show>
        <Show when={collapsedSidebar() && (counts().waiting || counts().error || counts().unread)}>
          <span class={`mini-dot ${counts().waiting ? "waiting" : counts().error ? "error" : "unread"}`} />
        </Show>
      </div>
      <Show when={!collapsedSidebar() && !folded() && sessions().length}>
        <div class="session-list">
          <For each={sessions()}>{(s) => <SessionRow s={s} />}</For>
        </div>
      </Show>
      <Show when={!collapsedSidebar() && !folded() && children().length}>
        <div class="worktree-list">
          <For each={children()}>{(c) => <ProjectRow project={c} />}</For>
        </div>
      </Show>
    </div>
  );
}

const navHint = commandHint;

const FILTERS = { all: msg("All projects"), running: msg("Projects with running sessions"), attention: msg("Projects that need you") } as const;

export function Sidebar() {
  const collapsed = () => store.state.settings.sidebarCollapsed;
  const searchChord = () => {
    const c = commands.find((x) => x.id === "palette");
    return c && chordOf(c) ? chordLabel(chordOf(c)!) : "";
  };
  const projects = createMemo<Project[]>((prev) => {
    const filter = store.state.settings.projectFilter ?? "all";
    let list = [...store.topProjects()];
    // A repository counts with its worktrees.
    const family = (p: Project) => [p.id, ...store.childrenOf(p.id).map((c) => c.id)];
    if (filter !== "all") {
      list = list.filter((p) =>
        family(p).includes(store.state.activeProject ?? "") ||
        family(p).some((id) => projectSessions(id).some((s) => (filter === "running" ? s.alive : ["waiting", "error", "done"].includes(sessionState(s))))),
      );
    }
    if (store.state.settings.projectSort === "recent") list.sort((a, b) => Math.max(...family(b).map(lastActivity)) - Math.max(...family(a).map(lastActivity)));
    return sameList(prev, list);
  });
  // Sessions whose project was removed from the sidebar keep running; list them here.
  const orphans = createMemo<SessionInfo[]>((prev) => {
    const known = new Set(store.state.projects.map((p) => p.id));
    const floating = new Set(store.state.floating?.sessions ?? []);
    const list = Object.values(store.sessions).filter((s) => (!s.project_id || !known.has(s.project_id)) && !floating.has(s.id)).sort((a, b) => a.created_at - b.created_at);
    return sameList(prev, list);
  });
  const cycleFilter = () => {
    const order = ["all", "running", "attention"] as const;
    const cur = store.state.settings.projectFilter ?? "all";
    store.setSettings({ projectFilter: order[(order.indexOf(cur) + 1) % order.length] });
  };

  return (
    <aside class={`sidebar${collapsed() ? " collapsed" : ""}`}>
      <div class="brand">
        <BrandMark />
        <Show when={!collapsed()}>
          <BrandWordmark />
          <span class="spacer" />
          <button class="icon-btn mini" title={t("Back") + navHint("nav.back")} disabled={!canGoBack()} onClick={navBack}><Icon name="back" size={14} /></button>
          <button class="icon-btn mini" title={t("Forward") + navHint("nav.forward")} disabled={!canGoForward()} onClick={navForward}><Icon name="forward" size={14} /></button>
        </Show>
      </div>
      <Show when={!collapsed()} fallback={<button class="icon-btn" style={{ margin: "0 auto 6px" }} title={t("Search")} onClick={() => store.setOverlay("palette")}><Icon name="search" /></button>}>
        <button class="search-btn" onClick={() => store.setOverlay("palette")}>
          <Icon name="search" size={14} />
          <span>{t("Search")}</span>
          <span class="kbd">{searchChord()}</span>
        </button>
        <div class="section-label">
          {t("Projects")}<span class="spacer" />
          <button class={`icon-btn mini${store.state.settings.projectFilter !== "all" ? " active" : ""}`} title={t("Filter: {filter}", { filter: t(FILTERS[store.state.settings.projectFilter ?? "all"]) })} onClick={cycleFilter}><Icon name="filter" size={13} /></button>
          <button class={`icon-btn mini${store.state.settings.projectSort === "recent" ? " active" : ""}`} title={store.state.settings.projectSort === "recent" ? t("Sorted by recent activity") : t("Manual order")}
            onClick={() => store.setSettings({ projectSort: store.state.settings.projectSort === "recent" ? "manual" : "recent" })}><Icon name="history" size={13} /></button>
          <button class="icon-btn mini" title={t("Add remote project (SSH)")} onClick={() => store.setOverlay("remote")}><Icon name="server" size={13} /></button>
          <button class="icon-btn mini" title={t("Add project folder")} onClick={() => void pickProjectFolder()}><Icon name="plus" size={14} /></button>
        </div>
      </Show>
      <div class="project-list">
        <For each={projects()}>{(p) => <ProjectRow project={p} />}</For>
        <Show when={!collapsed() && orphans().length}>
          <div class="section-label other-label">{t("Other sessions")}</div>
          <div class="session-list">
            <For each={orphans()}>{(s) => <SessionRow s={s} />}</For>
          </div>
        </Show>
        <Show when={collapsed()}>
          <button class="icon-btn" title={t("Add project")} onClick={() => void pickProjectFolder()} style={{ margin: "6px 6px" }}><Icon name="plus" /></button>
        </Show>
      </div>
      <div class="sidebar-footer">
        <button class="icon-btn" data-tip="dashboard" title={t("Agent dashboard")} onClick={() => store.setOverlay("dashboard")}><Icon name="dashboard" /></button>
        <button class="icon-btn" title={t("Workspace board") + navHint("board")} onClick={() => store.setOverlay("board")}><Icon name="board" /></button>
        <button class="icon-btn" data-tip="hosts" title={t("SSH hosts") + navHint("hosts")} onClick={() => openHosts()}><Icon name="server" /></button>
        <button class="icon-btn" title={t("Settings")} onClick={() => store.openSettings()}><Icon name="settings" /></button>
        <span style={{ flex: 1 }} />
        <Show when={kipShown()}><KipCompanion /></Show>
        <button class="icon-btn" title={t("Toggle sidebar")} onClick={() => store.setSettings({ sidebarCollapsed: !collapsed() })}><Icon name="sidebar" /></button>
      </div>
    </aside>
  );
}
