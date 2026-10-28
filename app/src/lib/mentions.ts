// @-mentions in the composer, expanded into what an agent can take. Pure: everything that
// touches the app (files, the selection, other sessions, redaction) comes in through the context,
// so the rules below are tested without a webview.
//
// Agents that read `@path` themselves (Claude, Codex, Gemini) get the path. Other agents get the
// content, fenced and labelled with its source. A shell (or any program that is not an agent) never
// gets content, only paths quoted for it. A snippet has at most 200 lines and all snippets together
// 64 KB; beyond that the agent gets a path to read instead (a temporary file for text that has no
// file of its own; on another machine the last lines instead, since a local file means nothing there).
import type { ReviewNote } from "./types";

export const MAX_SNIPPET_LINES = 200;
export const MAX_TOTAL_BYTES = 64 * 1024;

/** Agents whose own prompt reads `@path`. */
const AT_PATH_AGENTS = new Set(["claude", "codex", "gemini"]);
export const readsAtPath = (kind: string) => AT_PATH_AGENTS.has(kind);

export type MentionKind = "path" | "selection" | "diff" | "note" | "issue" | "terminal";

export interface Mention {
  kind: MentionKind;
  /** The file or folder, session or issue named (empty for the bare keywords). */
  arg: string;
  /** Where `@…` starts and ends in the text. */
  start: number;
  end: number;
}

const KEYWORDS = new Set(["selection", "diff", "note", "issue", "terminal"]);

/** A path or name as typed after `@`: quoted unless it is made of plain characters. */
function quoted(text: string): string {
  return /^[\p{L}\p{N}_./\\:+\-@#%~=,]*[\p{L}\p{N}_/\\+\-@#%~=]$/u.test(text) || text.includes('"') ? text : `"${text}"`;
}

/** `@path`, quoted when the path has spaces or other characters the parser would cut at: how a
 *  file is written in the composer. */
export const pathMention = (path: string) => `@${quoted(path)}`;

export type ShellStyle = "pwsh" | "posix" | "cmd";

/** Control characters, line separators and bidirectional marks: never part of a name sent anywhere. */
// eslint-disable-next-line no-control-regex
const UNSAFE_NAME = /[\u0000-\u001f\u007f-\u009f\u2028\u2029\u200e\u200f\u202a-\u202e\u2066-\u2069]/;
export const hasUnsafeName = (path: string) => UNSAFE_NAME.test(path);

/** The mention to write for a dropped file, or null for a name that cannot be written safely
 *  (a line break would make `@"..."` something the parser does not read as a mention at all). */
export const droppedFileMention = (path: string) => (hasUnsafeName(path) ? null : pathMention(path));

/** Whether the text holds anything that looks like a mention, parsed or not: such text is never
 *  submitted to a shell. */
export const looksLikeMention = (text: string) => /(^|\s)@/.test(text);

/** `path` as one word for a shell, or null when it cannot be written safely (control characters;
 *  for cmd also `% ^ ! & | < > ( ) "`). Plain names stay bare; anything else is single-quoted
 *  (`'` doubled in PowerShell, `'\''` in POSIX shells), so `$(…)`, `;`, `&` and backticks stay text. */
export function shellQuote(name: string, style: ShellStyle): string | null {
  if (UNSAFE_NAME.test(name)) return null;
  // A name that starts with `-` would be read as an option however it is quoted.
  const path = name.startsWith("-") ? (style === "cmd" ? ".\\" : "./") + name : name;
  const plain = style === "posix" ? /^[\p{L}\p{N}_./:+-]+$/u : /^[\p{L}\p{N}_./\\:+-]+$/u;
  if (plain.test(path)) return path;
  if (style === "cmd") return /[%^!&|<>()"]/.test(path) ? null : `"${path}"`;
  if (style === "posix") return `'${path.replace(/'/g, `'\\''`)}'`;
  return `'${path.replace(/['\u2018\u2019\u201a\u201b]/g, (c) => c + c)}'`;
}

