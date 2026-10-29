// Git worktrees as child projects: create (with the repository's `.kelpie.yaml` setup), find
// existing ones, and remove one cleanly (its sessions stop first; the branch is kept).
import { parse as parseYaml } from "yaml";
import { store } from "./store";
import * as ipc from "./ipc";
import { t } from "./i18n";
import { basename, normPath } from "./platform";
import { disposeView } from "./terminal-host";
import { markWorktreeRemoved } from "./runs";
import type { Project, SessionKind } from "./types";

/** `.kelpie.yaml` in a repository root. */
export interface KelpieConfig {
  /** Commands typed into a "setup" terminal of every new worktree. */
  setup: string[];
  /** Sessions opened in every new worktree: an agent/shell kind, or `{ command }`. */
  tabs: ({ kind: SessionKind } | { command: string })[];
  /** Ignored files copied from the repository into every new worktree (like `.worktreeinclude`). */
  copy: string[];
  /** Ignored folders shared with the repository through a link (e.g. node_modules). */
  link: string[];
  /** Commands Finish runs on the merged result before the base branch moves. One program and its
   *  arguments each; no shell (no `&&`, pipes or variables). */
  check: string[];
  /** The commands that run when a worktree starts (setup, tabs, copy, link) as canonical text: what the user
   *  approved is stored as this text and compared for equality (no hash to forge). `check:` is approved on its
   *  own (see `checksApproved`). */
  setupKey: string;
  /** The file it came from: `.kelpie.yaml`, or `.erka.yaml` from before the rename. */
  file?: string;
}

/** shell, a core agent or any preset agent id (checked when the tab starts). */
const KIND_RE = /^[a-z][a-z0-9-]*$/;
const MAX_CONFIG = 64 * 1024;

/** Parses `.kelpie.yaml` text. Throws a readable message when it is invalid. */
export function parseKelpieConfig(text: string): KelpieConfig {
  if (text.length > MAX_CONFIG) throw new Error(t(".kelpie.yaml is larger than 64 KB"));
  let raw: any;
  try {
    raw = parseYaml(text) ?? {};
  } catch (e) {
    throw new Error(t(".kelpie.yaml: {error}", { error: String(e).split("\n")[0] }));
  }
  const list = (v: unknown) => (v == null ? [] : Array.isArray(v) ? v : [v]);
  const setup = list(raw.setup).map(String).filter((c) => c.trim());
  const tabs: KelpieConfig["tabs"] = [];
  for (const tab of list(raw.tabs)) {
    if (typeof tab === "string" && KIND_RE.test(tab) && tab !== "custom") tabs.push({ kind: tab as SessionKind });
    else if (tab && typeof tab === "object" && typeof tab.command === "string" && tab.command.trim()) tabs.push({ command: tab.command.trim() });
    else throw new Error(t(".kelpie.yaml: unknown tab {tab} (use an agent such as claude, shell, or { command: … })", { tab: JSON.stringify(tab) }));
  }
  const paths = (v: unknown) => list(v).map(String).map((x) => x.trim()).filter(Boolean);
  const copy = paths(raw.copy);
  const link = paths(raw.link);
  return { setup, tabs, copy, link, check: list(raw.check).map(String).map((c) => c.trim()).filter(Boolean), setupKey: JSON.stringify([setup, tabs, copy, link]) };
}

/**
 * The `.kelpie.yaml` a new worktree based on `base` will contain — read from that commit, not
 * from the repository's working copy, so what the user reviews is what would run.
 */
export async function configAtRef(repo: Project, base: string): Promise<KelpieConfig | null> {
  // `.erka.yaml` is the name from before the rename to Kelpie; repositories may still have it.
  for (const name of [".kelpie.yaml", ".erka.yaml"]) {
    const text = await ipc.gitShowFile(repo.path, base || "HEAD", name);
    if (text !== null) return { ...parseKelpieConfig(text), file: name };
  }
  return null;
}

