// The inbox: every session across projects, grouped by what it needs from you, the longest waiting first.
import { For, Show } from "solid-js";
import { msg, t } from "./i18n";
import { groupSessions, labelOf, whereOf, type GroupKey } from "./inbox-order";
import { now, openSession, paintedFrom, projects, sessions } from "./phone-state";
import { waitedFor } from "../../src/lib/attention";

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
    </div>
  );
}
