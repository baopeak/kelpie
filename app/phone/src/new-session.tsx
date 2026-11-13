// The "+" sheet: start an agent in one of the computer's projects, with the computer's own settings for that agent, and an optional first prompt.
// Start opens the new session; what the computer refuses is said in plain words (start-session.ts).
import { createSignal, For, onMount, Show } from "solid-js";
import { t } from "./i18n";
import type { NewOptions } from "./link";
import { connected, loadNewOptions, sessions, startSession } from "./phone-state";
import { notOfferedText, projectChoices } from "./start-session";

export function NewSessionSheet(props: { onClose: () => void }) {
  const [options, setOptions] = createSignal<NewOptions | null>(null);
  const [message, setMessage] = createSignal<string | null>(null);
  const [project, setProject] = createSignal("");
  const [kind, setKind] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  let prompt!: HTMLTextAreaElement;

  const projects = () => projectChoices(options()?.projects ?? [], sessions());
  const agent = () => options()?.agents.find((a) => a.kind === kind());

  onMount(async () => {
    const r = await loadNewOptions();
    if (!r.ok) return setMessage(r.message);
    setOptions(r.options);
    setProject(projectChoices(r.options.projects, sessions())[0]?.id ?? "");
    setKind(r.options.agents[0]?.kind ?? "");
  });

  const start = async () => {
    if (busy() || !project() || !kind()) return;
    setBusy(true);
    setMessage(null);
    const r = await startSession(project(), kind(), prompt.value);
    setBusy(false);
    if (r.ok) props.onClose();
    else setMessage(r.message);
  };

  return (
    <div class="sheet-back" onClick={props.onClose}>
      <div class="sheet" onClick={(e) => e.stopPropagation()} role="dialog" aria-label={t("Start a session")} data-new-session>
        <h2>{t("Start a session")}</h2>
        <Show when={options()} fallback={<Show when={!message()}><p class="muted">{t("Asking your computer what can be started…")}</p></Show>}>
          {(o) => (
            <>
              <Show when={o().projects.length} fallback={<p class="muted" data-new-none="projects">{t("No project on your computer can be started from the phone.")}</p>}>
                <label class="row-setting">{t("Project")}
                  <select class="field" data-new-project value={project()} onChange={(e) => setProject(e.currentTarget.value)}>
                    <For each={projects()}>{(p) => <option value={p.id}>{p.name || p.id}</option>}</For>
                  </select>
                </label>
              </Show>
              <Show when={o().agents.length} fallback={<p class="muted" data-new-none="agents">{t("No agent installed on your computer can be started from the phone.")}</p>}>
                <label class="row-setting">{t("Agent")}
                  <select class="field" data-new-agent value={kind()} onChange={(e) => setKind(e.currentTarget.value)}>
                    <For each={o().agents}>{(a) => <option value={a.kind}>{a.label || a.kind}</option>}</For>
                  </select>
                </label>
                <Show when={agent()?.skips}><p class="notice" role="status" data-new-skips>{t("Does not ask before acting, as on your computer.")}</p></Show>
              </Show>
              <label class="row-setting">{t("First prompt (optional)")}
                <textarea class="field" data-new-prompt rows="3" ref={prompt} />
              </label>
              <p class="muted">{notOfferedText()}</p>
            </>
          )}
        </Show>
        <Show when={message()}><p class="notice" role="status" data-new-error>{message()}</p></Show>
        <div class="actions">
          <button class="btn primary" data-new-start disabled={busy() || !connected() || !project() || !kind()} onClick={() => void start()}>{busy() ? t("Starting the session…") : t("Start session")}</button>
          <button class="btn" onClick={props.onClose}>{t("Cancel")}</button>
        </div>
      </div>
    </div>
  );
}
