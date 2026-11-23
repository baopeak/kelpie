// Merging a pull request from the phone, as data: what the person is shown and what the passkey signs, what the computer's answers mean, and when the
// button is offered at all. The sheet (merge-sheet.tsx) and the actions (phone-state.ts) use these; the link only carries the messages.
import { t } from "./i18n";
import type { Outcome } from "./link";
import type { PullInfo } from "./pull-line";

export const MERGE_METHODS = ["squash", "merge", "rebase"] as const;
export type MergeMethod = (typeof MERGE_METHODS)[number];
/** What is chosen until the person chooses another. */
export const DEFAULT_METHOD: MergeMethod = "squash";

/** The only host a pull request is merged on. The computer spells it into the line the passkey signs. */
const HOST = "github.com";

/** What one merge is bound to: the project the computer finds the repository in, the repository, the pull request, the head commit the person was shown, and how. */
export interface MergeRequest {
  project: string;
  repo: string;
  number: number;
  head: string;
  method: MergeMethod;
}

/** The line the passkey signs: the same the computer builds from what the merge says (`pulls::bound_command`). If the computer describes another merge than
 *  the one the person chose, the lines differ and nothing is signed. */
export const mergeCommand = (m: Pick<MergeRequest, "repo" | "number" | "head" | "method">): string => `merge ${HOST} ${m.repo} #${m.number} ${m.head} ${m.method}`;

/** The request the assertion is over: only a merge's challenge has one of this form (`pulls::challenge_request`). */
export const mergeRequestId = (nonce: string): string => `merge:${nonce}`;

/** The head commit as the person reads it. */
export const shortHead = (head: string): string => head.slice(0, 7);

export function methodWords(method: MergeMethod): string {
  switch (method) {
    case "merge":
      return t("Create a merge commit");
    case "rebase":
      return t("Rebase and merge");
    default:
      return t("Squash and merge");
  }
}

/** Whether `name` is a method the computer takes. */
export const isMergeMethod = (name: string): name is MergeMethod => (MERGE_METHODS as readonly string[]).includes(name);

/** The merge the sheet asks for, from the line as it was read. */
export function mergeRequestOf(project: string, pr: PullInfo, method: MergeMethod): MergeRequest {
  return { project, repo: pr.repo, number: pr.number, head: pr.head, method };
}

/** Whether the line offers Merge: the computer says it can be merged, this phone has control right now, and the computer has the verbs. */
export const mergeOffered = (o: { pr: PullInfo | null; control: boolean; api: boolean }): boolean => o.api && o.control && !!o.pr?.can;

/** What a refusal of a merge says to the person; GitHub's own words (`merge_refused`) and any code this phone does not know are the computer's, as they are. */
export function refusalWords(code: string, message: string): string {
  switch (code) {
    case "no_control":
      return t("Merging needs control for this phone, on your computer's own HTTPS address through Tailscale, with the app lock on and unlocked.");
    case "no_passkey":
      return t("Merging needs this phone's passkey, and this connection has none.");
    case "challenge":
      return t("The passkey check took too long, or was already used. Start again.");
    case "recheck":
      return t("The passkey did not confirm this merge. Start again.");
    case "lock_refused":
      return t("The computer did not accept the passkey.");
    case "merges_waiting":
      return t("Too many merges are waiting for a passkey on this phone. Finish or cancel one, or wait a minute.");
    case "not_ready":
    case "no_project":
      return t("Open Kelpie on your computer so it can tell this phone about the project.");
    case "locked":
      return t("Kelpie is locked on this phone.");
    case "epoch":
      return t("Kelpie restarted on your computer since the page was opened. Look at the pull request again.");
    default:
      return message;
  }
}

const OFFLINE = () => t("Not connected to your computer, so nothing was merged.");
const NO_ANSWER = () => t("Your computer did not answer, so it is not known whether the pull request was merged. Look at it on GitHub.");

/** The challenge the computer gave for a merge: what to sign, and until when. `request` and `title` are what the computer itself read (the repository from the
 *  project's `origin`, the pull request of its branch), and what the person is shown from then on. An answer for another pull request or head than `asked`
 *  (the one the person was shown and chose), or whose line does not describe what it says, is refused. */
