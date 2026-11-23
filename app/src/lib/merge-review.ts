// What the git panel shows beside the merge line: the flags of a pull request's change (read from the diff against its base), which
// of them a person accepted for this head commit, and how each is worded. Pure: the reading of the diff and the persisting are
// done by the callers.

import { t } from "./i18n";
import type { FlagAcceptance } from "./types";
import type { PullRequest } from "./ipc";
import { mergeLine, openFlags, prRead, type ChangedFile, type Flag } from "./merge-line";

/** How many acceptances are kept; the oldest go first. */
export const MAX_ACCEPTANCES = 200;
/** How many bytes of the diff are read: a larger change is not judged from part of it. */
export const MAX_DIFF_BYTES = 2_000_000;

/** What was read of a pull request's change: its flags, or why they could not be read. */
export interface FlagRead {
  /** The head commit and base the read was made for: another head or base is a new read. */
  head: string;
  base: string;
  flags: Flag[];
  error?: string;
}

/** What the chip is given besides the pull request. */
export interface ReviewState {
  /** The flags no person has accepted for this head. */
  open: Flag[];
  /** The flags accepted for this head. */
  accepted: Flag[];
  /** Why the flags could not be read; the line is then shown without them. */
  error?: string;
}

const FILES_MARKER = "\nFiles:\n";

/** The patch in the change text `git_context` gives for a branch: the commits, then `Files:` and the stat, then the patch. */
export function patchOf(context: string): { patch: string; truncated: boolean } | null {
  const marker = context.lastIndexOf(FILES_MARKER);
  if (marker < 0) return null;
  const after = context.slice(marker + FILES_MARKER.length);
  // Stat lines begin with a space, so the first `diff --git` at the start of a line is where the patch begins.
  let patch = "";
  if (after.startsWith("diff --git ")) patch = after;
  else {
    const at = after.indexOf("\ndiff --git ");
    if (at >= 0) patch = after.slice(at + 1);
  }
  return { patch, truncated: context.endsWith("… (truncated)") };
}

/** A path as git writes it when it holds unusual characters: in quotes, with escapes (`\303\274` for a UTF-8 byte). */
function unquote(p: string): string {
  const raw = p.replace(/\t$/, "");
  if (raw.length < 2 || !raw.startsWith('"') || !raw.endsWith('"')) return raw;
  const named: Record<string, number> = { n: 10, t: 9, r: 13, '"': 34, "\\": 92 };
  const enc = new TextEncoder();
  const bytes: number[] = [];
  const body = raw.slice(1, -1);
  for (let i = 0; i < body.length; i++) {
    if (body[i] !== "\\") {
      bytes.push(...enc.encode(body[i]));
      continue;
    }
    const octal = /^[0-7]{3}/.exec(body.slice(i + 1));
    if (octal) {
      bytes.push(parseInt(octal[0], 8));
      i += 3;
    } else if (named[body[i + 1]] !== undefined) bytes.push(named[body[++i]]);
  }
  return new TextDecoder().decode(new Uint8Array(bytes));
}

