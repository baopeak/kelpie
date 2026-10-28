// Agent setup (Settings → Agent setup): MCP servers, instruction files, skills and the ACP registry.
// The app side (app/src-tauri/src/agent_config.rs) reads and writes the agents' files; this file
// holds the types, the calls and the pure logic the page needs: masking, provenance, the diff of
// a previewed change, and argument splitting.
import { invoke } from "@tauri-apps/api/core";
import { msg, t } from "./i18n";

export type Scope = "user" | "project";

export interface Target { agent: string; scope: Scope }

export interface McpView {
  name: string;
  transport: "stdio" | "http" | "sse" | "unknown";
  command: string;
  args: string[];
  url: string;
  /** Names only: the values never reach the page. */
  env_keys: string[];
  header_keys: string[];
  enabled: boolean;
  extra: string[];
}

export interface McpFile { agent: string; scope: Scope; path: string; exists: boolean; error: string | null; can_toggle: boolean; servers: McpView[] }

export interface InstructionFile { agent: string; scope: Scope; path: string; exists: boolean; bytes: number }
export interface SkillItem { kind: "skill" | "command" | "subagent"; agent: string; scope: Scope; name: string; description: string; path: string }

export interface Overview {
  project: string | null;
  writable: boolean;
  mcp: McpFile[];
  instructions: { files: InstructionFile[]; claude_imports_agents: boolean; gemini_reads_agents: boolean };
  skills: SkillItem[];
  elapsed_ms: number;
}

export interface NewServer { name: string; transport: "stdio" | "http" | "sse"; command: string; args: string[]; url: string }

export type Edit =
  | { kind: "toggle"; target: Target; name: string; enabled: boolean }
  | { kind: "remove"; target: Target; name: string }
  | { kind: "add"; targets: Target[]; server: NewServer }
  | { kind: "copy"; from: Target; name: string; to: Target[]; include_env: boolean; confirm_untrusted: boolean }
  | { kind: "instructions"; gemini: "setting" | "copy" }
  | { kind: "restore"; backup: string };

export interface FileDiff { path: string; real_path: string | null; created: boolean; removed: boolean; before: string[]; after: string[]; skipped_before: number; skipped_after: number; truncated: boolean }
/** Something to know about a change: a code the page translates, with its parameters. */
export interface Warn { code: string; params: Record<string, string> }
/** One variable or header of a project server, as its repository declares it. */
export interface UntrustedValue { key: string; value: string; reference: boolean; var: string | null; carried: boolean }
/** Everything a project server would run once copied to the user's own settings. */
export interface Untrusted {
  name: string;
  agent: string;
  argv: string[];
  url: string | null;
  cwd: string | null;
  env: UntrustedValue[];
  headers: UntrustedValue[];
  resolved: { from: string; to: string }[];
  /** Variables of the user's that the command line or URL reads. */
  arg_vars: string[];
}
export interface Preview { token: string; files: FileDiff[]; warnings: Warn[]; untrusted: Untrusted[]; needs_confirm: boolean; noop: boolean }
export interface Applied { files: { path: string; backup: string }[] }
export interface BackupInfo { id: string; path: string; at: number; existed: boolean; bytes: number }

/** An entry of the ACP registry as the app summarises it (nothing of it has run). */
export interface RegistryAgent {
  id: string;
  name: string;
  version: string;
  description: string;
  repository: string | null;
  distribution: string;
  command: string[];
  env_keys: string[];
  /** The exact version the command runs. */
  pinned: string | null;
  def: { id: string; label: string; bin: string; acp: string[] } | null;
  /** Why it cannot be saved, as a code (see `registryProblem`). */
  problem: string | null;
}

/** A registry agent the user saved; sent to the daemon with the plugin agents (`agent.set_custom`). */
export interface SavedRegistryAgent { id: string; label: string; bin: string; acp: string[]; version: string; registryId: string }

