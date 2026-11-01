// Finish a worktree: what will be committed, how it is merged, which approved checks run, then the
// whole sequence (lib/finish.ts) with a stop at the first thing that needs you.
import { For, Show, createEffect, createResource, createSignal, untrack } from "solid-js";
import { store } from "../lib/store";
import * as ipc from "../lib/ipc";
import { baseOf, changesOf, currentBranchOf, finishSucceeded, finishing, refreshChanges } from "../lib/review-queue";
import { configApproved } from "../lib/worktrees";
import { checkPrompt, conflictPrompt, defaultMessage, diffChecks, loadChecksPlan, needsDecision, runFinish, runnable, type Outcome, type Problem } from "../lib/finish";
import type { FinishStrategy } from "../lib/ipc";
import type { Project } from "../lib/types";
import { t } from "../lib/i18n";
import { LoadBar } from "./load-bar";
import "./review.css";

const STRATEGIES: { id: FinishStrategy; label: () => string; hint: () => string }[] = [
  { id: "squash", label: () => t("Squash"), hint: () => t("One commit on the base with all the changes.") },
  { id: "merge", label: () => t("Merge commit"), hint: () => t("Keeps the branch's commits and adds a merge commit.") },
  { id: "rebase", label: () => t("Rebase"), hint: () => t("Replays the branch's commits on top of the base, keeping authors; no merge commit.") },
];

const LARGE = 5 << 20;
const size = (n: number) => (n >= 1 << 20 ? `${(n / (1 << 20)).toFixed(1)} MB` : n >= 1024 ? `${Math.round(n / 1024)} KB` : `${n} B`);

/** What Finish works on, taken once: the worktree leaves the sidebar before the dialog is closed. */
interface Target {
  wt: Project;
  repo: Project;
  base: string;
  /** The repository's current branch when the dialog opened (it may differ from the base). */
  current: string | null;
}

