// What a worktree in review changed, and what you can do with it: the summary a Board card shows
// in the Review column and the Review tab lists. Loaded with the Board / the tab.
import { For, Show, createMemo, createResource, onMount } from "solid-js";
import { store } from "../lib/store";
import { baseOf, changesOf, openFinish, openReviewWith, refreshChanges } from "../lib/review-queue";
import { loadChecksPlan } from "../lib/finish";
import { chooseAgentAndSend } from "../lib/send-to-agent";
import { checksChip, countTurns, sessionsOf, worktreeCost } from "../lib/review-facts";
import { formatCost } from "../lib/usage-format";
import type { Project } from "../lib/types";
import { t } from "../lib/i18n";
import "./review.css";

/** Shows the worktree's diff against its base in the right panel. */
export function openReview(p: Project) {
  store.setOverlay(null);
  store.switchProject(p.id);
  store.showRightPanel("review");
  const first = changesOf(p.id)?.files[0];
  const base = baseOf(p);
  if (first && base) store.openFile(p.id, first, { diff: true, base });
}

/** Sends the review notes written on the diff to an agent in the worktree and moves the card back to In progress. */
export async function requestChanges(e: MouseEvent, p: Project) {
  const notes = store.notesFor(p.id);
  if (!notes.length) return store.notify(t("Write review notes first: click the + beside a line of the diff, then ask for changes."), undefined, 7000);
  const { notesPrompt } = await import("./diff-view");
  chooseAgentAndSend(e, p.id, notesPrompt(notes), async () => {
    // Only the notes that were sent go (a reviewer may have added one since); they are gone before the
    // card moves, so whoever sees it in progress finds none of them left.
    await store.removeNotes(p.id, notes.map((n) => n.id));
    store.setProjectStatus(p.id, "progress");
  });
}

export function ReviewBody(props: { project: Project; files?: boolean }) {
  const p = () => props.project;
  const c = () => changesOf(p().id);
  const repo = () => store.state.projects.find((x) => x.id === p().parentId);
  onMount(() => void refreshChanges([p()]));

  const planKey = () => {
    const cc = c();
    const b = baseOf(p());
    return cc?.ok && repo() && b ? `${repo()!.id}|${b}|${cc.config_changed}` : false;
  };
  const [plan] = createResource(planKey, () => loadChecksPlan(repo()!, baseOf(p())!, !!c()?.config_changed));

  const mine = createMemo(() => sessionsOf(p().id));
  const cost = () => worktreeCost(p().id);
  const [turns] = createResource(() => mine().map((s) => s.id).join(","), (ids) => countTurns(ids.split(",").filter(Boolean)));
  const chip = () => checksChip(plan());

  const openFile = (path: string) => {
    const base = baseOf(p());
    if (!base) return;
    // The diff opens in the worktree's own tabs; the Review tab stays showing.
    if (store.state.activeProject !== p().id) {
      store.switchProject(p().id);
      store.showRightPanel("review");
    }
    store.openFile(p().id, path, { diff: true, base });
  };

  return (
    <div class="review-body" onPointerDown={(e) => e.stopPropagation()}>
      <Show when={c()} fallback={<div class="review-line muted-inline">{t("Reading changes…")}</div>}>
        {(cc) => (
          <Show when={cc().ok} fallback={<div class="review-line muted-inline" title={cc().error ?? ""}>{t("Changes unavailable: {error}", { error: (cc().error ?? "").split("\n")[0] })}</div>}>
            <div class="review-line">
              <span>{cc().files.length === 1 ? t("1 file") : t("{n} files", { n: cc().files.length })}</span>
              <span class="review-add">+{cc().added}</span>
              <span class="review-del">−{cc().deleted}</span>
              <Show when={cc().commits}><span class="muted-inline">{cc().commits === 1 ? t("1 commit") : t("{n} commits", { n: cc().commits })}</span></Show>
              <Show when={cc().uncommitted}><span class="muted-inline" title={t("Not committed yet: Finish saves them in a WIP commit")}>{t("{n} uncommitted", { n: cc().uncommitted })}</span></Show>
            </div>
            <div class="review-line">
              <Show when={chip()}>{(k) => <span class={`chip review-chip ${k().cls}`} title={k().title}>{k().text}</span>}</Show>
              <Show when={cc().config_changed}><span class="chip review-chip warn" title={t("The change edits .kelpie.yaml (setup, links and checks). Read that diff before you finish.")}>{t(".kelpie.yaml changed")}</span></Show>
              <Show when={cost() > 0}><span class="muted-inline" title={t("What this worktree's sessions cost")}>{formatCost(cost(), false)}</span></Show>
              <Show when={(turns() ?? 0) > 0}><span class="muted-inline">{turns() === 1 ? t("1 turn") : t("{n} turns", { n: turns() ?? 0 })}</span></Show>
            </div>
            <Show when={props.files}>
              <div class="review-files">
                <For each={cc().files.slice(0, 200)}>{(f) => <button class="review-file" title={f} onClick={() => openFile(f)}>{f}</button>}</For>
                <Show when={cc().files.length > 200 || cc().truncated}><div class="muted-inline">{t("More files not listed.")}</div></Show>
              </div>
            </Show>
          </Show>
        )}
      </Show>
      <div class="review-actions">
        <button class="btn mini" onClick={() => openReview(p())}>{t("Open diff")}</button>
        <button class="btn mini" onClick={(e) => void requestChanges(e, p())}>{t("Request changes")}</button>
        <button class="btn mini" title={t("Ask another agent to review this worktree's changes")} disabled={!c()?.ok} onClick={() => openReviewWith(p().id)}>{t("Review with…")}</button>
        <button class="btn mini primary" disabled={!c()?.ok} onClick={() => { store.setOverlay(null); openFinish(p().id); }}>{t("Approve → Finish")}</button>
      </div>
    </div>
  );
}
