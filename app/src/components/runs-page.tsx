// Orchestration runs page: the runs on the left; the selected run's tasks in columns by
// dependency depth, with what each needs; and an editor for new runs (tasks written by hand or
// planned by an AI agent, then reviewed).
import { For, Index, Show, createMemo, createResource, createSignal, lazy, onCleanup, onMount } from "solid-js";
import { store } from "../lib/store";
import { kindInfo, launchableAgents } from "../lib/agents";
import { relTimeShort } from "../lib/platform";
import { formatRelative, t } from "../lib/i18n";
import { STATUS_LABEL, broughtText, cancelRun, createRun, deleteRun, finishedTask, lanes, planTasks, runList, runsById, taskAction, type Run, type RunTask, type TaskDraft } from "../lib/runs";
import { Icon, KindGlyph } from "./icons";
import { confirmDialog } from "../lib/confirm-dialog";

// Loaded when a run's candidates are compared.
const RunCompare = lazy(() => import("./run-compare"));

const [selected, setSelected] = createSignal<string | null>(null);
const runStatusText = (s: string) => (s === "running" ? t("running") : s === "done" ? t("done") : s === "failed" ? t("failed") : s === "cancelled" ? t("cancelled") : s);
const [editing, setEditing] = createSignal(false);

export function openRuns(newRun = false, runId?: string) {
  setEditing(newRun);
  if (runId) setSelected(runId);
  store.setOverlay("runs");
}

function Progress(props: { run: Run }) {
  const done = () => props.run.tasks.filter((t) => finishedTask(t.status)).length;
  return (
    <span class="run-progress" title={t("{done} of {total} tasks finished", { done: done(), total: props.run.tasks.length })}>
      <span class="meter-bar"><span style={{ width: `${(done() / Math.max(1, props.run.tasks.length)) * 100}%` }} /></span>
      {done()}/{props.run.tasks.length}
    </span>
  );
}

function TaskCard(props: { run: Run; task: RunTask }) {
  const [open, setOpen] = createSignal(false);
  const tk = () => props.task;
  const session = () => (tk().session_id ? store.sessions[tk().session_id!] : undefined);
  const act = (a: Parameters<typeof taskAction>[0]) => void taskAction(a, props.run.id, tk().id);
  // The agent of a task on a host may outlive its connection by a few minutes: another one needs the user's word.
  const retry = async () => {
    const host = tk().host;
    if (host && !(await confirmDialog(t("The previous agent on {host} may still be running there for a few minutes after its connection was lost. Start another anyway?", { host: host.dest })))) return;
    act("retry");
  };
  return (
    <div class={`run-task st-${tk().status}`} data-task={tk().id}>
      <div class="run-task-head">
        <KindGlyph kind={tk().kind} size={16} />
        <span class="run-task-title">{tk().title}</span>
        <span class={`run-status st-${tk().status}`}>{t(STATUS_LABEL[tk().status])}</span>
      </div>
      <div class="run-task-meta">
        <span class="mono">{tk().id}</span>
        <Show when={tk().deps.length}><span>{t("after {deps}", { deps: tk().deps.join(", ") })}</span></Show>
        <Show when={tk().branch}><span class="mono" title={tk().cwd ?? ""}><Icon name="branch" size={11} /> {tk().branch}</span></Show>
        <Show when={tk().approve}><span>{t("approval")}</span></Show>
        <Show when={tk().host}><span class="mono" title={`${tk().host!.dest}:${tk().host!.path}`}>{t("on {host}", { host: tk().host!.dest })}</span></Show>
        <Show when={tk().brought}><span title={broughtText(tk().brought!).full}>{broughtText(tk().brought!).short}</span></Show>
      </div>
      <Show when={tk().error}><div class="run-task-summary error">{tk().error}</div></Show>
      <Show when={tk().summary}>
        <div class="run-task-summary" onClick={() => setOpen(!open())} title={t("Click to expand")}>
          {open() ? tk().summary : tk().summary!.split("\n")[0]}
        </div>
      </Show>
      <div class="run-task-actions">
        <Show when={tk().status === "waiting_approval"}>
          <button class="btn primary" onClick={() => act("approve")}>{t("Approve & start")}</button>
        </Show>
        <Show when={session()}>
          <button class="btn" onClick={() => { store.setOverlay(null); store.revealSession(tk().session_id!); }}>{t("Open session")}</button>
        </Show>
        <Show when={tk().status === "running"}>
          <button class="btn" title={t("Mark the task done now")} onClick={() => act("task_done")}>{t("Mark done")}</button>
        </Show>
        <Show when={!finishedTask(tk().status) && tk().status !== "running" && tk().status !== "starting"}>
          <button class="btn" title={t("Do not run this task (tasks after it are skipped too)")} onClick={() => act("task_skip")}>{t("Skip")}</button>
        </Show>
        <Show when={tk().status === "failed" || tk().status === "skipped" || tk().status === "cancelled"}>
          <button class="btn" onClick={() => void retry()}><Icon name="restart" size={12} /> {t("Retry")}</button>
        </Show>
        <Show when={!tk().session_id && !finishedTask(tk().status)}>
          <button class="icon-btn" title={t("Show the prompt")} onClick={() => setOpen(!open())}><Icon name="note" size={13} /></button>
        </Show>
      </div>
      <Show when={open() && !tk().summary && !tk().error}><div class="run-task-prompt">{tk().prompt}</div></Show>
    </div>
  );
}

