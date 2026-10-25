// The candidates of a run side by side: what each agent did (status, time, cost, files, +/−,
// checks, turns), two candidates' versions of a file in a split diff, and "Use this one", which
// hands the winner to Finish and removes the others once it has merged. Loaded with the Runs page's
// Compare tab; it reads each worktree once and again when a task changes state, never on a timer
// (only the elapsed time of a candidate that is still running ticks).
import { For, Index, Show, createEffect, createMemo, createSignal, on, onCleanup } from "solid-js";
import { MergeView } from "@codemirror/merge";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { store } from "../lib/store";
import { gitFileVersions } from "../lib/ipc";
import { baseExtensions, languageFor } from "../lib/codemirror";
import { confirmDialog } from "../lib/confirm-dialog";
import { formatCost } from "../lib/usage-format";
import { buildCandidates, bestOf, changedFiles, discardCandidate, formatDuration, readFacts, useCandidate, type Candidate, type Facts } from "../lib/run-compare";
import { STATUS_LABEL, type Run } from "../lib/runs";
import { KindGlyph } from "./icons";
import { t } from "../lib/i18n";
import "./run-compare.css";

/** Two candidates' working-tree versions of one file, side by side. */
function CandidateDiff(props: { left: Candidate; right: Candidate; path: string }) {
  let host!: HTMLDivElement;
  let view: MergeView | undefined;
  const [missing, setMissing] = createSignal<string[]>([]);

  async function render() {
    const read = (c: Candidate) => gitFileVersions(c.cwd!, props.path, false).then((v) => v.modified, () => null);
    const [a, b] = await Promise.all([read(props.left), read(props.right)]);
    const lang = await languageFor(props.path);
    const exts = [...baseExtensions({ readOnly: true }), ...(lang ? [lang] : []), EditorView.editable.of(false), EditorState.readOnly.of(true)];
    view?.destroy();
    host.replaceChildren();
    setMissing([a == null ? props.left.title : "", b == null ? props.right.title : ""].filter(Boolean));
    view = new MergeView({ parent: host, a: { doc: a ?? "", extensions: exts }, b: { doc: b ?? "", extensions: exts }, gutter: true, highlightChanges: true, collapseUnchanged: { margin: 3, minSize: 8 } });
  }
  createEffect(on(() => [props.left.id, props.right.id, props.path], () => void render()));
  onCleanup(() => view?.destroy());

  return (
    <div class="compare-diff">
      <Show when={missing().length}><div class="muted-inline">{t("{names}: the file does not exist there.", { names: missing().join(", ") })}</div></Show>
      <div class="cm-host" ref={host} data-compare-diff />
    </div>
  );
}

