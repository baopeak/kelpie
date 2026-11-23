// The line a project's pull request gets in a session's view: whether GitHub would merge it, the repository and the title. Read when the view
// opens and when the person asks again, never on a timer.
import { createEffect, on, Show } from "solid-js";
import { Icon } from "./icons";
import { t } from "./i18n";
import { canReadPulls, connected, loadPull, pullStates } from "./phone-state";
import { pullHeadline, pullOf, pullSubline, pullViewWords } from "./pull-line";

export function PullLine(props: { project: string | null }) {
  const state = () => (props.project && canReadPulls() ? pullStates()[props.project] : undefined);
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
                </>
              )}
            </Show>
          </div>
          <button class="icon" data-pull-refresh aria-label={t("Look again")} disabled={s().busy || !connected()} onClick={() => props.project && void loadPull(props.project)}>
            <Icon name="refresh" size={18} />
          </button>
        </div>
      )}
    </Show>
  );
}
