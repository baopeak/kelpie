// The pull request (GitHub, via `gh`) or merge request (GitLab, its API) of each project's
// current branch, cached for the sidebar chip and the Changes panel. Fetched when the panel
// opens and after a push.
import { createRoot } from "solid-js";
import { createStore } from "solid-js/store";
import { ghPrStatus, gitContext, type PullRequest } from "./ipc";
import { changeFlags } from "./merge-line";
import { MAX_DIFF_BYTES, parseChange, patchOf, type FlagRead } from "./merge-review";
import { t } from "./i18n";
import { gitlabMrStatus, gitlabProject, type MergeRequest } from "./integrations";
import { automationEvents } from "./automation-events";

/** GitLab pipeline status → the checks summary GitHub PRs use. */
function checksOf(pipeline: string | null): PullRequest["checks"] {
  if (!pipeline) return "none";
  if (pipeline === "success") return "passing";
  if (["failed", "canceled"].includes(pipeline)) return "failing";
  return "pending";
}

export const fromMergeRequest = (mr: MergeRequest): PullRequest => ({
  number: mr.iid, title: mr.title, url: mr.url, draft: mr.draft, base: mr.target, provider: "gitlab",
  state: mr.state === "merged" ? "MERGED" : mr.state === "closed" ? "CLOSED" : "OPEN",
  checks: checksOf(mr.pipeline),
});

/** The pull request each project folder had failing checks on when last looked at: an automation hears about each failing
 *  pull request once, also across restarts of the app (kept in localStorage). */
const REPORTED_KEY = "kelpie.prChecksReported";
const reportedFailing = new Map<string, number>(
  (() => {
    try {
      return Object.entries(JSON.parse(localStorage.getItem(REPORTED_KEY) ?? "{}") as Record<string, number>);
    } catch {
      return [];
    }
  })(),
);

function reportChecks(root: string, pr: PullRequest | null) {
  const remember = () => {
    try {
      localStorage.setItem(REPORTED_KEY, JSON.stringify(Object.fromEntries(reportedFailing)));
    } catch {
      // Storage full or off: it is reported again after a restart, nothing worse.
    }
  };
  // Passing, pending or gone: the next failure is a new one.
  if (pr?.checks !== "failing") {
    if (reportedFailing.delete(root)) remember();
    return;
  }
  if (reportedFailing.get(root) === pr.number) return;
  reportedFailing.set(root, pr.number);
  remember();
  automationEvents.report({ kind: "pr_checks_failed", root, number: pr.number });
}

/**
 * What in a pull request's change needs a look: the change between its base branch as the remote has it (`origin/<base>`, or the
 * local branch of that name when the remote one is not there) and the worktree's HEAD. A failure is the result's `error`, not a throw.
 */
export async function readFlags(root: string, head: string, base: string): Promise<FlagRead> {
  const errors: string[] = [];
  for (const ref of [`origin/${base}`, base]) {
    try {
      const patch = patchOf(await gitContext(root, "branch", ref, MAX_DIFF_BYTES));
      if (!patch) throw new Error(t("the change could not be read"));
      if (patch.truncated) return { head, base, flags: [], error: t("the change is too large to read") };
      return { head, base, flags: changeFlags(parseChange(patch.patch)) };
    } catch (e) {
      errors.push(`${ref}: ${String(e)}`);
    }
  }
  return { head, base, flags: [], error: errors.join("; ") };
}

function createPrStore() {
  const [prs, setPrs] = createStore<Record<string, PullRequest | null>>({});
  /** The flags of each project's pull request change, read when its head commit or base changes (or when the last read failed). */
  const [flagReads, setFlagReads] = createStore<Record<string, FlagRead | undefined>>({});
  /** Where each project's remote lives (decides PR vs MR, `gh` vs the GitLab API). */
  const [providers, setProviders] = createStore<Record<string, "github" | "gitlab">>({});
  const inFlight = new Set<string>();

  async function refresh(projectId: string, root: string) {
    if (inFlight.has(projectId)) return;
    inFlight.add(projectId);
    try {
      const gitlab = await gitlabProject(root).catch(() => null);
      setProviders(projectId, gitlab ? "gitlab" : "github");
      if (gitlab) {
        const mr = await gitlabMrStatus(root);
        setPrs(projectId, mr ? fromMergeRequest(mr) : null);
        reportChecks(root, mr ? fromMergeRequest(mr) : null);
      } else {
        const pr = await ghPrStatus(root);
        setPrs(projectId, pr ? { ...pr, provider: "github" } : null);
        reportChecks(root, pr);
        void refreshFlags(projectId, root, pr);
      }
    } catch {
      // gh missing or signed out: no chip, no error toast for a background check.
      setPrs(projectId, null);
    } finally {
      inFlight.delete(projectId);
    }
  }

  /** Reads the flags of an open pull request's change unless they were read for this head and base already. */
  async function refreshFlags(projectId: string, root: string, pr: PullRequest | null) {
    if (!pr || pr.state !== "OPEN" || !pr.head) {
      if (flagReads[projectId]) setFlagReads(projectId, undefined);
      return;
    }
    const known = flagReads[projectId];
    if (known && known.head === pr.head && known.base === pr.base && !known.error) return;
    const read = await readFlags(root, pr.head, pr.base);
    // The pull request may have moved on while the diff was read.
    if (prs[projectId]?.head === pr.head) setFlagReads(projectId, read);
  }

  return { prs, providers, flagReads, refresh };
}

export const pullRequests = createRoot(createPrStore);

export const prLabel = (pr: PullRequest) => (pr.state === "MERGED" ? "merged" : pr.state === "CLOSED" ? "closed" : pr.draft ? "draft" : "open");
