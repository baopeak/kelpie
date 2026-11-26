// A project's pull request on the phone, as data: what the computer's `pr_get` answer means to the person, and the words for it. The view
// (pull-line-view.tsx) and the action (phone-state.ts) use these; the link only carries the question.
import { t } from "./i18n";
import type { Outcome, PullStatus } from "./link";

/** Why a pull request cannot be merged, as the computer names it (the first that holds). */
export type PullReason = "unknown" | "not_open" | "draft" | "conflicts" | "checks_failing" | "checks_running" | "blocked";

export interface PullInfo {
  repo: string;
  number: number;
  title: string;
  url: string;
  base: string;
  /** The head commit the line was read for: a merge is bound to it. */
  head: string;
  can: boolean;
  reason: PullReason | null;
  detail: string;
}

/** What the phone knows of a project's pull request: asked for, answered, or why not. "hidden": the project is not on GitHub (no `origin`, or one on
 *  another host), so there is no pull request to look for and no line is shown. */
export type PullView = { kind: "loading" } | { kind: "none" } | { kind: "hidden" } | { kind: "pr"; pr: PullInfo } | { kind: "unavailable"; message: string };

const REASONS: readonly string[] = ["unknown", "not_open", "draft", "conflicts", "checks_failing", "checks_running", "blocked"];

/** The pull request in an answer, when it is a whole one: a repository, a number, a head commit and a line. */
function infoOf(pr: PullStatus): PullInfo | null {
  const { repo, number, head, line } = pr;
  if (typeof repo !== "string" || typeof number !== "number" || typeof head !== "string" || !line || typeof line.can !== "boolean") return null;
  const reason = line.reason !== null && REASONS.includes(line.reason) ? (line.reason as PullReason) : null;
  // A line that cannot, with a reason this phone does not know, still cannot: it is shown as the computer not knowing.
  return {
    repo,
    number,
    title: pr.title ?? "",
    url: pr.url ?? "",
    base: pr.base ?? "",
    head,
    can: line.can,
    reason: line.can ? null : (reason ?? "unknown"),
    detail: line.detail ?? "",
  };
}

/** What the computer's answer to `pr_get` comes to. An answer that is not whole is "unavailable", never a pull request that can be merged. */
export function pullView(o: Outcome): PullView {
  if (o.t === "offline") return { kind: "unavailable", message: t("Not connected to your computer.") };
  if (o.t === "unknown") return { kind: "unavailable", message: t("Your computer did not answer.") };
  if (o.t === "error") return { kind: "unavailable", message: o.code === "not_ready" || o.code === "no_project" ? t("Open Kelpie on your computer so it can tell this phone about the project.") : o.code === "rate_limited" ? t("Asking too often: wait a moment.") : o.message };
  if (!o.pr) return { kind: "unavailable", message: t("Your computer did not answer.") };
  if (o.pr.none) return o.pr.github === false ? { kind: "hidden" } : { kind: "none" };
  const pr = infoOf(o.pr);
  return pr ? { kind: "pr", pr } : { kind: "unavailable", message: t("Your computer's answer about the pull request could not be read.") };
}

/** The pull request a view holds, if it holds one. */
export const pullOf = (view: PullView): PullInfo | null => (view.kind === "pr" ? view.pr : null);

/** Why it cannot be merged, in the phone's words (the computer's detail is used only where it is data: check names, the state). */
export function reasonWords(pr: PullInfo): string {
  switch (pr.reason) {
    case "not_open":
      return pr.detail === "merged" ? t("it is already merged") : pr.detail === "closed" ? t("it is closed") : t("it is not open");
    case "draft":
      return t("it is a draft");
    case "conflicts":
      return t("it conflicts with {base}", { base: pr.base });
    case "checks_failing":
      return pr.detail ? t("required checks failed: {names}", { names: pr.detail }) : t("required checks failed");
    case "checks_running":
      return pr.detail ? t("required checks are still running: {names}", { names: pr.detail }) : t("required checks are still running");
    case "blocked":
      return t("GitHub wants something more first, such as a review or an update of the branch");
    default:
      return t("GitHub has not said yet");
  }
}

/** The line: "Pull request #12: can merge", or why not. */
export function pullHeadline(pr: PullInfo): string {
  return pr.can ? t("Pull request #{number}: can merge", { number: pr.number }) : t("Pull request #{number}: cannot merge, {reason}", { number: pr.number, reason: reasonWords(pr) });
}

/** What shows under the line: where the pull request is, and what it is called. */
export function pullSubline(pr: PullInfo): string {
  return pr.title ? `${pr.repo} · ${pr.title}` : pr.repo;
}

/** Whether a view shows a line at all: not for a project that is not on GitHub. */
export const pullLineShown = (view: PullView): boolean => view.kind !== "hidden";

/** What a view of no pull request, or one not read, says (nothing for a hidden one). */
export function pullViewWords(view: PullView): string | null {
  switch (view.kind) {
    case "loading":
      return t("Looking for a pull request…");
    case "none":
      return t("No pull request for this project's branch.");
    case "unavailable":
      return t("Could not read the pull request: {why}", { why: view.message });
    default:
      return null;
  }
}
