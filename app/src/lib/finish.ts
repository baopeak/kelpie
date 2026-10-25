// Finish: from "the agent is done" to merged and cleaned up, in the order that never loses work
// (stop sessions, list and confirm the uncommitted files, WIP commit of exactly those, merge-tree,
// approved checks, compare-and-swap, branch only with proof, worktree last). The git steps are in
// app/src-tauri/src/git_finish.rs, which also re-checks everything that matters at its own boundary;
// this decides what may run and sequences it. Loaded with the Finish dialog and the review cards.
import * as ipc from "./ipc";
import { store } from "./store";
import { automationEvents } from "./automation-events";
import { t } from "./i18n";
import { kindInfo, isAgentKind } from "./agents";
import { configAtRef, checksApproved, removeWorktree, stopSessions, type KelpieConfig } from "./worktrees";
import { normPath } from "./platform";
import type { Project } from "./types";

export interface ChecksPlan {
  /** The `check:` list of the base branch's committed `.kelpie.yaml`. */
  commands: string[];
  config: KelpieConfig | null;
  /** This exact list was approved for the repository. */
  approved: boolean;
  /** The list that was approved before (to show what changed), when known. */
  previous: string[] | null;
  /** The branch edits `.kelpie.yaml`: its checks do not run unless the user says so. */
  blocked: boolean;
  /** The base's `.kelpie.yaml` could not be read or parsed. */
  error?: string;
}

/** Pure: what may run, from the base's file, the approval state and whether the branch edits the file. */
export function planChecks(i: { config: KelpieConfig | null; approved: boolean; previous: string[] | null; candidateEditsConfig: boolean; error?: string }): ChecksPlan {
  const commands = i.config?.check ?? [];
  return { commands, config: i.config, approved: i.approved, previous: i.previous, blocked: commands.length > 0 && i.candidateEditsConfig, error: i.error };
}

/** Whether `plan`'s commands may run now, given the user's override of a blocked plan. */
export const runnable = (plan: ChecksPlan, override: boolean) => plan.commands.length > 0 && plan.approved && (!plan.blocked || override);

/** Checks that cannot run (unreadable file, unapproved list, blocked) need the user to say "finish without them". */
export const needsDecision = (plan: ChecksPlan, override: boolean) => !!plan.error || (plan.commands.length > 0 && !runnable(plan, override));

export interface CheckDiffLine {
  text: string;
  kind: "same" | "added" | "removed";
}

/** The approved list against the new one, removed lines first. */
export function diffChecks(previous: string[], next: string[]): CheckDiffLine[] {
  const keep = new Set(next);
  const had = new Set(previous);
  return [
    ...previous.filter((c) => !keep.has(c)).map((text) => ({ text, kind: "removed" as const })),
    ...next.map((text) => ({ text, kind: had.has(text) ? ("same" as const) : ("added" as const) })),
  ];
}

/** Reads the checks from the base branch's committed file (never the branch's or the working copy). */
export async function loadChecksPlan(repo: Project, base: string, candidateEditsConfig: boolean): Promise<ChecksPlan> {
  try {
    const config = await configAtRef(repo, `refs/heads/${base}`);
    const previous = store.state.approvedChecks?.[normPath(repo.path)] ?? null;
    return planChecks({ config, approved: !!config && checksApproved(repo, config), previous, candidateEditsConfig });
  } catch (e) {
    return planChecks({ config: null, approved: false, previous: null, candidateEditsConfig, error: String(e).replace(/^Error: /, "") });
  }
}

/** The agent that worked in the folder most recently: names the WIP commit. */
export function agentLabel(p: Project): string {
  const s = Object.values(store.sessions)
    .filter((x) => x.project_id === p.id && isAgentKind(x.kind))
    .sort((a, b) => b.last_activity - a.last_activity)[0];
  return s ? kindInfo(s.kind).label : t("agent");
}

