// Specs in the repository, read as plain markdown: `plans/<name>/plan.md` with its `phase-*.md`,
// `specs/**`, `.kiro/specs/**` and `docs/plans/**`. Any agent can write them and any other can do
// the tasks, so there is no format of our own and no store beside the files: checkbox state is the
// file's, a task's run status is the Runs engine's. Parsing is marked's lexer (blocks for the whole file, inline
// tokens only for the few words shown); nothing here runs
// while the Plan tab is closed (the index is started by the tab and stopped with it).
//
// The files are untrusted. Their paths are only ever the ones found by walking the project (never
// a path named inside a file), symlinks and junctions are not entered, and the text of a heading
// or task that goes into an agent's prompt is scrubbed first (`safeLabel`).
import { Lexer, type Token, type Tokens } from "marked";
import { hasUnsafeName, pathMention } from "./mentions";

export const SPEC_ROOTS = ["plans", "specs", ".kiro/specs", "docs/plans"] as const;
export const DEBOUNCE_MS = 300;
/** Files listed at most, folders walked at most this deep below a root, bytes parsed per file. */
export const MAX_FILES = 300;
export const MAX_BYTES = 256 * 1024;
const MAX_DEPTH = 6;
/** More changed folders than this in one batch are cheaper to rescan than to reconcile. */
const RESCAN_AFTER = 40;

/** Work done by the index, to show that a closed tab costs nothing. */
export const stats = { scans: 0, reads: 0, parses: 0 };

/** "slow": the worker did not answer in time (see `spec-parse-client.ts`). */
export type SpecProblem = "big" | "text" | "slow";

export interface SpecTask {
  /** Run task id for this task: the same file and text give the same id, so its row finds its run. */
  id: string;
  /** 0-based line of the checkbox, -1 when it could not be located (shown, not tickable). */
  line: number;
  text: string;
  checked: boolean;
  /** Nesting level of the list it is in. */
  depth: number;
  /** The nearest heading above it. */
  section: string;
  /** The line as written, to tell that the file changed before a tick writes. */
  source: string;
}

export interface SpecDoc {
  path: string;
  name: string;
  title: string;
  mtime: number;
  /** Length in bytes. */
  size: number;
  digest: string;
  tasks: SpecTask[];
  /** List items of a file without any checkbox, so that an unknown format is still shown. */
  plain: string[];
  done: number;
  total: number;
  problem?: SpecProblem;
}

export interface SpecGroup {
  id: string;
  name: string;
  title: string;
  kind: "plan" | "spec";
  docs: SpecDoc[];
  done: number;
  total: number;
}

// ---------- where specs live ----------

/** The detection root `path` is in (or is), if any. Case-insensitive: Windows folders are. */
export function rootOf(path: string): string | null {
  const p = path.toLowerCase();
  return SPEC_ROOTS.find((r) => p === r || p.startsWith(r + "/")) ?? null;
}

/** A changed folder worth looking at: events elsewhere in the project are none of the index's business. */
export const inRoots = (dir: string) => rootOf(dir) !== null;

const depthIn = (dir: string, root: string) => dir.split("/").length - root.split("/").length;

/** Whether the index looks inside `dir`: `plans` only one level down (`plans/<name>/`), the rest to MAX_DEPTH. */
export function listable(dir: string): boolean {
  const root = rootOf(dir);
  if (!root) return false;
  return depthIn(dir, root) <= (root === "plans" ? 1 : MAX_DEPTH);
}

export function isSpecFile(path: string): boolean {
  const root = rootOf(path);
  if (!root || path.length <= root.length) return false;
  const rel = path.slice(root.length + 1).split("/");
  const name = rel[rel.length - 1].toLowerCase();
  if (!/\.(md|markdown)$/.test(name)) return false;
  return root === "plans" ? rel.length === 2 && (name === "plan.md" || /^phase-.+\.md$/.test(name)) : true;
}