export default function RunCompare(props: { run: Run }) {
  const [facts, setFacts] = createSignal<Record<string, Facts | undefined>>({});
  const [reading, setReading] = createSignal(false);
  const [now, setNow] = createSignal(Date.now());
  const rows = createMemo(() => buildCandidates(props.run, facts(), now()));
  const best = createMemo(() => bestOf(rows()));
  const files = createMemo(() => changedFiles(rows()));
  const [leftId, setLeftId] = createSignal<string | null>(null);
  const [rightId, setRightId] = createSignal<string | null>(null);
  const [path, setPath] = createSignal<string | null>(null);
  const byId = (id: string | null) => rows().find((r) => r.id === id);
  const left = () => byId(leftId()) ?? rows()[0];
  const right = () => byId(rightId()) ?? rows()[1] ?? rows()[0];

  async function load(force: boolean) {
    setReading(true);
    try {
      setFacts(await readFacts(props.run, force));
    } finally {
      setReading(false);
    }
  }
  // Read when the view opens and whenever a task starts, ends or moves.
  createEffect(on(() => props.run.tasks.map((x) => `${x.id}:${x.status}:${x.cwd}`).join("|"), () => void load(false)));
  // The elapsed time of a candidate that is still working moves; nothing ticks once all have ended.
  const ticking = createMemo(() => rows().some((r) => r.running && r.durationMs != null));
  createEffect(() => {
    if (!ticking()) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    onCleanup(() => clearInterval(timer));
  });

  const metrics: { key: string; label: () => string; value: (r: Candidate) => string; title?: (r: Candidate) => string; best?: "fastest" | "cheapest" | "smallest" }[] = [
    { key: "duration", label: () => t("Time"), value: (r) => formatDuration(r.durationMs), best: "fastest" },
    { key: "cost", label: () => t("Cost"), value: (r) => (r.cost ? formatCost(r.cost, false) : "–"), best: "cheapest" },
    { key: "files", label: () => t("Files"), value: (r) => (r.files ? String(r.files.length) : "–"), title: (r) => (r.uncommitted ? t("{n} uncommitted", { n: r.uncommitted }) : "") },
    { key: "lines", label: () => "+ / −", value: (r) => (r.added == null ? "–" : `+${r.added} −${r.deleted}`), best: "smallest" },
    { key: "checks", label: () => t("Checks"), value: (r) => r.checks?.text ?? "–", title: (r) => r.checks?.title ?? "" },
    { key: "turns", label: () => t("Turns"), value: (r) => (r.turns == null ? "–" : String(r.turns)) },
  ];

  const discard = async (r: Candidate) => {
    const message = t("Remove the worktree of “{name}”? Its agent is stopped and what it has not committed is saved in a commit on branch {branch}, which stays unless it is merged.", { name: r.title, branch: r.branch ?? "" });
    if (!(await confirmDialog(message))) return;
    try {
      await discardCandidate(props.run, r);
    } catch (e) {
      store.notify(String(e).replace(/^Error: /, ""), undefined, 8000);
    }
    void load(true);
  };

  return (
    <div class="run-compare" data-run-compare>
      <div class="compare-bar">
        <span class="muted-inline">{t("The same work by {n} agents. Pick the best: its Finish merges it and the others are removed.", { n: rows().length })}</span>
        <span style={{ flex: 1 }} />
        <button class="btn" disabled={reading()} onClick={() => void load(true)}>{reading() ? t("Reading…") : t("Refresh")}</button>
      </div>
      <div class="compare-grid" style={{ "--cols": rows().length }}>
        <div class="compare-label" />
        <For each={rows()}>
          {(r) => (
            <div class="compare-head" data-cand={r.id}>
              <KindGlyph kind={r.kind} size={16} />
              <span class="compare-title" title={r.branch ?? ""}>{r.title}</span>
              <span class={`run-status st-${r.status}`}>{t(STATUS_LABEL[r.status])}</span>
            </div>
          )}
        </For>
        <For each={metrics}>
          {(m) => (
            <>
              <div class="compare-label">{m.label()}</div>
              <For each={rows()}>
                {(r) => (
                  <div class={`compare-cell${m.best && best()[m.best] === r.id ? " best" : ""}`} data-cand={r.id} data-metric={m.key} title={m.title?.(r) || (m.best && best()[m.best] === r.id ? t("Best of the finished candidates") : "")}>
                    {m.value(r)}{m.best && best()[m.best] === r.id ? " ★" : ""}
                  </div>
                )}
              </For>
            </>
          )}
        </For>
        <div class="compare-label">{t("Result")}</div>
        <For each={rows()}>{(r) => <div class="compare-cell compare-summary" data-cand={r.id} data-metric="summary" title={r.summary ?? ""}>{r.summary?.split("\n")[0] ?? "–"}</div>}</For>
        <div class="compare-label" />
        <For each={rows()}>
          {(r) => (
            <div class="compare-actions" data-cand={r.id}>
              <Show when={r.sessionId && store.sessions[r.sessionId]}>
                <button class="btn" onClick={() => { store.setOverlay(null); store.revealSession(r.sessionId!); }}>{t("Open session")}</button>
              </Show>
              <button class="btn primary" data-use disabled={!r.pickable} title={r.pickable ? t("Finish this one and remove the others") : t("Its worktree is not available")} onClick={() => void useCandidate(props.run, rows(), r)}>{t("Use this one")}</button>
              <button class="btn" data-discard disabled={!r.projectId} onClick={() => void discard(r)}>{t("Discard…")}</button>
            </div>
          )}
        </For>
      </div>

      <Show when={files().length} fallback={<div class="muted-inline compare-empty">{reading() ? t("Reading changes…") : t("No candidate has changed a file yet.")}</div>}>
        <div class="compare-pick">
          <label class="field"><span>{t("Left")}</span>
            <select class="text-input" value={left()?.id} onChange={(e) => setLeftId(e.currentTarget.value)}>
              <For each={rows()}>{(r) => <option value={r.id}>{r.title}</option>}</For>
            </select>
          </label>
          <label class="field"><span>{t("Right")}</span>
            <select class="text-input" value={right()?.id} onChange={(e) => setRightId(e.currentTarget.value)}>
              <For each={rows()}>{(r) => <option value={r.id}>{r.title}</option>}</For>
            </select>
          </label>
        </div>
        <div class="compare-files">
          <Index each={files()}>
            {(f) => (
              <button class={`compare-file${path() === f().path ? " on" : ""}`} data-file={f().path} title={f().path} onClick={() => setPath(f().path)}>
                <span class="compare-file-name">{f().path}</span>
                <span class="muted-inline">{f().by.length === rows().length ? t("all") : f().by.map((id) => byId(id)?.title ?? id).join(", ")}</span>
              </button>
            )}
          </Index>
        </div>
        <Show when={path() && left()?.cwd && right()?.cwd && left()!.id !== right()!.id}
          fallback={<div class="muted-inline compare-empty">{path() ? t("Choose two different candidates.") : t("Click a file to see two candidates' versions side by side.")}</div>}>
          <CandidateDiff left={left()!} right={right()!} path={path()!} />
        </Show>
      </Show>
    </div>
  );
}
