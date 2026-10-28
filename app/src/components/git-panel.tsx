import { For, Show, createMemo, createResource, createSignal, onMount } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { store } from "../lib/store";
import { agentRun, ghDefaultBranch, ghPrCreate, gitAbort, gitCommit, gitContext, gitDiscard, gitRemote, gitRepoState, gitStage, type RemoteAction } from "../lib/ipc";
import { gitState } from "../lib/git-state";
import { prLabel, pullRequests } from "../lib/pull-requests";
import { gitlabDefaultBranch, gitlabMrCreate } from "../lib/integrations";
import { isMac } from "../lib/platform";
import { Icon } from "./icons";
import type { GitFile, Project } from "../lib/types";
import { t } from "../lib/i18n";
import { confirmDialog } from "../lib/confirm-dialog";
import { overlapsOf } from "../lib/review-queue";

function split(path: string) {
  const i = path.lastIndexOf("/");
  return { name: path.slice(i + 1), dir: i > 0 ? path.slice(0, i) : "" };
}

const COMMIT_PROMPT = `Write a git commit message for the changes below, in the conventional commit style
(type(scope): summary). First line at most 72 characters; then, only if useful, a blank line and a
short body explaining why. Reply with the message only — no quotes, no code fences, no commentary.

`;

const PR_PROMPT = `Write a GitHub pull request title and description for the branch below.
Reply in exactly this form, nothing else:
TITLE: <one line, at most 70 characters>
BODY:
<markdown: a short "Problem" and "Solution" section, then a bullet list of notable changes>

`;

/** Strips fences / quotes a model may wrap its answer in. */
const unwrap = (text: string) => text.replace(/^```[a-z]*\n?|\n?```$/g, "").replace(/^"([\s\S]*)"$/, "$1").trim();