function RunDetail(props: { run: Run }) {
  const project = () => store.state.projects.find((p) => p.id === props.run.project_id);
  const [comparing, setComparing] = createSignal(false);
  // Two or more tasks in worktrees of their own are candidates for the same work.
  const candidates = () => props.run.tasks.filter((x) => x.worktree && x.cwd).length >= 2;
  return (
    <div class="run-detail">
      <div class="run-detail-head">
        <div style={{ flex: 1, "min-width": 0 }}>
          <h3>{props.run.name} <span class={`run-status st-${props.run.status}`}>{runStatusText(props.run.status)}</span></h3>
          <Show when={props.run.objective}><div class="run-objective">{props.run.objective}</div></Show>
          <div class="run-task-meta"><span>{project()?.name ?? props.run.cwd}</span><span>{t("started {when}", { when: formatRelative(props.run.created_at) })}</span><Progress run={props.run} /></div>
        </div>
        <Show when={candidates()}>
          <button class={`toggle${comparing() ? " on" : ""}`} data-compare onClick={() => setComparing(!comparing())}>{t("Compare")}</button>
        </Show>
        <Show when={props.run.status === "running"} fallback={<button class="btn" onClick={async () => (await confirmDialog(t("Delete the run “{name}”? Its sessions stay open.", { name: props.run.name }))) && void deleteRun(props.run.id).then(() => setSelected(null))}>{t("Delete")}</button>}>
          <button class="btn danger" onClick={async () => (await confirmDialog(t("Cancel “{name}”? Running agents are stopped.", { name: props.run.name }))) && void cancelRun(props.run.id)}>{t("Cancel run")}</button>
        </Show>
      </div>
      <Show when={!(comparing() && candidates())} fallback={<RunCompare run={props.run} />}>
        <div class="run-lanes">
          <For each={lanes(props.run)}>
            {(lane, i) => (
              <div class="run-lane">
                <div class="run-lane-head">{i() === 0 ? t("Start") : t("Step {n}", { n: i() + 1 })}</div>
                <For each={lane}>{(t) => <TaskCard run={props.run} task={t} />}</For>
              </div>
            )}
          </For>
        </div>
      </Show>
    </div>
  );
}

const blankTask = (n: number, kind: string): TaskDraft => ({ id: `t${n}`, title: "", prompt: "", kind, deps: [], approve: false, worktree: false });

