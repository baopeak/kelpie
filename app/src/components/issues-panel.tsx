// Issues panel (right side): Linear, Jira and GitLab issues — assigned to you, or searched —
// and what to do with one: start an agent on it here or in a new worktree, send it to a running
// agent, move it to another state, open it.
import { For, Show, createEffect, createResource, createSignal, on } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { store } from "../lib/store";
import { chooseAgentAndSend } from "../lib/send-to-agent";
import { openWorktreeDialog } from "./worktree-dialog";
import { openContextMenu, type MenuItem } from "./context-menu";
import { Icon } from "./icons";
import {
  gitlabHosts, gitlabIssues, gitlabProject, issueBranch, issuePrompt, jiraIssues, jiraTransition, jiraTransitions, linearIssues, linearSetState, linearStates, startOnIssue,
  type Issue, type IssueSource,
} from "../lib/integrations";
import type { Project } from "../lib/types";
import { t, errorText } from "../lib/i18n";
import { LoadBar } from "./load-bar";

const LABEL: Record<IssueSource, string> = { linear: "Linear", jira: "Jira", gitlab: "GitLab" };

export function IssuesPanel(props: { project: Project }) {
  const settings = () => store.state.settings.integrations ?? { linear: false, jira: null, gitlabHosts: [] };
  // Re-checked when GitLab hosts are added or removed in Settings.
  const [gitlab] = createResource(() => `${props.project.path}|${gitlabHosts().join(",")}`, () => gitlabProject(props.project.path).catch(() => null));
  const sources = (): IssueSource[] => [
    ...(settings().linear ? ["linear" as const] : []),
    ...(settings().jira ? ["jira" as const] : []),
    ...(gitlab() ? ["gitlab" as const] : []),
  ];
  const [source, setSource] = createSignal<IssueSource | null>(null);
  const current = () => (source() && sources().includes(source()!) ? source() : sources()[0] ?? null);
  const [query, setQuery] = createSignal("");
  const [search, setSearch] = createSignal("");
  const [open, setOpen] = createSignal<string | null>(null);
  const [issues, { refetch }] = createResource(
    () => (current() ? { src: current()!, q: search(), root: props.project.path } : null),
    async ({ src, q, root }) => {
      if (src === "linear") return linearIssues(q || undefined);
      if (src === "jira") return jiraIssues(settings().jira!, q || undefined);
      return gitlabIssues(root, q || undefined);
    },
  );
  createEffect(on(current, () => setOpen(null), { defer: true }));

  const repoOf = () => props.project.parentId ?? props.project.id;
  const moveMenu = async (e: MouseEvent, issue: Issue) => {
    const at = { clientX: e.clientX, clientY: e.clientY } as MouseEvent;
    try {
      const states = issue.source === "linear" ? await linearStates(issue.id) : await jiraTransitions(settings().jira!, issue.key);
      const items: MenuItem[] = states.map((s) => ({
        label: `${s.name === issue.state ? "● " : ""}${s.name}`,
        run: () =>
          void (issue.source === "linear" ? linearSetState(issue.id, s.id) : jiraTransition(settings().jira!, issue.key, s.id))
            .then(() => { store.notify(`${issue.key} → ${s.name}`); void refetch(); })
            .catch((err) => store.notify(String(err), undefined, 8000)),
      }));
      openContextMenu(new MouseEvent("contextmenu", at), items.length ? items : [{ label: t("No other states"), run: () => {} }]);
    } catch (err) {
      store.notify(String(err), undefined, 8000);
    }
  };

  return (
    <div class="issues-panel">
      <LoadBar when={issues.loading} />
      <Show
        when={sources().length}
        fallback={
          <div class="muted issues-empty">
            {t("Connect Linear or Jira, or open a project whose remote is on GitLab, to see your issues here.")}
            <button class="btn" style={{ "margin-top": "10px" }} onClick={() => store.openSettings("integrations")}>{t("Settings → Integrations")}</button>
          </div>
        }
      >
        <div class="issues-head">
          <div class="toggles">
            <For each={sources()}>{(s) => <button class={`toggle${current() === s ? " on" : ""}`} onClick={() => setSource(s)}>{LABEL[s]}</button>}</For>
          </div>
          <button class="icon-btn" title={t("Refresh")} onClick={() => void refetch()}><Icon name="refresh" size={14} /></button>
        </div>
        <form class="issues-search" onSubmit={(e) => { e.preventDefault(); setSearch(query().trim()); }}>
          <Icon name="search" size={13} />
          <input placeholder={search() ? t("Search issues") : t("Assigned to you · type to search")} value={query()} onInput={(e) => { setQuery(e.currentTarget.value); if (!e.currentTarget.value) setSearch(""); }} />
        </form>
        <Show when={!issues.error} fallback={<div class="form-note error" style={{ margin: "8px" }}>{errorText(issues.error)}</div>}>
          <div class="issues-list">
            <Show when={!issues.loading || issues()} fallback={<div class="muted">{t("Loading…")}</div>}>
              <For each={issues()} fallback={<div class="muted">{search() ? t("Nothing matches.") : t("Nothing assigned to you is open.")}</div>}>
                {(it) => (
                  <div class={`issue-row${open() === it.id ? " open" : ""}`} data-issue={it.key}>
                    <div class="issue-line" onClick={() => setOpen(open() === it.id ? null : it.id)}>
                      <span class="issue-key">{it.key}</span>
                      <span class="issue-title" title={it.title}>{it.title}</span>
                      <span class={`issue-state k-${it.state_kind}`}>{it.state}</span>
                    </div>
                    <Show when={open() === it.id}>
                      <div class="issue-detail">
                        <Show when={it.priority || it.team}><div class="run-task-meta"><Show when={it.priority}><span>{it.priority}</span></Show><Show when={it.team}><span>{it.team}</span></Show></div></Show>
                        <Show when={it.description}><div class="issue-desc">{it.description}</div></Show>
                        <div class="run-task-actions">
                          <button class="btn primary" title={t("Start an agent on this issue in this project")} onClick={() => void startOnIssue(props.project, it)}>{t("Agent here")}</button>
                          <Show when={store.projectInfo[repoOf()]?.is_repo}>
                            <button class="btn" title={t("New worktree on branch {branch}, then an agent on the issue", { branch: issueBranch(it) })} onClick={() => openWorktreeDialog(repoOf(), issueBranch(it), (wt) => void startOnIssue(wt, it))}>
                              <Icon name="branch" size={12} /> {t("Worktree + agent")}
                            </button>
                          </Show>
                          <button class="btn" title={t("Send the issue to a running agent")} onClick={(e) => chooseAgentAndSend(e, props.project.id, issuePrompt(it))}>{t("Send to agent…")}</button>
                          <Show when={it.source !== "gitlab"}>
                            <button class="btn" onClick={(e) => void moveMenu(e, it)}>{t("Move…")}</button>
                          </Show>
                          <button class="icon-btn" title={t("Open in the browser")} onClick={() => void openUrl(it.url)}><Icon name="external" size={13} /></button>
                        </div>
                      </div>
                    </Show>
                  </div>
                )}
              </For>
            </Show>
          </div>
        </Show>
      </Show>
    </div>
  );
}