/** Gemini CLI's own `@` escaping: on Windows a name with any of these is quoted, elsewhere they are
 *  backslash-escaped (and quotes are not stripped). */
function geminiMention(path: string, windows: boolean): string {
  if (windows) return /[\s&()[\]{}^=;!'+,`~%$@#]/.test(path) ? `@"${path}"` : `@${path}`;
  return `@${path.replace(/([\s()[\]{};|*?$`'"#&<>!~\\])/g, "\\$1")}`;
}

/** How an agent of this kind is pointed at a file: `@path` where its prompt reads that, else the path. */
export function pathReference(kind: string, path: string, windows = false): string {
  if (kind === "gemini") return geminiMention(path, windows);
  return readsAtPath(kind) ? pathMention(path) : quoted(path);
}
export const terminalMention = (label: string) => `@terminal:${quoted(label)}`;
export const issueMention = (key: string) => `@issue:${quoted(key)}`;

// `@` after whitespace or at the start (so an e-mail address is not a mention), then either a
// quoted name or a run of non-space characters. `terminal:` / `issue:` may precede either.
const TOKEN = /(^|\s)@((?:terminal|issue):)?(?:"([^"\r\n]+)"|([^\s"]+))/g;
const TRAILING = /[.,;:!?)\]}]+$/;

/** Every mention in `text`, in order. Whether a path exists is decided at expansion. */
export function parseMentions(text: string): Mention[] {
  const out: Mention[] = [];
  for (const m of text.matchAll(TOKEN)) {
    const prefix = m[2]?.slice(0, -1);
    const start = m.index + m[1].length;
    let arg = m[3];
    let end = m.index + m[0].length;
    if (arg === undefined) {
      // Sentence punctuation after a bare mention is not part of it.
      arg = m[4].replace(TRAILING, "");
      end -= m[4].length - arg.length;
    }
    if (prefix) out.push({ kind: prefix as MentionKind, arg, start, end });
    else if (m[3] === undefined && KEYWORDS.has(arg)) out.push({ kind: arg as MentionKind, arg: "", start, end });
    else if (arg) out.push({ kind: "path", arg, start, end });
  }
  return out;
}

/** The mention being typed at the caret (`@` after whitespace, an open quote allowed), for
 *  completion: where its `@` is and what follows it so far. */
export function mentionAt(text: string, caret: number): { start: number; query: string } | null {
  const before = text.slice(0, caret);
  const m = /(^|\s)@("([^"\n]*)|[^\s"]*)$/.exec(before);
  return m ? { start: before.length - m[2].length - 1, query: m[3] ?? m[2] } : null;
}

export interface Selection {
  text: string;
  /** Terminal output carries whatever a program printed: it is masked like `@terminal`. */
  source?: "terminal";
  /** The file it came from (project path), with 1-based first and last line. */
  path?: string;
  startLine?: number;
  endLine?: number;
}

export interface MentionContext {
  /** What `path` is (a project path or an absolute one), or null when it does not exist. */
  stat(path: string): Promise<{ dir: boolean } | null>;
  /** The file's text, or null when it cannot be read as text. `big`: it was cut short. */
  readText(path: string): Promise<{ text: string; binary: boolean; big: boolean } | null>;
  selection(): Promise<Selection | null>;
  diff(): Promise<string>;
  /** The recent output of a session named by label or id; null when there is none. */
  terminal(ref: string): Promise<string | null>;
  /** The issue as text; null when no connected tracker has it. */
  issue(ref: string): Promise<string | null>;
  notes(): ReviewNote[];
  /** Masks credentials in text that comes from elsewhere. */
  redact(text: string): Promise<string>;
  /** Saves text too long to inline and returns the path of the file. */
  stash(label: string, text: string): Promise<string>;
}

