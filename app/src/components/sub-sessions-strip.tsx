// The bar under a pane's header for a session that is part of a sub-session tree: "Sub-session of <parent>" (click: the parent) and,
// when the session opened sub-sessions, one chip for each (status, agent, name, what it last reported; click: open it).
import { For, Show, createMemo } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import { relTimeShort } from "../lib/platform";
import { msg, t } from "../lib/i18n";
import { childrenOfSession } from "../lib/sub-sessions";
import { closeFinishedSubSessions, finishedOf } from "../lib/sub-session-actions";
import { KindGlyph } from "./icons";
import { StatusDot } from "./sidebar";
import { openContextMenu } from "./context-menu";
import type { SessionInfo, SubReport } from "../lib/types";

/** Chips shown before "+N". */
const MAX_CHIPS = 5;

/** What a report says as one word. */
const REPORT_WORDS: Record<string, string> = {
  progress: msg("working on it"),
  blocked: msg("blocked"),
  needs_user: msg("needs you"),
  done: msg("done"),
  failed: msg("failed"),
  idle: msg("idle"),
  exited: msg("exited"),
};
const reportWord = (r: SubReport) => t(REPORT_WORDS[r.status] ?? r.status);

function chipTitle(s: SessionInfo): string {
  const r = s.report;
  if (!r) return `${sessionLabel(s)}\n${t("No report yet")}`;
  return [`${sessionLabel(s)} · ${reportWord(r)} · ${relTimeShort(r.at)}`, r.summary].filter(Boolean).join("\n");
}

function Chip(props: { s: SessionInfo }) {
  return (
    <button class={`sub-chip${props.s.report ? ` r-${props.s.report.status}` : ""}`} title={chipTitle(props.s)} onClick={() => store.revealSession(props.s.id)}>
      <StatusDot s={props.s} />
      <KindGlyph kind={props.s.kind} size={13} />
      <span class="sub-chip-name">{sessionLabel(props.s)}</span>
      <Show when={props.s.report}>{(r) => <span class="sub-chip-report">{reportWord(r())}</span>}</Show>
    </button>
  );
}

export function SubSessionsBar(props: { sessionId: string }) {
  const session = () => store.sessions[props.sessionId];
  // Compared by identity so a status tick does not rebuild the chips.
  const kids = createMemo<SessionInfo[]>((prev) => {
    const next = childrenOfSession(store.sessionTree(), props.sessionId);
    return prev && prev.length === next.length && prev.every((x, i) => x === next[i]) ? prev : next;
  });
  const parent = () => {
    const id = store.sessionTree().nested.get(props.sessionId);
    return id ? store.sessions[id] : undefined;
  };
  const shown = () => kids().slice(0, MAX_CHIPS);
  const rest = () => kids().slice(MAX_CHIPS);
  const closable = () => {
    const s = session();
    return s ? finishedOf(s) : [];
  };
  const menu = (e: MouseEvent) => {
    const s = session();
    if (!s || !kids().length) return;
    openContextMenu(e, [
      ...(closable().length ? [{ label: t("Close finished sub-sessions ({n})", { n: closable().length }), run: () => void closeFinishedSubSessions(s) }] : []),
      ...(closable().length && rest().length ? [null] : []),
      ...rest().map((c) => ({ label: `${sessionLabel(c)}${c.report ? ` · ${reportWord(c.report)}` : ""}`, run: () => store.revealSession(c.id) })),
    ]);
  };
  return (
    <Show when={parent() || kids().length}>
      <div class="sub-strip" role="group" aria-label={t("Sub-sessions")} onContextMenu={menu}>
        <Show when={parent()}>
          {(p) => (
            <button class="sub-parent" title={t("Open the session that opened this one")} onClick={() => store.revealSession(p().id)}>
              {t("Sub-session of")} <b>{sessionLabel(p())}</b>
            </button>
          )}
        </Show>
        <Show when={kids().length}>
          <span class="sub-strip-label">{t("Sub-sessions")}</span>
          <div class="sub-chips">
            <For each={shown()}>{(c) => <Chip s={c} />}</For>
            <Show when={rest().length}>
              <button class="sub-chip more" title={t("More sub-sessions")} onClick={menu}>+{rest().length}</button>
            </Show>
          </div>
          <Show when={closable().length}>
            <button class="link-btn sub-close" title={t("Close the sub-sessions that are done or have exited")} onClick={() => void closeFinishedSubSessions(session()!)}>{t("Close finished")}</button>
          </Show>
        </Show>
      </div>
    </Show>
  );
}
