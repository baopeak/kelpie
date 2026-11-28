// Project sidebar: every project with its sessions (status, task title, model, last activity),
// branch subtitle, filter/sort, and context menus. Each row binds only to its own session's
// fields, so status ticks from 20 agents re-render single rows, not the list.
import { For, Show, createEffect, createMemo, createResource, createSignal, on } from "solid-js";
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
import { findPane } from "../lib/layout";
import { orderByScreen, screenPlaces, shareTakeoverPlaces } from "../lib/session-order";
import { sessionAccount } from "../lib/accounts";
import { isAgentKind } from "../lib/agents";
import { hostLabel } from "../lib/remote";
import { canGoBack, canGoForward, navBack, navForward } from "../lib/nav-history";
import { addExistingWorktrees, checkRemoval, removeWorktree } from "../lib/worktrees";
import { WORKTREE_FOLD_LIMIT, foldWorktrees, isNotableWorktree } from "../lib/worktree-fold";
import { removeFromSidebar, worktreeHealth } from "../lib/worktree-sync";
import { openWorktreeDialog } from "./worktree-dialog";
import { openFinish, overlapsOf } from "../lib/review-queue";
import { openContextMenu, type MenuItem } from "./context-menu";
import type { Project, SessionInfo } from "../lib/types";
import { t, msg } from "../lib/i18n";
import { continueAt, continueTime, continueTimeShort } from "../lib/continue-timers";
import { openHosts } from "./hosts-page";
import { confirmDialog } from "../lib/confirm-dialog";
import { ancestorsOf, childrenOfSession, foldedParents, subtreeSummary, toggleParentFold } from "../lib/sub-sessions";
import { pulseClass } from "../lib/reveal-pulse";
import { reportedState, sessionState, type SessionState } from "../lib/session-state";
import { closeFinishedSubSessions, finishedOf } from "../lib/sub-session-actions";
import { PRIORITY_API, boostLeft, priorities, projectTier } from "../lib/priorities";
import { boostSession, endBoost, openPriorities } from "../lib/priority-actions";

export const STATUS_WORDS = { idle: msg("idle"), working: msg("working"), waiting: msg("waiting"), exited: msg("exited") } as const;

export { sessionState, reportedState, type SessionState } from "../lib/session-state";

/** States that ask for the user: they get a glyph, a brighter row and (for the first two) a word. */
const GLYPH: Partial<Record<SessionState, string>> = { waiting: "alert", error: "close", done: "check" };
const LABEL: Partial<Record<SessionState, string>> = { waiting: msg("needs you"), error: msg("error") };

const continueTitle = (at: number, now?: number) => t("Stopped by the usage limit: continues by itself at {time}", { time: continueTime(at, now) });

function stateTitle(s: SessionInfo) {
  const at = continueAt(s);
  if (at) return continueTitle(at);
  switch (sessionState(s)) {
    case "waiting": return t("Needs you");
    case "error": return s.alive ? t("Last turn failed") : t("Exited with code {code}", { code: s.exit_code ?? "" });
    case "done": return t("Finished, unread");
    default: return t(STATUS_WORDS[s.alive ? s.status : "exited"]);
  }
}

/** The dot's tooltip: the state, and when it comes from the session's own report, say so. */
function reportedTitle(s: SessionInfo) {
  const own = sessionState(s);
  const shown = reportedState(s);
  if (shown === own) return stateTitle(s);
  return shown === "waiting" ? t("Reported: needs you") : t("Reported: failed");
}

