// The Review tab: this repository's worktrees that are in review (the Board's Review column,
// filtered), each with what it changed and the actions to open the diff, ask for changes or finish.
import { For, Show } from "solid-js";
import { store } from "../lib/store";
import { inReview } from "../lib/review-queue";
import { ReviewBody } from "./review-card";
import { Icon } from "./icons";
import type { Project } from "../lib/types";
import { t } from "../lib/i18n";
import "./review.css";

export default function ReviewPanel(props: { project: Project }) {
  const repo = () => store.state.projects.find((p) => p.id === (props.project.parentId ?? props.project.id));
  const items = () => store.state.projects.filter((p) => p.parentId === repo()?.id && inReview(p));
  return (
    <div class="review-panel">
      <Show
        when={items().length}
        fallback={
          <div class="muted">
            {t("No worktree of {repo} is in review.", { repo: repo()?.name ?? "" })}
            <p>{t("An agent can put its worktree here with `kelpied worktree set --status review`; you can also move its card to Review on the board.")}</p>
          </div>
        }
      >
        <For each={items()}>
          {(p) => (
            <div class={`review-item${store.state.activeProject === p.id ? " current" : ""}`}>
              <div class="review-item-head">
                <span class="wt-glyph" style={{ color: p.color }}><Icon name="branch" size={13} /></span>
                <span class="name" title={p.path} onClick={() => store.switchProject(p.id)}>{p.name}</span>
              </div>
              <ReviewBody project={p} files />
            </div>
          )}
        </For>
      </Show>
    </div>
  );
}
