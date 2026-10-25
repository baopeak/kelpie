// "Try with N agents": one prompt, several agents, a worktree each; Runs compares the results.
import { Index, Show, createResource, createSignal } from "solid-js";
import { store } from "../lib/store";
import { kindInfo, launchableAgents } from "../lib/agents";
import { trying } from "../lib/review-queue";
import { MAX_AGENTS, MIN_AGENTS, bringingText, planBringing, problem, startCandidates } from "../lib/try-agents";
import { KindGlyph } from "./icons";
import { KeyHints } from "./key-hints";
import { openRuns } from "./runs-page";
import { t } from "../lib/i18n";
import "./review.css";

export default function TryDialog() {
  const project = () => store.state.projects.find((p) => p.id === trying()?.projectId);
  const agents = () => launchableAgents(store.state.settings.hiddenAgents);
  // A different vendor in each slot to start with.
  const [kinds, setKinds] = createSignal<string[]>(Array.from({ length: 3 }, (_, i) => agents()[i] ?? agents()[0] ?? "claude"));
  const [prompt, setPrompt] = createSignal(trying()?.prompt ?? "");
  const [busy, setBusy] = createSignal(false);
  const [bringing] = createResource(project, (p) => planBringing(p));
  const close = () => store.setOverlay(null);
  const why = () => problem(kinds(), prompt());
  const setKind = (i: number, kind: string) => setKinds(kinds().map((k, j) => (j === i ? kind : k)));

  async function start() {
    const p = project();
    const b = bringing();
    if (!p || !b || why() || busy()) return;
    setBusy(true);
    try {
      const run = await startCandidates(p, kinds(), prompt(), b);
      store.setOverlay(null);
      openRuns(false, run.id);
    } catch (e) {
      setBusy(false);
      store.notify(t("Could not start the agents: {error}", { error: String(e).replace(/^Error: /, "") }), undefined, 8000);
    }
  }

  return (
    <div class="overlay" onPointerDown={(e) => e.target === e.currentTarget && close()}>
      <div class="modal try-dialog" onKeyDown={(e) => {
        if (e.key === "Escape") close();
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) void start();
      }}>
        <Show when={project()} fallback={<p>{t("Open a project first")}</p>}>
          <h2>{t("Try with several agents in {project}", { project: project()!.name })}</h2>
          <div class="form-row">
            <label>{t("Prompt")}</label>
            <textarea class="text-input try-prompt" rows={5} value={prompt()} placeholder={t("What should every agent do?")} onInput={(e) => setPrompt(e.currentTarget.value)} />
          </div>
          <div class="form-row">
            <label>{t("Agents ({n})", { n: kinds().length })}</label>
            <div class="try-slots">
              {/* Index, not For: choosing an agent replaces the slot's value and the select stays. */}
              <Index each={kinds()}>
                {(k, i) => (
                  <div class="try-slot" data-slot={i}>
                    <KindGlyph kind={k()} size={18} />
                    <select class="text-input" value={k()} onChange={(e) => setKind(i, e.currentTarget.value)}>
                      <Index each={agents()}>{(a) => <option value={a()}>{kindInfo(a()).label}</option>}</Index>
                    </select>
                    <button class="icon-btn" title={t("Remove agent")} disabled={kinds().length <= MIN_AGENTS} onClick={() => setKinds(kinds().filter((_, j) => j !== i))}>×</button>
                  </div>
                )}
              </Index>
              <button class="btn" style={{ "align-self": "flex-start" }} disabled={kinds().length >= MAX_AGENTS} onClick={() => setKinds([...kinds(), agents()[kinds().length % Math.max(1, agents().length)] ?? "claude"])}>{t("Add agent")}</button>
            </div>
          </div>
          <p class="muted-inline">{t("Each agent works in its own worktree and branch, started from where this folder is now. Runs shows them side by side; pick the best and Finish merges it.")}</p>
          <p class="muted-inline" data-bringing>{bringing.loading ? t("Measuring what each worktree gets…") : bringing() ? bringingText(bringing()!, kinds().length) : ""}</p>
          <div class="footer">
            <KeyHints items={[["Ctrl+Enter", t("Start")], ["Esc", t("Cancel")]]} />
            <span style={{ flex: 1 }} />
            <button class="btn" onClick={close}>{t("Cancel")}</button>
            <button class="btn primary" title={why() ?? ""} disabled={!!why() || busy() || !bringing()} onClick={() => void start()}>{t("Start {n} agents", { n: kinds().length })}</button>
          </div>
        </Show>
      </div>
    </div>
  );
}