export type Challenge = { ok: true; nonce: string; expiry: number; command: string; request: MergeRequest; title: string } | { ok: false; message: string };

const UNREADABLE = () => t("Your computer's answer about the merge could not be read.");
const OTHER_MERGE = () => t("Your computer described another merge than the one you chose, so nothing was merged.");

export function challengeOutcome(o: Outcome, asked: MergeRequest): Challenge {
  if (o.t === "offline") return { ok: false, message: OFFLINE() };
  if (o.t === "unknown") return { ok: false, message: t("Your computer did not answer. Try again.") };
  if (o.t === "error") return { ok: false, message: refusalWords(o.code, o.message) };
  const { nonce, expiry, command, repo, number, head, method, title } = o;
  if (typeof nonce !== "string" || !nonce || typeof expiry !== "number" || typeof command !== "string") return { ok: false, message: UNREADABLE() };
  if (typeof repo !== "string" || typeof number !== "number" || typeof head !== "string" || typeof title !== "string") return { ok: false, message: UNREADABLE() };
  // What the person tapped is the pull request and the head they were shown: a computer that read another has found it changed since.
  if (repo !== asked.repo || number !== asked.number || head !== asked.head) return { ok: false, message: t("The pull request changed since you looked (new commits, or another pull request). Nothing was merged. Look at it again.") };
  if (method !== asked.method) return { ok: false, message: OTHER_MERGE() };
  const request: MergeRequest = { project: asked.project, repo, number, head, method: asked.method };
  if (command !== mergeCommand(request)) return { ok: false, message: OTHER_MERGE() };
  return { ok: true, nonce, expiry, command, request, title };
}

/** What became of a merge: done, or why not. `unsure`: the request went out and no answer came (or the computer says it is still carrying it out), so the pull
 *  request may be merged, or is queued. */
export type MergeResult = { ok: true } | { ok: false; message: string; unsure: boolean };

export function mergeOutcome(o: Outcome): MergeResult {
  if (o.t === "offline") return { ok: false, message: OFFLINE(), unsure: false };
  if (o.t === "unknown") return { ok: false, message: NO_ANSWER(), unsure: true };
  if (o.t === "error") return o.code === "in_progress" ? { ok: false, message: NO_ANSWER(), unsure: true } : { ok: false, message: refusalWords(o.code, o.message), unsure: false };
  if (o.result === "merged") return { ok: true };
  if (o.result === "pending") return { ok: false, message: t("GitHub took the merge but the pull request is not merged yet (it may be queued). Look at it on GitHub."), unsure: true };
  return { ok: false, message: t("Your computer's answer about the merge could not be read. Look at the pull request on GitHub."), unsure: true };
}

/** Why Merge is not offered on this connection, in words, or null when it is: what the computer says about control here (`device.control`), or, from a
 *  computer that does not say, the permission the device has; and in any case the passkey must have opened this page (`unlocked`), because the merge asks it. */
export function mergeBlockWords(control: string | undefined, permission: string | undefined, unlocked: boolean): string | null {
  return controlBlock(control, permission) ?? (unlocked ? null : t("To merge, unlock Kelpie with your passkey."));
}

function controlBlock(control: string | undefined, permission: string | undefined): string | null {
  switch (control) {
    case "ok":
      return null;
    case "hosted_app":
    case "plain_page":
    case "via_relay":
      return t("To merge, open Kelpie through your computer's own HTTPS address (Tailscale), not the hosted app or the relay.");
    case "not_granted":
      return t("To merge, allow control for this phone on your computer: Kelpie, Settings, Mobile.");
    case "lock_off":
      return t("To merge, turn the app lock on, on your computer.");
    case "no_passkey":
      return t("To merge, set up this phone's passkey (the app lock).");
    case "locked":
      return t("To merge, unlock Kelpie with your passkey.");
    default:
      return permission === "control" ? null : t("To merge, allow control for this phone on your computer: Kelpie, Settings, Mobile.");
  }
}
