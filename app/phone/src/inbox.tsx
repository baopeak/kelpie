// The inbox: every session across projects, grouped by what it needs from you (the longest waiting first) or by the workspace it
// runs in, as chosen above the list or in Settings.
import { createSignal, For, Show } from "solid-js";
import { endedAgo } from "./conn-text";
import { intlLocale, msg, t } from "./i18n";
import { groupByWorkspace, groupOf, groupSessions, labelOf, shownRows, whereOf, type GroupKey } from "./inbox-order";
import type { SessionRow } from "./link";
import { canSeeScreen, connected, discardNotSent, notSent, now, openScreenPicker, openSession, paintedFrom, projects, sessions } from "./phone-state";
import { INBOX_VIEWS, inboxView, order, showStopped, type InboxView } from "./prefs";
import { waitedFor } from "../../src/lib/attention";

/** Messages this phone sent to chats that are gone from the computer's list (closed, or resumed as a new session) and that were
 *  never sent: their text, to copy and send again, until the person lets them go. */
function NotSent() {
  const [notice, setNotice] = createSignal<string | null>(null);
  const copy = async (text: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setNotice(t("Copied"));
    } catch {
      setNotice(t("Could not copy: select the text and copy it."));
    }
  };
  return (
    <Show when={notSent().length}>
      <section class="not-sent" data-group="not-sent">
        <div class="group">{t("Not sent")} · {notSent().reduce((n, c) => n + c.items.length, 0)}</div>
        <p class="muted not-sent-hint">{t("From chats that are no longer running here (closed, or resumed as a new chat). Copy the text to send it again.")}</p>
        <For each={notSent()}>
          {(c) => (
            <div class="chips kept-chips" data-chat={c.chat}>
              <div class="muted">{c.name ?? t("A chat that was closed")}</div>
              <For each={c.items}>
                {(p) => (
                  <div class="chip warn" role="status">
                    {/* Its fate is unknown: it may have reached the agent, so say so before anyone sends it twice. */}
                    <Show when={p.state === "unknown"}><span class="muted">{t("Not sure it was sent")}</span></Show>
                    <span class="chip-text">{p.text}</span>
                    <button class="link-btn" onClick={() => void copy(p.text)}>{t("Copy")}</button>
                    <button class="link-btn" onClick={() => discardNotSent(c.chat, p.cid)}>{t("Discard")}</button>
                  </div>
                )}
              </For>
            </div>
          )}
        </For>
        <Show when={notice()}><div class="notice" role="status">{notice()}</div></Show>
      </section>
    </Show>
  );
}

const TITLES: Record<GroupKey, string> = {
  needs: msg("Needs you"),
  working: msg("Working"),
  done: msg("Done"),
  idle: msg("Idle"),
  stopped: msg("Stopped"),
};

const Empty = () => <div class="card"><p class="muted">{t("No sessions are running.")}</p></div>;

/** The name of an inbox view, here and in Settings. */
export const inboxViewName = (v: InboxView) => (v === "status" ? t("By status") : t("By workspace"));

/** One session in the list. By workspace, the section already says where it runs, so the row says its state in a word instead. */
function SessionItem(props: { s: SessionRow; byWorkspace: boolean }) {
  const group = () => groupOf(props.s);
  const sub = () => {
    const s = props.s;
    return [
      props.byWorkspace ? t(TITLES[group()]) : "",
      // A stopped session says when it ended (its last activity), not only that it did.
      !s.alive && s.last_activity > 0 ? endedAgo(s.last_activity, now()) : "",
      s.kind,
      props.byWorkspace ? "" : whereOf(s, projects()),
      s.chat ? t("chat") : "",
    ].filter(Boolean).join(" · ");
  };
  return (
    <div class="row" data-id={props.s.id} role="button" tabIndex={0} onClick={() => openSession(props.s.id)} onKeyDown={(e) => e.key === "Enter" && openSession(props.s.id)}>
      <span class={`dot ${props.s.alive ? props.s.status : "exited"}`} />
      <div class="main">
        <div class="name">{labelOf(props.s)}</div>
        <div class="sub">{sub()}</div>
      </div>
      <Show when={group() === "needs" && props.s.waiting_since}><span class="waited">{waitedFor(props.s.waiting_since!, now())}</span></Show>
      <Show when={props.s.unread && group() !== "needs"}><span class="badge">{t("new")}</span></Show>
    </div>
  );
}

export function Inbox() {
  const shown = () => shownRows(sessions(), showStopped.get());
  const groups = () => groupSessions(shown().rows, order.get(), intlLocale());
  const workspaces = () => groupByWorkspace(shown().rows, projects(), order.get(), intlLocale());
  return (
    <div classList={{ painted: paintedFrom() !== null }}>
      {/* The computer's screen is reached from here too, not only from a session (a dialog may have nothing to do with one). */}
      <Show when={canSeeScreen()}>
        <div class="takeover-entry"><button class="btn" data-see-screen disabled={!connected()} onClick={openScreenPicker}>{t("See the computer's screen")}</button></div>
      </Show>
      <div class="view-switch" role="group" aria-label={t("Inbox view")} data-inbox-view={inboxView.get()}>
        <For each={INBOX_VIEWS}>
          {(v) => <button classList={{ on: inboxView.get() === v }} data-view={v} aria-pressed={inboxView.get() === v} onClick={() => inboxView.set(v)}>{inboxViewName(v)}</button>}
        </For>
      </div>
      <Show
        when={inboxView.get() === "workspace"}
        fallback={
          <For each={groups()} fallback={<Empty />}>
            {(g) => (
              <section data-group={g.key}>
                <div class="group">{t(TITLES[g.key])} · {g.rows.length}</div>
                <For each={g.rows}>{(s) => <SessionItem s={s} byWorkspace={false} />}</For>
              </section>
            )}
          </For>
        }
      >
        <For each={workspaces()} fallback={<Empty />}>
          {(w) => (
            <section class="workspace" data-workspace={w.key}>
              <div class="group">{w.name ?? t("Other folders")} · {w.rows.length}</div>
              <For each={w.rows}>{(s) => <SessionItem s={s} byWorkspace />}</For>
            </section>
          )}
        </For>
      </Show>
      <Show when={shown().hidden}>
        <button class="stopped-hidden" data-stopped-hidden={shown().hidden} onClick={() => showStopped.set(true)}>
          <span>{t("{n} stopped hidden", { n: shown().hidden })}</span>
          <span class="show">{t("Show")}</span>
        </button>
      </Show>
      <NotSent />
    </div>
  );
}
