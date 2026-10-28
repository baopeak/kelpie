import { For, Show, createMemo, onCleanup, onMount } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import { relTime } from "../lib/platform";
import { Icon, KindGlyph } from "./icons";
import { StatusDot, STATUS_WORDS } from "./sidebar";
import type { SessionInfo } from "../lib/types";
import { t, msg } from "../lib/i18n";
import { ANSWER_API, waitedFor, waitingSessions } from "../lib/attention";

/** A failed turn or a failed exit that nothing has followed yet. */
const failedNow = (s: SessionInfo) => !!s.failed && !(s.alive && (s.status === "waiting" || s.status === "working"));

const columns: { id: string; title: string; color: string; test: (s: SessionInfo) => boolean }[] = [
  { id: "waiting", title: msg("Needs you"), color: "var(--waiting)", test: (s) => s.alive && s.status === "waiting" },
  { id: "failed", title: msg("Failed"), color: "var(--danger)", test: failedNow },
  { id: "working", title: msg("Working"), color: "var(--working)", test: (s) => s.alive && s.status === "working" },
  { id: "done", title: msg("Done · unread"), color: "var(--idle)", test: (s) => s.alive && s.status === "idle" && s.unread && !failedNow(s) },
  { id: "idle", title: msg("Idle / exited"), color: "var(--text-faint)", test: (s) => (!s.alive || (s.status === "idle" && !s.unread)) && !failedNow(s) },
];

/** Every session in every project, grouped by what it needs — without rendering a single terminal. */
export function Dashboard() {
  const all = createMemo(() => Object.values(store.sessions).sort((a, b) => b.created_at - a.created_at));
  const onKey = (e: KeyboardEvent) => e.key === "Escape" && store.setOverlay(null);
  onMount(() => window.addEventListener("keydown", onKey));
  onCleanup(() => window.removeEventListener("keydown", onKey));
  const open = (s: SessionInfo) => {
    store.setOverlay(null);
    store.revealSession(s.id);
  };
  return (
    <div class="dashboard" tabIndex={-1} ref={(el) => queueMicrotask(() => el.focus())}>
      <div class="dash-head">
        <h2>{t("Agents")}</h2>
        <span class="stats">
          {t("{n} running", { n: all().filter((s) => s.alive).length })} · {store.state.projects.length === 1 ? t("{n} project", { n: 1 }) : t("{n} projects", { n: store.state.projects.length })}
        </span>
        <span class="spacer" />
        <button class="icon-btn" title={t("Close (Esc)")} onClick={() => store.setOverlay(null)}><Icon name="close" /></button>
      </div>
      <div class="dash-cols">
        <For each={columns}>
          {(col) => {
            // Needs you: the longest wait first.
            const items = createMemo(() => (col.id === "waiting" ? waitingSessions(all()) : all().filter(col.test)));
            return (
              <div class="dash-col">
                <h3><span class="dot" style={{ background: col.color }} />{t(col.title)}<span class="count">{items().length}</span></h3>
                <div class="dash-cards">
                  <For each={items()}>
                    {(s) => {
                      const project = () => store.state.projects.find((p) => p.id === s.project_id);
                      return (
                        <div class="card" onClick={() => open(s)}>
                          <div class="top">
                            <StatusDot s={s} />
                            <KindGlyph kind={s.kind} size={16} />
                            <span class="name">{sessionLabel(s)}</span>
                          </div>
                          <div class="meta">
                            <Show when={project()}>
                              {(p) => <span class="chip" style={{ "border-color": p().color, color: p().color }}>{p().name}</span>}
                            </Show>
                            <span>{s.alive ? t(STATUS_WORDS[s.status]) : t("exited {code}", { code: s.exit_code ?? "" })}</span>
                            <Show when={col.id === "waiting" && s.waiting_since} fallback={<span>· {relTime(s.created_at)}</span>}>
                              <span>· {t("waiting {time}", { time: waitedFor(s.waiting_since!) })}</span>
                            </Show>
                            <Show when={col.id === "waiting" && !s.chat && store.daemonApi() >= ANSWER_API}>
                              <button class="btn dash-answer" onClick={(e) => { e.stopPropagation(); store.openAnswer(s.id); }}>{t("Answer")}</button>
                            </Show>
                          </div>
                        </div>
                      );
                    }}
                  </For>
                  <Show when={items().length === 0}><div class="muted">{t("Nothing here")}</div></Show>
                </div>
              </div>
            );
          }}
        </For>
      </div>
    </div>
  );
}
