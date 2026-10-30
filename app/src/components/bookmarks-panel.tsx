// The "Saved" tab: replies saved from this project's chats. A click shows the reply in its chat when
// it is still there, and otherwise the saved text in a dialog (one instance, rendered by <App>).
import { For, Show, batch, createEffect, createMemo } from "solid-js";
import { bookmarks, sessionFor, type ChatBookmark } from "../lib/chat-bookmarks";
import { setCovering } from "../lib/overlays";
import { store } from "../lib/store";
import { t } from "../lib/i18n";
import { Markdown } from "./chat-pane";
import { copyText } from "./chat-copy";
import { clock, fullClock } from "./chat-rows";
import { Icon } from "./icons";
import type { Project } from "../lib/types";

/** More than two lines of the narrowest panel can hold; the rest is clamped away. */
const EXCERPT_CHARS = 400;

function open(b: ChatBookmark) {
  const chat = sessionFor(b, store.sessions);
  if (!chat || !store.connected()) return bookmarks.showSaved(b);
  // Together: a chat that is open already must find its tab in front when it scrolls.
  batch(() => {
    bookmarks.askReveal(b, chat.id);
    store.revealSession(chat.id);
  });
}

export function BookmarksPanel(props: { project: Project }) {
  const list = createMemo(() => bookmarks.inProject(props.project.id));
  return (
    <div class="saved-panel">
      <Show when={bookmarks.readOnly()}>
        <p class="saved-readonly">{t("Saved replies could not be read, so this list is read-only. chat-bookmarks.json in the data folder was left as it is: fix or remove it, then restart Kelpie.")}</p>
      </Show>
      <Show when={list().length} fallback={<Show when={!bookmarks.readOnly()}><div class="muted">{t("No saved replies yet. Use the bookmark on a reply to keep it here.")}</div></Show>}>
        <For each={list()}>
          {(b) => (
            <div class="list-row saved-row" data-bookmark={b.id} onClick={() => open(b)}>
              {/* For the keyboard: its click reaches the row, which takes the mouse in its padding too. */}
              <button class="saved-main">
                <span class="saved-excerpt">{b.text.slice(0, EXCERPT_CHARS)}</span>
                <span class="saved-meta">
                  <span class="saved-session" title={b.sessionName}>{b.sessionName}</span>
                  <span title={fullClock(b.at)}>{clock(b.at)}</span>
                  <Show when={b.truncated}><span class="saved-cut" title={t("This reply was longer than 64 KiB: only the start of it was saved.")}>{t("Cut short")}</span></Show>
                </span>
              </button>
              <span class="row-actions">
                <button class="icon-btn saved-remove" title={t("Remove")} aria-label={t("Remove")} onClick={(e) => {
                  e.stopPropagation();
                  void bookmarks.remove(b);
                }}><Icon name="close" size={13} /></button>
              </span>
            </div>
          )}
        </For>
      </Show>
    </div>
  );
}

/** A saved reply on its own, for when its session or its place in the transcript is gone. */
export function SavedReplyDialog() {
  createEffect(() => setCovering("saved-reply", !!bookmarks.shown()));
  return (
    <Show when={bookmarks.shown()} keyed>
      {(b) => (
        <div class="overlay" onPointerDown={(e) => e.target === e.currentTarget && bookmarks.closeSaved()}>
          {/* Focused on open, so keys reach the dialog and not the composer or pane behind it. */}
          <div class="modal saved-reply" tabIndex={-1} ref={(el) => queueMicrotask(() => el.focus())} onKeyDown={(e) => e.key === "Escape" && bookmarks.closeSaved()}>
            <h2>{b.sessionName}</h2>
            <span class="muted-inline">{fullClock(b.at)}</span>
            <Show when={b.truncated}><p class="update-warning">{t("This reply was longer than 64 KiB: only the start of it was saved.")}</p></Show>
            <div class="saved-reply-text"><Markdown text={b.text} project={store.state.projects.find((p) => p.id === b.projectId)} /></div>
            <div class="footer">
              <button class="btn saved-copy" onClick={(e) => copyText(b.text, e.currentTarget)}>
                <span class="i-copy"><Icon name="copy" size={13} /></span><span class="i-done"><Icon name="check" size={13} /></span> {t("Copy")}
              </button>
              <button class="btn primary saved-close" onClick={() => bookmarks.closeSaved()}>{t("Close")}</button>
            </div>
          </div>
        </div>
      )}
    </Show>
  );
}