/** Names the index lists: nothing that cannot be written into a prompt or a `@path` mention safely. */
const plainName = (name: string) => !hasUnsafeName(name) && !/["`]/.test(name);

const parentOf = (path: string) => path.slice(0, Math.max(0, path.lastIndexOf("/")));
const baseOf = (path: string) => path.slice(path.lastIndexOf("/") + 1);

// ---------- parsing ----------

/** FNV-1a over UTF-16 code units: ids and change detection, not security. */
function hash32(text: string): number {
  let h = 0x811c9dc5;
  for (let i = 0; i < text.length; i++) h = Math.imul(h ^ text.charCodeAt(i), 0x01000193) >>> 0;
  return h;
}
const hex8 = (n: number) => n.toString(16).padStart(8, "0");
const digestOf = (text: string) => `${text.length}.${hex8(hash32(text))}`;

const BREAKS = /\r\n|\r|\n/;
const LF = /\n/g;
const newlines = (raw: string) => raw.match(LF)?.length ?? 0;

/** `- [ ] `, `1. [x] `, `> - [X] `: the part of a task's line before and around the box. */
const CHECKBOX = /^(\ufeff?(?:[ \t]*>)*[ \t]*(?:[-*+]|\d{1,9}[.)])[ \t]+\[)([ xX])(\][ \t])/;

/** Characters that show nothing or reorder what is around them: zero-width and format characters, variation selectors,
 *  the Arabic letter mark, the invisible math operators, interlinear annotation marks and the Tags block (which can carry
 *  text a person never sees). They are dropped; controls and bidirectional marks (see `hasUnsafeName`) become spaces. */
const INVISIBLE = /[\u00ad\u061c\u180e\u200b-\u200d\u2060-\u2064\ufe00-\ufe0f\ufeff\ufff9-\ufffb\u{e0000}-\u{e007f}]/u;

/** A heading cannot carry a terminal sequence into a prompt, nor reorder or hide text around it. */
function scrub(text: string): string {
  if (!hasUnsafeName(text) && !INVISIBLE.test(text)) return text;
  return [...text].map((c) => (hasUnsafeName(c) ? " " : INVISIBLE.test(c) ? "" : c)).join("");
}

// eslint-disable-next-line no-control-regex
const CONTROLS = /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f-\u009f\u2028\u2029\u200e\u200f\u202a-\u202e\u2066-\u2069]/g;

/** A file's text for a prompt: no escape sequences (ESC, other C0 and C1 controls), bidirectional or invisible characters;
 *  tabs and line breaks stay. */
export const stripControls = (text: string) => text.replace(CONTROLS, "").replace(new RegExp(INVISIBLE.source, "gu"), "");

const clip = (text: string, max: number) => (text.length > max ? `${[...text].slice(0, max - 1).join("")}…` : text);

/** The words of inline tokens: no markup, no raw html. */
function plain(tokens: Token[] | undefined): string {
  let out = "";
  for (const t of tokens ?? []) {
    if (t.type === "html" || t.type === "checkbox") continue;
    if ("tokens" in t && t.tokens) out += plain(t.tokens);
    else if (t.type === "br") out += " ";
    else if ("text" in t && typeof t.text === "string") out += t.text;
  }
  return out;
}

interface Walk {
  lexer: Lexer;
  path: string;
  source: string[];
  tasks: SpecTask[];
  /** Top-level list items without a box: shown only for a file that has no box at all, so read at the end. */
  plain: Tokens.ListItem[];
  /** The current heading's markdown, and its words once something needs them. */
  heading: string;
  section: string | null;
  titleRaw: string;
  seen: Map<string, number>;
}

