// The inbox: every session across projects, grouped by what it needs from you (the longest waiting first) or by the workspace it
// runs in, as chosen above the list or in Settings.
import { createSignal, For, Show } from "solid-js";
import { endedAgo } from "./conn-text";
import { intlLocale, msg, t } from "./i18n";
import { groupByWorkspace, groupOf, groupSessions, labelOf, shownRows, whereOf, type GroupKey } from "./inbox-order";
import type { SessionRow } from "./link";
import { canSeeScreen, canStartSessions, connected, discardNotSent, notSent, now, openScreenPicker, openSession, paintedFrom, projects, sessions } from "./phone-state";
import { Icon, type IconName } from "./icons";
import kip from "../../src/assets/brand/kip-peek.svg";
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
        <div class="group">{t("Not sent")} <span class="count">{notSent().reduce((n, c) => n + c.items.length, 0)}</span></div>
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

/** A state in a word: the inbox's headings, and what a row or an open session says it is in. */
export const STATE_WORDS: Record<GroupKey, string> = {
  needs: msg("Needs you"),
  working: msg("Working"),
  done: msg("Done"),
  idle: msg("Idle"),
  stopped: msg("Stopped"),
};

/** Nothing runs: Kip, and where a session comes from. */
const Empty = () => (
  <div class="empty" data-inbox-empty>
    <img src={kip} alt="" />
    <h2>{t("No sessions are running.")}</h2>
    <p class="muted">{canStartSessions() ? t("Tap New session to start an agent in one of your projects.") : t("Sessions you start in Kelpie on your computer show up here.")}</p>
  </div>
);

/** Each state's glyph and the class its tile is drawn with (`exited`, `waiting`, `working`, `idle` as the daemon names them; done is idle and unread). */
const STATE_LOOK: Record<GroupKey, { icon: IconName; cls: string }> = {
  needs: { icon: "needs", cls: "waiting" },
  working: { icon: "working", cls: "working" },
  done: { icon: "done", cls: "idle done" },
  idle: { icon: "idle", cls: "idle" },
  stopped: { icon: "stopped", cls: "exited" },
};

/** The name of an inbox view, here and in Settings. */
export const inboxViewName = (v: InboxView) => (v === "status" ? t("By status") : t("By workspace"));

/** One session in the list. By workspace, the section already says where it runs, so the row says its state in a word instead. */
function SessionItem(props: { s: SessionRow; byWorkspace: boolean }) {
  const group = () => groupOf(props.s);
  const sub = () => {
    const s = props.s;
    return [
      props.byWorkspace ? t(STATE_WORDS[group()]) : "",
      // A stopped session says when it ended (its last activity), not only that it did.
      !s.alive && s.last_activity > 0 ? endedAgo(s.last_activity, now()) : "",
      s.kind,
      props.byWorkspace ? "" : whereOf(s, projects()),
      s.chat ? t("chat") : "",
    ].filter(Boolean).join(" · ");
  };
  return (
    <div class="row" data-id={props.s.id} role="button" tabIndex={0} onClick={() => openSession(props.s.id)} onKeyDown={(e) => e.key === "Enter" && openSession(props.s.id)}>
      <span class={`dot ${STATE_LOOK[group()].cls}`} title={t(STATE_WORDS[group()])}><Icon name={STATE_LOOK[group()].icon} size={20} /></span>
      <div class="main">
        <div class="name">{labelOf(props.s)}</div>
        <div class="sub">{sub()}</div>
      </div>
      <span class="trail">
        <Show when={group() === "needs" && props.s.waiting_since}><span class="waited">{waitedFor(props.s.waiting_since!, now())}</span></Show>
        <Show when={props.s.unread && group() !== "needs"}><span class="badge new">{t("new")}</span></Show>
        <Icon name="forward" size={18} class="chev" />
      </span>
    </div>
  );
}

export function Inbox() {
  const shown = () => shownRows(sessions(), showStopped.get());
  const groups = () => groupSessions(shown().rows, order.get(), intlLocale());
  const workspaces = () => groupByWorkspace(shown().rows, projects(), order.get(), intlLocale());
  return (
    <div class="inbox" classList={{ painted: paintedFrom() !== null }}>
      {/* The computer's screen is reached from here too, not only from a session (a dialog may have nothing to do with one). */}
      <Show when={canSeeScreen()}>
        <button class="entry" data-see-screen disabled={!connected()} onClick={openScreenPicker}>
          <span class="entry-icon"><Icon name="monitor" size={20} /></span>
          <span class="grow">{t("See the computer's screen")}</span>
          <Icon name="forward" size={18} class="chev" />
        </button>
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
                <div class="group">{t(STATE_WORDS[g.key])} <span class="count">{g.rows.length}</span></div>
                <div class="rows"><For each={g.rows}>{(s) => <SessionItem s={s} byWorkspace={false} />}</For></div>
              </section>
            )}
          </For>
        }
      >
        <For each={workspaces()} fallback={<Empty />}>
          {(w) => (
            <section class="workspace" data-workspace={w.key}>
              <div class="group">{w.name ?? t("Other folders")} <span class="count">{w.rows.length}</span></div>
              <div class="rows"><For each={w.rows}>{(s) => <SessionItem s={s} byWorkspace />}</For></div>
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
