// How a tool call reads in the transcript: a verb, its target and a detail ("Read app.ts ·
// lines 75-80", "Edit main.rs +3 −1", "Bash · Run the tests"), from whatever the agent sent —
// Claude tool names and inputs, Codex command actions, ACP tool kinds.
import type { ChatItem, PlanEntry, ToolCategory } from "./chat-model";
import { t } from "../lib/i18n";

type Tool = Extract<ChatItem, { kind: "tool" }>;

export interface ToolView {
  category: ToolCategory;
  verb: string;
  target: string;
  detail?: string;
  /** Shell command, shown as the IN block. */
  command?: string;
}

export const basename = (p: string) => p.replace(/[\\/]+$/, "").split(/[\\/]/).pop() || p;

const str = (v: unknown) => (typeof v === "string" ? v : "");

/** Claude Code's tool names → category (other agents report one themselves). */
function claudeCategory(name: string): ToolCategory {
  if (/^(Read|LS|NotebookRead)$/.test(name)) return "read";
  if (/^(Glob|Grep|ToolSearch)$/.test(name)) return "search";
  if (/^(Edit|MultiEdit|Write|NotebookEdit)$/.test(name)) return "edit";
  if (/^(Bash|BashOutput|KillShell|KillBash|PowerShell|Monitor)$/.test(name)) return "command";
  if (/^(WebFetch|WebSearch)$/.test(name)) return "fetch";
  if (/^(Task|Agent)$/.test(name)) return "agent";
  if (/^(TodoWrite|ExitPlanMode)$/.test(name)) return "plan";
  return "other";
}

export function describeTool(item: Tool): ToolView {
  const view = describe(item);
  // While a call streams in, only its summary (picked from the partial input) is known.
  return view.target ? view : { ...view, target: item.summary };
}