const MARKUP = /[*_`<&\\[~!]/;

/** The words of a markdown snippet (a heading or the first block of an item), without its markup. Only these few strings
 *  are lexed inline: that is most of what lexing a whole file costs. */
function words(lexer: Lexer, markdown: string, max: number): string {
  // Text with none of the characters that start emphasis, code, links, html, escapes or entities reads as it is written.
  const shown = MARKUP.test(markdown) ? plain(lexer.inlineTokens(markdown)) : markdown;
  return clip(scrub(shown).replace(/\s+/g, " ").trim(), max);
}

function itemText(item: Tokens.ListItem, w: Walk): string {
  const first = item.tokens.find((t) => t.type !== "checkbox" && t.type !== "space" && t.type !== "list");
  if (!first) return "";
  const raw = "text" in first && typeof first.text === "string" ? first.text : first.raw;
  return first.type === "code" ? clip(scrub(raw).replace(/\s+/g, " ").trim(), 300) : words(w.lexer, raw.replace(/^\[[ xX]\]\s+/, ""), 300);
}

/** `at` is the 0-based line the tokens start on; each token's raw text says how many lines it takes. */
function blocks(tokens: Token[], at: number, depth: number, top: boolean, w: Walk) {
  for (const tok of tokens) {
    if (tok.type === "heading" && top) {
      w.heading = (tok as Tokens.Heading).text;
      w.section = null;
      if (!w.titleRaw && (tok as Tokens.Heading).depth === 1) w.titleRaw = w.heading;
    } else if (tok.type === "list") {
      items(tok as Tokens.List, at, depth, w);
    } else if (tok.type === "blockquote") {
      blocks((tok as Tokens.Blockquote).tokens, at, depth, false, w);
    }
    at += newlines(tok.raw);
  }
}

function items(list: Tokens.List, at: number, depth: number, w: Walk) {
  for (const item of list.items) {
    if (item.task) {
      const text = itemText(item, w);
      // The same text twice in one file gets two ids, in order.
      const key = `${w.path}\0${text}`;
      const n = w.seen.get(key) ?? 0;
      w.seen.set(key, n + 1);
      const checked = !!item.checked;
      const source = w.source[at] ?? "";
      const box = CHECKBOX.exec(source);
      const located = !!box && (box[2] !== " ") === checked;
      w.tasks.push({ id: `spec-${hex8(hash32(`${key}\0${n}`))}`, line: located ? at : -1, text, checked, depth, section: (w.section ??= words(w.lexer, w.heading, 200)), source: located ? source : "" });
    } else if (depth === 0 && w.plain.length < 50) {
      w.plain.push(item);
    }
    blocks(item.tokens, at, depth + 1, false, w);
    at += newlines(item.raw);
  }
}

/** A document's tasks and headings, from the whole file (cutting it anywhere changes what is a task: html comments and
 *  blocks, nested fences and reference definitions all reach across lines). Never throws: text the lexer cannot make
 *  sense of comes back without tasks. Big files go through `parseSpecAsync`, which runs this same function in a worker. */
export function parseSpec(path: string, text: string, mtime: number, size = text.length): SpecDoc {
  stats.parses++;
  const w: Walk = { lexer: new Lexer({ gfm: true }), path, source: text.split(BREAKS), tasks: [], plain: [], heading: "", section: null, titleRaw: "", seen: new Map() };
  try {
    // What `marked.lexer` does first, minus the inline pass over every paragraph.
    const src = (text.charCodeAt(0) === 0xfeff ? text.slice(1) : text).replace(/\r\n|\r/g, "\n");
    blocks(w.lexer.blockTokens(src, []), 0, 0, true, w);
  } catch {
    w.tasks.length = 0;
    w.plain.length = 0;
  }
  const plainItems = w.tasks.length ? [] : w.plain.map((i) => itemText(i, w)).filter(Boolean);
  const done = w.tasks.filter((t) => t.checked).length;
  return {
    path, name: baseOf(path), title: w.titleRaw ? words(w.lexer, w.titleRaw, 200) : "", mtime, size, digest: digestOf(text), tasks: w.tasks,
    // List items are shown on their own only for a file that has no checkbox at all.
    plain: plainItems, done, total: w.tasks.length,
  };
}

// ---------- parsing off the main thread ----------

/** Below this a file is parsed where it is read: a few milliseconds. Above it (marked's list tokenizer slows with every
 *  item of one list: a 256 KB list takes several hundred) the page is not to wait for it: see `spec-parse-client.ts`. */
export const WORKER_BYTES = 8 * 1024;

/** The message to the worker (`spec-worker.ts`) and its answer: plain data. */
export interface ParseRequest { id: number; path: string; text: string; mtime: number; size: number }
export interface ParseReply { id: number; doc: SpecDoc }

export const problemDoc = (path: string, mtime: number, size: number, problem: SpecProblem): SpecDoc => ({ path, name: baseOf(path), title: "", mtime, size, digest: "", tasks: [], plain: [], done: 0, total: 0, problem });

const DECODER = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });
/** The file's text, or null when it is not UTF-8 text (writing it back would change bytes we cannot read). */
export function decodeText(bytes: Uint8Array): string | null {
  if (bytes.subarray(0, 8192).includes(0)) return null;
  try {
    return DECODER.decode(bytes);
  } catch {
    return null;
  }
}

/** POSIX `cksum` (CRC-32 of the bytes then of their count, complemented) as `cksum` prints it: "<crc> <bytes>". */
const CRC_TABLE = (() => {
  const t: number[] = [];
  for (let n = 0; n < 256; n++) {
    let c = n << 24;
    for (let k = 0; k < 8; k++) c = c & 0x80000000 ? (c << 1) ^ 0x04c11db7 : c << 1;
    t.push(c >>> 0);
  }
  return t;
})();

export function cksumOf(bytes: Uint8Array): string {
  let crc = 0;
  const feed = (b: number) => (crc = ((crc << 8) ^ CRC_TABLE[((crc >>> 24) ^ b) & 0xff]) >>> 0);
  for (const b of bytes) feed(b);
  for (let n = bytes.length; n > 0; n = Math.floor(n / 256)) feed(n % 256);
  return `${(~crc) >>> 0} ${bytes.length}`;
}

// ---------- grouping ----------

const FIRST = ["plan.md", "requirements.md", "design.md", "tasks.md"];
const natural = (a: string, b: string) => a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" });

/** Documents by folder: a plan is its `plan.md` and phases, a spec its requirements, design and tasks. Unfinished groups first. */
export function groupSpecs(docs: SpecDoc[]): SpecGroup[] {
  const by = new Map<string, SpecDoc[]>();
  for (const d of docs) {
    const dir = parentOf(d.path);
    (by.get(dir) ?? by.set(dir, []).get(dir)!).push(d);
  }
  const rank = (d: SpecDoc) => {
    const i = FIRST.indexOf(d.name.toLowerCase());
    return i < 0 ? FIRST.length : i;
  };
  const groups: SpecGroup[] = [...by].map(([dir, list]) => {
    const sorted = list.slice().sort((a, b) => rank(a) - rank(b) || natural(a.name, b.name));
    const root = rootOf(dir);
    const name = dir === root ? dir : baseOf(dir);
    const head = sorted.find((d) => d.name.toLowerCase() === "plan.md");
    return {
      id: dir, name, title: head?.title || name, kind: root === "plans" || root === "docs/plans" ? "plan" : "spec", docs: sorted,
      done: sorted.reduce((n, d) => n + d.done, 0), total: sorted.reduce((n, d) => n + d.total, 0),
    };
  });
  const state = (g: SpecGroup) => (g.total === 0 ? 1 : g.done < g.total ? 0 : 2);
  return groups.sort((a, b) => state(a) - state(b) || natural(a.id, b.id));
}

// ---------- ticking ----------

/** `text` with the box on the task's line set to `checked`, everything else byte for byte; null when the line is not that task's any more. */
export function tickText(text: string, task: Pick<SpecTask, "line" | "source" | "checked">, checked: boolean): string | null {
  if (task.line < 0) return null;
  let start = 0;
  for (let i = 0; i < task.line; i++) {
    const m = /\r\n|\r|\n/g;
    m.lastIndex = start;
    const hit = m.exec(text);
    if (!hit) return null;
    start = hit.index + hit[0].length;
  }
  const rest = /\r\n|\r|\n/.exec(text.slice(start));
  const end = rest ? start + rest.index : text.length;
  const line = text.slice(start, end);
  const box = CHECKBOX.exec(line);
  if (line !== task.source || !box || (box[2] !== " ") !== task.checked) return null;
  const at = start + box[1].length;
  return text.slice(0, at) + (checked ? "x" : " ") + text.slice(at + 1);
}

// ---------- prompts ----------

/** Heading or task text as it may appear in a prompt: one line, no control, bidirectional or invisible
 *  characters, nothing that reads as an `@` mention or closes the quotes around it. */
export function safeLabel(text: string, max = 120): string {
  return clip(scrub(text).replace(/@/g, "(at)").replace(/[`"]/g, "'").replace(/\s+/g, " ").trim(), max);
}

