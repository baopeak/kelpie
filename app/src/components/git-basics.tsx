// The Changes panel's branch switcher, compact log and stash list. Loaded when one of them is
// first opened; each reads git once when shown (and on Refresh), nothing polls.
import { For, Show, createMemo, createResource, createSignal, onMount } from "solid-js";
import { store } from "../lib/store";
import { gitBranches, gitLog, gitStashAction, gitStashes, gitSwitchBranch, type GitStash, type SwitchResult } from "../lib/ipc";
import { filterBranches, parseStashSubject } from "../lib/git-basics";
import { gitState } from "../lib/git-state";
import { confirmDialog } from "../lib/confirm-dialog";
import { normPath } from "../lib/platform";
import { errorText, formatDateTime, formatRelative, t } from "../lib/i18n";
import { Icon } from "./icons";
import type { Project } from "../lib/types";
import "./git-basics.css";

type Refusal = Exclude<SwitchResult, { result: "switched" }> & { branch: string };

/** Local branches; a click switches this worktree to one, from a clean working tree only.
 *  `onOpenWorktree`: offered when the switch is refused for uncommitted changes (absent where no worktree can be made). */
export function BranchSwitcher(props: { project: Project; onClose: () => void; onSwitched: () => void; onOpenWorktree?: (branch: string) => void }) {
  const root = () => props.project.path;
  const [branches] = createResource(root, (r) => gitBranches(r));
  const [query, setQuery] = createSignal("");
  const [busy, setBusy] = createSignal<string | null>(null);
  const [refusal, setRefusal] = createSignal<Refusal | null>(null);
  const shown = createMemo(() => filterBranches(branches() ?? [], query()));
  // One Tab stop for the list (the row last focused, else the first); ↑ / ↓ move inside it.
  const [active, setActive] = createSignal<string | null>(null);
  const tabStop = () => (shown().some((b) => b.name === active()) ? active() : shown()[0]?.name);
  const projectAt = (path: string) => store.state.projects.find((p) => normPath(p.path) === normPath(path));

  let input!: HTMLInputElement;
  let list!: HTMLDivElement;
  onMount(() => input.focus());

  /** ↑ / ↓ between the rows (↑ from the first goes back to the filter). */
  const moveFocus = (e: KeyboardEvent) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    const rows = [...list.querySelectorAll<HTMLElement>(".branch-row")];
    const i = rows.indexOf(document.activeElement as HTMLElement);
    if (i < 0) return;
    e.preventDefault();
    (e.key === "ArrowDown" ? rows[Math.min(i + 1, rows.length - 1)] : i === 0 ? input : rows[i - 1]).focus();
  };

  const pick = async (name: string) => {
    if (busy()) return;
    setBusy(name);
    setRefusal(null);
    try {
      const res = await gitSwitchBranch(root(), name);
      if (res.result === "switched") {
        store.notify(t("Switched to {branch}", { branch: name }));
        props.onSwitched();
        props.onClose();
      } else setRefusal({ ...res, branch: name });
    } catch (e) {
      store.notify(errorText(e), undefined, 8000);
    } finally {
      setBusy(null);
      // A clicked row is not focusable: keep typing and Escape working.
      if (input.isConnected) input.focus();
    }
  };

  return (
    <div class="branch-switcher" onKeyDown={(e) => e.key === "Escape" && props.onClose()}>
      <input ref={input} class="text-input" placeholder={t("Filter branches")} value={query()} onInput={(e) => setQuery(e.currentTarget.value)}
        onKeyDown={(e) => {
          // Enter switches only to a branch the filter names (never just the most recent one); ↓ moves into the list.
          if (e.key === "Enter" && query().trim()) { const b = shown().find((x) => !x.current); if (b) void pick(b.name); }
          if (e.key === "ArrowDown") { e.preventDefault(); list.querySelector<HTMLElement>(".branch-row")?.focus(); }
        }} />
      <Show when={refusal()}>
        {(r) => (
          <div class="git-note warn refusal" data-refusal={r().result}>
            <Show when={r().result === "dirty"}>
              {t("{branch}: this worktree has uncommitted changes ({n} file(s)). Commit or stash them first, or open the branch in a new worktree.", { branch: r().branch, n: (r() as { files: number }).files })}
              <Show when={props.onOpenWorktree}>
                <div><button class="btn" data-act="open-worktree" onClick={() => { props.onClose(); props.onOpenWorktree!(r().branch); }}><Icon name="branch" size={13} /> {t("Open in a new worktree…")}</button></div>
              </Show>
            </Show>
            <Show when={r().result === "in_worktree"}>
              {t("{branch} is already checked out in another worktree: {path}", { branch: r().branch, path: (r() as { path: string }).path })}
              <Show when={projectAt((r() as { path: string }).path)}>
                {(p) => <div><button class="btn" onClick={() => { props.onClose(); store.switchProject(p().id); }}>{t("Go to that worktree")}</button></div>}
              </Show>
            </Show>
            <Show when={r().result === "operation"}>
              {t("A {op} is in progress here: finish or abort it before switching branches.", { op: (r() as { operation: string }).operation })}
            </Show>
            <Show when={r().result === "orphans"}>
              {t("HEAD is detached on {n} commit(s) that no branch has: switching to {branch} would leave them behind. Create a branch for them first (git switch -c <name>).", { n: (r() as { commits: number }).commits, branch: r().branch })}
            </Show>
          </div>
        )}
      </Show>
      <div class="branch-list" role="listbox" aria-label={t("Local branches")} ref={list} onKeyDown={moveFocus}>
        <Show when={!branches.error} fallback={<div class="muted">{errorText(branches.error)}</div>}>
          <For each={shown()} fallback={<div class="muted">{branches.loading ? t("Loading…") : t("No branches")}</div>}>
            {(b) => (
              <div class="list-row branch-row" classList={{ current: b.current }} data-branch={b.name} title={b.worktree ? t("Checked out in {path}", { path: b.worktree }) : b.name}
                role="option" aria-selected={b.current} tabIndex={b.name === tabStop() ? 0 : -1} onFocus={() => setActive(b.name)}
                onClick={() => !b.current && void pick(b.name)}
                onKeyDown={(e) => { if ((e.key === "Enter" || e.key === " ") && !b.current) { e.preventDefault(); void pick(b.name); } }}>
                <span class="branch-mark">{b.current ? <Icon name="check" size={12} /> : null}</span>
                <span class="name">{b.name}</span>
                <Show when={b.worktree}><span class="chip">{t("worktree")}</span></Show>
                <span class="dir">{b.time ? formatRelative(b.time * 1000) : ""}</span>
                <Show when={busy() === b.name}><span class="dir">{t("Switching…")}</span></Show>
              </div>
            )}
          </For>
        </Show>
      </div>
    </div>
  );
}

