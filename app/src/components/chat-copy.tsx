// Copying from the transcript: the shared clipboard call and the small copy button of its rows. Next
// to it on a reply, the button that saves it.
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { createMemo } from "solid-js";
import { anchorOf, bookmarks } from "../lib/chat-bookmarks";
import { sessionLabel, store } from "../lib/store";
import { t } from "../lib/i18n";
import type { Project } from "../lib/types";
import { Icon } from "./icons";

/** Copies `text`, ticking the button that asked for a moment. */
export function copyText(text: string, button: HTMLElement) {
  void writeText(text).then(
    () => {
      button.classList.add("copied");
      setTimeout(() => button.classList.remove("copied"), 1200);
    },
    () => store.notify(t("Could not copy")),
  );
}

export function CopyButton(props: { text: string }) {
  return (
    <button class="msg-action" title={t("Copy")} aria-label={t("Copy")} onClick={(e) => copyText(props.text, e.currentTarget)}>
      <span class="i-copy"><Icon name="copy" size={12} /></span><span class="i-done"><Icon name="check" size={12} /></span>
    </button>
  );
}

/** Saves a finished reply, or takes it out of the saved ones. */
export function BookmarkButton(props: { text: string; sessionId: string; conversationId: string | null; project: Project }) {
  // Apart: a change to the list asks every mounted reply again, which must not hash them all again.
  const anchor = createMemo(() => anchorOf(props.text));
  const saved = createMemo(() => bookmarks.isSaved({ id: props.sessionId, conversationId: props.conversationId }, anchor()));
  const label = () => (bookmarks.readOnly() ? t("Saved replies are read-only: the file could not be read") : saved() ? t("Remove from saved") : t("Save this reply"));
  const toggle = () => {
    const s = store.sessions[props.sessionId];
    void bookmarks.toggle({ text: props.text }, { id: props.sessionId, conversationId: props.conversationId, name: s ? sessionLabel(s) : "" }, props.project);
  };
  return (
    <button class="msg-action bookmark" classList={{ saved: saved() }} disabled={bookmarks.readOnly()} title={label()} aria-label={label()} aria-pressed={saved()} onClick={toggle}>
      <Icon name={saved() ? "bookmarkFilled" : "bookmark"} size={12} />
    </button>
  );
}