/** What the agent is told about the task. Written after the file mention is expanded, so nothing in a
 *  heading can become a mention of its own. */
export function taskInstructions(task: Pick<SpecTask, "line" | "text" | "section">): string {
  const section = safeLabel(task.section);
  return [
    "Do one task from a spec file in this repository.",
    section ? `Section: "${section}"` : "",
    `Task${task.line >= 0 ? ` (line ${task.line + 1})` : ""}: "${safeLabel(task.text, 300)}"`,
    "Do only this task. When it is done, tick its checkbox in the file ([ ] to [x]) and change nothing else in it.",
  ].filter(Boolean).join("\n");
}

/** The only part of the prompt that is expanded as a mention. */
export const specFileMention = (path: string) => `Spec file: ${pathMention(path)}`;

/** The run's name: what the Runs page lists. */
export const taskRunTitle = (doc: Pick<SpecDoc, "name">, task: Pick<SpecTask, "text" | "line">) => safeLabel(task.text, 60) || (task.line >= 0 ? `${doc.name} line ${task.line + 1}` : doc.name);

// ---------- new spec ----------

export const SPEC_FILES = ["requirements.md", "design.md", "tasks.md"] as const;

/** `name` as a folder name under `specs/`: ASCII letters, digits and dashes; null when nothing is left or Windows would refuse it. */
export function specSlug(name: string): string | null {
  const ascii = name.normalize("NFD").replace(/[\u0300-\u036f]/g, "").replace(/đ/gi, "d");
  const slug = ascii.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "").slice(0, 48).replace(/-+$/, "");
  return !slug || /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/.test(slug) ? null : slug;
}

