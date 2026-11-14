// The bar under a pane's header for a session that is part of a sub-session tree: the pane's sub-tabs. One chip for the session at
// the top and one for each session below it (status, agent, name, what it last reported); a click shows that session in this pane,
// so a family of sessions lives in one tab instead of one tab each.
import { For, Show, createMemo } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import { relTimeShort } from "../lib/platform";
import { msg, t } from "../lib/i18n";
import { familyOf, familyRoot, type FamilyMember } from "../lib/sub-sessions";
import { closeFinishedSubSessions, finishedOf } from "../lib/sub-session-actions";
import { KindGlyph } from "./icons";
import { StatusDot } from "./sidebar";
import { openContextMenu } from "./context-menu";
import type { SessionInfo, SubReport } from "../lib/types";

/** Chips shown before "+N" (the one on screen is always among them). */
const MAX_CHIPS = 6;

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

function chipTitle(s: SessionInfo, top: boolean): string {
  if (top) return `${sessionLabel(s)}\n${t("The session that opened the others")}`;
  const r = s.report;
  if (!r) return `${sessionLabel(s)}\n${t("No report yet")}`;
  return [`${sessionLabel(s)} · ${reportWord(r)} · ${relTimeShort(r.at)}`, r.summary].filter(Boolean).join("\n");
}

function Chip(props: { m: FamilyMember; active: boolean; onPick: () => void }) {
  const s = () => props.m.s;
  return (
    <button
      class={`sub-chip${s().report && props.m.depth ? ` r-${s().report!.status}` : ""}${props.active ? " active" : ""}${props.m.depth ? "" : " top"}`}
      role="tab"
      aria-selected={props.active}
      title={chipTitle(s(), !props.m.depth)}
      onClick={() => props.onPick()}
    >
      {/* Deeper than one level: one mark per extra level, so a grandchild reads as one. */}
      <Show when={props.m.depth > 1}><span class="sub-chip-depth">{"›".repeat(props.m.depth - 1)}</span></Show>
      <StatusDot s={s()} />
      <KindGlyph kind={s().kind} size={13} />
      <span class="sub-chip-name">{sessionLabel(s())}</span>
      <Show when={props.m.depth ? s().report : undefined}>{(r) => <span class="sub-chip-report">{reportWord(r())}</span>}</Show>
    </button>
  );
}

export function SubSessionsBar(props: { sessionId: string; projectId: string; tabId: string; paneId: string }) {
  const root = () => store.sessions[familyRoot(store.sessionTree(), props.sessionId)];
  // Compared by identity so a status tick does not rebuild the chips.
  const family = createMemo<FamilyMember[]>((prev) => {
    const r = root();
    const next = r ? familyOf(store.sessionTree(), r) : [];
    return prev && prev.length === next.length && prev.every((x, i) => x.s === next[i].s && x.depth === next[i].depth) ? prev : next;
  });
  const shown = createMemo(() => {
    const all = family();
    if (all.length <= MAX_CHIPS) return all;
    const first = all.slice(0, MAX_CHIPS);
    const here = all.find((m) => m.s.id === props.sessionId);
    return here && !first.includes(here) ? [...first.slice(0, MAX_CHIPS - 1), here] : first;
  });
  const rest = () => family().filter((m) => !shown().includes(m));
  const pick = (id: string) => store.showInPane(props.projectId, props.tabId, props.paneId, id);
  const closable = () => {
    const r = root();
    return r ? finishedOf(r, true) : [];
  };
  const menu = (e: MouseEvent) => {
    const r = root();
    if (!r || family().length < 2) return;
    openContextMenu(e, [
      ...(closable().length ? [{ label: t("Close finished sub-sessions ({n})", { n: closable().length }), run: () => void closeFinishedSubSessions(r, true) }] : []),
      ...(closable().length && rest().length ? [null] : []),
      ...rest().map((m) => ({ label: `${sessionLabel(m.s)}${m.s.report ? ` · ${reportWord(m.s.report)}` : ""}`, run: () => pick(m.s.id) })),
    ]);
  };
  return (
    <Show when={family().length > 1}>
      <div class="sub-strip" role="group" aria-label={t("Sub-sessions")} onContextMenu={menu}>
        <div class="sub-chips" role="tablist">
          <For each={shown()}>{(m) => <Chip m={m} active={m.s.id === props.sessionId} onPick={() => pick(m.s.id)} />}</For>
          <Show when={rest().length}>
            <button class="sub-chip more" title={t("More sub-sessions")} onClick={menu}>+{rest().length}</button>
          </Show>
        </div>
        <Show when={closable().length}>
          <button class="link-btn sub-close" title={t("Close the sub-sessions that are done or have exited")} onClick={() => void closeFinishedSubSessions(root()!, true)}>{t("Close finished")}</button>
        </Show>
      </div>
    </Show>
  );
}
