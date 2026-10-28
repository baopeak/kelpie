// "Review with…": another agent (often another vendor) reviews what a worktree changed. It starts
// in the same worktree with an editable review prompt and the bounded diff against the branch the
// work will be merged into, credentials masked, and can leave notes on lines of the diff that
// appear in the normal diff view (`kelpied review add`, review-notes.ts).
import { store } from "./store";
import * as ipc from "./ipc";
import { fence } from "./mentions";
import { baseOf } from "./review-queue";
import { NOTES_API } from "./review-notes";
import { startWithPrompt } from "./send-to-agent";
import { sessionsOf } from "./review-facts";
import { launchableAgents } from "./agents";
import type { Project } from "./types";

/** The diff text that goes to the reviewer, at most. */
export const REVIEW_DIFF_MAX = 48_000;

export interface ReviewInput {
  prompt: string;
  name: string;
  branch: string | null;
  base: string;
  /** Commits and files since the base (`git_context` "branch"). */
  committed: string;
  /** Uncommitted changes (`git_context` "worktree"). */
  uncommitted: string;
  /** The `kelpied` program a reviewing agent runs to leave notes (null: the daemon cannot take them). */
  exe: string | null;
}

const SAFE_REF = /^[A-Za-z0-9._/~^@-]+$/;

/** The prompt, how to leave notes on lines, and the diff in fences the diff itself cannot close. */
export function reviewMessage(i: ReviewInput): string {
  const parts = [i.prompt.trim()];
  if (i.exe) {
    const base = SAFE_REF.test(i.base) && !i.base.startsWith("-") ? ` --base ${i.base}` : "";
    parts.push(
      `To comment on a line of the diff below, run this shell command (PowerShell needs \`&\` in front); the author sees your comments in their diff:\n  "${i.exe.replace(/\\/g, "/")}" review add${base} <file>:<line> "<comment>"\n(<line> is the line in the new file; add --old for a removed line, counted in the old file.)`,
    );
  }
  parts.push(`Changes in ${i.branch ?? i.name} against ${i.base}:\n${fence(i.committed.trim() || "(no committed changes)", "diff")}`);
  if (i.uncommitted.trim()) parts.push(`Uncommitted changes in the worktree:\n${fence(i.uncommitted, "diff")}`);
  return parts.join("\n\n");
}

/** The bounded diffs of a worktree against its base: committed work first, what is not committed after. */
export async function collectDiff(p: Project, base: string): Promise<{ committed: string; uncommitted: string }> {
  const [committed, uncommitted] = await Promise.all([
    ipc.gitContext(p.path, "branch", base, Math.floor(REVIEW_DIFF_MAX * 0.7)),
    ipc.gitContext(p.path, "worktree", undefined, Math.floor(REVIEW_DIFF_MAX * 0.3)).catch(() => ""),
  ]);
  return { committed, uncommitted };
}

/** The reviewer a user probably wants: an agent that did not write the changes. */
export function suggestedReviewer(p: Project): string | undefined {
  const writers = new Set(sessionsOf(p.id).map((s) => s.kind));
  const all = launchableAgents(store.state.settings.hiddenAgents);
  return all.find((k) => !writers.has(k)) ?? all[0];
}

/** Starts the review: a new session of `kind` in the worktree that begins with the message (masked in `startWithPrompt`). */
export async function startReview(p: Project, kind: string, prompt: string) {
  const base = baseOf(p);
  if (!base) throw new Error("no base branch");
  const { committed, uncommitted } = await collectDiff(p, base);
  const exe = store.daemonApi() >= NOTES_API ? store.daemonExe() : null;
  const text = reviewMessage({ prompt, name: p.name, branch: p.branch ?? null, base, committed, uncommitted, exe });
  store.switchProject(p.id);
  return startWithPrompt(p.id, kind, text);
}