export const agentSetupOverview = (project: string | null) => invoke<Overview>("agent_setup_overview", { project });
export const agentSetupPreview = (project: string | null, edit: Edit) => invoke<Preview>("agent_setup_preview", { project, edit });
export const agentSetupApply = (project: string | null, edit: Edit, token: string) => invoke<Applied>("agent_setup_apply", { project, edit, token });
export const agentSetupBackups = () => invoke<BackupInfo[]>("agent_setup_backups");
export const agentSetupReveal = (project: string | null, path: string) => invoke<void>("agent_setup_reveal", { project, path });
export const agentRegistryFetch = () => invoke<RegistryAgent[]>("agent_registry_fetch");

// ---------- agents ----------

/** The agents with MCP files, in the order the page lists them. */
export const SETUP_AGENTS: { id: string; label: string }[] = [
  { id: "claude", label: "Claude Code" },
  { id: "codex", label: "Codex" },
  { id: "gemini", label: "Gemini" },
  { id: "opencode", label: "OpenCode" },
  { id: "cursor", label: "Cursor" },
];

export const agentLabel = (id: string) => SETUP_AGENTS.find((a) => a.id === id)?.label ?? id;

export const sameTarget = (a: Target, b: Target) => a.agent === b.agent && a.scope === b.scope;

// ---------- provenance and trust ----------

/** Where a server came from. A project's file arrives with the repository, so it is untrusted
 *  until the user says otherwise; a user-level file is the user's own. */
export function provenance(scope: Scope): { tag: Scope; untrusted: boolean } {
  return { tag: scope, untrusted: scope === "project" };
}

/** Copying a project server into a user-level file would skip the agent's own approval prompt. */
export const copyNeedsConfirm = (from: Scope, to: Scope[]) => from === "project" && to.includes("user");

/** Environment values may be written only to the user's own files, and only when asked for. */
export const envOptIn = (to: Scope[]) => to.includes("user");

/** The files a server can be copied to: every other MCP file that exists as a target. */
export const copyTargets = (files: McpFile[], from: Target): McpFile[] => files.filter((f) => !sameTarget(f, from));

// ---------- warnings ----------

const WARNINGS: Record<string, string> = {
  replaces: msg("{file} already has a server named {name}: it is replaced."),
  env_referenced: msg("{file}: environment values were not copied: {keys} now read the variable of the same name."),
  env_unexpressible: msg("{file}: {agent} cannot say how {keys} are set the way the source does, so they were left out and must be set again there."),
  reference_kept: msg("{file}: {keys} refer to variables in a way {agent} may not expand; they were copied as written."),
  env_not_carried: msg("{file}: the project server's environment and header entries were not copied: {keys}. Tick the box to copy them (the confirmation lists every value)."),
  headers_not_copied: msg("{file}: header values were not copied: {keys}."),
  not_carried: msg("{file}: not carried over: {keys}."),
  relative_resolved: msg("{file}: “{from}” is relative to the project. It now points at {to}, so it keeps running that file in every project."),
  secret_in_project_command: msg("{file}: the command or URL looks like it contains a secret, and a project file is shared with the repository."),
  untrusted_source: msg("This server comes from the project, which Kelpie cannot vouch for. Copying it to your user settings skips the agent's own approval prompt."),
  restore_note: msg("The file goes back to the saved copy. What it holds now is backed up first, so this can be undone too."),
  already_linked: msg("CLAUDE.md and AGENTS.md are already the same file (a link), so neither is changed."),
  already_same_text: msg("CLAUDE.md and AGENTS.md already say the same thing, so neither is changed."),
  claude_has_text: msg("CLAUDE.md has text of its own: it keeps it and now also imports AGENTS.md. Merge them by hand if they say the same thing."),
  gemini_exists: msg("GEMINI.md already exists and was left as it is; delete it first to replace it with a copy."),
};

/** A warning in the interface language; an unknown code shows as it is. */
export function warningText(w: Warn): string {
  const text = WARNINGS[w.code];
  return text ? t(text, w.params) : w.code;
}