/** A first draft of the squash/merge message: the card's comment, else the branch name as words. */
export function defaultMessage(p: Project, branch: string): string {
  const comment = store.metaFor(p)?.comment?.split("\n")[0].trim();
  if (comment) return comment;
  const words = branch.replace(/^[a-z]+\//, "").replace(/[-_]+/g, " ").trim();
  return words ? words[0].toUpperCase() + words.slice(1) : branch;
}

export type Problem =
  | { why: "sessions" }
  | { why: "not_on_branch"; branch: string | null }
  | { why: "embedded"; folders: string[] }
  | { why: "files_changed"; files: string[] }
  | { why: "conflict"; files: string[] }
  | { why: "checks"; checks: ipc.CheckResult[]; cleanup: string | null }
  | { why: "config_blocked"; files: string[] }
  | { why: "base_moved" }
  | { why: "checkout_dirty"; path: string; files: string[] }
  | { why: "cancelled" }
  | { why: "error"; message: string };

export type Outcome =
  | { kind: "finished"; commit: string | null; notes: string[] }
  | { kind: "merged"; commit: string | null; notes: string[]; problem: string }
  | { kind: "stopped"; problem: Problem; notes: string[] };

export interface FinishInput {
  worktree: Project;
  repo: Project;
  /** The branch the dialog showed; the backend refuses unless the worktree is really on it. */
  branch: string;
  base: string;
  strategy: ipc.FinishStrategy;
  message: string;
  /** The files the user confirmed as the WIP commit (the dialog listed them). */
  confirmed: string[];
  /** The commands to run: already approved, from the base's file. */
  checks: string[];
  copy: string[];
  link: string[];
  /** Run the base's checks although the branch edits `.kelpie.yaml`. */
  allowConfigChange: boolean;
  /** Set by the dialog's Cancel: honoured at every step boundary (the backend ends a merge or check that is running). */
  cancel?: { requested: boolean };
}

const errorText = (e: unknown) => String(e).replace(/^Error: /, "");

/**
 * Runs Finish. Stops at the first thing that needs the user and says what is safe: every stop
 * before the base moves leaves the branch (with a WIP commit of the confirmed files) as it was.
 * `onStep` reports progress for the dialog.
 */
export async function runFinish(input: FinishInput, onStep: (text: string, pastMerge?: boolean) => void = () => {}): Promise<Outcome> {
  const { worktree: wt, repo, branch, base } = input;
  const notes: string[] = [];
  // Named before the sessions stop (they leave the store once closed).
  const who = agentLabel(wt);
  const cancelled = () => !!input.cancel?.requested;
  const stopped = (): Outcome => ({ kind: "stopped", problem: { why: "cancelled" }, notes });
  // Set once the backend may move the base: from then on an error is never "nothing changed".
  let integrating = false;
  try {
    // A Cancel left by an earlier Finish of this worktree must not reach this one.
    await ipc.finishReset(wt.path).catch(() => {});
    onStep(t("Stopping the worktree's sessions…"));
    if (!(await stopSessions(wt))) return { kind: "stopped", problem: { why: "sessions" }, notes };
    if (cancelled()) return stopped();

    // Only now is the folder still: list what is in it, and commit only what was confirmed.
    onStep(t("Reading the worktree…"));
    const now = await ipc.finishInspect(repo.path, wt.path, base);
    if (cancelled()) return stopped();
    if (now.branch !== branch) return { kind: "stopped", problem: { why: "not_on_branch", branch: now.branch }, notes };
    if (now.embedded.length) return { kind: "stopped", problem: { why: "embedded", folders: now.embedded }, notes };
    if (now.operation || now.conflicts.length) return { kind: "stopped", problem: { why: "error", message: now.operation ? t("A {op} is in progress in the worktree; finish or abort it first.", { op: now.operation }) : t("The worktree has unresolved conflicts; resolve them first.") }, notes };
    const files = now.uncommitted.map((f) => f.path);
    if (files.some((f) => !input.confirmed.includes(f))) return { kind: "stopped", problem: { why: "files_changed", files }, notes };
    if (files.length) {
      onStep(t("Saving uncommitted work as a WIP commit…"));
      const wip = await ipc.finishWipCommit(wt.path, branch, `WIP from ${who}`, files);
      if (wip.extra.length) return { kind: "stopped", problem: { why: "files_changed", files: [...files, ...wip.extra.filter((f) => !files.includes(f))] }, notes };
      if (wip.commit) notes.push(t("Uncommitted work saved in a WIP commit ({id})", { id: wip.commit.slice(0, 8) }));
    }

    if (cancelled()) return stopped();
    onStep(input.checks.length ? t("Merging and running the checks…") : t("Merging…"));
    integrating = true;
    const r = await ipc.finishIntegrate({
      root: repo.path, worktree: wt.path, branch, base, strategy: input.strategy, message: input.message,
      checks: input.checks, copy: input.copy, link: input.link, allow_config_change: input.allowConfigChange, delete_branch: true,
    });
    switch (r.state) {
      case "conflict":
        return { kind: "stopped", problem: { why: "conflict", files: r.files }, notes };
      case "check_failed":
        return { kind: "stopped", problem: { why: "checks", checks: r.checks, cleanup: r.cleanup_error }, notes };
      case "config_blocked":
        return { kind: "stopped", problem: { why: "config_blocked", files: r.files }, notes };
      case "base_moved":
        return { kind: "stopped", problem: { why: "base_moved" }, notes };
      case "checkout_dirty":
        return { kind: "stopped", problem: { why: "checkout_dirty", path: r.path, files: r.files }, notes };
      case "worktree_changed":
        return { kind: "stopped", problem: { why: "files_changed", files: r.files }, notes };
      case "cancelled":
        return { kind: "stopped", problem: { why: "cancelled" }, notes };
    }
    if (r.state === "merged") {
      notes.push(t("{base} now includes {branch}", { base, branch }));
      notes.push(r.checkout_updated ? t("The main checkout was brought up to date") : t("{base} is not checked out anywhere: only the branch moved", { base }));
    } else {
      notes.push(t("{branch} had nothing new for {base}", { branch, base }));
    }
    notes.push(r.branch_deleted ? t("Branch {branch} deleted (its work is in {base})", { branch, base }) : t("Branch {branch} kept: {why}", { branch, why: r.branch_note ?? "" }));
    const commit = r.state === "merged" ? r.commit : null;

    // The merge is done and recorded: the card is Done whatever happens to the folder.
    store.setProjectStatus(wt.id, "done");
    automationEvents.report({ kind: "finished", root: repo.path, branch, commit: commit ?? undefined });
    if (cancelled()) {
      // Past the point where Cancel can mean "nothing changed": leave a normal worktree and say what is merged.
      if (r.branch_deleted) await ipc.finishRestoreBranch(repo.path, branch, r.tip, wt.path).catch(() => {});
      return { kind: "merged", commit, notes, problem: t("Cancelled before the worktree was removed. The work is merged into {base}; the worktree and its branch stay.", { base }) };
    }
    onStep(t("Removing the worktree…"), true);
    try {
      // The daemon's snapshots of this folder go first: a turn-end snapshot still reading it would hold files open.
      await ipc.daemon("checkpoint.drop", { worktree: wt.path, root: repo.path }).catch(() => {});
      const { leftover } = await removeWorktree(wt, { force: false });
      if (leftover) notes.push(t("The folder could not be fully deleted (a program still uses it): {path}", { path: wt.path }));
    } catch (e) {
      // The worktree still points at the branch we deleted: put the branch back so it is a normal worktree again.
      if (r.branch_deleted) await ipc.finishRestoreBranch(repo.path, branch, r.tip, wt.path).catch(() => {});
      return { kind: "merged", commit, notes, problem: t("The work is merged into {base}, but the worktree could not be removed: {error}", { base, error: errorText(e) }) };
    }
    return { kind: "finished", commit, notes };
  } catch (e) {
    // Nothing above this point moves the base without returning; an error is a stop, and the backend says what was left.
    if (cancelled() && !integrating) return stopped();
    return { kind: "stopped", problem: { why: "error", message: errorText(e) }, notes };
  } finally {
    await ipc.finishReset(wt.path).catch(() => {});
  }
}

/** The prompt that sends an agent to resolve a merge conflict in the worktree. */
export function conflictPrompt(branch: string, base: string, files: string[]): string {
  return `Merging ${branch} into ${base} conflicts in:\n${files.map((f) => `- ${f}`).join("\n")}\n\nIn this worktree run \`git merge ${base}\`, resolve every conflict (keep the intent of both sides), run the project's tests, then commit. Do not push, rebase or delete anything.`;
}

/** The prompt that sends an agent to fix a failed check. */
export function checkPrompt(branch: string, base: string, c: ipc.CheckResult): string {
  return `The check \`${c.command}\` failed on ${branch} merged into ${base} (exit ${c.code ?? "?"}). Find and fix the cause in this worktree, run the check again, then commit. Do not push or bypass the check.\n\nOutput:\n${c.output.slice(-4000)}`;
}
