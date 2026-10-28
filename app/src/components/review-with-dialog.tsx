// "Review with…": pick the agent that reviews a worktree's changes and read or edit the prompt it
// starts with. The diff goes along (bounded, credentials masked); its notes land in the diff view.
import { For, Show, createSignal } from "solid-js";
import { store } from "../lib/store";
import { kindInfo, launchableAgents } from "../lib/agents";
import { baseOf, changesOf, reviewing } from "../lib/review-queue";
import { reviewPrompt } from "../lib/quick-commands";
import { REVIEW_DIFF_MAX, startReview, suggestedReviewer } from "../lib/review-with";
import { NOTES_API } from "../lib/review-notes";
import { KindGlyph } from "./icons";
import { KeyHints } from "./key-hints";
import { t } from "../lib/i18n";
import "./review.css";

export default function ReviewWithDialog() {
  const project = () => store.state.projects.find((p) => p.id === reviewing());
  const kinds = () => launchableAgents(store.state.settings.hiddenAgents);
  const [kind, setKind] = createSignal<string | undefined>(project() ? suggestedReviewer(project()!) : undefined);
  const [prompt, setPrompt] = createSignal(reviewPrompt(project()?.id ?? null));
  const [busy, setBusy] = createSignal(false);
  const close = () => store.setOverlay(null);
  const c = () => (project() ? changesOf(project()!.id) : undefined);

  async function start() {
    const p = project(), k = kind();
    if (!p || !k || !prompt().trim() || busy()) return;
    setBusy(true);
    try {
      store.setOverlay(null);
      await startReview(p, k, prompt());
    } catch (e) {
      store.notify(t("Could not start the review: {error}", { error: String(e).replace(/^Error: /, "") }), undefined, 8000);
    }
  }

  return (
    <div class="overlay" onPointerDown={(e) => e.target === e.currentTarget && close()}>
      <div class="modal review-with" onKeyDown={(e) => {
        if (e.key === "Escape") close();
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) void start();
      }}>
        <Show when={project()} fallback={<p>{t("This worktree is not available.")}</p>}>
          <h2>{t("Review {name} with…", { name: project()!.name })}</h2>
          <div class="launcher" style={{ "grid-template-columns": "repeat(auto-fill, minmax(92px, 1fr))" }}>
            <For each={kinds()}>
              {(k) => (
                <div class="launch-card" data-kind={k} style={{ "flex-direction": "column", "border-color": kind() === k ? "var(--accent)" : undefined }} onClick={() => setKind(k)}>
                  <KindGlyph kind={k} size={30} />
                  <div style={{ "font-size": "12px" }}>{kindInfo(k).label}</div>
                </div>
              )}
            </For>
          </div>
          <div class="form-row">
            <label>{t("Review prompt")}</label>
            <textarea class="text-input review-prompt" rows={6} value={prompt()} onInput={(e) => setPrompt(e.currentTarget.value)} />
          </div>
          <p class="muted-inline">{t("To change the default, save a quick command called “Review prompt” (Settings → Quick commands).")}</p>
          <p class="muted-inline">
            <Show when={c()?.ok} fallback={t("Reading changes…")}>
              {t("The diff against {base} goes with it ({files} files, +{added} −{deleted}; at most {kb} KB, credentials masked).", { base: baseOf(project()!) ?? "", files: c()!.files.length, added: c()!.added, deleted: c()!.deleted, kb: Math.round(REVIEW_DIFF_MAX / 1000) })}
            </Show>
            {" "}
            {store.daemonApi() >= NOTES_API ? t("Its comments appear on the lines of the diff.") : t("This session daemon is older: the reviewer answers in its terminal only.")}
          </p>
          <div class="footer">
            <KeyHints items={[["Ctrl+Enter", t("Start")], ["Esc", t("Cancel")]]} />
            <span style={{ flex: 1 }} />
            <button class="btn" onClick={close}>{t("Cancel")}</button>
            <button class="btn primary" disabled={!kind() || !prompt().trim() || busy()} onClick={() => void start()}>{t("Start review")}</button>
          </div>
        </Show>
      </div>
    </div>
  );
}