export function GitPanel(props: { project: Project }) {
  const [message, setMessage] = createSignal("");
  const [busy, setBusy] = createSignal<string | null>(null);
  const [amend, setAmend] = createSignal(false);
  const [prForm, setPrForm] = createSignal(false);
  const st = () => gitState.status();
  const [repo, { refetch: refetchRepo }] = createResource(() => props.project.path, (root) => gitRepoState(root).catch(() => null));
  const staged = createMemo(() => (st()?.files ?? []).filter((f) => f.index !== "." && f.index !== "?" && f.index !== "U"));
  const changes = createMemo(() => (st()?.files ?? []).filter((f) => f.worktree !== "." || f.index === "?" || f.index === "U"));
  const conflicts = createMemo(() => (st()?.files ?? []).filter((f) => f.index === "U"));
  const root = () => props.project.path;
  const pr = () => pullRequests.prs[props.project.id];

  onMount(() => void pullRequests.refresh(props.project.id, root()));

  const run = async (label: string, fn: () => Promise<unknown>) => {
    setBusy(label);
    try {
      await fn();
    } catch (e) {
      store.notify(String(e), undefined, 8000);
    } finally {
      setBusy(null);
      void gitState.refresh();
      void refetchRepo();
      store.refreshProjectInfo(props.project.id, 0);
    }
  };

  /** Starts the AI agent with a repair prompt (the agent fixes; it does not commit). */
  const fixWithAi = (what: string, output: string) => {
    const files = (staged().length ? staged() : changes()).map((f) => f.path).join("\n");
    const prompt = `${what} failed in this repository. Find and fix the cause; do not commit, push or bypass hooks.\n\nOutput:\n${output.slice(0, 6000)}\n\nFiles involved:\n${files}`;
    void store.newSession(props.project.id, store.state.settings.aiAgent ?? "claude", { prompt });
  };

  const commit = () =>
    run("commit", async () => {
      if (!message().trim() && !amend()) throw new Error(t("Enter a commit message"));
      if (!staged().length && !amend()) await gitStage(root(), changes().map((f) => f.path), true);
      try {
        await gitCommit(root(), message().trim(), amend());
      } catch (e) {
        store.notify(t("Commit failed: {error}", { error: String(e).split("\n").slice(-3).join(" ") }), { label: t("Fix with AI"), run: () => fixWithAi("git commit", String(e)) }, 12000);
        return;
      }
      setMessage("");
      setAmend(false);
      store.notify(t("Committed"));
    });

  const generateMessage = () =>
    run("generate", async () => {
      const diff = await gitContext(root(), "staged");
      if (!diff.trim()) throw new Error(t("No changes to describe"));
      setMessage(unwrap(await agentRun(store.state.settings.aiAgent ?? "claude", COMMIT_PROMPT + diff, root(), store.profileLaunch(store.state.settings.aiAgent ?? "claude").env)));
    });

  const remote = (action: RemoteAction) =>
    run(action, async () => {
      if (action === "force_push" && !(await confirmDialog(t("Force-push {branch} with lease? Commits on the remote that you do not have locally make it fail instead of being overwritten.", { branch: st()?.branch ?? "" })))) return;
      const out = await gitRemote(root(), action);
      store.notify(out.split("\n").filter(Boolean).slice(-1)[0] || t("Done"));
      if (action !== "pull") void pullRequests.refresh(props.project.id, root());
    });

  const discard = async (files: GitFile[]) => {
    if (!(await confirmDialog(t("Discard changes in {n} file(s)? Untracked files go to the trash.", { n: files.length })))) return;
    void run("discard", () => gitDiscard(root(), files.filter((f) => f.index !== "?").map((f) => f.path), files.filter((f) => f.index === "?").map((f) => f.path)));
  };

  const Row = (p: { f: GitFile; staged: boolean }) => {
    const letter = () => (p.staged ? p.f.index : p.f.index === "?" ? "U" : p.f.index === "U" ? "C" : p.f.worktree);
    const parts = split(p.f.path);
    return (
      <div class="list-row" onClick={() => store.openFile(props.project.id, p.f.path, { diff: true, staged: p.staged })} title={p.f.path}>
        <span class={`ficon g-${letter()}`} style={{ "font-family": "var(--font-mono)", "font-size": "11px" }}>{letter()}</span>
        <span class="name">{parts.name}</span>
        <span class="dir">{parts.dir}</span>
        <span class="row-actions">
          <button class="icon-btn" title={t("Open file")} onClick={(e) => { e.stopPropagation(); store.openFile(props.project.id, p.f.path); }}><Icon name="files" size={13} /></button>
          <Show when={!p.staged}>
            <button class="icon-btn" title={t("Discard")} onClick={(e) => { e.stopPropagation(); discard([p.f]); }}><Icon name="undo" size={13} /></button>
          </Show>
          <button class="icon-btn" title={p.staged ? t("Unstage") : t("Stage")} onClick={(e) => { e.stopPropagation(); void run("stage", () => gitStage(root(), [p.f.path], !p.staged)); }}>
            <Icon name={p.staged ? "minus" : "plus"} size={13} />
          </button>
        </span>
      </div>
    );
  };

  const ahead = () => st()?.ahead ?? 0;
  const behind = () => st()?.behind ?? 0;

  return (
    <Show when={st()?.is_repo !== false} fallback={<div class="muted">{t("Not a git repository.")}</div>}>
      <div class="git-head">
        <div class="branch">
          <Icon name="git" size={14} />
          <span class="branch-name" title={st()?.upstream ? `${st()?.branch} → ${st()?.upstream}` : st()?.branch ?? ""}>{st()?.branch ?? "…"}</span>
          <Show when={st()?.upstream}><span style={{ color: "var(--text-faint)" }}>↑{ahead()} ↓{behind()}</span></Show>
          <span style={{ flex: 1 }} />
          <Show when={repo()?.has_remote}>
            <button class="icon-btn" title={t("Pull (fast-forward only)")} disabled={!!busy() || !st()?.upstream} onClick={() => remote("pull")}><Icon name="pull" size={14} /></button>
            <button class="icon-btn" title={st()?.upstream ? t("Push") : t("Publish branch (push and set upstream)")} disabled={!!busy()} onClick={() => remote("push")}><Icon name="push" size={14} /></button>
            <button class="icon-btn" title={t("Sync (pull, then push)")} disabled={!!busy()} onClick={() => remote("sync")}><Icon name="sync" size={14} /></button>
          </Show>
          <button class="icon-btn" title={t("Refresh")} onClick={() => { void gitState.refresh(); void refetchRepo(); void pullRequests.refresh(props.project.id, root()); }}><Icon name="refresh" size={14} /></button>
        </div>
        <Show when={overlapsOf(props.project.id).length}>
          <div class="git-note warn">
            <For each={overlapsOf(props.project.id)}>
              {(o) => <div title={o.files.join("\n")}><b>{t("Also changed in {name}", { name: o.name })}</b>: {o.files.slice(0, 4).join(", ")}{o.files.length > 4 ? ` +${o.files.length - 4}` : ""}</div>}
            </For>
          </div>
        </Show>
        <Show when={ahead() > 0 && behind() > 0}>
          <div class="git-note">
            {t("The branch and its upstream have diverged (history rewritten?).")}
            <button class="link-btn" disabled={!!busy()} onClick={() => remote("force_push")}>{t("Force push with lease ({ahead} replace {behind})", { ahead: ahead(), behind: behind() })}</button>
          </div>
        </Show>
        <Show when={repo()?.operation}>
          {(op) => (
            <div class="git-note warn">
              <b>{t("{op} in progress", { op: op() })}</b>{conflicts().length ? ` · ${t("{n} conflicted file(s)", { n: conflicts().length })}` : ""}. {t("Resolve, stage, then continue in a terminal")} (<code>git {op()} --continue</code>).
              <div style={{ display: "flex", gap: "6px", "margin-top": "6px" }}>
                <button class="btn" disabled={!!busy()} onClick={async () => (await confirmDialog(t("Abort the {op}? Its changes are dropped.", { op: op() }))) && run("abort", () => gitAbort(root()).then((m) => store.notify(m)))}>{t("Abort {op}", { op: op() })}</button>
                <Show when={conflicts().length}>
                  <button class="btn" onClick={() => fixWithAi(`git ${op()}`, `Conflicts in:\n${conflicts().map((f) => f.path).join("\n")}\nResolve every conflict (keep both sides' intent), then stage the files.`)}><Icon name="sparkle" size={13} /> {t("Resolve with AI")}</button>
                </Show>
              </div>
            </div>
          )}
        </Show>
        <div class="commit-wrap">
          <textarea
            class="commit-box"
            placeholder={staged().length ? t("Commit message ({key}+Enter to commit)", { key: isMac ? "⌘" : "Ctrl" }) : t("Commit message ({key}+Enter to commit all changes)", { key: isMac ? "⌘" : "Ctrl" })}
            value={message()}
            onInput={(e) => setMessage(e.currentTarget.value)}
            onKeyDown={(e) => { if (e.key === "Enter" && (isMac ? e.metaKey : e.ctrlKey)) void commit(); }}
          />
          <button class="icon-btn commit-ai" title={t("Generate the message with {agent}", { agent: store.state.settings.aiAgent === "codex" ? "Codex" : "Claude Code" })} disabled={!!busy() || (!staged().length && !changes().length)} onClick={() => void generateMessage()}>
            <Icon name="sparkle" size={14} />
          </button>
        </div>
        <Show when={busy() === "generate"}><div class="muted" style={{ padding: 0 }}>{t("Drafting a message from the diff…")}</div></Show>
        <div style={{ display: "flex", gap: "8px", "align-items": "center" }}>
          <button class="btn primary" style={{ flex: 1 }} disabled={!!busy() || (!staged().length && !changes().length && !amend())} onClick={() => void commit()}>
            <Icon name="check" size={14} /> {amend() ? t("Amend last commit") : staged().length ? t("Commit {n} staged", { n: staged().length }) : t("Commit all")}
          </button>
          <label class="check-label" title={t("Rewrite the last commit with these changes (and this message, if any)")}>
            <input type="checkbox" checked={amend()} onChange={(e) => setAmend(e.currentTarget.checked)} /> {t("Amend")}
          </label>
        </div>
        <PullRequestRow project={props.project} pr={pr()} hasRemote={!!repo()?.has_remote} branch={st()?.branch ?? null} open={prForm()} setOpen={setPrForm} />
      </div>
      <div class="list">
        <Show when={staged().length}>
          <div class="group-label">{t("Staged")} <span class="spacer" />
            <button class="icon-btn" style={{ width: "20px", height: "20px" }} title={t("Unstage all")} onClick={() => void run("stage", () => gitStage(root(), staged().map((f) => f.path), false))}><Icon name="minus" size={13} /></button>
          </div>
          <For each={staged()}>{(f) => <Row f={f} staged />}</For>
        </Show>
        <div class="group-label">{t("Changes")} <span style={{ "text-transform": "none" }}>{changes().length}</span><span class="spacer" />
          <Show when={changes().length}>
            <button class="icon-btn" style={{ width: "20px", height: "20px" }} title={t("Discard all")} onClick={() => discard(changes())}><Icon name="undo" size={13} /></button>
            <button class="icon-btn" style={{ width: "20px", height: "20px" }} title={t("Stage all")} onClick={() => void run("stage", () => gitStage(root(), changes().map((f) => f.path), true))}><Icon name="plus" size={13} /></button>
          </Show>
        </div>
        <For each={changes()}>{(f) => <Row f={f} staged={false} />}</For>
        <Show when={st() && !changes().length && !staged().length}><div class="muted">{t("Working tree clean.")}</div></Show>
        <Show when={st()?.truncated}><div class="muted">{t("Showing the first 5000 changes.")}</div></Show>
      </div>
    </Show>
  );
}