/** The files of a unified diff (`git diff` without colour), each with its status and changed lines. */
export function parseChange(patch: string): ChangedFile[] {
  const files: ChangedFile[] = [];
  let cur: ChangedFile | null = null;
  let inHunk = false;
  let from: string | null = null;
  let to: string | null = null;
  let renamedTo: string | null = null;
  const finish = () => {
    if (!cur) return;
    // A rename carries its new name, a removed file its old one, the rest the name after the change.
    const path = cur.status === "renamed" ? renamedTo : cur.status === "removed" ? (from ?? to) : (to ?? from);
    if (path) files.push({ ...cur, path });
    cur = null;
  };
  for (const line of patch.split("\n")) {
    if (line.startsWith("diff --git ")) {
      finish();
      inHunk = false;
      from = to = renamedTo = null;
      cur = { path: "", status: "modified", added: [], removed: [] };
      // Without `---`/`+++` lines (a mode change, an empty file) the header is all there is; it reads when both names are the same.
      const same = /^diff --git a\/(.+) b\/\1$/.exec(line);
      if (same) from = to = same[1];
      continue;
    }
    if (!cur) continue;
    if (inHunk) {
      if (line.startsWith("+")) cur.added.push(line.slice(1));
      else if (line.startsWith("-")) cur.removed.push(line.slice(1));
      continue;
    }
    if (line.startsWith("new file mode")) cur.status = "added";
    else if (line.startsWith("deleted file mode")) cur.status = "removed";
    else if (line.startsWith("rename from ")) cur.status = "renamed";
    else if (line.startsWith("rename to ")) renamedTo = unquote(line.slice("rename to ".length));
    else if (line.startsWith("--- ")) from = line.startsWith("--- /dev/null") ? from : unquote(line.slice(4)).replace(/^a\//, "");
    else if (line.startsWith("+++ ")) to = line.startsWith("+++ /dev/null") ? to : unquote(line.slice(4)).replace(/^b\//, "");
    else if (line.startsWith("@@")) inHunk = true;
  }
  finish();
  return files;
}

/** Each flag kind as the person reads it. */
export function flagLabel(what: string): string {
  switch (what) {
    case "workflow": return t("workflow");
    case "check-script": return t("check script");
    case "baseline": return t("baseline");
    case "test-file-removed": return t("test file removed");
    case "test-removed": return t("tests removed");
    case "test-off": return t("test switched off");
    default: return what;
  }
}

/** The flags of a pull request's change and which are accepted for its head. Nothing read yet is no flags, not an error. */
export function reviewOf(read: FlagRead | undefined, acceptances: FlagAcceptance[] | undefined, project: string, head: string | undefined): ReviewState {
  if (!read || !head || read.head !== head) return { open: [], accepted: [] };
  const mine = (acceptances ?? []).filter((a) => a.project === project);
  const open = openFlags(read.flags, mine, head);
  return { open, accepted: read.flags.filter((f) => !open.includes(f)), error: read.error };
}

/** The acceptance list after accepting `flags` for `head`: each once, the oldest dropped beyond the bound. */
export function withAcceptances(list: FlagAcceptance[] | undefined, project: string, head: string, flags: Flag[]): FlagAcceptance[] {
  const out = [...(list ?? [])];
  for (const f of flags) {
    if (!out.some((a) => a.project === project && a.head === head && a.path === f.path && a.what === f.what)) out.push({ project, head, path: f.path, what: f.what });
  }
  return out.slice(-MAX_ACCEPTANCES);
}

/** Whether the pull request can be merged, as a chip; null where that is not known (see `prRead`). `review` is what in its change needs a look. */
export function mergeChip(pr: PullRequest, review: ReviewState = { open: [], accepted: [] }): { text: string; cls: "checks-passing" | "checks-pending" | "checks-failing"; title: string } | null {
  const read = prRead(pr);
  if (!read || pr.state !== "OPEN") return null;
  const line = mergeLine(read, review.open, 0);
  // What a person accepted for this commit, and what could not be read, stay in the title whatever the line says.
  const notes = [
    ...review.accepted.map((f) => t("{path} ({what}): accepted for this commit", { path: f.path, what: flagLabel(f.what) })),
    ...(review.error ? [t("The change could not be checked for what needs a look: {error}", { error: review.error })] : []),
  ];
  if (line.can) {
    const title = [...line.rerun.map((r) => t("{name} passed only on run {n}", { name: r.name, n: r.attempts })), ...notes].join("\n");
    return { text: t("can merge"), cls: "checks-passing", title };
  }
  const waiting = line.reason === "checks-running" || line.reason === "unknown";
  const reason =
    line.reason === "checks-failing" ? t("checks failing ({what})", { what: line.detail })
    : line.reason === "checks-running" ? t("checks still running")
    : line.reason === "draft" ? t("it is a draft")
    : line.reason === "conflicts" ? t("conflicts with {base}", { base: line.detail })
    : line.reason === "blocked" ? t("GitHub blocks it ({state})", { state: line.detail })
    : line.reason === "touches" ? t("a change needs a look")
    : line.reason === "not-open" ? t("it is not open")
    : t("GitHub has not said yet");
  return { text: t("cannot merge: {reason}", { reason }), cls: waiting ? "checks-pending" : "checks-failing", title: [...(line.reason === "checks-running" || line.reason === "unknown" ? [line.detail] : []), ...notes].join("\n") };
}
