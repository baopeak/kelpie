// The programs each session of the project runs, as a tree: name, PID, memory, CPU and the ports they
// listen on. A program can be stopped (asked to end, forced after 3 s), or everything a session started
// at once while the session itself keeps running. Both ask first and name what would end.
// Looks only while this panel is open (every 2 s); see lib/processes.ts for the chips.
import { For, Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { createStore } from "solid-js/store";
import { store, sessionLabel } from "../lib/store";
import { confirmDialog } from "../lib/confirm-dialog";
import { errorText, t } from "../lib/i18n";
import { KindGlyph } from "./icons";
import { formatBytes, killChildren, killProcess, killSummary, listProcesses, openPort, portOwner, type KillReport, type Port, type ProcInfo, type ProcessList } from "../lib/processes";
import { rows, subtree, type Row } from "../lib/process-tree";
import type { Project, SessionInfo } from "../lib/types";

/** How often the open panel looks. */
const LOOK_MS = 2000;
/** Most programs named in a confirmation. */
const NAMED = 8;

/** The lines of a confirmation: what would end, with its ports, and how. */
function preview(head: string, victims: ProcInfo[]): string {
  const lines = victims.slice(0, NAMED).map((v) => `  ${v.name} (PID ${v.pid})`);
  if (victims.length > NAMED) lines.push(t("  … and {n} more", { n: victims.length - NAMED }));
  const ports = [...new Set(victims.flatMap((v) => v.ports.map((x) => `:${x.port}`)))];
  return [head, ...lines, ...(ports.length ? [t("Frees {ports}", { ports: ports.join(" ") })] : []), "", t("Each is asked to end first and forced after 3 seconds.")].join("\n");
}

function Chips(props: { ports: Port[]; session: string }) {
  const unique = createMemo(() => props.ports.filter((p, i) => props.ports.findIndex((q) => q.port === p.port) === i));
  return (
    <For each={unique()}>
      {(p) => <button class="port-chip" data-port={p.port} title={`http://${p.host}:${p.port}/`} onClick={() => openPort(p, props.session, "tab")}>:{p.port}</button>}
    </For>
  );
}

function SessionGroup(props: { s: SessionInfo; list: ProcessList | null; busy: Set<number>; onStop: (s: SessionInfo, p: ProcInfo, victims: ProcInfo[]) => void; onStopChildren: (s: SessionInfo, victims: ProcInfo[]) => void }) {
  const tree = createMemo(() => (props.list ? rows(props.list) : []));
  const kids = createMemo(() => (props.list ? props.list.children.filter((c) => !props.list!.keep.includes(c.pid)) : []));
  const row = (r: Row, orphan = false) => (
    <div class={`proc-row${r.depth === 0 && !orphan ? " root" : ""}`} data-pid={r.p.pid}>
      <span class="proc-name" style={{ "padding-left": `${r.depth * 12}px` }} title={r.p.exe ?? r.p.name}>{r.p.name}</span>
      <span class="proc-num">{r.p.pid}</span>
      <span class="proc-num">{formatBytes(r.p.mem)}</span>
      <span class="proc-num">{Math.round(r.p.cpu)}%</span>
      <span class="proc-ports"><Chips ports={r.p.ports} session={props.s.id} /></span>
      <span class="proc-act">
        <Show when={orphan || r.depth > 0}>
          <button class="link-btn danger" disabled={props.busy.has(r.p.pid)} onClick={() => props.onStop(props.s, r.p, orphan ? [r.p] : subtree(props.list!, r.p))}>{t("Kill")}</button>
        </Show>
      </span>
    </div>
  );
  return (
    <section class="proc-session" data-session={props.s.id}>
      <header>
        <KindGlyph kind={props.s.kind} size={13} />
        <span class="proc-title">{sessionLabel(props.s)}</span>
        <Show when={kids().length}>
          <button class="link-btn danger" data-act="kill-children" disabled={props.busy.has(-1)} title={t("Stops everything this session started; the session keeps running")} onClick={() => props.onStopChildren(props.s, kids())}>{t("Kill children")}</button>
        </Show>
      </header>
      <Show when={props.list} fallback={<div class="proc-empty">…</div>}>
        <Show when={tree().length > 1 || (props.list?.orphans.length ?? 0) > 0} fallback={<div class="proc-empty">{t("Nothing is running under this session.")}</div>}>
          <For each={tree()}>{(r) => row(r)}</For>
          <Show when={props.list!.orphans.length}>
            <div class="proc-orphans" title={t("Still running, but no longer part of this session's processes")}>{t("Left the session")}</div>
            <For each={props.list!.orphans}>{(p) => row({ p, depth: 0 }, true)}</For>
          </Show>
        </Show>
      </Show>
    </section>
  );
}

export default function ProcessesPanel(props: { project: Project }) {
  const sessions = createMemo(() =>
    Object.values(store.sessions)
      .filter((s) => s.project_id === props.project.id && s.alive && !s.remote)
      .sort((a, b) => a.created_at - b.created_at),
  );
  const [lists, setLists] = createStore<Record<string, ProcessList | null>>({});
  const [error, setError] = createSignal<string | null>(null);
  // Programs being stopped (-1: a session's "Kill children"), so a second click cannot repeat the stop.
  const [busy, setBusy] = createSignal(new Set<number>(), { equals: false });
  let timer: ReturnType<typeof setTimeout> | undefined;
  let gone = false;

  async function look() {
    try {
      const got = await Promise.all(sessions().map((s) => listProcesses(s.id).then((l) => [s.id, l] as const)));
      if (gone) return;
      for (const [id, l] of got) setLists(id, l);
      setError(null);
    } catch (e) {
      if (!gone) setError(errorText(e));
    }
  }

  function loop() {
    if (gone) return;
    timer = setTimeout(async () => {
      if (!document.hidden) await look();
      loop();
    }, LOOK_MS);
  }

  onMount(() => {
    void look().then(loop);
  });
  onCleanup(() => {
    gone = true;
    clearTimeout(timer);
  });

  async function stop(key: number, victims: ProcInfo[], run: () => Promise<KillReport>) {
    setBusy((b) => b.add(key));
    try {
      const report = await run();
      const n = report.closed.length + report.forced.length + report.gone.length;
      const refused = killSummary(report);
      store.notify(refused ?? (n === 1 ? t("Stopped {n} process", { n }) : t("Stopped {n} processes", { n })), undefined, refused ? 8000 : 3500);
      await look();
      // A port that is still held after the stop: say who has it.
      for (const port of new Set(victims.flatMap((v) => v.ports.map((p) => p.port)))) {
        const owner = await portOwner(port).catch(() => null);
        if (owner) store.notify(t("Port {port} is still in use by {owner}", { port, owner: `${owner.name ?? "?"} (PID ${owner.pid})` }), undefined, 8000);
      }
    } catch (e) {
      store.notify(errorText(e), undefined, 8000);
      void look();
    } finally {
      setBusy((b) => (b.delete(key), b));
    }
  }

  const onStop = async (s: SessionInfo, p: ProcInfo, victims: ProcInfo[]) => {
    const head = victims.length > 1 ? t("Stop {name} (PID {pid}) and the {n} programs it started?", { name: p.name, pid: p.pid, n: victims.length - 1 }) : t("Stop {name} (PID {pid})?", { name: p.name, pid: p.pid });
    if (await confirmDialog(preview(head, victims))) await stop(p.pid, victims, () => killProcess(s.id, p));
  };
  const onStopChildren = async (s: SessionInfo, victims: ProcInfo[]) => {
    const head = t("Stop everything {name} started? The session itself keeps running.", { name: sessionLabel(s) });
    if (await confirmDialog(preview(head, victims))) await stop(-1, victims, () => killChildren(s.id));
  };

  return (
    <div class="proc-panel">
      <div class="proc-head">
        <span>{t("Name")}</span><span>{t("PID")}</span><span>{t("RAM")}</span><span>{t("CPU")}</span><span>{t("Ports")}</span><span />
      </div>
      <Show when={error()}>{(e) => <div class="proc-error">{t("Could not list processes: {error}", { error: e() })}</div>}</Show>
      <Show when={sessions().length} fallback={<div class="proc-empty">{t("No running sessions in this project.")}</div>}>
        <For each={sessions()}>
          {(s) => <SessionGroup s={s} list={lists[s.id] ?? null} busy={busy()} onStop={onStop} onStopChildren={onStopChildren} />}
        </For>
      </Show>
    </div>
  );
}
