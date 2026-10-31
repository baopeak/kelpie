// The inbox: every session across projects, grouped by what it needs from you, the longest waiting first.
import { createSignal, For, Show } from "solid-js";
import { msg, t } from "./i18n";
import { groupSessions, labelOf, whereOf, type GroupKey } from "./inbox-order";
import { discardNotSent, notSent, now, openSession, paintedFrom, projects, sessions } from "./phone-state";
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

export function Inbox() {
  const groups = () => groupSessions(sessions());
  return (
    <div classList={{ painted: paintedFrom() !== null }}>
      <For each={groups()} fallback={<div class="card"><p class="muted">{t("No sessions are running.")}</p></div>}>
        {(g) => (
          <section data-group={g.key}>
            <div class="group">{t(TITLES[g.key])} · {g.rows.length}</div>
            <For each={g.rows}>
              {(s) => (
                <div class="row" data-id={s.id} role="button" tabIndex={0} onClick={() => openSession(s.id)} onKeyDown={(e) => e.key === "Enter" && openSession(s.id)}>
                  <span class={`dot ${s.alive ? s.status : "exited"}`} />
                  <div class="main">
                    <div class="name">{labelOf(s)}</div>
                    <div class="sub">{s.kind} · {whereOf(s, projects())}{s.chat ? ` · ${t("chat")}` : ""}</div>
                  </div>
                  <Show when={g.key === "needs" && s.waiting_since}><span class="waited">{waitedFor(s.waiting_since!, now())}</span></Show>
                  <Show when={s.unread && g.key !== "needs"}><span class="badge">{t("new")}</span></Show>
                </div>
              )}
            </For>
          </section>
        )}
      </For>
      <NotSent />
    </div>
  );
}
