// Turn checkpoints: what the daemon recorded for each agent turn (a snapshot of the working
// folder at the end of the turn), and the pure helpers the turn views use. Loaded only with
// those views.
import { marked } from "marked";
import { daemon } from "./ipc";
import { msg } from "./i18n";

export interface TurnRecord {
  /** Snapshot number: 0 is the baseline; turns without file changes take none. */
  n: number;
  kind: "baseline" | "turn" | "undo";
  at: number;
  started: number;
  /** hook | chat | idle | spawn | restart | undo */
  source: string;
  /** The boundary is a guess (the agent reports no turns: output went quiet). */
  approximate: boolean;
  files: number;
  added: number;
  removed: number;
  paths: string[];
  truncated: boolean;
  undo_of: number | null;
  base_n: number | null;
}

export interface TurnList {
  repo: string | null;
  enabled: boolean;
  repo_off: boolean;
  /** Why there are no snapshots and will be none: off | slow | not-git | remote | shell. */
  why: string | null;
  records: TurnRecord[];
  /** The oldest record is only the base of the next one (the history is full). */
  first_is_base: boolean;
}

export interface TurnFile {
  path: string;
  /** A added, M modified, D deleted, T type changed */
  status: string;
  added: number | null;
  removed: number | null;
  binary: boolean;
}

export interface PlanItem {
  path: string;
  action: "restore" | "delete" | "skip";
  /** clean | modified (changed since the turn) | shared (another session touched it) */
  state: "clean" | "modified" | "shared";
  with: { session: string; name: string }[];
  reason: string | null;
}

export interface UndoPlan {
  items: PlanItem[];
  owner_working: boolean;
  n: number;
  /** Files git could not read when the snapshot was taken (another program held them). */
  unreadable?: string[];
}

export interface UndoResult {
  applied: boolean;
  files?: string[];
  skipped?: { path: string; reason: string | null }[];
  needs_confirmation?: PlanItem[];
}

export interface OwnerRow {
  path: string;
  last: number;
  shared: boolean;
  sessions: { session: string; name: string; turns: number; last: number; n: number }[];
}

export interface BlobReply {
  exists: boolean;
  size?: number;
  too_large?: boolean;
  binary?: boolean;
  text?: string;
  base64?: string;
}

export const listTurns = (session: string) => daemon<TurnList>("checkpoint.list", { session });
export const turnFiles = (session: string, n: number) => daemon<{ files: TurnFile[]; total: number; base_n: number | null }>("checkpoint.diff", { session, n });
/** A file at the end of turn `n`, or (`before`) as it was when that turn began. */
export const turnBlob = (session: string, n: number, path: string, base64 = false, before = false) => daemon<BlobReply>("checkpoint.blob", { session, n, path, base64, before });
export const undoPlan = (session: string, n: number) => daemon<UndoPlan>("checkpoint.undo_plan", { session, n });
export const undoTurn = (session: string, n: number, paths: string[], confirm: string[]) => daemon<UndoResult>("checkpoint.undo", { session, n, paths, confirm });
export const turnOwners = (session: string) => daemon<{ files: OwnerRow[]; total: number }>("checkpoint.owners", { session });
/** `repo`: switches that repository back on after it was turned off for being slow. */
export const setTurnSnapshots = (enabled?: boolean, repo?: string) => daemon("checkpoint.set_enabled", { enabled, repo });

/** Where the project's folder sits below the repository top: "" at the top (or when unknown),
 *  else "sub/dir/". The daemon names files from the repository top; panes name them from the project. */
export function projectPrefix(repo: string | null, projectPath: string): string {
  if (!repo) return "";
  const norm = (p: string) => p.replace(/^\\\\\?\\/, "").replace(/\\/g, "/").replace(/\/+$/, "");
  const r = norm(repo);
  const p = norm(projectPath);
  if (p.toLowerCase().startsWith(r.toLowerCase() + "/")) return p.slice(r.length + 1) + "/";
  return "";
}

/** `path` (from the repository top) as the project names it, or null when it is outside the project. */
export function inProject(path: string, prefix: string): string | null {
  if (!prefix) return path;
  return path.toLowerCase().startsWith(prefix.toLowerCase()) ? path.slice(prefix.length) : null;
}

/** The records to list, newest first: the base of the oldest turn is not a row of its own. */
export function visibleTurns(list: TurnList): TurnRecord[] {
  const recs = list.first_is_base ? list.records.slice(1) : list.records;
  return recs.slice().reverse();
}

/** Text for a record's origin, for the tooltip of its row. */
export function sourceNote(rec: TurnRecord, agentKind: string | undefined): string {
  if (rec.kind === "baseline") return rec.source === "restart" ? msg("The session was resumed here") : msg("The session started here");
  if (rec.kind === "undo") return msg("Right after an undo");
  if (rec.approximate) return msg("This agent reports no turn boundaries: a turn is a stretch of work followed by quiet, so two turns may be merged.");
  if (agentKind === "codex") return msg("Codex reports only when a turn completes: shown from one turn-complete to the next.");
  return "";
}