const prText = (pr: import("../lib/ipc").PullRequest) => {
  const l = prLabel(pr);
  return l === "merged" ? t("merged") : l === "closed" ? t("closed") : l === "draft" ? t("draft") : t("open");
};
const checksText = (c: string) => (c === "passing" ? t("passing") : c === "failing" ? t("failing") : c === "pending" ? t("pending") : c);

/** The branch's pull request, or a form to open one (GitHub, through `gh`). */
function PullRequestRow(props: { project: Project; pr: import("../lib/ipc").PullRequest | null | undefined; hasRemote: boolean; branch: string | null; open: boolean; setOpen: (v: boolean) => void }) {
  const [title, setTitle] = createSignal("");
  const [body, setBody] = createSignal("");
  const [base, setBase] = createSignal("");
  const [draft, setDraft] = createSignal(false);
  const [busy, setBusy] = createSignal<string | null>(null);
  const root = () => props.project.path;
  const gitlab = () => pullRequests.providers[props.project.id] === "gitlab";
  /** GitLab calls them merge requests (!12), GitHub pull requests (#12). */
  const noun = () => (gitlab() ? t("merge request") : t("pull request"));

  const openForm = async () => {
    props.setOpen(true);
    if (!base()) setBase((await (gitlab() ? gitlabDefaultBranch(root()) : ghDefaultBranch(root())).catch(() => null)) ?? "main");
  };

  const generate = async () => {
    setBusy("generate");
    try {
      const ctx = await gitContext(root(), "branch", base() || "main");
      const text = unwrap(await agentRun(store.state.settings.aiAgent ?? "claude", PR_PROMPT + ctx, root(), store.profileLaunch(store.state.settings.aiAgent ?? "claude").env));
      const m = text.match(/TITLE:\s*(.+)\n+BODY:\s*\n?([\s\S]*)/i);
      setTitle((m?.[1] ?? text.split("\n")[0]).trim());
      setBody((m?.[2] ?? text.split("\n").slice(1).join("\n")).trim());
    } catch (e) {
      store.notify(String(e), undefined, 8000);
    } finally {
      setBusy(null);
    }
  };

  const create = async () => {
    setBusy("create");
    try {
      // The branch must be on the remote first.
      await gitRemote(root(), "push");
      const url = gitlab() ? await gitlabMrCreate(root(), title(), body(), base(), draft()) : await ghPrCreate(root(), title(), body(), base(), draft());
      props.setOpen(false);
      store.notify(gitlab() ? t("Merge request created") : t("Pull request created"), { label: t("Open"), run: () => void openUrl(url) }, 8000);
      void pullRequests.refresh(props.project.id, root());
    } catch (e) {
      store.notify(String(e), undefined, 10000);
    } finally {
      setBusy(null);
    }
  };

  return (
    <Show
      when={props.pr}
      fallback={
        <Show when={props.hasRemote && props.branch}>
          <Show when={props.open} fallback={<button class="btn" onClick={() => void openForm()}><Icon name="pr" size={14} /> {t("Create {noun}…", { noun: noun() })}</button>}>
            <div class="pr-form">
              <div style={{ display: "flex", gap: "6px", "align-items": "center" }}>
                <span class="muted" style={{ padding: 0 }}>{props.branch} →</span>
                <input class="text-input" style={{ flex: 1 }} value={base()} onInput={(e) => setBase(e.currentTarget.value)} placeholder={t("base branch")} />
              </div>
              <input class="text-input" value={title()} onInput={(e) => setTitle(e.currentTarget.value)} placeholder={t("Title")} />
              <textarea class="commit-box" value={body()} onInput={(e) => setBody(e.currentTarget.value)} placeholder={t("Description (markdown)")} />
              <div style={{ display: "flex", gap: "6px", "align-items": "center", "flex-wrap": "wrap" }}>
                <button class="btn" disabled={!!busy()} onClick={() => void generate()}><Icon name="sparkle" size={13} /> {busy() === "generate" ? t("Writing…") : t("Generate with AI")}</button>
                <label class="check-label"><input type="checkbox" checked={draft()} onChange={(e) => setDraft(e.currentTarget.checked)} /> {t("Draft")}</label>
                <span style={{ flex: 1 }} />
                <button class="btn" onClick={() => props.setOpen(false)}>{t("Cancel")}</button>
                <button class="btn primary" disabled={!!busy() || !title().trim()} onClick={() => void create()}>{busy() === "create" ? t("Creating…") : t("Push & create")}</button>
              </div>
            </div>
          </Show>
        </Show>
      }
    >
      {(pr) => (
        <div class="pr-row" title={pr().title}>
          <Icon name="pr" size={14} />
          <a href="#" onClick={(e) => { e.preventDefault(); void openUrl(pr().url); }}>{gitlab() ? "!" : "#"}{pr().number}</a>
          <span class="pr-title">{pr().title}</span>
          <span class={`chip pr-${prLabel(pr())}`}>{prText(pr())}</span>
          <Show when={pr().checks !== "none"}><span class={`chip checks-${pr().checks}`}>{t("checks {state}", { state: checksText(pr().checks) })}</span></Show>
          <Show when={pr().checks === "failing"}>
            <button class="link-btn" onClick={() => void store.newSession(props.project.id, store.state.settings.aiAgent ?? "claude", {
              prompt: gitlab()
                ? `The pipeline of merge request ${pr().url} is failing. Look at the failed jobs (with \`glab ci view\` / \`glab ci trace\` if glab is installed, else the pipeline page) to find out why, then fix the cause.`
                : `The CI checks of pull request ${pr().url} are failing. Use \`gh pr checks\` and \`gh run view --log-failed\` to find out why, then fix the cause.`,
            })}>{t("Fix with AI")}</button>
          </Show>
        </div>
      )}
    </Show>
  );
}