/** The three documents of a new spec, plan first: what is wanted, how it will be built, then the tasks. */
export function specTemplates(title: string): Record<(typeof SPEC_FILES)[number], string> {
  const t = safeLabel(title, 80) || "New spec";
  return {
    "requirements.md": `# Requirements: ${t}\n\n## Goal\nWhat this delivers and for whom, in a sentence or two.\n\n## Requirements\n- The system shall ...\n\n## Out of scope\n- ...\n\n## Acceptance criteria\n- ...\n`,
    "design.md": `# Design: ${t}\n\n## Approach\nHow the requirements will be met, and why this way.\n\n## Components\n- ...\n\n## Risks and open questions\n- ...\n`,
    "tasks.md": `# Tasks: ${t}\n\nTick a box when its task is done. Each task should be small enough for one agent to finish.\n\n- [ ] Describe the first task\n- [ ] Describe the second task\n`,
  };
}

// ---------- the index ----------

export interface SpecEntry { name: string; path: string; is_dir: boolean; is_symlink: boolean }

/** What the index needs from the project's folder; `path` is always relative to the project root. */
export interface SpecIo {
  /** Entries of a folder, or null when it does not exist. */
  list(dir: string): Promise<SpecEntry[] | null>;
  read(path: string): Promise<{ bytes: Uint8Array; mtime: number; truncated: boolean } | null>;
  /** `expectedCksum` is the POSIX `cksum` of what was read ("<crc> <bytes>"): on a host, seconds cannot tell two edits apart. */
  write(path: string, text: string, expectedMtime: number, expectedCksum: string): Promise<{ ok: boolean; conflict: boolean; mtime: number }>;
  /** Creates an empty file; fails when something is there already. */
  create(path: string): Promise<void>;
  /** Parses a file's text: `parseSpec`, or the same function elsewhere (a worker) for a big one. Default: here. */
  parse?(path: string, text: string, mtime: number, size: number): Promise<SpecDoc>;
  /** Folders that changed, as the watcher reports them; returns the unsubscribe. */
  watch(cb: (dirs: string[]) => void): () => void;
}

export type TickResult = "ok" | "stale" | "busy" | "failed";