function RunEditor(props: { onDone: (id?: string) => void }) {
  const agents = () => launchableAgents(store.state.settings.hiddenAgents);
  const firstAgent = () => agents()[0] ?? "claude";
  const [name, setName] = createSignal("");
  const [objective, setObjective] = createSignal("");
  const [projectId, setProjectId] = createSignal(store.state.activeProject ?? store.state.projects[0]?.id ?? "");
  const [tasks, setTasks] = createSignal<TaskDraft[]>([blankTask(1, firstAgent())]);
  const [planning, setPlanning] = createSignal(false);
  const [idle, setIdle] = createSignal(300);
  const project = () => store.state.projects.find((p) => p.id === projectId());
  // Tasks with a worktree of their own get the repository's approved `copy:` / `link:` files: what that
  // is, and how large, is read (and shown) while the tasks are written.
  const worktreeTasks = () => tasks().filter((t) => t.worktree).length;
  const [bringing] = createResource(
    () => (worktreeTasks() && project() ? project()! : false),
    async (p) => {
      const m = await import("../lib/try-agents");
      return { plan: await m.planBringing(p), text: m.bringingText };
    },
  );
  const update = (i: number, patch: Partial<TaskDraft>) => setTasks(tasks().map((t, j) => (j === i ? { ...t, ...patch } : t)));
  const remove = (i: number) => {
    const gone = tasks()[i].id;
    setTasks(tasks().filter((_, j) => j !== i).map((t) => ({ ...t, deps: t.deps.filter((d) => d !== gone) })));
  };
  const add = () => {
    const used = new Set(tasks().map((t) => t.id));
    let n = tasks().length + 1;
    while (used.has(`t${n}`)) n++;
    setTasks([...tasks(), blankTask(n, firstAgent())]);
  };
  const plan = async () => {
    if (!objective().trim() || !project()) return store.notify(t("Write the objective and choose a project first"));
    setPlanning(true);
    try {
      const p = await planTasks(objective(), project()!.path, store.state.settings.aiAgent ?? "claude", firstAgent());
      setTasks(p.tasks);
      if (!name().trim() && p.name) setName(p.name);
    } catch (e) {
      store.notify(t("Planning failed: {error}", { error: String(e) }), undefined, 8000);
    } finally {
      setPlanning(false);
    }
  };
  const valid = () => !!project() && !!name().trim() && tasks().length > 0 && tasks().every((t) => t.title.trim() && t.prompt.trim() && t.id.trim());
  const start = async () => {
    if (!valid()) return;
    try {
      const run = await createRun({
        name: name().trim(), objective: objective().trim(), cwd: project()!.path, project_id: project()!.id, idle_done_secs: idle(),
        // Each task runs with its agent profile's flags and the active account.
        tasks: tasks().map((t) => ({ ...t, ...store.profileLaunch(t.kind) })),
        // Only what the user approved in `.kelpie.yaml`; nothing for a repository whose file was not.
        ...(worktreeTasks() && bringing()?.plan.approved ? { materialize: { copy: bringing()!.plan.copy, link: bringing()!.plan.link } } : {}),
      });
      props.onDone(run.id);
    } catch (e) {
      store.notify(String(e), undefined, 8000);
    }
  };
  return (
    <div class="run-editor">
      <h3>{t("New run")}</h3>
      <div class="inline-fields">
        <label class="field" style={{ flex: 2 }}><span>{t("Name")}</span><input class="text-input" value={name()} placeholder={t("Add CSV export")} onInput={(e) => setName(e.currentTarget.value)} /></label>
        <label class="field" style={{ flex: 2 }}>
          <span>{t("Project")}</span>
          <select class="text-input" value={projectId()} onChange={(e) => setProjectId(e.currentTarget.value)}>
            <For each={store.state.projects.filter((p) => !p.remote)}>{(p) => <option value={p.id}>{p.name}</option>}</For>
          </select>
        </label>
        <label class="field" style={{ flex: 1 }} title={t("A running task whose agent worked and then stayed idle this long counts as done (0 = only when the agent reports or exits)")}>
          <span>{t("Done after idle (s)")}</span>
          <input class="text-input" type="number" min="0" max="86400" value={idle()} onInput={(e) => setIdle(Math.max(0, Math.min(86400, +e.currentTarget.value || 0)))} />
        </label>
      </div>
      <label class="field"><span>{t("Objective")}</span>
        <textarea class="text-input" rows={2} value={objective()} placeholder={t("What the run should achieve (given to every agent; used for planning)")} onInput={(e) => setObjective(e.currentTarget.value)} />
      </label>
      <div class="inline-fields">
        <button class="btn" disabled={planning() || !objective().trim()} onClick={() => void plan()}>
          <Icon name="sparkle" size={13} /> {planning() ? t("Planning…") : t("Plan tasks with AI")}
        </button>
        <span class="muted-inline">{t("Splits the objective into tasks you can edit before starting.")}</span>
      </div>
      <div class="run-editor-tasks">
        {/* Index, not For: editing a field replaces the task object, and the row (with the
            focused input) must stay. */}
        <Index each={tasks()}>
          {(tk, i) => (
            <div class="run-editor-task" data-draft={i}>
              <div class="inline-fields">
                <label class="field" style={{ width: "92px", flex: "none" }}><span>{t("Id")}</span><input class="text-input mono" value={tk().id} onInput={(e) => update(i, { id: e.currentTarget.value.replace(/[^A-Za-z0-9_-]/g, "") })} /></label>
                <label class="field" style={{ flex: 3 }}><span>{t("Title")}</span><input class="text-input" value={tk().title} onInput={(e) => update(i, { title: e.currentTarget.value })} /></label>
                <label class="field" style={{ flex: 1 }}>
                  <span>{t("Agent")}</span>
                  <select class="text-input" value={tk().kind} onChange={(e) => update(i, { kind: e.currentTarget.value })}>
                    <For each={agents()}>{(k) => <option value={k}>{kindInfo(k).label}</option>}</For>
                  </select>
                </label>
                <button class="icon-btn" style={{ "align-self": "flex-end" }} title={t("Remove task")} onClick={() => remove(i)}><Icon name="close" size={13} /></button>
              </div>
              <textarea class="text-input" rows={3} value={tk().prompt} placeholder={t("Instructions for this agent")} onInput={(e) => update(i, { prompt: e.currentTarget.value })} />
              <div class="inline-fields run-editor-opts">
                <Show when={tasks().length > 1}>
                  <span class="muted-inline">{t("After:")}</span>
                  <For each={tasks().filter((o) => o.id !== tk().id)}>
                    {(o) => (
                      <button class={`toggle${tk().deps.includes(o.id) ? " on" : ""}`} onClick={() => update(i, { deps: tk().deps.includes(o.id) ? tk().deps.filter((d) => d !== o.id) : [...tk().deps, o.id] })}>{o.id}</button>
                    )}
                  </For>
                </Show>
                <span style={{ flex: 1 }} />
                <label class="check-label"><input type="checkbox" checked={tk().worktree} onChange={(e) => update(i, { worktree: e.currentTarget.checked })} /> {t("Own worktree")}</label>
                <label class="check-label"><input type="checkbox" checked={tk().approve} onChange={(e) => update(i, { approve: e.currentTarget.checked })} /> {t("Ask before starting")}</label>
              </div>
            </div>
          )}
        </Index>
        <button class="btn" style={{ "align-self": "flex-start" }} onClick={add}><Icon name="plus" size={13} /> {t("Add task")}</button>
      </div>
      <Show when={worktreeTasks() > 0 ? bringing() : undefined}>
        {(b) => <p class="muted-inline" data-bringing>{b().text(b().plan, worktreeTasks())}</p>}
      </Show>
      <div class="inline-fields" style={{ "justify-content": "flex-end" }}>
        <button class="btn" onClick={() => props.onDone()}>{t("Cancel")}</button>
        <button class="btn primary" disabled={!valid()} onClick={() => void start()}>{t("Start run")}</button>
      </div>
    </div>
  );
}