/** The last 100 commits of HEAD; read again when the branch changes (a switch here or in a terminal). */
export function CommitLog(props: { project: Project }) {
  const key = () => `${props.project.path}\n${gitState.status()?.branch ?? ""}`;
  const [log, { refetch }] = createResource(key, () => gitLog(props.project.path));
  return (
    <div class="git-log">
      <div class="git-section-tools">
        <span class="muted">{log() ? t("Last {n} commits", { n: log()!.length }) : ""}</span>
        <button class="icon-btn" title={t("Refresh")} onClick={() => void refetch()}><Icon name="refresh" size={12} /></button>
      </div>
      <Show when={!log.error} fallback={<div class="muted">{errorText(log.error)}</div>}>
        <For each={log()} fallback={<div class="muted">{log.loading ? t("Loading…") : t("No commits yet.")}</div>}>
          {(c) => (
            <div class="list-row commit-row" title={`${c.hash}\n${c.author} · ${formatDateTime(c.time * 1000)}\n\n${c.subject}`}>
              <span class="hash">{c.short}</span>
              <span class="subject">{c.subject}</span>
              <span class="meta">{c.author} · {formatRelative(c.time * 1000)}</span>
            </div>
          )}
        </For>
      </Show>
    </div>
  );
}

/** The repository's stashes (shared by all its worktrees): apply, or drop after a confirmation. */
export function StashList(props: { project: Project }) {
  const root = () => props.project.path;
  const [stashes, { refetch }] = createResource(root, (r) => gitStashes(r));
  const [busy, setBusy] = createSignal(false);

  const act = async (s: GitStash, action: "apply" | "drop") => {
    if (busy()) return;
    if (action === "drop" && !(await confirmDialog(t("Drop {name} ({message})? Its changes are deleted.", { name: s.name, message: parseStashSubject(s.subject).message })))) return;
    setBusy(true);
    try {
      await gitStashAction(root(), s.name, s.oid, action);
      store.notify(action === "apply" ? t("Applied {name}", { name: s.name }) : t("Dropped {name}", { name: s.name }));
    } catch (e) {
      store.notify(errorText(e), undefined, 10000);
    } finally {
      setBusy(false);
      void refetch();
      if (action === "apply") {
        void gitState.refresh();
        store.refreshProjectInfo(props.project.id, 0);
      }
    }
  };

  return (
    <div class="git-stashes">
      <div class="git-note stash-shared">{t("Stashes are shared by every worktree of this repository: a stash made in one shows up (and can be dropped) in all of them.")}</div>
      <div class="git-section-tools">
        <span class="spacer" />
        <button class="icon-btn" title={t("Refresh")} onClick={() => void refetch()}><Icon name="refresh" size={12} /></button>
      </div>
      <Show when={!stashes.error} fallback={<div class="muted">{errorText(stashes.error)}</div>}>
        <For each={stashes()} fallback={<div class="muted">{stashes.loading ? t("Loading…") : t("No stashes.")}</div>}>
          {(s) => {
            const p = parseStashSubject(s.subject);
            return (
              <div class="list-row stash-row" data-stash={s.name} title={`${s.name} · ${formatDateTime(s.time * 1000)}\n${s.subject}`}>
                <span class="subject">{p.message || s.name}</span>
                <Show when={p.branch}><span class="chip">{p.branch}</span></Show>
                <span class="meta">{formatRelative(s.time * 1000)}</span>
                <span class="stash-actions">
                  <button class="link-btn" data-act="apply" disabled={busy()} title={t("Apply these changes to this worktree (the stash is kept)")} onClick={() => void act(s, "apply")}>{t("Apply")}</button>
                  <button class="link-btn danger" data-act="drop" disabled={busy()} onClick={() => void act(s, "drop")}>{t("Drop")}</button>
                </span>
              </div>
            );
          }}
        </For>
      </Show>
    </div>
  );
}
