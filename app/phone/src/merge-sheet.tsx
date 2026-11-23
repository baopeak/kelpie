// Merging a pull request from the phone: what is merged and how, then the passkey over exactly that, then what came of it. Every step is a sheet over the
// session's view, and the passkey is asked on a press of its own button, which a ceremony needs.
import { For, Show } from "solid-js";
import { t } from "./i18n";
import { isMergeMethod, MERGE_METHODS, methodWords, shortHead } from "./merge";
import { askMerge, chooseMergeMethod, closeMerge, confirmMerge, connected, mergeSheet, type MergeSheet as MergeState } from "./phone-state";

/** The sheet as it is at `step`, or null at any other. */
function at<S extends MergeState["step"]>(sheet: MergeState, step: S): Extract<MergeState, { step: S }> | null {
  return sheet.step === step ? (sheet as Extract<MergeState, { step: S }>) : null;
}

export function MergeSheet() {
  return (
    <Show when={mergeSheet()}>
      {(sheet) => (
        <div class="sheet-back">
          <div class="sheet" role="dialog" aria-label={t("Merge this pull request")} data-merge-sheet={sheet().step}>
            <Show when={at(sheet(), "choose")}>
              {(s) => (
                <>
                  <h2>{t("Merge this pull request?")}</h2>
                  <dl class="merge-facts">
                    <dt>{t("Repository")}</dt>
                    <dd data-merge-repo>{s().pr.repo}</dd>
                    <dt>{t("Pull request")}</dt>
                    <dd data-merge-pr>{`#${s().pr.number}${s().pr.title ? ` ${s().pr.title}` : ""}`}</dd>
                    <dt>{t("Head commit")}</dt>
                    <dd class="mono" data-merge-head>{shortHead(s().pr.head)}</dd>
                    <dt>{t("How")}</dt>
                    <dd>
                      <select
                        class="field"
                        data-merge-method
                        aria-label={t("How to merge")}
                        onChange={(e) => {
                          const chosen = e.currentTarget.value;
                          if (isMergeMethod(chosen)) chooseMergeMethod(chosen);
                        }}
                      >
                        <For each={MERGE_METHODS}>{(m) => <option value={m} selected={m === s().method}>{methodWords(m)}</option>}</For>
                      </select>
                    </dd>
                  </dl>
                  <p class="muted">{t("GitHub merges it only if its required checks pass and the branch has not changed since you looked. Your passkey is asked next.")}</p>
                  <Show when={s().error}><p class="muted err" role="status" data-merge-error>{s().error}</p></Show>
                  <div class="actions">
                    <button class="btn primary" data-merge-continue disabled={s().busy || !connected()} onClick={() => void askMerge()}>{s().busy ? t("Asking…") : t("Continue")}</button>
                    <button class="btn" data-merge-cancel onClick={closeMerge}>{t("Cancel")}</button>
                  </div>
                </>
              )}
            </Show>
            <Show when={at(sheet(), "verify")}>
              {(s) => (
                <>
                  <h2>{t("Confirm with your passkey")}</h2>
                  <p class="muted">{t("Your passkey signs exactly this, and the computer merges only that:")}</p>
                  <pre class="code command" data-merge-command>{s().command}</pre>
                  <p class="muted" data-merge-read>{`${methodWords(s().request.method)} · ${s().request.repo} #${s().request.number}${s().pr.title ? ` ${s().pr.title}` : ""} · ${shortHead(s().request.head)}`}</p>
                  <Show when={s().error}><p class="muted err" role="status" data-merge-error>{s().error}</p></Show>
                  <div class="actions">
                    <button class="btn primary" data-merge-go disabled={s().busy || !connected()} onClick={() => void confirmMerge()}>{s().busy ? t("Asking…") : t("Merge with passkey")}</button>
                    <button class="btn" data-merge-cancel onClick={closeMerge}>{t("Cancel")}</button>
                  </div>
                </>
              )}
            </Show>
            <Show when={at(sheet(), "done")}>
              {(s) => (
                <>
                  <h2>{s().ok ? t("Merged") : t("Not merged")}</h2>
                  <p class={s().ok ? "" : "muted err"} role="status" data-merge-result>{s().message}</p>
                  <div class="actions">
                    <button class="btn primary" data-merge-close onClick={closeMerge}>{t("Close")}</button>
                  </div>
                </>
              )}
            </Show>
          </div>
        </div>
      )}
    </Show>
  );
}
