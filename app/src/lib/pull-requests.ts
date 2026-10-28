// The pull request (GitHub, via `gh`) or merge request (GitLab, its API) of each project's
// current branch, cached for the sidebar chip and the Changes panel. Fetched when the panel
// opens and after a push.
import { createRoot } from "solid-js";
import { createStore } from "solid-js/store";
import { ghPrStatus, type PullRequest } from "./ipc";
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

function createPrStore() {
  const [prs, setPrs] = createStore<Record<string, PullRequest | null>>({});
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
      }
    } catch {
      // gh missing or signed out: no chip, no error toast for a background check.
      setPrs(projectId, null);
    } finally {
      inFlight.delete(projectId);
    }
  }

  return { prs, providers, refresh };
}

export const pullRequests = createRoot(createPrStore);

export const prLabel = (pr: PullRequest) => (pr.state === "MERGED" ? "merged" : pr.state === "CLOSED" ? "closed" : pr.draft ? "draft" : "open");