export interface Expanded {
  text: string;
  /** Content of files, terminals, … went into the text (not just paths). */
  inlined: boolean;
  /** Keywords that found nothing (no selection, no changes, unknown session): left as typed. */
  problems: Mention[];
  /** Mentions that cannot be sent at all: a name no shell can take safely, text that would need a
   *  local file for a session on another machine. */
  refused: { mention: Mention; reason: "unsafe-name" | "remote" }[];
}

export interface ExpandOptions {
  /** false: nothing but paths goes into the text (a terminal that would run pasted lines). */
  inline?: boolean;
  /** The target is a shell or another program that is not an agent: paths only, quoted for it. */
  shell?: ShellStyle;
  /** The session runs on another machine: no local temporary files. */
  remote?: boolean;
  /** The app runs on Windows (Gemini's `@` escaping). */
  windows?: boolean;
}

class Refuse extends Error {
  constructor(readonly reason: "unsafe-name" | "remote") {
    super(reason);
  }
}

const bytes = (text: string) => new TextEncoder().encode(text).length;
const lineCount = (text: string) => text.replace(/\r?\n$/, "").split(/\r?\n/).length;

/** A fence longer than any run of backticks inside, so the content cannot close it. */
export function fence(content: string, lang: string): string {
  const longest = Math.max(0, ...[...content.matchAll(/`+/g)].map((m) => m[0].length));
  const ticks = "`".repeat(Math.max(3, longest + 1));
  return `${ticks}${lang}\n${content.replace(/\r?\n$/, "")}\n${ticks}`;
}

/** The file's extension as the fence's language tag ("" when there is none). */
const extOf = (path: string) => {
  const name = path.split(/[\\/]/).pop() ?? "";
  const e = name.includes(".") ? name.split(".").pop()! : "";
  return /^[a-z0-9+#-]{1,8}$/i.test(e) ? e.toLowerCase() : "";
};

/** The notes as the text an agent reads. */
export function formatNotes(notes: ReviewNote[]): string {
  return notes
    .slice()
    .sort((a, b) => a.path.localeCompare(b.path) || a.line - b.line)
    .map((n) => `- ${n.path}:${n.line}${n.side === "old" ? " (removed line)" : ""}${n.text.trim() ? ` \`${n.text.trim().slice(0, 120)}\`` : ""}: ${n.note}`)
    .join("\n");
}

/** The last lines of `content` that fit `maxLines` and `maxBytes`. */
function tailOf(content: string, maxLines: number, maxBytes: number): string[] {
  const lines = content.replace(/\r?\n$/, "").split(/\r?\n/).slice(-maxLines);
  while (lines.length > 1 && bytes(lines.join("\n")) > maxBytes) lines.shift();
  return lines;
}

/** Replaces the mentions in `text` with what `kind` of agent can use. */
export async function expandMentions(text: string, ctx: MentionContext, kind: string, opts: ExpandOptions = {}): Promise<Expanded> {
  // Whatever the caller says, a shell or custom command never gets content.
  const shell = opts.shell ?? (kind === "shell" || kind === "custom" ? "posix" : undefined);
  const atPath = readsAtPath(kind);
  const pathRef = (path: string): string => {
    if (!shell) return pathReference(kind, path, opts.windows);
    const word = shellQuote(path, shell);
    if (word === null) throw new Refuse("unsafe-name");
    return word;
  };
  const blocks: string[] = [];
  const problems: Mention[] = [];
  const refused: Expanded["refused"] = [];
  let budget = opts.inline === false || shell ? -1 : MAX_TOTAL_BYTES;
  let out = "";
  let cursor = 0;

  /** Inline `content` as a labelled block when it fits, else `overflow()` says where to read it.
   *  `tailable`: on another machine the last lines go in instead of a local file. */
  async function snippet(label: string, what: string, lang: string, content: string, overflow: () => Promise<string>, tailable = false): Promise<string> {
    if (lineCount(content) <= MAX_SNIPPET_LINES && bytes(content) <= budget) {
      budget -= bytes(content);
      blocks.push(`${label}:\n${fence(content, lang)}`);
      return what;
    }
    if (opts.remote && tailable && budget > 0) {
      const lines = tailOf(content, MAX_SNIPPET_LINES, budget);
      const tail = lines.join("\n");
      if (bytes(tail) <= budget) {
        budget -= bytes(tail);
        blocks.push(`${label} (last ${lines.length} lines):\n${fence(tail, lang)}`);
        return what;
      }
    }
    return overflow();
  }
  /** Text with no file of its own goes to a temporary one (on this machine). */
  const stash = async (name: string, content: string) => {
    if (opts.remote) throw new Refuse("remote");
    return pathRef(await ctx.stash(name, content));
  };
  const stashed = (name: string, content: string) => () => stash(name, content);

  for (const m of parseMentions(text)) {
    let ref: string | null = null;
    try {
      switch (m.kind) {
        case "path": {
          const st = await ctx.stat(m.arg);
          if (!st) break;
          if (atPath || st.dir || shell) {
            ref = pathRef(m.arg);
            break;
          }
          const file = await ctx.readText(m.arg);
          if (!file || file.binary || file.big) ref = pathRef(m.arg);
          else ref = await snippet(`File \`${m.arg}\``, `\`${m.arg}\``, extOf(m.arg), file.text, async () => pathRef(m.arg));
          break;
        }
        case "selection": {
          const found = await ctx.selection();
          if (!found?.text.trim()) {
            problems.push(m);
            break;
          }
          const sel = found.source === "terminal" ? { ...found, text: await ctx.redact(found.text) } : found;
          const lines = sel.startLine ? `${sel.startLine}${sel.endLine && sel.endLine !== sel.startLine ? `-${sel.endLine}` : ""}` : null;
          const range = sel.path && lines ? `${sel.path}:${lines}` : null;
          const where = range || sel.path ? `Selection from \`${range ?? sel.path}\`` : "Selection";
          ref = await snippet(where, "the selection below", sel.path ? extOf(sel.path) : "", sel.text, async () =>
            // A selection of a file is read from the file, by its lines.
            range ? (atPath ? `${pathRef(sel.path!)} (lines ${lines})` : pathRef(range)) : stash("selection", sel.text),
            true,
          );
          break;
        }
        case "diff": {
          const diff = await ctx.diff();
          if (!diff.trim()) {
            problems.push(m);
            break;
          }
          ref = await snippet("Uncommitted changes (git diff)", "the diff below", "diff", diff, stashed("diff", diff), true);
          break;
        }
        case "terminal": {
          const raw = await ctx.terminal(m.arg);
          if (raw === null) {
            problems.push(m);
            break;
          }
          // Output carries whatever the program printed: credentials are masked first.
          const safe = await ctx.redact(raw);
          ref = await snippet(`Recent output of terminal \`${m.arg || "terminal"}\``, "the terminal output below", "text", safe, stashed("terminal", safe), true);
          break;
        }
        case "issue": {
          const raw = await ctx.issue(m.arg);
          if (raw === null) {
            problems.push(m);
            break;
          }
          const safe = await ctx.redact(raw);
          ref = await snippet(`Issue \`${m.arg || "issue"}\``, "the issue below", "text", safe, stashed("issue", safe), true);
          break;
        }
        case "note": {
          const notes = ctx.notes();
          if (!notes.length) {
            problems.push(m);
            break;
          }
          const body = formatNotes(notes);
          ref = await snippet("Review notes on the current changes", "the review notes below", "", body, stashed("notes", body), true);
          break;
        }
      }
    } catch (e) {
      if (!(e instanceof Refuse)) throw e;
      refused.push({ mention: m, reason: e.reason });
      continue;
    }
    if (ref === null) continue;
    out += text.slice(cursor, m.start) + ref;
    cursor = m.end;
  }
  out += text.slice(cursor);
  return { text: blocks.length ? `${out.replace(/\s+$/, "")}\n\n${blocks.join("\n\n")}` : out, inlined: blocks.length > 0, problems, refused };
}
