// The question "may this session open a sub-session?": a card at the bottom right that does not block the window, one per
// open request (the first three; the Activity panel lists them all). The daemon waits for the answer and drops the request
// after five minutes; the card goes when it is answered here or anywhere else (`sub.request_done`).
import { For, Show, createSignal, onCleanup } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import { kindInfo } from "../lib/agents";
import { t } from "../lib/i18n";
import { folderName, postponeRequest, shownRequests, type SubDecision, type SubRequest } from "../lib/sub-sessions";
import { confirmDialog } from "../lib/confirm-dialog";
import { answerSubRequest } from "../lib/sub-session-actions";
import { KindGlyph } from "./icons";

/** Cards shown at once; more wait in the Activity panel. */
const MAX_CARDS = 3;

/** The buttons work this long after a card appears, so a click meant for what is under it cannot answer it. */
const ARM_MS = 500;

export function SubRequestCard(props: { request: SubRequest; inbox?: boolean }) {
  const [busy, setBusy] = createSignal(false);
  const [armed, setArmed] = createSignal(false);
  const timer = setTimeout(() => setArmed(true), ARM_MS);
  onCleanup(() => clearTimeout(timer));
  const off = () => busy() || !armed();
  const r = () => props.request;
  const asker = () => store.sessions[r().session];
  const who = () => (asker() ? sessionLabel(asker()!) : r().from_label || t("A session"));
  const agent = () => kindInfo(r().agent).label;
  const answer = async (decision: SubDecision) => {
    if (off()) return;
    // The same question as in Settings: this turns the question off for every session.
    if (decision === "always" && !(await confirmDialog(t("Let agents open sub-sessions without asking you? Each one is another agent that uses tokens and works in your folders, and one opened by an agent that skips permission checks does too.")))) return;
    setBusy(true);
    // The card goes with the request (`sub.request_done`); a refusal leaves it for another try.
    if (!(await answerSubRequest(r().id, decision))) setBusy(false);
  };
  return (
    <section class="sub-card" role="group" tabindex="-1" aria-label={t("{name} wants to open a sub-session", { name: who() })} data-sub-request={r().id}>
      <div class="sub-card-title">
        <Show when={asker()}>{(s) => <KindGlyph kind={s().kind} size={14} />}</Show>
        <b>{who()}</b> {t("wants to open a sub-session")}
      </div>
      <div class="sub-card-what">
        <KindGlyph kind={r().agent} size={14} />
        <b>{r().name || agent()}</b>
        <Show when={r().name}><span class="muted-inline">{agent()}</span></Show>
        <span class="muted-inline" title={r().cwd}>{t("in {folder}", { folder: folderName(r().cwd) })}</span>
        <Show when={r().worktree}><span class="chip">{t("new worktree")}</span></Show>
        <Show when={r().model}><span class="chip">{r().model}</span></Show>
      </div>
      <Show when={r().task}>
        <div class="sub-card-task" title={r().task}>{r().task}</div>
      </Show>
      <Show when={r().skips_prompts}>
        <div class="sub-card-warn" role="alert">
          <b>{t("It will run without asking before it acts.")}</b> {t("{name} runs with permission checks off, and a sub-session it opens does the same.", { name: who() })}
        </div>
      </Show>
      <div class="sub-card-actions">
        <button class="btn primary" data-sub-primary disabled={off()} onClick={() => void answer("once")}>{t("Allow once")}</button>
        <button class="btn" disabled={off()} onClick={() => void answer("session")} title={t("Until the session daemon restarts, this session opens sub-sessions without asking.")}>{t("Allow for this session")}</button>
        <button class="btn" disabled={off()} onClick={() => void answer("always")} title={t("No session asks any more. You can change this in Settings → Agents → Sub-sessions.")}>{t("Always allow")}</button>
        <button class="btn danger" disabled={off()} onClick={() => void answer("deny")}>{t("Deny")}</button>
        <Show when={!props.inbox}>
          <button class="btn" disabled={busy()} onClick={() => postponeRequest(r().id)} title={t("Hide this card; the request stays in Activity (the bell in the status bar) until you answer it or it runs out of time.")}>{t("Later")}</button>
        </Show>
      </div>
    </section>
  );
}

/** The cards, over the window's bottom right corner. Nothing is rendered without a request. */
export function SubRequestCards() {
  const shown = () => shownRequests().slice(0, MAX_CARDS);
  const more = () => Math.max(0, shownRequests().length - MAX_CARDS);
  return (
    <Show when={shownRequests().length}>
      <div class="sub-cards" role="region" aria-live="polite" aria-label={t("Sub-session requests")}>
        <For each={shown()}>{(r) => <SubRequestCard request={r} />}</For>
        <Show when={more()}>
          <button class="sub-cards-more" onClick={() => window.dispatchEvent(new CustomEvent("kelpie:inbox"))}>{t("{n} more waiting: open Activity", { n: more() })}</button>
        </Show>
      </div>
    </Show>
  );
}
