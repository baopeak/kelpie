// "Try with N agents": one prompt, 2 to 5 agents of any vendor, each working in a worktree of its
// own. It is an ordinary Run of N independent tasks (`worktree: true`, no dependencies), so the Runs
// page follows it and its compare view picks the best result.
import * as ipc from "./ipc";
import { store } from "./store";
import { createRun, type Run, type TaskDraft } from "./runs";
import { kindInfo } from "./agents";
import { branchFromPrompt, configApproved, configAtRef } from "./worktrees";
import { NOTES_API } from "./review-notes";
import { t } from "./i18n";
import type { Project } from "./types";

export const MIN_AGENTS = 2;
export const MAX_AGENTS = 5;
/** Longest prompt sent to every agent (the composer's limit). */
export const MAX_PROMPT = 64_000;

/** Task ids from the agents chosen: `claude`, `codex`, `claude-2`. */
export function taskIds(kinds: string[]): string[] {
  const seen = new Map<string, number>();
  return kinds.map((k) => {
    const base = k.replace(/[^A-Za-z0-9_-]/g, "-").slice(0, 30) || "agent";
    const n = (seen.get(base) ?? 0) + 1;
    seen.set(base, n);
    return n === 1 ? base : `${base}-${n}`;
  });
}

/** The N independent tasks of the run. */
export function candidateTasks(kinds: string[], prompt: string): TaskDraft[] {
  const ids = taskIds(kinds);
  return kinds.map((kind, i) => {
    const same = kinds.filter((k) => k === kind).length;
    const nth = kinds.slice(0, i + 1).filter((k) => k === kind).length;
    return { id: ids[i], title: same > 1 ? `${kindInfo(kind).label} #${nth}` : kindInfo(kind).label, prompt, kind, deps: [], approve: false, worktree: true };
  });
}

/** `Try: fix the login redirect` */
export const runTitle = (prompt: string) => `Try: ${branchFromPrompt(prompt, "agents").replace(/-/g, " ")}`.slice(0, 60);

/** Why this cannot start (null: it can). */
export function problem(kinds: string[], prompt: string): string | null {
  if (kinds.length < MIN_AGENTS || kinds.length > MAX_AGENTS) return t("Choose between {min} and {max} agents.", { min: MIN_AGENTS, max: MAX_AGENTS });
  if (!prompt.trim()) return t("Write the prompt every agent gets.");
  if (prompt.length > MAX_PROMPT) return t("The prompt is too long for an agent's input; use @file mentions or a shorter text.");
  return null;
}

/** What each worktree of the run gets from the repository's ignored files, and what is left out. */
export interface Bringing {
  /** `.kelpie.yaml` was approved for this repository: only then do a run's worktrees bring anything. */
  approved: boolean;
  copy: string[];
  link: string[];
  /** `.kelpie.yaml` lists copies or links that the user has not approved for this repository. */
  unapproved: boolean;
  estimate: ipc.MaterializeEstimate | null;
}

const size = (n: number) => (n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : n >= 1024 ? `${Math.round(n / 1024)} KB` : `${n} B`);

/** One line for the dialog: what a worktree starts with, before the run begins. */
export function bringingText(b: Bringing, agents: number): string {
  const e = b.estimate;
  if (b.unapproved) return t("This repository's .kelpie.yaml lists copies or links but has not been approved (New worktree shows it): each worktree starts as a clean checkout.");
  if (!e || (!e.copied.length && !e.linked.length)) return t("Each worktree starts as a clean checkout.");
  const parts = [e.linked.length ? t("links {files} (shared, nothing copied)", { files: e.linked.join(", ") }) : "", e.copied.length ? t("copies {files} ({size} in {n} entries)", { files: e.copied.join(", "), size: size(e.copy_bytes), n: e.copy_entries }) : ""].filter(Boolean);
  const each = t("Each worktree {what}.", { what: parts.join(" and ") });
  return e.copy_bytes ? `${each} ${t("Copies for {agents} worktrees: {size}.", { agents, size: size(e.copy_bytes * agents) })}` : each;
}

/** Reads `.kelpie.yaml` of the folder's current commit and measures what its approved copies and links bring. */
export async function planBringing(project: Project): Promise<Bringing> {
  const repo = store.state.projects.find((p) => p.id === project.parentId) ?? project;
  const config = await configAtRef(project, "").catch(() => null);
  const listed = !!config && (config.copy.length > 0 || config.link.length > 0);
  const approved = !!config && configApproved(repo, config);
  const copy = approved ? config!.copy : [];
  const link = approved ? config!.link : [];
  // Only what will really be brought is measured: nothing approved, nothing to show.
  const estimate = approved && store.daemonApi() >= NOTES_API ? await ipc.worktreeMaterializeEstimate(project.path, copy, link).catch(() => null) : null;
  return { approved, copy, link, unapproved: listed && !approved, estimate };
}

/** Creates the run: the prompt is masked once for all of them, each agent starts in its own worktree with its profile's flags and account. */
export async function startCandidates(project: Project, kinds: string[], prompt: string, bringing: Bringing): Promise<Run> {
  const why = problem(kinds, prompt);
  if (why) throw new Error(why);
  const safe = await ipc.redact(prompt.trim());
  return createRun({
    name: runTitle(safe),
    objective: "",
    cwd: project.path,
    project_id: project.id,
    tasks: candidateTasks(kinds, safe).map((task) => ({ ...task, ...store.profileLaunch(task.kind) })),
    // Nothing approved, nothing brought (not even `.worktreeinclude`); an older daemon ignores it: clean worktrees.
    materialize: bringing.approved ? { copy: bringing.copy, link: bringing.link } : undefined,
  });
}
