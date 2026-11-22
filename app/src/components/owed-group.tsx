// The Activity panel's "Checks still owed" group of one project: what a change could not check for itself, open entries only.
// Closing asks for the evidence (a path, a link, the owner's word) and calls the daemon; the list follows `owed.changed`.
import { For, Show, createSignal } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import * as ipc from "../lib/ipc";
import { relTimeShort } from "../lib/platform";
import { openOwed, owedRows, type OwedRow } from "../lib/owed";
import { t } from "../lib/i18n";
import { Icon } from "./icons";

function OwedEntry(props: { row: OwedRow }) {
  const [closing, setClosing] = createSignal(false);
  const [evidence, setEvidence] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");

  const close = async () => {
    const text = evidence().trim();
    if (!text || busy()) return;
    setBusy(true);
    setError("");
    try {
      await ipc.owedClose(props.row.id, text);
    } catch (e) {
      setError(String(e).replace(/^Error: /, ""));
      setBusy(false);
    }
  };

  return (
    <div class="inbox-item owed">
      <span class="inbox-icon"><Icon name="alert" size={13} /></span>
      <span class="inbox-text">
        <span class="inbox-title"><b>{props.row.kind}</b>{props.row.pr ? ` · PR #${props.row.pr}` : ""} · {props.row.what}</span>
        <span class="inbox-body">{t("Closes with: {how}", { how: props.row.closesBy })}</span>
        <span class="inbox-body">{t("Opened by {who}", { who: props.row.by })}</span>
        <Show when={closing()}>
          <input
            class="text-input"
            placeholder={t("Evidence: a path, a link, or your word")}
            value={evidence()}
            autofocus
            onInput={(e) => setEvidence(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void close();
              else if (e.key === "Escape") setClosing(false);
            }}
          />
          <Show when={error()}><span class="inbox-body error">{error()}</span></Show>
          <span class="owed-actions">
            <button class="link-btn" disabled={busy() || !evidence().trim()} onClick={() => void close()}>{t("Close check")}</button>
            <button class="link-btn" onClick={() => setClosing(false)}>{t("Cancel")}</button>
          </span>
        </Show>
      </span>
      <span class="age">{relTimeShort(Date.now() - props.row.ageMs)}</span>
      <Show when={!closing()}>
        <button class="link-btn" onClick={() => setClosing(true)}>{t("Close")}</button>
      </Show>
    </div>
  );
}

/** One group per project that has open entries, in the order of the sidebar. */
export function OwedGroups() {
  const groups = () =>
    store.state.projects
      .map((p) => ({ project: p, rows: owedRows(openOwed[p.id] ?? [], Date.now(), (id) => (store.sessions[id] ? sessionLabel(store.sessions[id]) : undefined)) }))
      .filter((g) => g.rows.length);
  return (
    <For each={groups()}>
      {(g) => (
        <>
          <div class="group-label">{t("Checks still owed · {project}", { project: g.project.name })}</div>
          <For each={g.rows}>{(row) => <OwedEntry row={row} />}</For>
        </>
      )}
    </For>
  );
}