export function StatusDot(props: { s: SessionInfo }) {
  const state = () => reportedState(props.s);
  return (
    <span class={`dot ${state()}`} title={reportedTitle(props.s)}>
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

/** A project's sessions in a stable order: oldest first, a session switched between terminal and chat (a new session that took
 *  over the conversation) in the place of the one it replaced (`store.sessionSince`). The rows follow the screen on top of it
 *  (`projectRows`); this order breaks their ties and orders the sessions on screen nowhere. */
function projectSessions(projectId: string): SessionInfo[] {
  return Object.values(store.sessions)
    .filter((s) => s.project_id === projectId)
    .sort((a, b) => store.sessionSince(a) - store.sessionSince(b) || a.handle - b.handle);
}

/** A session that renders under its parent's row instead of in its project's list. A parent shown nowhere in the sidebar (the
 *  floating terminal's sessions) does not take its children with it: they stay where they would be. */
const isNested = (s: SessionInfo) => {
  const parent = store.sessionTree().nested.get(s.id);
  if (!parent || store.state.floating?.sessions.includes(parent)) return false;
  // Under a parent of another project it would sit in a list that project's filters and folds can hide: it stays in its own.
  return (store.sessions[parent]?.project_id ?? null) === (s.project_id ?? null);
};

/** The row a session is listed under: its own, or for a sub-session the top of its family. */
const rowOf = (id: string) => {
  let s: SessionInfo | undefined = store.sessions[id];
  for (let guard = 0; s && isNested(s) && guard < 10_000; guard++) s = store.sessions[store.sessionTree().nested.get(s.id)!];
  return s?.id;
};

/** The sessions listed directly under a project: its own, less the sub-sessions shown under their parents, in the order they are
 *  on screen: tabs left to right, each tab's panes in layout order (a side-by-side split left before right, a stacked one top
 *  before bottom, each half whole before the next; pane sizes play no part). A family (a parent and its sub-sessions)
 *  is one block at the place of its first member on screen, so a child shown in its parent's pane moves nothing. Sessions on
 *  screen nowhere follow, oldest first. A conversation switched between terminal and chat keeps its row's place throughout (the
 *  new session and the one it replaces share a place while the pane passes from one to the other). Only the layout moves a row:
 *  never activity, status, focus or a click on a row on screen. (Counts, filters and the "recent" order look at all of them: a
 *  sub-session that needs you still lights its project.) */
const projectRows = (projectId: string) =>
  orderByScreen(
    projectSessions(projectId).filter((s) => !isNested(s)),
    shareTakeoverPlaces(screenPlaces(store.workspace(projectId).tabs), store.takeovers()),
    rowOf,
  );

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

/** What a boosted session's mark says: how long the boost has left. */
function boostTitle(id: string, now: number): string {
  const until = priorities()?.effective[id]?.boost_until;
  return t("Boosted: gets the most CPU and memory ({left})", { left: until ? boostLeft(until, now) : t("until this turn ends") });
}

function sessionMenu(s: SessionInfo): MenuItem[] {
  const items: MenuItem[] = [
    { label: t("Rename…"), run: () => setRenaming(`s:${s.id}`) },
    s.unread ? { label: t("Mark as read"), run: () => void store.markUnread(s.id, false) } : { label: t("Mark as unread"), run: () => void store.markUnread(s.id, true) },
  ];
  // Its priority: a boost, or the Priorities page on its line (a daemon before API 52 has none).
  // A session on another computer has nothing here to share out: no priority for it.
  if (s.alive && !s.remote && store.daemonApi() >= PRIORITY_API) {
    const eff = priorities()?.effective[s.id];
    items.push(null);
    if (eff?.boosted) {
      items.push({ label: eff.boost_until ? t("End boost ({left} left)", { left: boostLeft(eff.boost_until, Date.now()) }) : t("End boost"), run: () => void endBoost(s) });
    } else {
      items.push({ label: t("Boost for 1 hour"), run: () => void boostSession(s, "1h") }, { label: t("Boost until this turn ends"), run: () => void boostSession(s, "turn") });
    }
    items.push({ label: t("Priority & reservation…"), run: () => openPriorities(s.id) });
  }
  if (!s.alive && !s.chat) items.push({ label: store.resumable(s) ? t("Resume") : t("Restart"), run: () => void daemon("session.restart", { handle: s.handle }).catch((e) => store.notify(String(e))) });
  // Chat view runs the agent on this computer: never for a session on a remote host.
  if (s.alive && supportsChat(s.kind) && !s.remote) {
    items.push(s.chat ? { label: t("Continue in terminal (TUI)"), run: () => void inPane(s, switchToTerminal) } : { label: t("Continue in chat view"), run: () => void inPane(s, switchToChat) });
  }
  const finished = finishedOf(s);
  if (finished.length) items.push(null, { label: t("Close finished sub-sessions ({n})", { n: finished.length }), run: () => void closeFinishedSubSessions(s) });
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
  const health = worktree ? worktreeHealth(p.id) : undefined;
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
    ...(store.daemonApi() >= PRIORITY_API ? [{ label: t("Priority & reservation…"), run: () => openPriorities(p.id) }] : []),
    null,
    ...(worktree && !p.remote && !health ? [{ label: t("Finish…"), run: () => openFinish(p.id) }] : []),
    ...(worktree && !health ? [{ label: t("Remove worktree…"), danger: true, run: () => void confirmRemoveWorktree(p) }] : []),
    // Git no longer has it as a worktree: only the row can go (nothing on disk is touched).
    ...(worktree && health
      ? [{
          label: t("Remove from sidebar"),
          run: async () => {
            const text = health === "missing"
              ? t("Remove {name} from the sidebar? Its folder no longer exists. Its sessions are not stopped.", { name: p.name })
              : t("Remove {name} from the sidebar? Its folder and files are not touched.", { name: p.name });
            if (await confirmDialog(text)) await removeFromSidebar(p);
          },
        }]
      : []),
    // A marked worktree has Remove from sidebar above instead.
    ...(health ? [] : [{
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
    }]),
  ];
}

/** What the worst state below a parent is called in its badge (the quiet ones need no words). */
const SUB_STATE_WORDS: Partial<Record<SessionState, string>> = { waiting: msg("one needs you"), error: msg("one failed"), working: msg("one is working"), done: msg("one finished, unread") };

/** The sessions opened by `id`'s agent, oldest first (same array while the set is the same, so a status tick rebuilds nothing). */
const subChildren = (id: () => string) => createMemo<SessionInfo[]>((prev) => sameList(prev, childrenOfSession(store.sessionTree(), id()).filter(isNested)));

/** A parent's compact summary: how many sessions are below it and the worst state among them. */
function SubBadge(props: { id: string }) {
  const sum = createMemo(() => subtreeSummary(store.sessionTree(), props.id, reportedState));
  // Colour is never the only cue: the worst state is also said in words (for a screen reader and the tooltip) and, when it asks for
  // the user, drawn as its glyph.
  const label = () => {
    const n = sum().count;
    const what = n === 1 ? t("1 sub-session") : t("{n} sub-sessions", { n });
    const state = SUB_STATE_WORDS[sum().worst];
    return state ? `${what}: ${t(state)}` : what;
  };
  return (
    <Show when={sum().count}>
      <span class={`sub-badge ${sum().worst}`} title={label()} aria-label={label()} role="img">
        <Show when={GLYPH[sum().worst]}>{(g) => <Icon name={g()} size={10} />}</Show>
        {sum().count}
      </span>
    </Show>
  );
}

/** A session's row and, under it on tree lines, the sessions it opened (to any depth, oldest first). A session with sub-sessions
 *  and its whole tree sit in one card (`session-family`), so the top-level row after it reads as separate. `nested`: a child's
 *  node, laid out as tree line | row, then tree line | its own children. */
function SessionNode(props: { s: SessionInfo; nested?: boolean }) {
  const kids = subChildren(() => props.s.id);
  const list = () => (
    <Show when={kids().length && !foldedParents[props.s.id]}>
      <div class="sub-list" role="group" aria-label={t("Sub-sessions of {name}", { name: sessionLabel(props.s) })}>
        <For each={kids()}>{(c) => <SessionNode s={c} nested />}</For>
      </div>
    </Show>
  );
  return (
    <Show when={props.nested} fallback={
      <div class={kids().length ? "session-family" : "session-solo"}>
        <SessionRow s={props.s} kids={kids().length} />
        {list()}
      </div>
    }>
      <div class="sub-node">
        <span class="sub-elbow" aria-hidden="true" />
        <SessionRow s={props.s} kids={kids().length} />
        <span class="sub-rail" aria-hidden="true" />
        {list()}
      </div>
    </Show>
  );
}

function SessionRow(props: { s: SessionInfo; kids: number }) {
  const state = () => reportedState(props.s);
  const hasKids = () => props.kids > 0;
  const modelLine = () => [props.s.model && shortModel(props.s.model), sessionAccount(props.s)?.label].filter(Boolean).join(" · ");
  const selected = () => store.focusedSession() === props.s.id;
  /** Folded, with the session on screen somewhere below it: the row stands in for the hidden one. */
  const holdsSelected = () => {
    const sel = store.focusedSession();
    return hasKids() && !!foldedParents[props.s.id] && !!sel && ancestorsOf(store.sessionTree(), sel).includes(props.s.id);
  };
  let el!: HTMLDivElement;
  // The row of the session on screen stays in view: on a click elsewhere (a chip, the palette, a notice) the list follows.
  createEffect(on(() => selected() || holdsSelected(), (mark) => mark && queueMicrotask(() => el?.scrollIntoView({ block: "nearest" }))));
  return (
    <div
      ref={el}
      class={`session-row ${state()}${hasKids() ? " has-kids" : ""}${store.isSessionVisible(props.s.id) ? " visible" : ""}${selected() ? " selected" : ""}${holdsSelected() ? " holds-selected" : ""}${pulseClass(props.s.id)}`}
      data-row-session={props.s.id}
      aria-current={selected() ? "true" : undefined}
      onClick={() => store.goToSession(props.s.id)}
      onContextMenu={(e) => openContextMenu(e, sessionMenu(props.s))}
      title={[props.s.title, props.s.model, props.s.cwd, relTimeShort(props.s.last_activity || props.s.created_at, now())].filter(Boolean).join("\n")}
    >
      <StatusDot s={props.s} />
      <KindGlyph kind={props.s.kind} size={14} />
      <span class="session-text">
        <Show when={renaming() === `s:${props.s.id}`} fallback={<span class="name">{sessionLabel(props.s)}</span>}>
          <RenameInput value={props.s.name ?? sessionLabel(props.s)} onDone={(v) => v !== null && void store.renameSession(props.s.id, v.trim() || null)} />
        </Show>
        {/* An agent keeps the line while its model is not known yet, so its row is as tall as its neighbours' and does not grow later. */}
        <Show when={isAgentKind(props.s.kind) || modelLine()}>
          <span class="model">{modelLine() || " "}</span>
        </Show>
      </span>
      {/* The fold arrow sits with the count, at the right: a parent's dot, icon and name line up with the rows beside it. */}
      <Show when={hasKids()}>
        <SubBadge id={props.s.id} />
        <button class="chev sub-chev" title={foldedParents[props.s.id] ? t("Show sub-sessions") : t("Hide sub-sessions")} aria-expanded={!foldedParents[props.s.id]}
          onClick={(e) => { e.stopPropagation(); toggleParentFold(props.s.id); }}>
          <Icon name={foldedParents[props.s.id] ? "chevronRight" : "chevronDown"} size={10} />
        </button>
      </Show>
      <ContextRing s={props.s} />
      <Show when={priorities()?.effective[props.s.id]?.boosted}>
        <span class="prio-mark" title={boostTitle(props.s.id, now())}><Icon name="bolt" size={10} /></span>
      </Show>
      <Show when={continueAt(props.s)} fallback={
        <Show when={LABEL[state()]} fallback={<span class="age">{relTimeShort(props.s.last_activity || props.s.created_at, now())}</span>}>
          {(label) => <span class={`age state-label ${state()}`}>{t(label())}</span>}
        </Show>
      }>
        {/* A clock and the short time, so that the name keeps its room; the full sentence is in the tooltip. */}
        {(at) => (
          <span class="age state-label limit-wait" data-continue-at={at()} role="img" title={continueTitle(at(), now())} aria-label={continueTitle(at(), now())}>
            <Icon name="clock" size={10} />{continueTimeShort(at(), now())}
          </span>
        )}
      </Show>
    </div>
  );
}

/** A session that asks for the user even after it ended: it keeps its worktree in view and lights the "needs you" filter. */
const asksForUser = (s: SessionInfo) => ["waiting", "error", "done"].includes(reportedState(s));

function ProjectRow(props: { project: Project }) {
  const index = createMemo(() => store.topProjects().indexOf(props.project));
  const worktree = () => !!props.project.parentId;
  const children = createMemo<Project[]>((prev) => sameList(prev, store.childrenOf(props.project.id)));
  // Many worktrees: the quiet ones fold under one "N more worktrees" row (see worktree-fold.ts).
  const wtFold = createMemo(() => {
    const kids = children();
    // Checked here as well as in foldWorktrees so that a small repository's rows do not track every session.
    if (kids.length <= WORKTREE_FOLD_LIMIT) return { shown: kids, hidden: [] as Project[] };
    const watched = new Set<string>();
    for (const s of Object.values(store.sessions)) if (s.project_id && (s.alive || asksForUser(s))) watched.add(s.project_id);
    // A marked worktree (folder missing, not a worktree) stays in view: the mark is there to be seen.
    return foldWorktrees(kids, (c) => !!worktreeHealth(c.id) || isNotableWorktree({ active: store.state.activeProject === c.id, sessions: watched.has(c.id), info: store.projectInfo[c.id] }));
  });
  const wtShown = createMemo<Project[]>((prev) => sameList(prev, wtFold().shown));
  const wtHidden = createMemo<Project[]>((prev) => sameList(prev, wtFold().hidden));
  const wtExpanded = () => !!store.state.worktreesExpanded?.[props.project.id];
  const active = () => store.state.activeProject === props.project.id;
  const sessions = createMemo(() => projectSessions(props.project.id));
  /** The rows listed under the project: its sessions that are not shown under a parent. */
  const rows = createMemo<SessionInfo[]>((prev) => sameList(prev, projectRows(props.project.id)));
  const counts = createMemo(() => {
    let working = 0, waiting = 0, unread = 0, error = 0;
    for (const s of sessions()) {
      const state = reportedState(s);
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
  const health = () => (worktree() ? worktreeHealth(props.project.id) : undefined);
  const healthText = () =>
    health() === "missing"
      ? t("This worktree's folder no longer exists. It stays while it has sessions; remove it from the sidebar when you are done with them.")
      : t("Git no longer lists this folder as a worktree of its repository. Nothing was deleted.");
  /** A worktree row is one line, so its branch (when it differs from the name) and host move to the tooltip. */
  const rowTitle = () => {
    const lines = [props.project.path];
    const i = info();
    if (worktree() && i?.is_repo && i.branch && i.branch !== props.project.name) lines.push(t("Branch: {branch}", { branch: i.branch }));
    if (worktree() && props.project.remote) lines.push(props.project.remote.tmux ? t("On {host} over SSH · sessions kept in tmux", { host: hostLabel(props.project) }) : t("On {host} over SSH", { host: hostLabel(props.project) }));
    if (health()) lines.push(healthText());
    return lines.join("\n") + (index() >= 0 && index() < 9 ? `  (${switchChord()})` : "");
  };
  const overlaps = () => overlapsOf(props.project.id);
  const wtCountLabel = () => (children().length === 1 ? t("1 worktree") : t("{n} worktrees", { n: children().length }));

  return (
    <div class={`project-block${active() ? " active" : ""}`}>
      <div
        class={`project${worktree() ? " wt-row" : ""}${health() ? ` wt-${health()}` : ""}${active() ? " active" : ""}`}
        style={{ "--kc": props.project.color }}
        onClick={() => store.switchProject(props.project.id)}
        onContextMenu={(e) => openContextMenu(e, projectMenu(props.project))}
        title={rowTitle()}
        data-project={props.project.id}
      >
        <Show when={!collapsedSidebar() && (sessions().length || children().length)}>
          <button class="chev" title={folded() ? t("Show sessions") : t("Hide sessions")} onClick={(e) => { e.stopPropagation(); store.setCollapsed(props.project.id, !folded()); }}>
            <Icon name={folded() ? "chevronRight" : "chevronDown"} size={11} />
          </button>
        </Show>
        <Show when={worktree()} fallback={<span class="project-swatch" style={{ "--kc": props.project.color }}>{props.project.name.slice(0, 1).toUpperCase()}</span>}>
          <span class="wt-glyph" style={{ "--kc": props.project.color }}><Icon name="branch" size={12} /></span>
        </Show>
        <Show when={!collapsedSidebar()}>
          <span class="project-text">
            <Show when={renaming() === `p:${props.project.id}`} fallback={
              <span class="project-name">{props.project.name}
                {/* A tier other than normal shows beside the name. */}
                <Show when={priorities() && projectTier(priorities(), props.project.id) !== "normal"}>
                  <span class={`tier-chip ${projectTier(priorities(), props.project.id)}`}>
                    {projectTier(priorities(), props.project.id) === "critical" ? t("critical") : t("background")}
                  </span>
                </Show>
              </span>
            }>
              <RenameInput value={props.project.name} onDone={(v) => v?.trim() && store.renameProject(props.project.id, v.trim())} />
            </Show>
            <Show when={renaming() === `c:${props.project.id}`}>
              <RenameInput value={store.metaFor(props.project)?.comment ?? ""} onDone={(v) => v !== null && void store.setComment(props.project, v)} />
            </Show>
            <Show when={props.project.remote && !worktree()}>
              <span class="project-sub remote-host" title={props.project.remote!.tmux ? t("On {host} over SSH · sessions kept in tmux", { host: hostLabel(props.project) }) : t("On {host} over SSH", { host: hostLabel(props.project) })}>
                <Icon name="server" size={10} /> {hostLabel(props.project)}
              </span>
            </Show>
            <Show when={!worktree() && info()?.is_repo}>
              <span class="project-sub">
                <Icon name="branch" size={10} /> <span class="branch-text">{info()!.branch ?? t("detached")}</span>
                {info()!.dirty ? "*" : ""}
                <Show when={info()!.ahead || info()!.behind}> ↑{info()!.ahead}↓{info()!.behind}</Show>
                <Show when={children().length}><span class="primary-tag">{t("primary")}</span></Show>
                <Show when={overlaps().length}><OverlapTag overlaps={overlaps()} /></Show>
                <Show when={pullRequests.prs[props.project.id]}>{(pr) => <PrTag pr={pr()} />}</Show>
              </span>
            </Show>
            <Show when={store.metaFor(props.project)?.comment && renaming() !== `c:${props.project.id}`}>
              <span class="project-comment" title={store.metaFor(props.project)!.comment!}>{store.metaFor(props.project)!.comment}</span>
            </Show>
          </span>
          <Show when={health()}>
            <span class="wt-status wt-health-box">
              <span class="primary-tag wt-health" title={healthText()}>{health() === "missing" ? t("folder missing") : t("not a worktree")}</span>
            </span>
          </Show>
          {/* A worktree's git state, only what means something: a detached HEAD, uncommitted changes, commits ahead/behind. */}
          <Show when={worktree() && info()?.is_repo}>
            <span class="wt-status">
              <Show when={overlaps().length}><OverlapTag overlaps={overlaps()} /></Show>
              <Show when={pullRequests.prs[props.project.id]}>{(pr) => <PrTag pr={pr()} />}</Show>
              <Show when={!info()!.branch}><span>{t("detached")}</span></Show>
              <Show when={info()!.dirty}><span class="wt-dirty" title={t("Uncommitted changes")}>*</span></Show>
              <Show when={info()!.ahead}><span title={info()!.ahead === 1 ? t("1 commit not pushed") : t("{n} commits not pushed", { n: info()!.ahead })}>↑{info()!.ahead}</span></Show>
              <Show when={info()!.behind}><span title={info()!.behind === 1 ? t("1 commit behind upstream") : t("{n} commits behind upstream", { n: info()!.behind })}>↓{info()!.behind}</span></Show>
            </span>
          </Show>
          <span class="project-badges">
            <Show when={counts().waiting}><span class="badge waiting" title={t("Needs you")}>{counts().waiting}</span></Show>
            <Show when={counts().error}><span class="badge error" title={t("Failed")}>{counts().error}</span></Show>
            <Show when={counts().working}><span class="badge working" title={t("Working")}>{counts().working}</span></Show>
            <Show when={counts().unread}><span class="badge unread" title={t("Finished, unread")}>{counts().unread}</span></Show>
            <Show when={folded() && children().length}>
              <span class="wt-count" role="img" title={wtCountLabel()} aria-label={wtCountLabel()}>
                <Icon name="branch" size={10} />{children().length}
              </span>
            </Show>
          </span>
        </Show>
        <Show when={collapsedSidebar() && (counts().waiting || counts().error || counts().unread)}>
          <span class={`mini-dot ${counts().waiting ? "waiting" : counts().error ? "error" : "unread"}`} />
        </Show>
      </div>
      <Show when={!collapsedSidebar() && !folded() && rows().length}>
        <div class="session-list">
          <For each={rows()}>{(s) => <SessionNode s={s} />}</For>
        </div>
      </Show>
      <Show when={!collapsedSidebar() && !folded() && children().length}>
        <div class="worktree-list">
          {/* Expanded, the list keeps the repository's own order, so rows do not jump when one becomes notable. */}
          <For each={wtExpanded() && wtHidden().length ? children() : wtShown()}>{(c) => <ProjectRow project={c} />}</For>
          <Show when={wtHidden().length}>
            <button class="wt-more" aria-expanded={wtExpanded()} onClick={() => store.setWorktreesExpanded(props.project.id, !wtExpanded())}>
              <Icon name={wtExpanded() ? "chevronDown" : "chevronRight"} size={10} />
              <span>{wtExpanded() ? t("Show fewer worktrees") : t("{n} more worktrees", { n: wtHidden().length })}</span>
            </button>
          </Show>
        </div>
      </Show>
    </div>
  );
}

/** "Also changed in …": another worktree of the repository changed the same files. */
function OverlapTag(props: { overlaps: ReturnType<typeof overlapsOf> }) {
  return (
    <span class="primary-tag overlap" title={props.overlaps.map((o) => `${o.name}: ${o.files.slice(0, 5).join(", ")}${o.files.length > 5 ? "…" : ""}`).join("\n")}>
      {t("Also changed in {name}", { name: props.overlaps[0].name })}{props.overlaps.length > 1 ? ` +${props.overlaps.length - 1}` : ""}
    </span>
  );
}

/** The project's open pull/merge request with its state. */
function PrTag(props: { pr: NonNullable<(typeof pullRequests.prs)[string]> }) {
  return (
    <span class={`primary-tag pr-${prLabel(props.pr)}`} title={t("{title} ({state}, checks {checks})", { title: props.pr.title, state: prLabel(props.pr), checks: props.pr.checks })}>
      {props.pr.provider === "gitlab" ? "!" : "#"}{props.pr.number}
    </span>
  );
}

const navHint = commandHint;

const FILTERS = { all: msg("All projects"), running: msg("Projects with running sessions"), attention: msg("Projects that need you") } as const;

export function Sidebar(props: { inert?: boolean }) {
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
        family(p).some((id) => projectSessions(id).some((s) => (filter === "running" ? s.alive : asksForUser(s)))),
      );
    }
    if (store.state.settings.projectSort === "recent") list.sort((a, b) => Math.max(...family(b).map(lastActivity)) - Math.max(...family(a).map(lastActivity)));
    return sameList(prev, list);
  });
  // Sessions whose project was removed from the sidebar keep running; list them here.
  const orphans = createMemo<SessionInfo[]>((prev) => {
    const known = new Set(store.state.projects.map((p) => p.id));
    const floating = new Set(store.state.floating?.sessions ?? []);
    const list = Object.values(store.sessions).filter((s) => (!s.project_id || !known.has(s.project_id)) && !floating.has(s.id) && !isNested(s)).sort((a, b) => store.sessionSince(a) - store.sessionSince(b) || a.handle - b.handle);
    return sameList(prev, list);
  });
  const cycleFilter = () => {
    const order = ["all", "running", "attention"] as const;
    const cur = store.state.settings.projectFilter ?? "all";
    store.setSettings({ projectFilter: order[(order.indexOf(cur) + 1) % order.length] });
  };

  return (
    <aside class={`sidebar${collapsed() ? " collapsed" : ""}`} inert={props.inert}>
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
            <For each={orphans()}>{(s) => <SessionNode s={s} />}</For>
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