export default function FinishDialog() {
  const liveWt = () => store.state.projects.find((p) => p.id === finishing());
  const liveRepo = () => store.state.projects.find((p) => p.id === liveWt()?.parentId);
  const [target, setTarget] = createSignal<Target | null>(null);
  const [branchName, setBranchName] = createSignal<string | null>(null);
  createEffect(() => {
    if (untrack(target)) return;
    const wt = liveWt(), repo = liveRepo();
    const base = wt && baseOf(wt);
    if (wt && repo && base) setTarget({ wt: { ...wt }, repo: { ...repo }, base, current: currentBranchOf(wt) });
  });
  const wt = () => target()?.wt;
  const repo = () => target()?.repo;
  const base = () => target()?.base ?? null;
  const branch = () => branchName();
  const close = () => store.setOverlay(null);

  const [strategy, setStrategy] = createSignal<FinishStrategy>("squash");
  const [message, setMessage] = createSignal("");
  /** The files the user ticked as the WIP commit: exactly these are committed. */
  const [confirmed, setConfirmed] = createSignal<string[]>([]);
  const [baseConfirmed, setBaseConfirmed] = createSignal(false);
  const [override, setOverride] = createSignal(false);
  const [skipChecks, setSkipChecks] = createSignal(false);
  const [step, setStep] = createSignal<string | null>(null);
  const [outcome, setOutcome] = createSignal<Outcome | null>(null);
  /** Past the merge Cancel can only stop the cleanup. */
  const [pastMerge, setPastMerge] = createSignal(false);
  let cancelState = { requested: false };

  const [seen, { refetch }] = createResource(
    () => !!target(),
    async () => {
      // Forced: `.kelpie.yaml` edited a moment ago must show.
      await refreshChanges([wt()!], true);
      return ipc.finishInspect(repo()!.path, wt()!.path, base()!);
    },
  );
  createEffect(() => {
    const b = seen()?.branch;
    if (b && !untrack(branchName)) {
      setBranchName(b);
      setMessage(defaultMessage(wt()!, b));
    }
  });
  const [plan, { refetch: refetchPlan }] = createResource(
    () => !!target(),
    async () => {
      await refreshChanges([wt()!], true);
      return loadChecksPlan(repo()!, base()!, !!changesOf(wt()!.id)?.config_changed);
    },
  );

  const uncommitted = () => seen()?.uncommitted ?? [];
  const recorded = () => wt()?.baseBranch;
  const baseNeedsConfirm = () => !recorded() || recorded() !== target()?.current;
  const decision = () => !!plan() && needsDecision(plan()!, override());
  const blockedWhy = (): string | null => {
    if (seen.error) return t("The worktree could not be read: {error}", { error: String(seen.error).replace(/^Error: /, "") });
    const s = seen();
    if (!s) return t("Reading the worktree…");
    if (!s.branch) return t("The worktree is on a detached HEAD: commits made there would be lost with it. Create a branch for them first (git switch -c <name>), then finish.");
    if (s.operation) return t("A {op} is in progress in the worktree; finish or abort it first.", { op: s.operation });
    if (s.conflicts.length) return t("The worktree has unresolved conflicts; resolve them first.");
    if (s.embedded.length) return t("These folders hold a git repository of their own and cannot be committed as files: {folders}. Commit inside them or move them out, then check again.", { folders: s.embedded.join(", ") });
    if (s.base_checkout && s.base_checkout_dirty) return t("{path} (where {base} is checked out) has changes to tracked files. Commit or stash them, then check again.", { path: s.base_checkout, base: base() ?? "" });
    return null;
  };
  const canFinish = () =>
    !!wt() && !!repo() && !!base() && !!branch() && !step() && !blockedWhy() && !seen()?.merge_conflicts.length && !!plan() &&
    (!uncommitted().length || confirmed().length > 0) && (!decision() || skipChecks()) && (!baseNeedsConfirm() || baseConfirmed()) &&
    (strategy() === "rebase" || message().trim().length > 0);

  const tick = (on: boolean) => setConfirmed(on ? uncommitted().map((f) => f.path) : []);

  async function start() {
    const w = wt(), r = repo(), b = branch();
    if (!w || !r || !base() || !b) return;
    setOutcome(null);
    setPastMerge(false);
    cancelState = { requested: false };
    setStep(t("Starting…"));
    const run = plan() && runnable(plan()!, override()) && plan()!.config ? plan()!.commands : [];
    // Approving the checks approves nothing else: the check folder gets the base's copy: / link: only when that
    // section was approved too (the branch's own scripts run there).
    const cfg = plan()?.config;
    const setupOk = !!cfg && configApproved(r, cfg);
    const result = await runFinish({
      worktree: w, repo: r, branch: b, base: base()!, strategy: strategy(), message: message().trim(), confirmed: confirmed(),
      checks: run, copy: run.length && setupOk ? cfg?.copy ?? [] : [], link: run.length && setupOk ? cfg?.link ?? [] : [], allowConfigChange: override(), cancel: cancelState,
    }, (text, past) => {
      setStep(text);
      if (past) setPastMerge(true);
    });
    setStep(null);
    setOutcome(result);
    if (result.kind === "finished") finishSucceeded(w.id);
    if (result.kind === "stopped") {
      // Files may have changed while the sessions stopped: the list is read again and has to be confirmed again.
      if (result.problem.why === "files_changed" || result.problem.why === "not_on_branch") setConfirmed([]);
      void refetch();
    }
  }

  const cancel = () => {
    cancelState.requested = true;
    setStep(t("Cancelling…"));
    if (wt()) void ipc.finishCancel(wt()!.path);
  };

  const approveChecks = () => {
    const pl = plan();
    if (!pl?.config || !repo()) return;
    // Only the checks: approving them says nothing about the file's setup or tabs.
    store.approveChecks(repo()!.path, pl.config.check);
    void refetchPlan();
  };

  /** Sends an agent into the worktree with a repair prompt (credentials in command output are masked). */
  async function withAgent(text: string) {
    const w = wt();
    if (!w) return;
    const safe = await ipc.redact(text);
    close();
    store.switchProject(w.id);
    void store.newSession(w.id, store.state.settings.aiAgent ?? "claude", { prompt: safe });
  }

  const explain = (p: Problem) => {
    switch (p.why) {
      case "sessions": return t("Some processes in this worktree did not stop. Close them and try again; nothing was changed.");
      case "not_on_branch": return p.branch ? t("The worktree is on branch {other} now, not {branch}. Nothing was changed.", { other: p.branch, branch: branch() ?? "" }) : t("The worktree is on a detached HEAD now: commits made there would be lost with it. Create a branch for them first (git switch -c <name>). Nothing was changed.");
      case "embedded": return t("These folders hold a git repository of their own and cannot be committed as files: {folders}. Nothing was changed.", { folders: p.folders.join(", ") });
      case "files_changed": return t("The files in the worktree changed after you confirmed the list (the agent was still writing). Nothing was committed; read the list again and confirm it.");
      case "conflict": return t("The branch conflicts with {base}. Nothing was merged; your work is on the branch.", { base: base() ?? "" });
      case "checks": return t("A check failed. {base} was not changed.", { base: base() ?? "" });
      case "config_blocked": return t("The branch edits {files}, so its checks were not run. {base} was not changed.", { files: p.files.join(", "), base: base() ?? "" });
      case "base_moved": return t("{base} moved while Finish was working, so it was not changed. Try again.", { base: base() ?? "" });
      case "checkout_dirty": return p.files.length
        ? t("The merge would overwrite files that exist in {path} but are not tracked there (untracked or ignored): {files}. Move them away, then try again. {base} was not changed.", { path: p.path, files: p.files.join(", "), base: base() ?? "" })
        : t("{path} has changes to tracked files, so {base} was not changed. Commit or stash them, then try again.", { path: p.path, base: base() ?? "" });
      case "cancelled": return t("Cancelled. Nothing was changed on {base}.", { base: base() ?? "" });
      case "error": return p.message;
    }
  };

  const retryable = (p: Problem) => ["base_moved", "checkout_dirty", "sessions", "error", "cancelled", "files_changed", "not_on_branch"].includes(p.why);

  return (
    <div class="overlay" onPointerDown={(e) => e.target === e.currentTarget && !step() && close()}>
      <div class="modal finish-dialog" onKeyDown={(e) => e.key === "Escape" && !step() && close()}>
        <LoadBar when={step() || seen.loading || plan.loading} />
        <Show when={target()} fallback={<p>{liveWt() && !baseOf(liveWt()!) ? t("The repository has no current branch to merge into; switch it to a branch first.") : t("This worktree is not available.")}</p>}>
          <h2>{t("Finish {name}", { name: wt()!.name })}</h2>
          <span class="muted-inline">{branch() ? t("{branch} → {base}", { branch: branch()!, base: base()! }) : seen() ? t("Detached HEAD") : t("Reading the worktree…")}</span>

          <Show when={!outcome()}>
            <Show when={blockedWhy()}>{(why) => <p class="finish-note warn">{why()} <button class="link-btn" onClick={() => void refetch()}>{t("Check again")}</button></p>}</Show>

            <Show when={baseNeedsConfirm() && branch()}>
              <p class="finish-note warn">
                {recorded()
                  ? t("This worktree was created from {recorded}, but the main checkout is on {current} now. It is merged into {recorded}.", { recorded: recorded()!, current: target()!.current ?? t("no branch") })
                  : t("Kelpie does not know which branch this worktree was created from. It would be merged into {base}, the main checkout's current branch.", { base: base()! })}
                <label class="check-label"><input type="checkbox" checked={baseConfirmed()} onChange={(e) => setBaseConfirmed(e.currentTarget.checked)} /> {t("Merge into {base}", { base: base()! })}</label>
              </p>
            </Show>

            <Show when={seen()?.merge_conflicts.length}>
              <p class="finish-note bad">
                {t("The branch already conflicts with {base} in: {files}. Finish would stop there.", { base: base()!, files: seen()!.merge_conflicts.join(", ") })}
                {" "}<button class="btn mini" onClick={() => void withAgent(conflictPrompt(branch() ?? "", base() ?? "", seen()!.merge_conflicts))}>{t("Resolve with agent")}</button>
              </p>
            </Show>

            <Show when={uncommitted().length}>
              <h3>{t("Uncommitted files")}</h3>
              <div class="finish-list">
                <For each={uncommitted()}>
                  {(f) => (
                    <div title={f.size >= LARGE ? t("Large file: it would stay in the history for good") : ""}>
                      <span class="code">{f.status}</span><span>{f.path}</span><span class="muted-inline" style={{ "margin-left": "auto" }}>{f.size >= LARGE ? `⚠ ${size(f.size)}` : f.size ? size(f.size) : ""}</span>
                    </div>
                  )}
                </For>
              </div>
              <label class="check-label" style={{ "margin-top": "6px" }}>
                <input type="checkbox" checked={confirmed().length > 0} onChange={(e) => tick(e.currentTarget.checked)} /> {t("Commit exactly these files first (as a WIP commit on the branch)")}
              </label>
            </Show>

            <h3>{t("How to merge")}</h3>
            <div class="finish-strategies">
              <For each={STRATEGIES}>
                {(s) => (
                  <label>
                    <input type="radio" name="strategy" checked={strategy() === s.id} onChange={() => setStrategy(s.id)} />
                    <span><b>{s.label()}</b> · <span class="muted-inline">{s.hint()}</span></span>
                  </label>
                )}
              </For>
            </div>
            <Show when={strategy() !== "rebase"}>
              <div class="form-row">
                <label>{t("Commit message")}</label>
                <textarea class="text-input" rows={2} value={message()} onInput={(e) => setMessage(e.currentTarget.value)} />
              </div>
            </Show>

            <h3>{t("Checks")}</h3>
            <Show when={plan()} fallback={<span class="muted-inline">{t("Reading .kelpie.yaml from {base}…", { base: base() ?? "" })}</span>}>
              {(pl) => (
                <>
                  <Show when={pl().error}><p class="finish-note warn">{t("The .kelpie.yaml on {base} could not be read: {error}", { base: base() ?? "", error: pl().error! })}</p></Show>
                  <Show when={!pl().error && !pl().commands.length}><span class="muted-inline">{t("No checks in the .kelpie.yaml on {base}.", { base: base() ?? "" })}</span></Show>
                  <Show when={pl().commands.length}>
                    <p class="muted-inline" style={{ margin: "0 0 4px" }}>{t("The list comes from the .kelpie.yaml committed on {base}, never from the branch. Each command runs without a shell in a temporary worktree of the merged result, so it runs the branch's own scripts and build files (npm test runs the branch's package.json).", { base: base() ?? "" })}</p>
                    <div class="finish-list finish-checks">
                      <For each={!pl().approved && pl().previous ? diffChecks(pl().previous!, pl().commands) : pl().commands.map((text) => ({ text, kind: "same" as const }))}>
                        {(l) => <div><span class="code">{l.kind === "added" ? "+" : l.kind === "removed" ? "−" : ""}</span><span class={l.kind === "same" ? "" : l.kind}>{l.text}</span></div>}
                      </For>
                    </div>
                    <Show when={pl().config && (pl().config!.copy.length || pl().config!.link.length) && !configApproved(repo()!, pl().config!)}>
                      <p class="finish-note">{t("The file's copy: and link: are not approved, so the checks run without them. Approve them by creating a worktree with the file ticked.")}</p>
                    </Show>
                    <Show when={pl().blocked}>
                      <p class="finish-note warn">
                        {t("This branch changes .kelpie.yaml, so its checks are off. Read that change in the diff; if you trust it, you can still run the checks from {base}.", { base: base() ?? "" })}
                        <label class="check-label"><input type="checkbox" checked={override()} onChange={(e) => setOverride(e.currentTarget.checked)} /> {t("Run the checks from {base} anyway", { base: base() ?? "" })}</label>
                      </p>
                    </Show>
                    <Show when={!pl().approved && (!pl().blocked || override())}>
                      <p class="finish-note warn">
                        {pl().previous ? t("The checks changed since you approved them (shown above). Approve before they run.") : t("These checks have not been approved yet. They run commands on your computer.")}
                        {" "}<button class="btn mini" onClick={approveChecks}>{t("Approve these checks")}</button>
                      </p>
                    </Show>
                  </Show>
                  <Show when={decision()}>
                    <label class="check-label"><input type="checkbox" checked={skipChecks()} onChange={(e) => setSkipChecks(e.currentTarget.checked)} /> {t("Finish without running the checks")}</label>
                  </Show>
                </>
              )}
            </Show>

            <p class="muted-inline" style={{ "margin-top": "10px" }}>
              {t("Then {base} moves only if it has not changed meanwhile, the branch is deleted only when its work is in {base}, and the worktree is removed last. Nothing is forced.", { base: base() ?? "" })}
            </p>
            <Show when={step()}><div class="finish-steps"><span>{step()}</span></div></Show>
            <div class="footer">
              <Show when={!step()} fallback={<button class="btn" disabled={pastMerge()} title={pastMerge() ? t("The work is merged; only the cleanup is left") : ""} onClick={cancel}>{t("Cancel Finish")}</button>}>
                <button class="btn" onClick={close}>{t("Cancel")}</button>
              </Show>
              <button class="btn primary" disabled={!canFinish()} onClick={() => void start()}>{step() ? t("Working…") : t("Finish")}</button>
            </div>
          </Show>

          <Show when={outcome()}>
            {(o) => (
              <>
                <Show when={o().kind === "finished"}>
                  <p class="finish-note">{t("Finished. The work is in {base} and the worktree is gone.", { base: base() ?? "" })}</p>
                </Show>
                <Show when={o().kind === "merged"}>
                  <p class="finish-note warn">{(o() as Extract<Outcome, { kind: "merged" }>).problem}</p>
                </Show>
                <Show when={o().kind === "stopped"}>
                  {(() => {
                    const p = (o() as Extract<Outcome, { kind: "stopped" }>).problem;
                    return (
                      <>
                        <p class="finish-note bad">{explain(p)}</p>
                        <Show when={p.why === "conflict"}>
                          <div class="finish-list"><For each={(p as Extract<Problem, { why: "conflict" }>).files}>{(f) => <div><span class="code">U</span><span>{f}</span></div>}</For></div>
                          <div class="footer"><button class="btn primary" onClick={() => void withAgent(conflictPrompt(branch() ?? "", base() ?? "", (p as Extract<Problem, { why: "conflict" }>).files))}>{t("Resolve with agent")}</button></div>
                        </Show>
                        <Show when={p.why === "files_changed"}>
                          <div class="finish-list"><For each={(p as Extract<Problem, { why: "files_changed" }>).files}>{(f) => <div><span>{f}</span></div>}</For></div>
                        </Show>
                        <Show when={p.why === "checks"}>
                          {(() => {
                            const checks = (p as Extract<Problem, { why: "checks" }>).checks;
                            const failed = checks[checks.length - 1];
                            return (
                              <>
                                <b>{failed.command}</b> <span class="muted-inline">{failed.code != null ? t("exit {code}", { code: failed.code }) : ""}</span>
                                <pre class="finish-output">{failed.output || t("(no output)")}</pre>
                                <Show when={(p as Extract<Problem, { why: "checks" }>).cleanup}>{(c) => <p class="finish-note warn">{t("The temporary worktree could not be fully removed: {error}", { error: c() })}</p>}</Show>
                                <div class="footer"><button class="btn primary" onClick={() => void withAgent(checkPrompt(branch() ?? "", base() ?? "", failed))}>{t("Fix with agent")}</button></div>
                              </>
                            );
                          })()}
                        </Show>
                      </>
                    );
                  })()}
                </Show>
                <Show when={(o() as { notes: string[] }).notes.length}>
                  <div class="finish-steps"><For each={(o() as { notes: string[] }).notes}>{(n) => <span>• {n}</span>}</For></div>
                </Show>
                <div class="footer">
                  <Show when={o().kind === "stopped" && retryable((o() as Extract<Outcome, { kind: "stopped" }>).problem)}>
                    <button class="btn" onClick={() => setOutcome(null)}>{t("Try again")}</button>
                  </Show>
                  <button class="btn primary" onClick={close}>{t("Close")}</button>
                </div>
              </>
            )}
          </Show>
        </Show>
      </div>
    </div>
  );
}