const PROBLEMS: Record<string, string> = {
  downloaded_program: msg("Shipped as a downloaded program: Kelpie only saves entries started with npx or uvx."),
  command_form: msg("The entry's command is not in a form Kelpie will save."),
  id_long: msg("The entry's id is too long to save."),
  package_name: msg("The package is not a plain registry name (no URLs, paths or git specs), so it cannot be pinned."),
  version_not_exact: msg("The registry gives no exact version for this entry, so it cannot be pinned."),
};

/** Why a registry entry cannot be saved, in the interface language. */
export const registryProblem = (code: string) => (PROBLEMS[code] ? t(PROBLEMS[code]) : code);

/** An argument as it would be typed: quoted when it has spaces or quotes, so the boundaries show. */
export function quoteArg(arg: string): string {
  return arg === "" || /[\s"']/.test(arg) ? `"${arg.replace(/"/g, '\\"')}"` : arg;
}

// ---------- masking ----------

export const MASK = "••••••••";

const SECRET_NAME = /(token|secret|passw(or)?d|passwd|api[-_]?key|apikey|auth|bearer|credential|private[-_]?key|access[-_]?key)/i;

/** Credentials with a recognisable shape (the same families Kelpie's own redaction knows). */
const TOKEN_LIKE = /(?<![A-Za-z0-9_-])(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{20,}|sk-[A-Za-z0-9_-]{16,}|xox[abprs]-[A-Za-z0-9-]{10,}|AKIA[0-9A-Z]{16}|AIza[0-9A-Za-z_-]{30,}|glpat-[A-Za-z0-9_-]{15,}|eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,})/g;

/** `KEY=••••` for an environment table: names are shown, values never are. */
export const maskedEnv = (keys: string[]) => keys.map((k) => `${k}=${MASK}`);

/** One command-line argument with anything that looks like a secret hidden: `--token=abc`,
 *  `API_KEY=abc`, a password in a URL, a token in its query. */
export function maskArg(raw: string): string {
  const arg = raw.replace(TOKEN_LIKE, MASK);
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(arg)) {
    const noPassword = arg.replace(/^([a-z][a-z0-9+.-]*:\/\/[^\s/@:]*):[^\s/@]*@/i, `$1:${MASK}@`);
    const q = noPassword.indexOf("?");
    if (q < 0) return noPassword;
    const query = noPassword.slice(q + 1).split("&").map((p) => {
      const [k, ...v] = p.split("=");
      return v.length && (SECRET_NAME.test(k) || /^(key|sig|signature)$/i.test(k)) ? `${k}=${MASK}` : p;
    });
    return `${noPassword.slice(0, q)}?${query.join("&")}`;
  }
  const eq = arg.indexOf("=");
  if (eq > 0 && SECRET_NAME.test(arg.slice(0, eq).replace(/^-+/, ""))) return `${arg.slice(0, eq)}=${MASK}`;
  return arg;
}

/** The arguments of a command with secrets hidden, including the value after a bare
 *  `--token`-style flag. */
export function maskArgs(args: string[]): string[] {
  const out: string[] = [];
  for (let i = 0; i < args.length; i++) {
    const a = args[i];
    out.push(maskArg(a));
    if (/^--?[A-Za-z][\w-]*$/.test(a) && SECRET_NAME.test(a) && i + 1 < args.length && !args[i + 1].startsWith("-")) {
      out.push(MASK);
      i++;
    }
  }
  return out;
}

/** A server's command or URL as shown on the page. */
export function commandText(s: Pick<McpView, "transport" | "command" | "args" | "url">): string {
  return s.transport === "stdio" ? [s.command, ...maskArgs(s.args)].join(" ") : maskArg(s.url);
}

// ---------- diff ----------

export interface DiffLine { op: " " | "+" | "-" | "…"; text: string }

/** Beyond this many cells the middle is shown as removed then added, not aligned. */
const LCS_CELLS = 4_000_000;

/** Line diff of two windows of a file: shared start and end are kept as they are, the middle is
 *  aligned by longest common subsequence. */
export function diffLines(before: string[], after: string[]): DiffLine[] {
  let start = 0;
  while (start < before.length && start < after.length && before[start] === after[start]) start++;
  let end = 0;
  while (end < before.length - start && end < after.length - start && before[before.length - 1 - end] === after[after.length - 1 - end]) end++;
  const a = before.slice(start, before.length - end);
  const b = after.slice(start, after.length - end);
  const out: DiffLine[] = before.slice(0, start).map((text) => ({ op: " ", text }));
  if (a.length * b.length > LCS_CELLS) {
    out.push(...a.map((text): DiffLine => ({ op: "-", text })), ...b.map((text): DiffLine => ({ op: "+", text })));
  } else {
    const w = b.length + 1;
    const len = new Uint32Array((a.length + 1) * w);
    for (let i = a.length - 1; i >= 0; i--) {
      for (let j = b.length - 1; j >= 0; j--) {
        len[i * w + j] = a[i] === b[j] ? len[(i + 1) * w + j + 1] + 1 : Math.max(len[(i + 1) * w + j], len[i * w + j + 1]);
      }
    }
    let i = 0, j = 0;
    while (i < a.length && j < b.length) {
      if (a[i] === b[j]) { out.push({ op: " ", text: a[i] }); i++; j++; }
      else if (len[(i + 1) * w + j] >= len[i * w + j + 1]) out.push({ op: "-", text: a[i++] });
      else out.push({ op: "+", text: b[j++] });
    }
    while (i < a.length) out.push({ op: "-", text: a[i++] });
    while (j < b.length) out.push({ op: "+", text: b[j++] });
  }
  out.push(...before.slice(before.length - end).map((text): DiffLine => ({ op: " ", text })));
  return out;
}

/** Long runs of unchanged lines folded into one marker, `context` lines kept next to a change. */
export function foldUnchanged(lines: DiffLine[], context = 3): DiffLine[] {
  const keep = new Array<boolean>(lines.length).fill(false);
  lines.forEach((l, i) => {
    if (l.op === " ") return;
    for (let k = Math.max(0, i - context); k <= Math.min(lines.length - 1, i + context); k++) keep[k] = true;
  });
  const changed = lines.some((l) => l.op !== " ");
  const out: DiffLine[] = [];
  let hidden = 0;
  lines.forEach((l, i) => {
    if (keep[i] || !changed) {
      if (hidden) out.push({ op: "…", text: String(hidden) });
      hidden = 0;
      out.push(l);
    } else hidden++;
  });
  if (hidden) out.push({ op: "…", text: String(hidden) });
  return out;
}

/** (+added, −removed) lines of a file's change. */
export function diffCounts(lines: DiffLine[]): { added: number; removed: number } {
  return { added: lines.filter((l) => l.op === "+").length, removed: lines.filter((l) => l.op === "-").length };
}

// ---------- arguments ----------

/** Splits a typed command line into arguments: spaces separate them, quotes keep spaces in one. */
export function splitArgs(line: string): string[] {
  const out: string[] = [];
  let cur = "";
  let quote: string | null = null;
  let has = false;
  for (const c of line) {
    if (quote) {
      if (c === quote) quote = null;
      else cur += c;
    } else if (c === '"' || c === "'") {
      quote = c;
      has = true;
    } else if (/\s/.test(c)) {
      if (cur || has) out.push(cur);
      cur = "";
      has = false;
    } else cur += c;
  }
  if (cur || has) out.push(cur);
  return out;
}

// ---------- registry ----------

/** The full command a registry entry would run, as one line. */
export const registryCommandLine = (a: Pick<RegistryAgent, "command">) => a.command.join(" ");

/** The saved form of an entry, or null when Kelpie cannot run it. */
export function savedFromRegistry(a: RegistryAgent): SavedRegistryAgent | null {
  if (!a.def) return null;
  return { id: a.def.id, label: a.def.label, bin: a.def.bin, acp: a.def.acp, version: a.version, registryId: a.id };
}

/** Size as "12 KB". */
export function sizeText(bytes: number): string {
  return bytes >= 1 << 20 ? `${(bytes / (1 << 20)).toFixed(1)} MB` : bytes >= 1024 ? `${Math.round(bytes / 1024)} KB` : `${bytes} B`;
}
