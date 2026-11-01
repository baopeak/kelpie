// Turns of an agent session: what each turn changed, who touched which files, the files a turn
// produced, and a previewed undo. Loaded only when the panel is opened.
import { For, Match, Show, Switch, createEffect, createMemo, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import { isAgentKind } from "../lib/agents";
import { findPane } from "../lib/layout";
import { relTimeShort } from "../lib/platform";
import { setCovering } from "../lib/overlays";
import { errorText, t } from "../lib/i18n";
import {
  artifactKind, attemptUndo, inProject, listTurns, projectPrefix, safelyTicked, setTurnSnapshots, sourceNote, turnFiles, turnOwners, turnStats, undoPlan, undoTurn, visibleTurns,
  type PlanItem, type TurnFile, type TurnRecord, type UndoPlan,
} from "../lib/checkpoints";
import { Icon } from "./icons";
import type { Project, SessionInfo } from "../lib/types";
import "./turn-timeline.css";

const filesText = (n: number) => (n === 1 ? t("{n} file", { n }) : t("{n} files", { n }));

export default function TurnTimeline(props: { project: Project }) {
  const [view, setView] = createSignal<"turns" | "files">("turns");
  const [choice, setChoice] = createSignal<string | null>(null);
  const [version, setVersion] = createSignal(0);
  /** The turn an undo was asked for: fixed when the dialog opens, whatever the panel shows later. */
  const [undoing, setUndoing] = createSignal<{ session: string; name: string; n: number } | null>(null);

  const sessions = createMemo(() =>
    Object.values(store.sessions)
      .filter((s) => s.project_id === props.project.id && isAgentKind(s.kind) && !s.remote)
      .sort((a, b) => b.last_activity - a.last_activity),
  );
  /** The session in the pane being looked at, else the most recently active one. */
  const focused = (): string | undefined => {
    const ws = store.workspace(props.project.id);
    const tab = ws.tabs.find((x) => x.id === ws.activeTab);
    return (tab && findPane(tab.root, tab.focus)?.sessionId) || undefined;
  };
  // The panel stays on one session until the user picks another (agents printing reorder the list).
  createEffect(() => {
    const list = sessions();
    if (!list.some((s) => s.id === choice())) setChoice((list.find((s) => s.id === focused()) ?? list[0])?.id ?? null);
  });
  const session = (): SessionInfo | undefined => sessions().find((s) => s.id === choice());

  const [turns] = createResource(
    () => {
      const s = session();
      return s ? { id: s.id, v: version() } : false;
    },
    ({ id }) => listTurns(id).catch(() => null),
  );

  // A snapshot finished (any session): refresh when it is the one shown.
  onMount(() => {
    const on = (e: Event) => {
      const sid = (e as CustomEvent).detail?.session;
      if (!sid || sid === session()?.id) setVersion((v) => v + 1);
    };
    window.addEventListener("kelpie:checkpoint", on);
    onCleanup(() => window.removeEventListener("kelpie:checkpoint", on));
  });

  const rows = createMemo(() => (turns() ? visibleTurns(turns()!) : []));
  const prefix = createMemo(() => projectPrefix(turns()?.repo ?? null, props.project.path));
  const why = createMemo(() => turns()?.why ?? null);

  const turnOn = async () => {
    const list = turns();
    try {
      if (why() === "off") {
        store.setSettings({ checkpoints: true });
        await setTurnSnapshots(true);
      } else if (list?.repo) await setTurnSnapshots(undefined, list.repo);
      setVersion((v) => v + 1);
    } catch (e) {
      store.notify(errorText(e));
    }
  };

  const whyText = (w: string) =>
    ({
      off: t("Turn snapshots are off. Turn them on in Settings, Agents."),
      slow: t("Snapshots of this repository were turned off: they took over a second, twice in a row."),
      "not-git": t("This folder is not a git repository, so its turns are not recorded."),
      remote: t("Sessions on another machine are not recorded."),
      shell: t("Shells are not recorded."),
    })[w] ?? w;

  return (
    <div class="turns">
      <div class="explorer-tools">
        <Show when={sessions().length} fallback={<div class="muted">{t("No agent sessions in this project yet")}</div>}>
          <select class="text-input" value={session()?.id} onChange={(e) => setChoice(e.currentTarget.value)} title={t("Session")}>
            <For each={sessions()}>{(s) => <option value={s.id}>{sessionLabel(s)}</option>}</For>
          </select>
          <div class="seg">
            <button class={view() === "turns" ? "on" : ""} onClick={() => setView("turns")}>{t("Turns")}</button>
            <button class={view() === "files" ? "on" : ""} title={t("Who touched which files in this folder")} onClick={() => setView("files")}>{t("Files")}</button>
          </div>
        </Show>
      </div>
      <div class="list turns-list">
        <Show when={session()}>
          {(s) => (
            <Show when={turns()} fallback={<div class="muted">{turns.loading ? t("Loading…") : t("Turns could not be read")}</div>}>
              {(list) => (
                <>
                  <Show when={why()}>
                    {(w) => (
                      <div class="turns-notice">
                        <span>{whyText(w())}</span>
                        <Show when={w() === "off" || w() === "slow"}>
                          <button class="btn" onClick={() => void turnOn()}>{w() === "off" ? t("Turn on") : t("Turn on again")}</button>
                        </Show>
                      </div>
                    )}
                  </Show>
                  <Switch>
                    <Match when={view() === "turns"}>
                      <For each={rows()} fallback={<Show when={!why()}><div class="muted">{t("No turns recorded yet. A snapshot is taken when the agent finishes each turn.")}</div></Show>}>
                        {(rec) => <TurnRow project={props.project} session={s()} rec={rec} prefix={prefix()} onUndo={() => setUndoing({ session: s().id, name: sessionLabel(s()), n: rec.n })} />}
                      </For>
                      <Show when={list().records.length}>
                        <div class="turns-foot muted">{t("The last 50 turns are kept. Snapshots live in this repository under refs/kelpie and never touch your index, branch or files.")}</div>
                      </Show>
                    </Match>
                    <Match when={view() === "files"}>
                      <OwnersList project={props.project} session={s()} version={version()} prefix={prefix()} />
                    </Match>
                  </Switch>
                </>
              )}
            </Show>
          )}
        </Show>
      </div>
      <Show when={undoing()}>
        {(u) => (
          <UndoDialog
            sessionId={u().session}
            sessionName={u().name}
            n={u().n}
            onClose={(changed) => {
              setUndoing(null);
              if (changed) setVersion((v) => v + 1);
            }}
          />
        )}
      </Show>
    </div>
  );
}

function TurnRow(props: { project: Project; session: SessionInfo; rec: TurnRecord; prefix: string; onUndo: () => void }) {
  const [open, setOpen] = createSignal(false);
  const [files] = createResource(
    () => open() && props.rec.kind !== "baseline" && { id: props.session.id, n: props.rec.n },
    (k) => turnFiles(k.id, k.n).catch((e) => (store.notify(errorText(e)), null)),
  );
  const title = () => {
    const r = props.rec;
    if (r.kind === "baseline") return r.source === "restart" ? t("Session resumed") : t("Session started");
    if (r.kind === "undo") return t("Undid #{n}", { n: r.undo_of ?? 0 });
    return t("Turn");
  };
  const note = () => sourceNote(props.rec, props.session.kind);
  const rev = () => ({ session: props.session.id, n: props.rec.n, prefix: props.prefix });
  const diff = (f: TurnFile) => {
    const rel = inProject(f.path, props.prefix);
    if (rel) store.openFile(props.project.id, rel, { diff: true, rev: rev() });
  };
  const view = (f: TurnFile) => {
    const rel = inProject(f.path, props.prefix);
    if (rel) store.openFile(props.project.id, rel, { rev: rev() });
  };
  const artifacts = createMemo(() => (files()?.files ?? []).filter((f) => f.status !== "D" && artifactKind(f.path) && inProject(f.path, props.prefix)));
  const hasChanges = () => props.rec.kind !== "baseline";

  return (
    <div class={`turn-row ${props.rec.kind}`}>
      <div class="turn-head" onClick={() => hasChanges() && setOpen((o) => !o)}>
        <span class="turn-n">#{props.rec.n}</span>
        <div class="history-text">
          <div class="history-title">
            {title()}
            <Show when={props.rec.approximate}><span class="turn-approx" title={t(note())}>{t("approx.")}</span></Show>
          </div>
          <div class="history-sub" title={note() ? t(note()) : undefined}>
            <span>{relTimeShort(props.rec.at)}</span>
            <Show when={hasChanges()}><span>{turnStats(props.rec, filesText(props.rec.files))}</span></Show>
            <Show when={props.rec.kind === "baseline" || (props.rec.kind === "turn" && props.session.kind === "codex")}><span>{props.rec.kind === "baseline" ? "" : t("turn-complete to turn-complete")}</span></Show>
          </div>
        </div>
        <div class="row-actions">
          <Show when={props.rec.kind === "turn" && props.rec.files > 0}>
            <button class="icon-btn" title={t("Undo this turn…")} onClick={(e) => { e.stopPropagation(); props.onUndo(); }}><Icon name="undo" size={13} /></button>
          </Show>
        </div>
      </div>
      <Show when={open()}>
        <div class="turn-files">
          <Show when={files()} fallback={<div class="muted">{files.loading ? t("Loading…") : t("Nothing to show")}</div>}>
            {(f) => (
              <>
                <For each={f().files}>
                  {(file) => (
                    <div class="turn-file" title={inProject(file.path, props.prefix) ? file.path : t("Outside this project")} style={inProject(file.path, props.prefix) ? undefined : { opacity: 0.55, cursor: "default" }} onClick={() => diff(file)}>
                      <span class={`turn-status s-${file.status}`}>{file.status}</span>
                      <span class="turn-path">&lrm;{file.path}</span>
                      <span class="turn-delta">{file.binary ? t("binary") : `+${file.added ?? 0} −${file.removed ?? 0}`}</span>
                    </div>
                  )}
                </For>
                <Show when={f().total > f().files.length}><div class="muted">{t("{n} more files", { n: f().total - f().files.length })}</div></Show>
                <Show when={artifacts().length}>
                  <div class="turn-artifacts">
                    <span class="muted-inline">{t("Produced")}</span>
                    <For each={artifacts()}>
                      {(file) => <button class="turn-chip" title={file.path} onClick={() => view(file)}>{file.path.split("/").pop()}</button>}
                    </For>
                  </div>
                </Show>
              </>
            )}
          </Show>
        </div>
      </Show>
    </div>
  );
}

function OwnersList(props: { project: Project; session: SessionInfo; version: number; prefix: string }) {
  const [owners] = createResource(
    () => ({ id: props.session.id, v: props.version }),
    ({ id }) => turnOwners(id).catch(() => null),
  );
  const [query, setQuery] = createSignal("");
  const rows = createMemo(() => {
    const q = query().trim().toLowerCase();
    return (owners()?.files ?? []).filter((r) => !q || r.path.toLowerCase().includes(q));
  });
  return (
    <>
      <div class="explorer-tools"><input class="text-input" placeholder={t("Filter files")} value={query()} onInput={(e) => setQuery(e.currentTarget.value)} /></div>
      <For each={rows()} fallback={<div class="muted">{owners.loading ? t("Loading…") : t("No files touched by agents yet")}</div>}>
        {(row) => (
          <div class={`turn-file owner${row.shared ? " shared" : ""}`} title={row.path}>
            <span
              class="turn-path"
              onClick={() => {
                const rel = inProject(row.path, props.prefix);
                if (rel && row.sessions.length) store.openFile(props.project.id, rel, { diff: true, rev: { session: row.sessions[0].session, n: row.sessions[0].n, prefix: props.prefix } });
              }}
            >
              &lrm;{row.path}
            </span>
            <span class="turn-owners">
              <Show when={row.shared}><span class="turn-shared" title={t("More than one session changed this file")}>{t("shared")}</span></Show>
              <For each={row.sessions}>{(o) => <span class="turn-chip static" title={t("{name}: {n} turns, last {when}", { name: o.name, n: o.turns, when: relTimeShort(o.last) })}>{o.name}</span>}</For>
            </span>
          </div>
        )}
      </For>
      <Show when={(owners()?.total ?? 0) > rows().length && !query()}><div class="muted">{t("{n} more files", { n: (owners()?.total ?? 0) - rows().length })}</div></Show>
    </>
  );
}

/** Every file an undo would restore or delete, with what makes it risky; nothing happens until
 *  the user confirms, and files another session touched or that changed since need their own tick. */
export function UndoDialog(props: { sessionId: string; sessionName: string; n: number; onClose: (changed: boolean) => void }) {
  const [plan, setPlan] = createSignal<UndoPlan | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [picked, setPicked] = createSignal<Set<string>>(new Set());
  const [busy, setBusy] = createSignal(false);
  setCovering("turn-undo", true);
  onCleanup(() => setCovering("turn-undo", false));

  onMount(async () => {
    try {
      const p = await undoPlan(props.sessionId, props.n);
      setPlan(p);
      setPicked(safelyTicked(p));
    } catch (e) {
      setError(errorText(e));
    }
  });

  const items = () => plan()?.items ?? [];
  const risky = (i: PlanItem) => i.state !== "clean";
  const toggle = (path: string) =>
    setPicked((cur) => {
      const next = new Set(cur);
      if (!next.delete(path)) next.add(path);
      return next;
    });
  const chosen = () => items().filter((i) => picked().has(i.path));
  const working = () => !!plan()?.owner_working;

  const run = async () => {
    setBusy(true);
    setError(null);
    const r = await attemptUndo({ plan: () => undoPlan(props.sessionId, props.n), undo: (paths, confirm) => undoTurn(props.sessionId, props.n, paths, confirm) }, items(), picked());
    if (r.kind === "applied") {
      store.notify(t("Undid turn #{n}: {files}", { n: props.n, files: filesText(r.result.files?.length ?? 0) }));
      props.onClose(true);
      return;
    }
    if (r.plan) setPlan(r.plan);
    setPicked(r.picked);
    setError(r.kind === "refused" ? t("Some files changed: check the list again. Only the safe ones are ticked.") : t("The undo stopped: {error}. Files already put back show as done in the list.", { error: errorText(r.error) }));
    setBusy(false);
  };

  const verb = (i: PlanItem) => (i.action === "delete" ? t("Delete") : i.action === "restore" ? t("Restore") : t("Skip"));
  const why = (i: PlanItem) => {
    if (i.state === "shared") return i.with.length ? t("Also worked on by {names}", { names: i.with.map((w) => w.name).join(", ") }) : t("Also worked on by another session");
    if (i.state === "modified") {
      const r = i.reason ?? "";
      if (r.startsWith("changed between")) return t("Changed between turns, before this turn started");
      if (r.startsWith("changed before")) return t("Changed before this turn started, not by it");
      if (r.startsWith("could not be read just now")) return t("Could not be read just now: another program may hold it");
      if (r.startsWith("could not be read")) return t("Could not be read when a snapshot was taken");
      if (r.startsWith("a file now stands")) return t("A file now stands where its folder was");
      if (r.startsWith("removed before")) return t("Removed before this turn started");
      return t("Changed since this turn");
    }
    if (i.action !== "skip") return "";
    return i.reason?.startsWith("already") ? t("Already as it was before the turn") : t("A link or submodule: restore it yourself");
  };

  return (
    <div class="overlay" onPointerDown={(e) => e.target === e.currentTarget && !busy() && props.onClose(false)}>
      {/* Focused on open, so keys reach the dialog and not the composer or pane behind it. */}
      <div class="modal turn-undo" tabIndex={-1} ref={(el) => queueMicrotask(() => el.focus())} onKeyDown={(e) => e.key === "Escape" && !busy() && props.onClose(false)}>
        <h2>{t("Undo turn #{n} of {name}?", { n: props.n, name: props.sessionName })}</h2>
        <span class="muted-inline">{t("These files go back to how they were before the turn. Your index, branch and other files are not touched.")}</span>
        <span class="muted-inline">{t("Files come back with the line endings your git settings give them, which can differ from before.")}</span>
        <Show when={working()}><p class="update-warning">{t("This session is still working. Undo a turn once it is idle.")}</p></Show>
        <Show when={error()}><p class="update-error">{error()}</p></Show>
        <Show when={plan()} fallback={<Show when={!error()}><div class="muted">{t("Loading…")}</div></Show>}>
          <Show when={plan()?.unreadable?.length}>
            <p class="muted-inline">{t("Not in this snapshot (another program held them open): {files}", { files: plan()!.unreadable!.join(", ") })}</p>
          </Show>
          <Show when={items().some(risky)}>
            <p class="muted-inline">{t("Files marked with a warning stay unticked: tick one to confirm that it may be overwritten.")}</p>
          </Show>
          <div class="turn-undo-list">
            <For each={items()}>
              {(i) => (
                <label class={`turn-undo-row${risky(i) ? " risky" : ""}${i.action === "skip" ? " skip" : ""}`}>
                  <input type="checkbox" checked={picked().has(i.path)} disabled={i.action === "skip" || busy()} onChange={() => toggle(i.path)} />
                  <span class={`turn-verb ${i.action}`}>{verb(i)}</span>
                  <span class="turn-path" title={i.path}>&lrm;{i.path}</span>
                  <Show when={why(i)}><span class="turn-why">{risky(i) ? "⚠ " : ""}{why(i)}</span></Show>
                </label>
              )}
            </For>
          </div>
        </Show>
        <div class="footer">
          <button class="btn" disabled={busy()} onClick={() => props.onClose(false)}>{t("Cancel")}</button>
          <button class="btn primary" disabled={busy() || working() || !chosen().length} onClick={() => void run()}>
            {chosen().length ? t("Undo {files}", { files: filesText(chosen().length) }) : t("Undo")}
          </button>
        </div>
      </div>
    </div>
  );
}
