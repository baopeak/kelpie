// The line a project's pull request gets in a session's view: whether GitHub would merge it, the repository and the title. Read when the view
// opens and when the person asks again, never on a timer. The Merge button shows only while the line says it can be merged and this phone may merge;
// on a phone that may not, the reason stands in its place.
import { createEffect, on, Show } from "solid-js";
import { Icon } from "./icons";
import { t } from "./i18n";
import { mergeOffered } from "./merge";
import { canMerge, canReadPulls, connected, loadPull, mergeBlock, openMerge, pullStates } from "./phone-state";
import { pullHeadline, pullLineShown, pullOf, pullSubline, pullViewWords } from "./pull-line";

export function PullLine(props: { project: string | null }) {
  // A project that is not on GitHub has no line: no error, no "no pull request".
  const state = () => {
    const s = props.project && canReadPulls() ? pullStates()[props.project] : undefined;
    return s && pullLineShown(s.view) ? s : undefined;
  };
  createEffect(
    on(
      () => [props.project, connected(), canReadPulls()] as const,
      ([project, up, can]) => {
        if (project && up && can) void loadPull(project);
      },
    ),
  );
  return (
    <Show when={state()}>
      {(s) => (
        <div class="pull-line" data-pull-line classList={{ can: !!pullOf(s().view)?.can }} role="status">
          <div class="pull-text">
            <Show when={pullOf(s().view)} fallback={<span class="muted">{pullViewWords(s().view)}</span>}>
              {(pr) => (
                <>
                  <strong data-pull-headline>{pullHeadline(pr())}</strong>
                  <span class="muted pull-sub">{pullSubline(pr())}</span>
                  {/* In place of the Merge button: a pull request that can be merged, and why this phone cannot merge it. */}
                  <Show when={pr().can && mergeBlock()}>{(why) => <span class="muted pull-sub" data-pull-merge-why>{why()}</span>}</Show>
                </>
              )}
            </Show>
          </div>
          <Show when={props.project && mergeOffered({ pr: pullOf(s().view), control: canMerge(), api: canReadPulls() }) ? pullOf(s().view) : null}>
            {(pr) => (
              <button class="btn" data-pull-merge disabled={s().busy || !connected()} onClick={() => props.project && openMerge(props.project, pr())}>{t("Merge")}</button>
            )}
          </Show>
          <button class="icon" data-pull-refresh aria-label={t("Look again")} disabled={s().busy || !connected()} onClick={() => props.project && void loadPull(props.project)}>
            <Icon name="refresh" size={18} />
          </button>
        </div>
      )}
    </Show>
  );
}
