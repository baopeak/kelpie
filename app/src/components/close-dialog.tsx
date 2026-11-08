// Closing Kelpie: listens for the native side's `quit-requested` (src-tauri/src/quit.rs), runs the close flow
// (lib/close-daemon.ts) and, when something still runs, asks before the session daemon stops.
import { For, Show, createEffect, createSignal, onMount } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import { t } from "../lib/i18n";
import { daemon, listSessions, onEvent, quitApp, quitGuard, quitSeen } from "../lib/ipc";
import { setCovering } from "../lib/overlays";
import { createCloseFlow, runningWork, workLines, type CloseOutcome, type RunningWork } from "../lib/close-daemon";

/** The names shown; the rest are counted. */
const MAX_NAMES = 6;
/** How long the close waits to learn what runs before it stops without naming it. */
const WORK_WAIT_MS = 3000;

type Prompt = { work: RunningWork; answer: (a: { dontAsk: boolean } | null) => void };

const [prompt, setPrompt] = createSignal<Prompt | null>(null);
/** "Close and stop" was clicked: the daemon is being stopped and the app exits. */
const [stopping, setStopping] = createSignal(false);

/** What runs on the daemon now, from the daemon itself; null when it cannot be asked in time. */
async function currentWork(): Promise<RunningWork | null> {
  if (!store.connected()) return null;
  const projectName = (id: string | null | undefined) => store.state.projects.find((p) => p.id === id)?.name;
  const read = (async () => {
    const list = await listSessions();
    const [info, runs] = await Promise.all([
      daemon<{ transfers?: number }>("daemon.info").catch(() => ({ transfers: 0 })),
      daemon<{ status: string; tasks: { status: string }[] }[]>("run.list").catch(() => []),
    ]);
    const sessions = list.map((s) => {
      const project = projectName(s.project_id);
      return { alive: s.alive, label: project ? `${sessionLabel(s)} (${project})` : sessionLabel(s) };
    });
    return runningWork(sessions, info.transfers ?? 0, runs);
  })().catch(() => null);
  return Promise.race([read, new Promise<null>((done) => setTimeout(() => done(null), WORK_WAIT_MS))]);
}

const closeFlow = createCloseFlow({
  settings: () => (store.loaded() ? store.state.settings : null),
  work: currentWork,
  ask: (work) => new Promise((answer) => setPrompt({ work, answer })),
  dontAskAgain: () => store.setSettings({ askBeforeStop: false }),
  save: () => (store.canPersist() ? store.saveNow() : Promise.resolve()),
  quit: quitApp,
});

async function onQuitRequested() {
  let outcome: CloseOutcome | null = null;
  try {
    outcome = await closeFlow();
  } catch (e) {
    store.notify(t("Could not close Kelpie: {error}", { error: String(e) }));
  } finally {
    // A close while the question is up leaves it up. After a stop the app is exiting; after a cancel or an error the
    // window stays as it was.
    if (outcome !== "busy") {
      setPrompt(null);
      setStopping(false);
    }
  }
}

export function CloseDialogHost() {
  onMount(() => {
    // Listening first, then telling the native side: no close is handed to a window that would miss it.
    void onEvent("quit-requested", () => {
      void quitSeen().catch(() => {});
      void onQuitRequested();
    }).then(() => quitGuard().catch(() => {}));
  });
  createEffect(() => setCovering("close-daemon", !!prompt()));
  return <Show when={prompt()} keyed>{(p) => <CloseDialog prompt={p} />}</Show>;
}

function CloseDialog(props: { prompt: Prompt }) {
  const [dontAsk, setDontAsk] = createSignal(false);
  let stopBtn!: HTMLButtonElement;
  onMount(() => stopBtn.focus());
  const cancel = () => !stopping() && props.prompt.answer(null);
  const stop = () => {
    if (stopping()) return;
    setStopping(true);
    props.prompt.answer({ dontAsk: dontAsk() });
  };
  const names = () => props.prompt.work.sessions.slice(0, MAX_NAMES);
  const more = () => props.prompt.work.sessions.length - names().length;
  return (
    <div class="overlay close-daemon-overlay" onPointerDown={(e) => e.target === e.currentTarget && cancel()}>
      <div class="modal close-daemon" data-close-dialog onKeyDown={(e) => e.key === "Escape" && cancel()}>
        <h2>{t("Stop the session daemon and close Kelpie?")}</h2>
        <ul class="close-daemon-work">
          <For each={workLines(props.prompt.work)}>{(line) => <li>{line}</li>}</For>
        </ul>
        <Show when={names().length}>
          <div class="close-daemon-names">
            <For each={names()}>{(name) => <span class="chip">{name}</span>}</For>
            <Show when={more() > 0}><span class="muted-inline">{t("and {n} more", { n: more() })}</span></Show>
          </div>
        </Show>
        <p class="close-daemon-text">
          {t("They stop now; Kelpie does not wait for them. When you open Kelpie again, sessions come back interrupted and agents resume their conversation (Settings → Agents → Resume after a restart). File transfers show Interrupted, with Resume.")}
        </p>
        <p class="muted-inline">{t("To keep them running with the window closed, choose Keep it running in the background in Settings → Session Daemon.")}</p>
        <label class="check-label">
          <input type="checkbox" data-field="close-dont-ask" checked={dontAsk()} onChange={(e) => setDontAsk(e.currentTarget.checked)} />
          {t("Don't ask again")}
        </label>
        <div class="footer">
          <button class="btn" data-action="close-cancel" disabled={stopping()} onClick={cancel}>{t("Cancel")}</button>
          <button ref={stopBtn} class="btn danger" data-action="close-stop" disabled={stopping()} onClick={stop}>
            {stopping() ? t("Stopping…") : t("Close and stop")}
          </button>
        </div>
      </div>
    </div>
  );
}
