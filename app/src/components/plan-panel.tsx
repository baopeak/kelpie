// The Plan tab: the project's specs (plans/<name>/plan.md + phase-*.md, specs/**, .kiro/specs/**,
// docs/plans/**) as phases and tasks with their checkboxes and progress. A task can be handed to an
// agent (Run with…: here, in a worktree of its own, or on an SSH host) through the Runs engine; the row then
// shows that run's status. Ticking a box writes the file, only if it is still as it was read. A remote project
// (a folder on a host) has the tab too: its files are read and written over ssh, and there is no watcher, so
// the panel is read again on demand.
import { For, Index, Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { store } from "../lib/store";
import { kindInfo, launchableAgents } from "../lib/agents";
import { b64ToBytes, createEntry, listDir, onEvent, readFile, redact, writeFile } from "../lib/ipc";
import { expandMentions, MAX_TOTAL_BYTES, type MentionContext } from "../lib/mentions";
import { RUN_HOST_API, STATUS_LABEL, createSpecRun, finishedTask, specTaskRuns } from "../lib/runs";
import { MAX_BYTES, SpecError, createSpecIndex, groupSpecs, specFileMention, specSlug, stripControls, taskInstructions, taskRunTitle, type SpecDoc, type SpecGroup, type SpecIo, type SpecTask } from "../lib/spec-index";
import { parseSpecAsync } from "../lib/spec-parse-client";
import { newSpecAsked, setNewSpecAsked } from "../lib/quick-commands";
import { planBringing } from "../lib/try-agents";
import { projectTarget } from "../lib/remote";
import { projectHosts } from "../lib/ssh-hosts";
import { confirmRootStart } from "../lib/root-user";
import { prefetchHost, resolveRunOn, runOnAgents } from "./host-target";
import { gitState } from "../lib/git-state";
import { errorText, t } from "../lib/i18n";
import { confirmDialog } from "../lib/confirm-dialog";
import { openRuns } from "./runs-page";
import { Icon } from "./icons";
import type { Project, SessionTarget } from "../lib/types";
import "./plan-panel.css";

const windows = /Win/.test(navigator.platform);

/** The project's folder as the index sees it: every call is confined to the project root by the backend. */
function diskIo(root: string): SpecIo {
  return {
    list: (dir) => listDir(root, dir).catch(() => null),
    read: (path) =>
      readFile(root, path, MAX_BYTES + 1, true).then(
        (f) => ({ bytes: b64ToBytes(f.base64 ?? ""), mtime: f.mtime, truncated: f.truncated }),
        () => null,
      ),
    write: (path, text, expectedMtime, expectedCksum) => writeFile(root, path, text, expectedMtime, false, expectedCksum, true),
    create: (path) => createEntry(root, path, false),
    // A big file is parsed in a worker, so the page never waits for it.
    parse: parseSpecAsync,
    watch(cb) {
      let off: (() => void) | undefined;
      let stopped = false;
      void onEvent<{ root: string; dirs: string[] }>("fs-changed", (e) => e.root === root && cb(e.dirs)).then((un) => (stopped ? un() : (off = un)));
      return () => {
        stopped = true;
        off?.();
      };
    },
  };
}

/** The prompt for one task: what to do, then the spec file as the agent's own kind takes it (`@path`, or the file's content). */
async function taskPrompt(project: Project, doc: SpecDoc, task: SpecTask, kind: string, onHost: boolean): Promise<string | null> {
  const ctx: MentionContext = {
    stat: (path) => readFile(project.path, path, 1).then(() => ({ dir: false }), () => null),
    readText: (path) => readFile(project.path, path, MAX_TOTAL_BYTES + 1).then((f) => ({ text: f.text ?? "", binary: f.binary, big: f.truncated }), () => null),
    selection: async () => null,
    diff: async () => "",
    terminal: async () => null,
    issue: async () => null,
    notes: () => [],
    redact,
    // Only the file's own path is mentioned: nothing here has text without a file to stash.
    stash: async () => {
      throw new Error("nothing to stash");
    },
  };
  const out = await expandMentions(specFileMention(doc.path), ctx, kind, { inline: true, windows: windows && !onHost, remote: onHost });
  if (out.refused.length || out.problems.length) {
    store.notify(t("{file} cannot be given to the agent: it is missing or its name cannot be written safely", { file: doc.path }), undefined, 8000);
    return null;
  }
  // The task's own words come after the expansion: nothing in a heading can become a mention.
  // Nothing from the file that a terminal would act on: no escape sequences, no bidirectional or invisible characters.
  return `${taskInstructions(task)}\n\n${stripControls(out.text)}`;
}

function Progress(props: { done: number; total: number }) {
  return (
    <span class="run-progress" title={t("{done} of {total} tasks done", { done: props.done, total: props.total })}>
      <span class="meter-bar"><span style={{ width: `${(props.done / Math.max(1, props.total)) * 100}%` }} /></span>
      {props.done}/{props.total}
    </span>
  );
}

export default function PlanPanel(props: { project: Project }) {
  const project = props.project;
  const [docs, setDocs] = createSignal<SpecDoc[]>([]);
  const [ready, setReady] = createSignal(false);
  const [capped, setCapped] = createSignal(false);
  const index = createSpecIndex(diskIo(project.path), (list) => {
    setDocs(list);
    setCapped(index.capped());
    setReady(true);
  });
  onMount(() => index.start());
  onCleanup(() => index.stop());

  const groups = createMemo(() => groupSpecs(docs()));
  // The latest run of each task, built once per change of the runs: rows only look their id up.
  const byTask = createMemo(() => specTaskRuns(project.id));
  const totals = createMemo(() => groups().reduce((n, g) => ({ done: n.done + g.done, total: n.total + g.total }), { done: 0, total: 0 }));

  const [toggled, setToggled] = createSignal<Record<string, boolean>>({});
  const isOpen = (g: SpecGroup) => toggled()[g.id] ?? g.done < g.total;

  // ---------- ticking ----------
  async function tick(doc: SpecDoc, task: SpecTask) {
    if (task.line < 0) return store.notify(t("This line could not be located in {file}; edit the box in the file", { file: doc.name }), undefined, 6000);
    const result = await index.tick(doc, task, !task.checked);
    if (result === "stale") store.notify(t("{file} changed on disk. It was read again and nothing was written; tick the box again.", { file: doc.name }), undefined, 7000);
    else if (result === "busy") store.notify(t("{file} is still being written: tick the box again in a moment", { file: doc.name }), undefined, 4000);
    else if (result === "failed") store.notify(t("Could not update {file}: it is not a plain text file in this project", { file: doc.name }), undefined, 7000);
  }

  // ---------- Run with… ----------
  // Where: "here" (the project's folder; for a remote project, its host), "worktree", or `host:<id>` for a saved
  // host linked to this local project (the project's folder there). A task on a host needs a daemon that knows it.
  const hostsKnown = () => store.daemonApi() >= RUN_HOST_API;
  const [runFor, setRunFor] = createSignal<string | null>(null);
  const [where, setWhere] = createSignal("here");
  const canWorktree = () => !project.remote && !!store.projectInfo[project.parentId ?? project.id]?.is_repo;
  const choice = () => (where() === "worktree" && !canWorktree()) || (where().startsWith("host:") && (project.remote || !hostsKnown())) ? "here" : where();
  const hostId = () => (choice().startsWith("host:") ? choice().slice(5) : null);
  const agents = () => {
    const hidden = store.state.settings.hiddenAgents;
    const own = runOnAgents(project, { hostId: hostId(), path: "" }, hidden);
    return own.length ? own : launchableAgents(hidden);
  };
  const [agent, setAgent] = createSignal(agents()[0] ?? "claude");
  const kindNow = () => (agents().includes(agent()) ? agent() : agents()[0] ?? agent());
  const [starting, setStarting] = createSignal(false);
  /** A remote project's tasks run on its host: that needs a daemon that knows hosts. */
  const runnable = () => !project.remote || hostsKnown();
  const uncommitted = (doc: SpecDoc) => choice() === "worktree" && !!gitState.status()?.files.some((f) => f.path === doc.path);

  async function start(doc: SpecDoc, task: SpecTask) {
    if (starting()) return;
    setStarting(true);
    try {
      const kind = kindNow();
      const wt = choice() === "worktree";
      // The folder on the host: the remote project's own, or the one linked to this project there.
      let host: SessionTarget | undefined = projectTarget(project);
      if (hostId()) {
        const target = await resolveRunOn(project, { hostId: hostId(), path: "" }, kind);
        if (!target) return;
        host = target;
      }
      if (host && !host.path.startsWith("/")) return store.notify(t("The folder on {host} must be an absolute path", { host: host.dest }), undefined, 8000);
      if (host && !(await confirmRootStart(kind, store.profileLaunch(kind).args, host, project.id))) return;
      // The agent of a failed run on a host may still be running there: another would work on the same files.
      const before = byTask().get(task.id);
      if (host && before?.task.host && before.task.status === "failed" && !(await confirmDialog(t("The previous agent on {host} may still be running there for a few minutes after its connection was lost. Start another anyway?", { host: before.task.host.dest })))) return;
      const prompt = await taskPrompt(project, doc, task, kind, !!host);
      if (prompt === null) return;
      // Only what the user approved in `.kelpie.yaml` is brought into a worktree.
      const bringing = wt ? await planBringing(project) : undefined;
      await createSpecRun(project, { id: task.id, title: taskRunTitle(doc, task), prompt: await redact(prompt) }, kind, wt, bringing?.approved ? { copy: bringing.copy, link: bringing.link } : undefined, host);
      setRunFor(null);
    } catch (e) {
      store.notify(t("Could not start the agent: {error}", { error: errorText(e) }), undefined, 8000);
    } finally {
      setStarting(false);
    }
  }

  // ---------- New spec ----------
  const [creating, setCreating] = createSignal(false);
  const [name, setName] = createSignal("");
  let nameInput: HTMLInputElement | undefined;
  const slug = () => specSlug(name());
  createEffect(() => {
    if (!newSpecAsked()) return;
    setNewSpecAsked(false);
    setCreating(true);
    queueMicrotask(() => nameInput?.focus());
  });

  async function create() {
    if (!slug()) return;
    try {
      const made = await index.createSpec(name());
      setCreating(false);
      setName("");
      store.openFile(project.id, made.files[0]);
    } catch (e) {
      const why = e instanceof SpecError ? e : null;
      store.notify(
        why?.code === "name" ? t("Use letters or digits in the name of the spec")
          : why?.code === "exists" ? t("{file} already exists. Nothing was written.", { file: why.message })
          : why?.code === "symlink" ? t("{dir} is not a plain folder of this project (it is a link or a file). Nothing was written.", { dir: why.message })
          : t("Could not write the spec: {error}", { error: errorText(e) }),
        undefined, 8000,
      );
    }
  }

  // ---------- rows ----------
  function Row(rp: { doc: SpecDoc; task: SpecTask }) {
    const found = () => byTask().get(rp.task.id);
    const live = () => !!found() && !finishedTask(found()!.task.status);
    return (
      <>
        <div class={`plan-task${rp.task.checked ? " done" : ""}`} data-task={rp.task.id} data-line={rp.task.line + 1} style={{ "padding-left": `${6 + rp.task.depth * 14}px` }}>
          <button class="plan-box" role="checkbox" aria-checked={rp.task.checked} aria-label={rp.task.text} title={rp.task.checked ? t("Mark as not done") : t("Mark as done")} onClick={() => void tick(rp.doc, rp.task)}>
            <Show when={rp.task.checked}><Icon name="check" size={12} /></Show>
          </button>
          <span class="plan-text" title={`${rp.task.section ? rp.task.section + " · " : ""}${t("line {n}", { n: rp.task.line + 1 })}`} onClick={() => store.openFile(project.id, rp.doc.path, { line: rp.task.line + 1 })}>{rp.task.text || t("(empty)")}</span>
          <Show when={found()}>
            {(f) => (
              <button class={`run-status plan-run st-${f().task.status}`} title={f().task.error ?? t("Open this run")} onClick={() => openRuns(false, f().run.id)}>{t(STATUS_LABEL[f().task.status])}</button>
            )}
          </Show>
          <Show when={runnable() && !live() && !rp.task.checked}>
            <button class="icon-btn plan-run-with" title={t("Run with…")} aria-label={t("Run with…")} onClick={() => setRunFor(runFor() === rp.task.id ? null : rp.task.id)}><Icon name="bolt" size={13} /></button>
          </Show>
        </div>
        <Show when={runFor() === rp.task.id}>
          <div class="plan-runwith" data-runwith={rp.task.id}>
            <select class="text-input" value={kindNow()} onChange={(e) => setAgent(e.currentTarget.value)}>
              <Index each={agents()}>{(k) => <option value={k()}>{kindInfo(k()).label}</option>}</Index>
            </select>
            <select class="text-input" value={choice()} onChange={(e) => { setWhere(e.currentTarget.value); prefetchHost(e.currentTarget.value.startsWith("host:") ? e.currentTarget.value.slice(5) : null); }}>
              <option value="here">{project.remote ? t("On {host}", { host: project.remote.dest }) : t("Here (this folder)")}</option>
              <Show when={!project.remote}>
                <option value="worktree" disabled={!canWorktree()}>{t("In a worktree of its own")}</option>
                <Show when={hostsKnown()}>
                  <For each={projectHosts(project)}>{(l) => <option value={`host:${l.host.id}`}>{t("On {host}", { host: l.host.label })}</option>}</For>
                </Show>
              </Show>
            </select>
            <button class="btn primary" disabled={starting()} onClick={() => void start(rp.doc, rp.task)}>{t("Start")}</button>
            <button class="btn" onClick={() => setRunFor(null)}>{t("Cancel")}</button>
            <Show when={hostId() || project.remote}>
              <p class="muted-inline">{t("On a host the agent runs in the ssh session, not in tmux: the task is done when the agent exits cleanly, and failed when it stops or the connection drops. The agent ends with its connection once the host notices, which can take a few minutes: starting it again before then needs your confirmation.")}</p>
            </Show>
            <Show when={hostId()}>
              <p class="muted-inline">{t("The agent reads its own copy of this file in the project's folder on the host; ticks are made there.")}</p>
            </Show>
            <Show when={uncommitted(rp.doc)}>
              <p class="muted-inline">{t("A worktree starts from the last commit: this file has changes that are not committed, which the agent will not see.")}</p>
            </Show>
          </div>
        </Show>
      </>
    );
  }

  function Doc(dp: { doc: SpecDoc; heading: boolean }) {
    const d = () => dp.doc;
    return (
      <div class="plan-doc" data-path={d().path}>
        <Show when={dp.heading}>
          <div class="plan-doc-head">
            <span class="plan-doc-name" title={d().path} onClick={() => store.openFile(project.id, d().path)}>{d().name}</span>
            <Show when={d().total}><Progress done={d().done} total={d().total} /></Show>
          </div>
        </Show>
        <Show when={d().problem}>
          <div class="plan-problem muted-inline">{d().problem === "big" ? t("Too large to read here; open it in the editor.") : d().problem === "slow" ? t("Took too long to read here; open it in the editor.") : t("Not UTF-8 text; open it in the editor.")}</div>
        </Show>
        {/* Index: a tick or an agent's edit gives the tasks new objects, and the rows stay where they are. */}
        <Index each={d().tasks}>{(task) => <Row doc={d()} task={task()} />}</Index>
        <Show when={!d().problem && !d().total && d().plain.length}>
          <ul class="plan-plain" title={t("No checkboxes in this file; its list items are shown as they are.")}>
            <For each={d().plain}>{(item) => <li>{item}</li>}</For>
          </ul>
        </Show>
      </div>
    );
  }

  return (
    <div class="plan-panel">
      <div class="plan-head">
        <Show when={totals().total} fallback={<span class="muted-inline">{ready() ? t("No tasks yet") : t("Reading specs…")}</span>}>
          <Progress done={totals().done} total={totals().total} />
        </Show>
        <span style={{ flex: 1 }} />
        <Show when={project.remote}><span class="muted-inline" title={t("Changes on the host are not watched: use Refresh")}>{t("on {host}", { host: project.remote!.dest })}</span></Show>
        <button class="icon-btn" title={t("Read the specs again")} aria-label={t("Read the specs again")} onClick={() => void index.refresh()}><Icon name="refresh" size={13} /></button>
        <button class="btn" data-new-spec onClick={() => { setCreating(!creating()); queueMicrotask(() => nameInput?.focus()); }}><Icon name="plus" size={12} /> {t("New spec")}</button>
      </div>
      <Show when={creating()}>
        <form class="plan-new" onSubmit={(e) => { e.preventDefault(); void create(); }}>
          <input ref={nameInput} class="text-input" value={name()} placeholder={t("Name of the spec")} maxLength={80} onInput={(e) => setName(e.currentTarget.value)} onKeyDown={(e) => e.key === "Escape" && setCreating(false)} />
          <Show when={slug()} fallback={<span class="muted-inline">{t("Requirements, then design, then tasks are written to specs/<name>/.")}</span>}>
            <span class="muted-inline mono">{`specs/${slug()}/`}{" "}{t("requirements, design, tasks")}</span>
          </Show>
          <div class="plan-new-actions">
            <button type="submit" class="btn primary" disabled={!slug()}>{t("Create")}</button>
            <button type="button" class="btn" onClick={() => setCreating(false)}>{t("Cancel")}</button>
          </div>
        </form>
      </Show>
      <div class="plan-list">
        <For each={groups()} fallback={
          <Show when={ready()}>
            <div class="muted plan-empty">
              {t("No specs found here.")}
              <p>{t("Plans in plans/<name>/ (plan.md and phase-*.md), specs/, .kiro/specs/ and docs/plans/ are listed with their checkboxes. Any agent can write them; New spec starts one.")}</p>
            </div>
          </Show>
        }>
          {(g) => (
            <div class="plan-group" data-group={g.id}>
              <button class="plan-group-head" aria-expanded={isOpen(g)} onClick={() => setToggled({ ...toggled(), [g.id]: !isOpen(g) })}>
                <Icon name={isOpen(g) ? "chevronDown" : "chevronRight"} size={12} />
                <span class="plan-group-name" title={g.id}>{g.title}</span>
                <Show when={g.title !== g.name}><span class="plan-group-dir">{g.name}</span></Show>
                <span style={{ flex: 1 }} />
                <Show when={g.total}><Progress done={g.done} total={g.total} /></Show>
              </button>
              <Show when={isOpen(g)}>
                <For each={g.docs}>{(d) => <Doc doc={d} heading={g.docs.length > 1} />}</For>
              </Show>
            </div>
          )}
        </For>
        <Show when={capped()}><p class="muted-inline plan-empty">{t("Only the first files are listed.")}</p></Show>
      </div>
    </div>
  );
}