export function RunsPage() {
  let el!: HTMLDivElement;
  const list = createMemo(runList);
  const current = () => (selected() && runsById[selected()!]) || list()[0];
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape" && !e.defaultPrevented && document.activeElement?.closest(".runs-page")) {
      if (editing()) setEditing(false);
      else store.setOverlay(null);
    }
  };
  onMount(() => {
    window.addEventListener("keydown", onKey);
    el.focus();
  });
  onCleanup(() => window.removeEventListener("keydown", onKey));
  return (
    <div class="runs-page" ref={el} tabIndex={-1}>
      <div class="board-head">
        <button class="settings-back" onClick={() => store.setOverlay(null)} title={t("Back (Esc)")}><Icon name="back" size={14} /></button>
        <h2>{t("Runs")}</h2>
        <span class="muted-inline">{t("Agents working on tasks in order, each reporting back when done.")}</span>
        <span style={{ flex: 1 }} />
        <button class="btn primary" onClick={() => setEditing(true)}><Icon name="plus" size={13} /> {t("New run")}</button>
      </div>
      <div class="runs-body">
        <div class="runs-list">
          <For each={list()} fallback={<div class="muted">{t("No runs yet.")}</div>}>
            {(r) => (
              <div class={`runs-item${current()?.id === r.id && !editing() ? " on" : ""}`} data-run={r.id} onClick={() => { setSelected(r.id); setEditing(false); }}>
                <div class="runs-item-name">{r.name}</div>
                <div class="run-task-meta"><span class={`run-status st-${r.status}`}>{runStatusText(r.status)}</span><Progress run={r} /><span>{relTimeShort(r.created_at)}</span></div>
              </div>
            )}
          </For>
        </div>
        <div class="runs-main">
          <Show when={!editing()} fallback={<RunEditor onDone={(id) => { setEditing(false); if (id) setSelected(id); }} />}>
            <Show when={current()} fallback={<div class="empty"><div class="welcome"><h1>{t("Runs")}</h1><div style={{ color: "var(--text-dim)" }}>{t("Describe an objective, split it into tasks (or let an agent plan them) and Kelpie runs the agents in order — each task can get its own worktree and wait for your approval.")}</div><button class="btn primary" onClick={() => setEditing(true)}>{t("New run")}</button></div></div>}>
              {(r) => <RunDetail run={r()} />}
            </Show>
          </Show>
        </div>
      </div>
    </div>
  );
}