/** Whether these exact worktree-start commands (setup, tabs, copy, link) were approved for the repository before. */
export const configApproved = (repo: Project, config: KelpieConfig) => store.state.approvedSetups?.[normPath(repo.path)] === config.setupKey;

/** Whether exactly this `check:` list was approved for the repository before. */
export const checksApproved = (repo: Project, config: KelpieConfig) => {
  const approved = store.state.approvedChecks?.[normPath(repo.path)];
  return !!approved && approved.length === config.check.length && approved.every((c, i) => c === config.check[i]);
};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Runs `.kelpie.yaml` for a project: setup commands in a "setup" terminal, then default tabs. */
export async function applyKelpieConfig(project: Project, config: KelpieConfig) {
  if (config.setup.length) {
    const s = await store.newSession(project.id, "shell", { name: "setup" });
    // One command per line, as if typed; the shell runs them in order.
    if (s) await ipc.sendInput(s.handle, config.setup.join("\r") + "\r");
  }
  for (const t of config.tabs) {
    if ("kind" in t) await store.newSession(project.id, t.kind);
    else await store.newSession(project.id, "custom", { args: (t.command.match(/"[^"]*"|\S+/g) ?? []).map((a) => a.replace(/^"|"$/g, "")), name: t.command });
  }
}

const BRANCH_RE = /^(?!-)(?!.*\.\.)(?!.*\/\/)(?!.*\.lock$)[A-Za-z0-9._\/-]+(?<![./])$/;
export const validBranch = (b: string) => BRANCH_RE.test(b);

/** Suggested folder for a worktree: `<repo parent>/<repo>-worktrees/<branch>`. */
export function worktreeFolder(repo: Project, branch: string): string {
  const sep = repo.path.includes("\\") ? "\\" : "/";
  const parent = repo.path.replace(/[\\/]+$/, "").replace(/[\\/][^\\/]+$/, "");
  return [parent, `${basename(repo.path)}-worktrees`, branch.replace(/[\\/]/g, "-")].join(sep);
}

/**
 * Creates the worktree and its project. `config` (already shown to and approved by the user)
 * runs afterwards; its failures are reported, not thrown — the worktree exists by then.
 * `existing`: the worktree checks out the existing local branch `branch` (no new branch, no base).
 */
export async function createWorktree(repo: Project, opts: { branch: string; base?: string; path: string; config: KelpieConfig | null; existing?: boolean }) {
  // A new branch's name must be a safe one; an existing branch is checked by the native side (`refs/heads/` only).
  if (!opts.existing && !validBranch(opts.branch)) throw new Error(t("\"{name}\" is not a valid branch name", { name: opts.branch }));
  // The branch this one forks from is what Finish merges into later (the main checkout may be on another branch by then).
  const requested = opts.existing ? undefined : opts.base?.trim();
  let baseBranch: string | undefined;
  if (!requested || requested === "HEAD") baseBranch = store.projectInfo[repo.id]?.branch ?? undefined;
  else if ((await ipc.gitRefs(repo.path).catch(() => [])).some((r) => r.kind === "local" && r.name === requested)) baseBranch = requested;
  if (baseBranch === opts.branch) baseBranch = undefined;
  await ipc.gitWorktreeAdd(repo.path, opts.path.trim(), opts.branch, requested || undefined, opts.existing || undefined);
  // Ignored files a checkout lacks (.env, node_modules): before setup runs, which may need them. Only
  // with the repository's `.kelpie.yaml` approved (`.worktreeinclude` too): without it the worktree is a
  // clean checkout.
  const files = opts.config
    ? await ipc.worktreeMaterialize(repo.path, opts.path.trim(), opts.config.copy, opts.config.link).catch((e) => {
        store.notify(t("Copying ignored files failed: {error}", { error: String(e) }), undefined, 8000);
        return null;
      })
    : null;
  const id = store.addProject(opts.path.trim(), { parentId: repo.id, branch: opts.branch, baseBranch, name: opts.branch });
  const project = store.state.projects.find((p) => p.id === id)!;
  const parts = [files?.copied.length ? t("copied {files}", { files: files.copied.join(", ") }) : "", files?.linked.length ? t("linked {files}", { files: files.linked.join(", ") }) : ""].filter(Boolean);
  store.addInbox({ kind: "worktree", title: t("Worktree {name}", { name: opts.branch }), body: parts.length ? t("Created in {repo} · {details}", { repo: repo.name, details: parts.join(" · ") }) : t("Created in {repo}", { repo: repo.name }), projectId: id });
  if (files?.skipped.length) store.notify(t("Not brought into the worktree: {files}", { files: files.skipped.map((x) => `${x.path} (${x.reason})`).join("; ") }), undefined, 12000);
  if (opts.config) {
    void applyKelpieConfig(project, opts.config).catch((e) => store.notify(t(".kelpie.yaml: {error}", { error: String(e) })));
  }
  return project;
}

/** A branch name from the first words of a prompt: `fix-the-login-redirect`. */
export function branchFromPrompt(prompt: string, fallback: string): string {
  const words = prompt.toLowerCase().normalize("NFD").replace(/[\u0300-\u036f]/g, "").replace(/đ/g, "d").match(/[a-z0-9]+/g) ?? [];
  let slug = "";
  for (const w of words.slice(0, 6)) {
    if ((slug + "-" + w).length > 40 && slug) break;
    slug = slug ? `${slug}-${w}` : w.slice(0, 40);
  }
  return slug || fallback;
}

/** `name`, or `name-2`, `name-3`… when taken (case does not count: refs collide on Windows and macOS). */
export function uniqueName(name: string, taken: Iterable<string>, folderTaken: (name: string) => boolean = () => false): string {
  const all = new Set([...taken].map((n) => n.toLowerCase()));
  const free = (n: string) => !all.has(n.toLowerCase()) && !folderTaken(n);
  if (free(name)) return name;
  for (let i = 2; ; i++) if (free(`${name}-${i}`)) return `${name}-${i}`;
}

/**
 * A fresh worktree for an agent session, named from its first prompt. The repository's
 * `.kelpie.yaml` runs only when this exact file was approved before (nothing is asked here).
 */
export async function createAgentWorktree(repo: Project, prompt: string, kind: string): Promise<Project> {
  const stamp = new Date().toISOString().slice(5, 16).replace(/\D/g, "");
  const branches = await ipc.gitRefs(repo.path).then((r) => r.filter((x) => x.kind === "local").map((x) => x.name)).catch(() => []);
  const folders = new Set<string>();
  const wanted = branchFromPrompt(prompt, `${kind}-${stamp}`);
  // A folder left over from an earlier worktree would make `git worktree add` fail.
  for (let i = 1; i <= 20; i++) {
    const name = i === 1 ? wanted : `${wanted}-${i}`;
    if (await ipc.isDir(worktreeFolder(repo, name)).catch(() => false)) folders.add(name);
  }
  const branch = uniqueName(wanted, branches, (n) => folders.has(n));
  const config = await configAtRef(repo, "").catch(() => null);
  const approved = config && configApproved(repo, config) ? config : null;
  if (config && !approved) store.notify(t("This repository's .kelpie.yaml has not been approved: the worktree starts without its setup. Approve it from New worktree."), undefined, 8000);
  return createWorktree(repo, { branch, path: worktreeFolder(repo, branch), config: approved });
}

/** Adds the repository's other worktrees that are not in the sidebar yet; returns how many. */
export async function addExistingWorktrees(repo: Project): Promise<number> {
  const list = await ipc.gitWorktrees(repo.path);
  // The first entry is always the main worktree: only a project that *is* the main worktree
  // gets the others as children (not a subfolder of the repository, not a linked worktree).
  // Compare canonical forms: the project may be spelled with 8.3 short names.
  const canon = async (p: string) => normPath(await ipc.canonicalPath(p).catch(() => p));
  if (!list.length || (await canon(list[0].path)) !== (await canon(repo.path))) return 0;
  const known = await Promise.all(store.state.projects.map((p) => canon(p.path)));
  let added = 0;
  for (const w of list.slice(1)) {
    if (w.bare || w.prunable || known.includes(await canon(w.path))) continue;
    store.addProject(w.path, { parentId: repo.id, branch: w.branch ?? undefined, name: w.branch ?? basename(w.path), open: false });
    added++;
  }
  return added;
}

export interface RemovalCheck {
  /** Live branch (null when HEAD is detached). */
  branch: string | null;
  /** Uncommitted or untracked files. */
  changes: number;
  /** Commits only this worktree's HEAD reaches (lost with it). */
  orphanCommits: number;
}

/** What removing `wt` would lose, before anything is stopped. */
export async function checkRemoval(wt: Project): Promise<RemovalCheck> {
  const status = await ipc.gitStatus(wt.path);
  const branch = status.branch ?? null;
  const orphanCommits = branch ? 0 : await ipc.gitOrphanCommits(wt.path).catch(() => 0);
  return { branch, changes: status.files.length, orphanCommits };
}

/**
 * Stops every session belonging to the worktree or running inside its folder, and waits for
 * their processes to exit (Windows refuses to delete a folder a process still uses).
 */
export async function stopSessions(wt: Project, timeoutMs = 10_000) {
  const root = normPath(wt.path);
  const inside = (cwd: string) => normPath(cwd) === root || normPath(cwd).startsWith(root + "/");
  const mine = () => Object.values(store.sessions).filter((s) => s.project_id === wt.id || inside(s.cwd));
  const pids = mine().map((s) => s.pid).filter((p): p is number => typeof p === "number");
  for (const s of mine()) {
    disposeView(s.id);
    await ipc.daemon("session.close", { handle: s.handle }).catch(() => {});
  }
  const end = Date.now() + timeoutMs;
  while (Date.now() < end) {
    const alive = pids.length ? await ipc.processesAlive(pids).catch(() => []) : [];
    if (!mine().length && !alive.length) return true;
    await sleep(200);
  }
  return false;
}

/**
 * Removes a worktree the user confirmed (see `checkRemoval`): leaves it (so Kelpie stops
 * watching its folder), stops its sessions, then `git worktree remove`. The branch is never
 * deleted. If git already deleted the worktree but could not delete a folder still in use,
 * the record is pruned and the leftover folder reported.
 */
export async function removeWorktree(wt: Project, opts: { force: boolean }): Promise<{ leftover: boolean }> {
  const repo = store.state.projects.find((p) => p.id === wt.parentId);
  if (!repo) throw new Error(t("This worktree's repository is not in the sidebar"));
  if (store.state.activeProject === wt.id) store.switchProject(repo.id);
  if (!(await stopSessions(wt))) throw new Error(t("Some processes in this worktree did not stop; close them and try again"));
  // Where its notes are filed, asked while the folder exists; they are cleared under that exact key once it is gone.
  const noteKey = await store.noteKeyFor(wt);
  // The file watcher holds the folder open; leaving the project only stops it in the background.
  await ipc.unwatchRoot(wt.path).catch(() => {});
  for (let attempt = 0; ; attempt++) {
    try {
      await ipc.gitWorktreeRemove(repo.path, wt.path, opts.force);
      break;
    } catch (e) {
      const msg = String(e);
      if (/is not a working tree|not a working tree/i.test(msg)) {
        // An earlier attempt got as far as deleting git's record: finish its cleanup.
        await ipc.gitWorktreePrune(repo.path).catch(() => {});
        break;
      }
      if (attempt >= 5) throw e;
      await sleep(500);
    }
  }
  markWorktreeRemoved(wt.path);
  await store.clearNotesAt(wt.id, noteKey);
  store.removeProject(wt.id);
  // A deleted folder that something still had open lingers until that handle closes.
  for (let i = 0; i < 15; i++) {
    if (!(await ipc.isDir(wt.path).catch(() => false))) return { leftover: false };
    await sleep(200);
  }
  return { leftover: true };
}