export type ArtifactKind = "markdown" | "image" | "mermaid" | "csv";

const IMAGE_MIME: Record<string, string> = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", webp: "image/webp", svg: "image/svg+xml", bmp: "image/bmp", avif: "image/avif" };

export const extOf = (path: string) => path.split(".").pop()?.toLowerCase() ?? "";

/** Files a turn produced that read better rendered. */
export function artifactKind(path: string): ArtifactKind | null {
  const e = extOf(path);
  if (e === "md" || e === "markdown") return "markdown";
  if (IMAGE_MIME[e]) return "image";
  if (e === "mmd" || e === "mermaid") return "mermaid";
  if (e === "csv" || e === "tsv") return "csv";
  return null;
}

export const imageMime = (path: string) => IMAGE_MIME[extOf(path)] ?? "application/octet-stream";

/** Top-level blocks of a markdown document with the line each starts on (1-based), so a
 *  comment on a rendered block becomes a review note on that line of the source. */
export function markdownBlocks(source: string): { line: number; raw: string }[] {
  const out: { line: number; raw: string }[] = [];
  let line = 1;
  for (const tok of marked.lexer(source, { gfm: true })) {
    const lead = tok.raw.length - tok.raw.trimStart().length;
    if (tok.type !== "space" && tok.raw.trim()) out.push({ line: line + (tok.raw.slice(0, lead).match(/\n/g)?.length ?? 0), raw: tok.raw });
    line += tok.raw.match(/\n/g)?.length ?? 0;
  }
  return out;
}

/** How long after a message the daemon may note its turn as started: a replayed message carries
 *  the log's clock, written at most once a second. */
const TURN_START_SLACK_MS = 1000;

/** The undoable turns each chat message led to: the turns with changed files, not undone yet,
 *  that have their base (the Turns panel's rows). A turn belongs to the last message sent before
 *  it started — not before it ended: the record's time is when its snapshot finished, which a
 *  queued or quick next message can precede. Messages without a time, and turns that started
 *  before the first of them (an earlier conversation), match nothing. */
export function turnsByMessage(list: TurnList, messages: { id: string; at?: number }[]): Map<string, TurnRecord[]> {
  const timed = messages.filter((m) => m.at != null).sort((a, b) => a.at! - b.at!);
  const undone = new Set(list.records.filter((r) => r.kind === "undo").map((r) => r.undo_of));
  const out = new Map<string, TurnRecord[]>();
  for (const rec of list.first_is_base ? list.records.slice(1) : list.records) {
    if (rec.kind !== "turn" || rec.files === 0 || undone.has(rec.n)) continue;
    const start = rec.started ? rec.started + TURN_START_SLACK_MS : rec.at;
    const owner = timed.findLast((m) => m.at! <= start);
    if (owner) out.set(owner.id, [...(out.get(owner.id) ?? []), rec]);
  }
  return out;
}

/** The files of a plan that start ticked: the safe ones. Risky ones wait for a deliberate tick. */
export function safelyTicked(plan: UndoPlan): Set<string> {
  return new Set(plan.items.filter((i) => i.action !== "skip" && i.state === "clean").map((i) => i.path));
}

export type UndoAttempt =
  | { kind: "applied"; result: UndoResult }
  /** The daemon refused or the undo stopped part way: nothing further happens without a new decision,
   *  on a new plan. `plan` is null when that could not be read either. */
  | { kind: "refused" | "failed"; error?: unknown; plan: UndoPlan | null; picked: Set<string> };

/** One press of Undo. A confirmation names a path, not a version, so ticks made on the old plan must
 *  not survive a refusal: the very next press would overwrite the save the refusal just protected. */
export async function attemptUndo(api: { plan: () => Promise<UndoPlan>; undo: (paths: string[], confirm: string[]) => Promise<UndoResult> }, items: PlanItem[], picked: Set<string>): Promise<UndoAttempt> {
  const chosen = items.filter((i) => picked.has(i.path));
  const again = async () => {
    const plan = await api.plan().catch(() => null);
    return { plan, picked: plan ? safelyTicked(plan) : new Set<string>() };
  };
  try {
    const result = await api.undo(chosen.map((i) => i.path), chosen.filter((i) => i.state !== "clean").map((i) => i.path));
    return result.applied ? { kind: "applied", result } : { kind: "refused", ...(await again()) };
  } catch (error) {
    return { kind: "failed", error, ...(await again()) };
  }
}

/** "3 files" with the lines added and removed after it, when there are any. */
export function turnStats(rec: Pick<TurnRecord, "added" | "removed">, filesText: string): string {
  return rec.added || rec.removed ? `${filesText} +${rec.added} −${rec.removed}` : filesText;
}