function describe(item: Tool): ToolView {
  const input = (item.input ?? {}) as Record<string, any>;
  const category = item.category ?? claudeCategory(item.name);
  const path = str(input.file_path) || str(input.notebook_path) || str(input.path);
  switch (item.name) {
    case "Read": {
      const from = Number(input.offset) || 0;
      const lines = input.limit ? t("lines {from}-{to}", { from: String(from || 1), to: String((from || 1) + Number(input.limit) - 1) }) : from ? t("from line {n}", { n: String(from) }) : undefined;
      return { category, verb: t("Read"), target: basename(path), detail: lines };
    }
    case "Write":
      return { category, verb: t("Write"), target: basename(path), detail: t("{n} lines", { n: String(str(input.content).split("\n").length) }) };
    case "Edit":
    case "MultiEdit":
    case "NotebookEdit":
      return { category, verb: t("Edit"), target: basename(path) };
    case "Glob":
      return { category, verb: t("Search"), target: str(input.pattern), detail: path ? basename(path) : undefined };
    case "Grep":
      return { category, verb: t("Search"), target: str(input.pattern), detail: [str(input.glob), path && basename(path)].filter(Boolean).join(" · ") || undefined };
    case "Bash":
    case "PowerShell":
      return { category, verb: item.name, target: str(input.description) || str(input.command), command: str(input.command) };
    case "WebFetch":
      return { category, verb: t("Fetch"), target: str(input.url).replace(/^https?:\/\//, "") };
    case "WebSearch":
      return { category, verb: t("Web search"), target: str(input.query) || item.summary };
    case "Task":
    case "Agent":
      return { category, verb: t("Agent"), target: str(input.description) || item.summary, detail: str(input.subagent_type) || undefined };
    case "TodoWrite": {
      const todos: any[] = Array.isArray(input.todos) ? input.todos : [];
      return { category, verb: t("Update plan"), target: t("{done}/{n} done", { done: String(todos.filter((x) => x.status === "completed").length), n: String(todos.length) }) };
    }
    case "Skill":
      return { category, verb: t("Skill"), target: str(input.skill) || str(input.command) || item.summary };
  }
  const mcp = /^mcp__(.+?)__(.+)$/.exec(item.name);
  if (mcp) return { category, verb: `${mcp[1]} · ${mcp[2]}`, target: item.summary };
  // Codex shells and ACP calls carry their own category; their name is the agent's title.
  if (category === "command") return { category, verb: item.name === "Shell" ? t("Run") : item.name, target: item.summary, command: item.summary };
  if (category === "edit" && item.diff?.length) return { category, verb: t("Edit"), target: item.diff.map((d) => basename(d.path)).join(", ") };
  if (category === "read" || category === "search") return { category, verb: category === "read" ? t("Read") : t("Search"), target: item.summary || item.name };
  return { category, verb: item.name, target: item.summary };
}

/** Lines added / removed in a unified-style diff text. */
export function diffStats(text: string): { adds: number; dels: number } {
  let adds = 0;
  let dels = 0;
  for (const l of text.split("\n")) {
    if (l.startsWith("+") && !l.startsWith("+++")) adds++;
    else if (l.startsWith("-") && !l.startsWith("---")) dels++;
  }
  return { adds, dels };
}

/** Above this many line pairs, a replacement is shown whole instead of diffed. */
const DIFF_LIMIT = 1_000_000;

/** A line diff of `before` → `after` with `context` unchanged lines around each change, as
 *  prefixed lines (" ", "-", "+") and "@@" between distant hunks. */
export function lineDiff(before: string, after: string, context = 3): string {
  const all = { a: before.split("\n"), b: after.split("\n") };
  // Lines the two share at the start and the end need no table (most edits change a few lines).
  let head = 0;
  while (head < all.a.length && head < all.b.length && all.a[head] === all.b[head]) head++;
  let tail = 0;
  while (tail < all.a.length - head && tail < all.b.length - head && all.a[all.a.length - 1 - tail] === all.b[all.b.length - 1 - tail]) tail++;
  const a = all.a.slice(head, all.a.length - tail);
  const b = all.b.slice(head, all.b.length - tail);
  const ops: string[] = all.a.slice(0, head).map((l) => " " + l);
  const n = a.length;
  const m = b.length;
  if (n * m > DIFF_LIMIT) {
    ops.push(...a.map((l) => "-" + l), ...b.map((l) => "+" + l));
  } else {
    // Longest common subsequence table, from the end.
    const lcs: Uint32Array[] = Array.from({ length: n + 1 }, () => new Uint32Array(m + 1));
    for (let i = n - 1; i >= 0; i--) for (let j = m - 1; j >= 0; j--) lcs[i][j] = a[i] === b[j] ? lcs[i + 1][j + 1] + 1 : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    let i = 0;
    let j = 0;
    while (i < n || j < m) {
      if (i < n && j < m && a[i] === b[j]) {
        ops.push(" " + a[i++]);
        j++;
      } else if (i < n && (j >= m || lcs[i + 1][j] >= lcs[i][j + 1])) ops.push("-" + a[i++]);
      else ops.push("+" + b[j++]);
    }
  }
  ops.push(...all.a.slice(all.a.length - tail).map((l) => " " + l));
  // Keep changed lines and their context; elide the rest.
  const keep = ops.map(() => false);
  ops.forEach((op, k) => {
    if (op[0] !== " ") for (let x = Math.max(0, k - context); x <= Math.min(ops.length - 1, k + context); x++) keep[x] = true;
  });
  const out: string[] = [];
  let gap = false;
  ops.forEach((op, k) => {
    if (keep[k]) {
      if (gap && out.length) out.push("@@");
      gap = false;
      out.push(op);
    } else gap = true;
  });
  return out.join("\n");
}

/** Files a unified diff (`git diff` style, as Codex reports a turn) touches, with counts. */
export function unifiedFiles(diff: string): { path: string; adds: number; dels: number }[] {
  const files: { path: string; adds: number; dels: number }[] = [];
  let cur: { path: string; adds: number; dels: number } | null = null;
  for (const l of diff.split("\n")) {
    const head = /^diff --git a\/(.+?) b\/(.+)$/.exec(l);
    if (head) {
      cur = { path: head[2], adds: 0, dels: 0 };
      files.push(cur);
    } else if (cur && l.startsWith("+") && !l.startsWith("+++")) cur.adds++;
    else if (cur && l.startsWith("-") && !l.startsWith("---")) cur.dels++;
  }
  return files;
}

/** Plan entry status from any agent's spelling. */
export function planStatus(s: unknown): PlanEntry["status"] {
  return s === "completed" ? "done" : s === "in_progress" || s === "inProgress" ? "active" : "pending";
}
