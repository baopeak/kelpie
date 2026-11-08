// A file in the way: one question at a time (the oldest waiting transfer's first), with both sides' size and date.
// Mounted once for the whole app: a question shows in a Files view, or from the status bar's chip or a notice anywhere.
// "Apply to all in this transfer" answers the rest of that transfer only.
import { Show, createSignal } from "solid-js";
import { t } from "../../lib/i18n";
import * as ipc from "../../lib/ipc";
import { formatSize, formatTime } from "../../lib/file-pane";
import { conflictsShown, nextQuestion } from "../../lib/transfers";
import type { XferChoice } from "../../lib/types";

export function ConflictDialog() {
  const [all, setAll] = createSignal(false);
  const answer = (choice: XferChoice) => {
    const n = nextQuestion();
    if (!n) return;
    void ipc.xferAnswer(n.job.id, choice, all(), n.q.id).catch(() => {});
    setAll(false);
  };
  return (
    <Show when={conflictsShown() && nextQuestion()} keyed>
      {(n) => (
        <div class="overlay">
          <div class="modal conflict-dialog" role="dialog" aria-modal="true" aria-label={t("A file is in the way")} data-conflict>
            <h3>{n.q.changed ? t("Changed on the server since you opened it") : n.q.kind_clash ? t("A file and a folder have the same name") : t("A file with this name is already there")}</h3>
            <div class="mono conflict-path">{n.q.path}</div>
            <table class="conflict-sides">
              <tbody>
                <tr><td>{t("Copying")}</td><td>{formatSize(n.q.src.size)}</td><td>{formatTime(n.q.src.mtime)}</td></tr>
                <tr><td>{t("Already there")}</td><td>{n.q.dst.kind === "dir" ? t("folder") : formatSize(n.q.dst.size)}</td><td>{formatTime(n.q.dst.mtime)}</td></tr>
              </tbody>
            </table>
            <div class="inline-fields" style={{ "flex-wrap": "wrap" }}>
              <Show when={n.q.changed} fallback={
                <>
                  <Show when={!n.q.kind_clash}>
                    <button class="btn primary" data-choice="replace" onClick={() => answer("replace")}>{t("Replace")}</button>
                    <button class="btn" data-choice="newer" onClick={() => answer("newer")}>{t("Replace if newer")}</button>
                  </Show>
                  <button class="btn" data-choice="keep_both" onClick={() => answer("keep_both")}>{t("Keep both")}</button>
                  <Show when={n.q.identical}><button class="btn" data-choice="skip_identical" onClick={() => answer("skip_identical")}>{t("Skip identical")}</button></Show>
                  <button class="btn" data-choice="skip" onClick={() => answer("skip")}>{t("Skip")}</button>
                </>
              }>
                <button class="btn primary" data-choice="overwrite" onClick={() => answer("overwrite")}>{t("Overwrite")}</button>
                <button class="btn" data-choice="save_copy" onClick={() => answer("save_copy")}>{t("Save as a copy")}</button>
                <button class="btn" data-choice="discard" onClick={() => answer("discard")}>{t("Discard my upload")}</button>
              </Show>
            </div>
            <Show when={!n.q.changed}>
              <label class="check-inline"><input type="checkbox" data-field="apply-all" checked={all()} onChange={(e) => setAll(e.currentTarget.checked)} /> {t("Apply to all in this transfer")}</label>
            </Show>
          </div>
        </div>
      )}
    </Show>
  );
}
