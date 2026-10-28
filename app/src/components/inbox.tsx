// Notification inbox and activity feed: agents that finished or need you, automation runs,
// new worktrees — newest first, across every project. Opened from the status-bar bell.
import { For, Show, createMemo, createSignal } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import { relTimeShort } from "../lib/platform";
import { Icon } from "./icons";
import { Kip, kipShown } from "./brand";
import type { InboxItem } from "../lib/types";
import { waitedFor, waitingSessions } from "../lib/attention";
import { t } from "../lib/i18n";

const ICON: Record<InboxItem["kind"], string> = { finished: "check", waiting: "bell", automation: "clock", worktree: "branch", error: "close", cost: "alert" };

export function InboxPanel(props: { onClose: () => void }) {
  const [filter, setFilter] = createSignal<"all" | "unread">("all");
  const [query, setQuery] = createSignal("");
  const items = createMemo(() => {
    const q = query().trim().toLowerCase();
    return (store.state.inbox ?? []).filter((i) => (filter() === "all" || !i.read) && (!q || `${i.title} ${i.body}`.toLowerCase().includes(q)));
  });
  // Working agents stay on top: the feed is for catching up on everything else.
  const working = createMemo(() => Object.values(store.sessions).filter((s) => s.alive && s.status === "working" && s.kind !== "shell"));
  // Those waiting on the user first, the longest wait on top: answered right here.
  const waiting = createMemo(() => waitingSessions(Object.values(store.sessions)));

  const open = (i: InboxItem) => {
    store.markInbox(i.id);
    if (i.sessionId && store.sessions[i.sessionId]) store.revealSession(i.sessionId);
    else if (i.projectId && store.state.projects.some((p) => p.id === i.projectId)) store.switchProject(i.projectId);
    props.onClose();
  };

  return (
    <div class="inbox" onPointerDown={(e) => e.stopPropagation()} onClick={(e) => e.stopPropagation()}>
      <div class="inbox-head">
        <b>{t("Activity")}</b>
        <div class="toggles">
          <button class={`toggle${filter() === "all" ? " on" : ""}`} onClick={() => setFilter("all")}>{t("All")}</button>
          <button class={`toggle${filter() === "unread" ? " on" : ""}`} onClick={() => setFilter("unread")}>{t("Unread")} {store.unreadInbox() || ""}</button>
        </div>
        <span style={{ flex: 1 }} />
        <button class="link-btn" onClick={() => store.markInbox("all")}>{t("Mark all read")}</button>
        <button class="link-btn" onClick={() => store.clearInbox()}>{t("Clear")}</button>
      </div>
      <input class="text-input" placeholder={t("Filter")} value={query()} onInput={(e) => setQuery(e.currentTarget.value)} style={{ margin: "0 10px 8px", width: "calc(100% - 20px)" }} />
      <div class="inbox-list">
        <Show when={waiting().length}>
          <div class="group-label">{t("Needs you")}</div>
          <For each={waiting()}>
            {(s) => (
              <div class="inbox-item" onClick={() => { store.openAnswer(s.id); props.onClose(); }}>
                <span class="dot waiting" />
                <span class="inbox-text">
                  <span class="inbox-title">{sessionLabel(s)}</span>
                  <span class="inbox-body">{s.waiting_since ? t("waiting {time}", { time: waitedFor(s.waiting_since) }) : store.state.projects.find((p) => p.id === s.project_id)?.name ?? s.cwd}</span>
                </span>
              </div>
            )}
          </For>
        </Show>
        <Show when={working().length}>
          <div class="group-label">{t("Working now")}</div>
          <For each={working()}>
            {(s) => (
              <div class="inbox-item" onClick={() => { store.revealSession(s.id); props.onClose(); }}>
                <span class="dot working" />
                <span class="inbox-text"><span class="inbox-title">{sessionLabel(s)}</span><span class="inbox-body">{store.state.projects.find((p) => p.id === s.project_id)?.name ?? s.cwd}</span></span>
              </div>
            )}
          </For>
          <div class="group-label">{t("Earlier")}</div>
        </Show>
        <For each={items()} fallback={<div class="inbox-empty"><Show when={kipShown()}><Kip mood="sleep" size={96} /></Show><div class="muted">{t("Nothing here yet. Agents that finish or need you while you look elsewhere show up here.")}</div></div>}>
          {(i) => (
            <div
              class={`inbox-item${i.read ? "" : " unread"} k-${i.kind}`}
              onClick={() => open(i)}
              onContextMenu={(e) => { e.preventDefault(); store.markInbox(i.id, !i.read); }}
              title={i.read ? t("Right-click: mark unread") : t("Right-click: mark read")}
            >
              <span class="inbox-icon"><Icon name={ICON[i.kind]} size={13} /></span>
              <span class="inbox-text">
                <span class="inbox-title">{i.title}</span>
                <span class="inbox-body">{i.body}</span>
              </span>
              <span class="age">{relTimeShort(i.at, Date.now())}</span>
            </div>
          )}
        </For>
      </div>
    </div>
  );
}