export class SpecError extends Error {
  constructor(readonly code: "name" | "exists" | "symlink" | "failed", message: string) {
    super(message);
  }
}

const same = (a: string, b: string) => a.toLowerCase() === b.toLowerCase();

/** Whether every folder of `path` and the file itself (a folder, with `folder`) is a plain entry (no symlink or junction) right now. */
async function plainPath(io: SpecIo, path: string, folder = false): Promise<boolean> {
  const parts = path.split("/");
  let dir = "";
  for (let i = 0; i < parts.length; i++) {
    const entries = await io.list(dir);
    const hit = entries?.find((e) => same(e.name, parts[i]));
    if (!hit || hit.is_symlink || (i < parts.length - 1 || folder) !== hit.is_dir) return false;
    dir = hit.path;
  }
  return true;
}

export function createSpecIndex(io: SpecIo, onChange: (docs: SpecDoc[]) => void, debounceMs = DEBOUNCE_MS) {
  const docs = new Map<string, SpecDoc>();
  /** Folders listed so far (lower case): events about any other folder are not about specs we show. */
  const known = new Set<string>();
  const pending = new Set<string>();
  const ticking = new Set<string>();
  let active = false;
  let gen = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let unwatch: (() => void) | undefined;
  let queue: Promise<void> = Promise.resolve();
  let capped = false;
  let lookForRoots = false;

  const emit = () => onChange([...docs.values()]);

  async function load(path: string): Promise<void> {
    stats.reads++;
    const r = await io.read(path).catch(() => null);
    if (!r) {
      docs.delete(path);
      return;
    }
    const old = docs.get(path);
    // Times on a host are whole seconds: two edits of one second apart look alike there, so only a time with a fraction is trusted.
    if (old && old.mtime === r.mtime && r.mtime % 1000 !== 0 && old.size === r.bytes.length && !old.problem) return;
    if (r.truncated || r.bytes.length > MAX_BYTES) return void docs.set(path, problemDoc(path, r.mtime, r.bytes.length, "big"));
    const text = decodeText(r.bytes);
    docs.set(path, text === null ? problemDoc(path, r.mtime, r.bytes.length, "text") : await (io.parse ?? ((p, t, m, n) => Promise.resolve(parseSpec(p, t, m, n))))(path, text, r.mtime, r.bytes.length));
  }

  async function loadMany(paths: string[]) {
    for (let i = 0; i < paths.length; i += 8) await Promise.all(paths.slice(i, i + 8).map(load));
  }

  const split = (entries: SpecEntry[]) => ({
    files: entries.filter((e) => !e.is_dir && !e.is_symlink && plainName(e.name) && isSpecFile(e.path)),
    dirs: entries.filter((e) => e.is_dir && !e.is_symlink && plainName(e.name) && listable(e.path)),
  });

  function room(files: SpecEntry[]) {
    const fresh = files.filter((f) => !docs.has(f.path));
    const free = Math.max(0, MAX_FILES - docs.size);
    if (fresh.length > free) capped = true;
    return files.filter((f) => docs.has(f.path) || fresh.indexOf(f) < free);
  }

  async function scanDir(dir: string): Promise<void> {
    stats.scans++;
    const entries = await io.list(dir).catch(() => null);
    if (!entries) return;
    known.add(dir.toLowerCase());
    const { files, dirs } = split(entries);
    await loadMany(room(files).map((f) => f.path));
    for (const d of dirs) await scanDir(d.path);
  }

  /** The root's entry when it is a plain folder reached through plain folders only: a `docs` or `.kiro` that is a link may lead
   *  out of the project, and so may the root itself. */
  async function plainRoot(root: string, listings: Map<string, SpecEntry[] | null>): Promise<SpecEntry | null> {
    let dir = "";
    for (const name of root.split("/")) {
      if (!listings.has(dir)) {
        stats.scans++;
        listings.set(dir, await io.list(dir).catch(() => null));
      }
      const hit = listings.get(dir)?.find((e) => same(e.name, name));
      if (!hit?.is_dir || hit.is_symlink) return null;
      dir = hit.path;
    }
    return { name: baseOf(dir), path: dir, is_dir: true, is_symlink: false };
  }

  async function scanAll(): Promise<void> {
    docs.clear();
    known.clear();
    capped = false;
    const listings = new Map<string, SpecEntry[] | null>();
    for (const root of SPEC_ROOTS) {
      const hit = await plainRoot(root, listings);
      if (hit) await scanDir(hit.path);
    }
  }

  function purge(dir: string) {
    const low = dir.toLowerCase();
    for (const p of [...docs.keys()]) if (p.toLowerCase().startsWith(low + "/")) docs.delete(p);
    for (const k of [...known]) if (k === low || k.startsWith(low + "/")) known.delete(k);
  }

  async function reloadDir(dir: string): Promise<void> {
    // Only folders already walked: a new one is found through its parent, whose listing is checked for symlinks.
    if (!known.has(dir.toLowerCase())) return;
    // A folder that has become a link since it was walked is not read through it.
    if (!(await plainPath(io, dir, true))) return purge(dir);
    stats.scans++;
    const entries = await io.list(dir).catch(() => null);
    if (!entries) return purge(dir);
    const { files, dirs } = split(entries);
    const keep = new Set(files.map((f) => f.path.toLowerCase()));
    for (const p of [...docs.keys()]) if (same(parentOf(p), dir) && !keep.has(p.toLowerCase())) docs.delete(p);
    await loadMany(room(files).map((f) => f.path));
    const present = new Set(dirs.map((d) => d.path.toLowerCase()));
    for (const k of [...known]) if (same(parentOf(k), dir) && !present.has(k)) purge(k);
    for (const d of dirs) if (!known.has(d.path.toLowerCase())) await scanDir(d.path);
  }

  /** Jobs run one at a time, and a job that started before `stop` throws its result away. */
  function enqueue(job: () => Promise<void>): Promise<void> {
    const mine = gen;
    queue = queue.then(async () => {
      if (!active || mine !== gen) return;
      try {
        await job();
      } catch {
        /* a folder vanished mid-walk: the next event reconciles */
      }
      if (active && mine === gen) emit();
    });
    return queue;
  }

  /** Roots that were not there when walked (a new `specs/`, `docs/plans/`, `.kiro/specs/`): looked for in the parent's listing, which also says whether it is a link. */
  async function findRoots(): Promise<void> {
    const listings = new Map<string, SpecEntry[] | null>();
    for (const root of SPEC_ROOTS) {
      if (known.has(root)) continue;
      const hit = await plainRoot(root, listings);
      if (hit) await scanDir(hit.path);
    }
  }

  function flush() {
    timer = undefined;
    const dirs = [...pending];
    pending.clear();
    const roots = lookForRoots;
    lookForRoots = false;
    if (dirs.length || roots) void enqueue(async () => {
      if (dirs.length > RESCAN_AFTER) return scanAll();
      if (roots) await findRoots();
      for (const d of dirs) await reloadDir(d);
    });
  }

  /** The folder `dir` is, or holds, a root that has not been walked: the shallow event for the project root, `docs` or `.kiro`, or one for the new root itself. */
  const mayHoldNewRoot = (dir: string) => {
    const low = dir.toLowerCase();
    const inside = rootOf(low);
    return (inside !== null && !known.has(inside)) || SPEC_ROOTS.some((r) => !known.has(r) && parentOf(r) === low);
  };

  function onFs(dirs: string[]) {
    if (!active) return;
    const hits = dirs.filter(inRoots);
    const roots = dirs.some(mayHoldNewRoot);
    if (!hits.length && !roots) return;
    for (const d of hits) pending.add(d);
    lookForRoots ||= roots;
    clearTimeout(timer);
    timer = setTimeout(flush, debounceMs);
  }

  return {
    /** Scans and follows the watcher. Idempotent. */
    start() {
      if (active) return;
      active = true;
      gen++;
      unwatch = io.watch(onFs);
      void enqueue(scanAll);
    },
    /** Stops watching and drops what is pending; the tab is closed and costs nothing from here. */
    stop() {
      active = false;
      gen++;
      clearTimeout(timer);
      timer = undefined;
      pending.clear();
      lookForRoots = false;
      unwatch?.();
      unwatch = undefined;
    },
    /** Rescans everything now. */
    refresh: () => enqueue(scanAll),
    /** Resolves when the queued work is done. */
    settled: () => queue,
    docs: () => [...docs.values()],
    capped: () => capped,

    /** Sets or clears a task's box in the file, if the file is still as it was parsed. "stale": it changed (nothing was written; the file is read again). */
    async tick(doc: SpecDoc, task: SpecTask, checked: boolean): Promise<TickResult> {
      if (ticking.has(doc.path)) return "busy";
      ticking.add(doc.path);
      const mine = gen;
      try {
        const stale = async (): Promise<TickResult> => {
          // Dropped first: an edit that kept the size and time would otherwise look unchanged.
          docs.delete(doc.path);
          await enqueue(async () => reloadDir(parentOf(doc.path)));
          return "stale";
        };
        if (!(await plainPath(io, doc.path))) return "failed";
        stats.reads++;
        const fresh = await io.read(doc.path);
        const text = fresh && !fresh.truncated ? decodeText(fresh.bytes) : null;
        if (!fresh || text === null) return "failed";
        if (fresh.mtime !== doc.mtime || digestOf(text) !== doc.digest) return stale();
        const next = tickText(text, task, checked);
        if (next === null) return stale();
        const res = await io.write(doc.path, next, fresh.mtime, cksumOf(fresh.bytes));
        if (!res.ok) return res.conflict ? stale() : "failed";
        if (active && mine === gen) {
          docs.set(doc.path, parseSpec(doc.path, next, res.mtime, new TextEncoder().encode(next).length));
          emit();
        }
        return "ok";
      } catch {
        return "failed";
      } finally {
        ticking.delete(doc.path);
      }
    },

    /** Writes `specs/<name>/` with requirements, design and tasks. Never overwrites: an existing file, or a symlink in the way, stops it before anything is written. */
    async createSpec(input: string): Promise<{ dir: string; files: string[] }> {
      const slug = specSlug(input);
      if (!slug) throw new SpecError("name", "name");
      const dir = `specs/${slug}`;
      const top = await io.list("");
      const specs = top?.find((e) => same(e.name, "specs"));
      if (specs && (specs.is_symlink || !specs.is_dir)) throw new SpecError("symlink", "specs");
      const mine = specs ? (await io.list(specs.path))?.find((e) => same(e.name, slug)) : undefined;
      if (mine && (mine.is_symlink || !mine.is_dir)) throw new SpecError("symlink", dir);
      const files = SPEC_FILES.map((f) => `${mine?.path ?? dir}/${f}`);
      // The listing, not a read, says what is there: a link whose target is missing cannot be read, yet creating the
      // file would create (and write) its target, wherever that is.
      const taken = mine ? ((await io.list(mine.path)) ?? []).filter((e) => SPEC_FILES.some((f) => same(f, e.name))) : [];
      const odd = taken.find((e) => e.is_symlink || e.is_dir);
      if (odd) throw new SpecError("symlink", odd.path);
      if (taken.length) throw new SpecError("exists", taken.sort((a, b) => SPEC_FILES.findIndex((f) => same(f, a.name)) - SPEC_FILES.findIndex((f) => same(f, b.name)))[0].path);
      const text = specTemplates(input);
      for (const [i, f] of files.entries()) {
        try {
          await io.create(f);
          // Whatever the new file is by now, it is a plain file of this project or nothing is written to it.
          if (!(await plainPath(io, f))) throw new SpecError("symlink", f);
          const made = await io.read(f);
          const res = made ? await io.write(f, text[SPEC_FILES[i]], made.mtime, cksumOf(made.bytes)) : null;
          if (!res?.ok) throw new SpecError("failed", f);
        } catch (e) {
          throw e instanceof SpecError ? e : new SpecError("exists", f);
        }
      }
      // `specs/` may be new: the whole index is walked again.
      if (active) void enqueue(scanAll);
      return { dir: mine?.path ?? dir, files };
    },
  };
}
