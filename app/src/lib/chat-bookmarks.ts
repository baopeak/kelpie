// Replies saved from chats, per project, in their own file: they outlast their session. The list is
// read once at start, and each change goes to the native side, which applies it to the file as it is
// then and returns the result. A file that could not be read is never written: the list stays read-only
// until the next start, and the user is told once.
import { createSignal } from "solid-js";
import type { ChatItem } from "../chat/chat-model";
import { errorText, t } from "./i18n";
import { applyBookmarks, loadBookmarks } from "./ipc";
import { uid } from "./platform";
import { store, type Toast } from "./store";

export interface ChatBookmark {
  id: string;
  projectId: string;
  sessionId: string;
  /** The agent's conversation, which outlives a session: a resume or a mode switch starts a new one. */
  conversationId?: string;
  /** The label when saved: the session may be gone later. */
  sessionName: string;
  /** See `anchorOf`; of the whole reply, also when `text` was cut. */
  anchor: string;
  text: string;
  truncated?: boolean;
  at: number;
}

export interface BookmarkChange {
  add?: ChatBookmark;
  remove?: string;
}

/** A chat, as far as a saved reply is concerned. */
export interface ChatRef {
  id: string;
  conversationId?: string | null;
}

export const MAX_TEXT_BYTES = 64 * 1024;
/** How long a chat gets to show a saved reply (it may have to replay its log first). */
export const REVEAL_WAIT_MS = 10_000;

/** What finds a reply again: item ids are not stable across replays, and an adapter may chunk or trim
 *  the text differently, so white space does not count (FNV-1a). Identical replies share an anchor. */
export function anchorOf(text: string): string {
  const plain = text.trim().replace(/\s+/g, " ");
  let hash = 0x811c9dc5;
  for (let i = 0; i < plain.length; i++) hash = Math.imul(hash ^ plain.charCodeAt(i), 0x01000193);
  return (hash >>> 0).toString(16);
}

/** At most `MAX_TEXT_BYTES` of UTF-8, ending on a whole character. */
function cut(text: string): Pick<ChatBookmark, "text" | "truncated"> {
  const { read } = new TextEncoder().encodeInto(text, new Uint8Array(MAX_TEXT_BYTES));
  return read < text.length ? { text: text.slice(0, read), truncated: true } : { text };
}

/** The first finished reply with this anchor. */
export const findReply = (items: ChatItem[], anchor: string) => items.find((i) => i.kind === "assistant" && !i.streaming && anchorOf(i.text) === anchor);

/** The chat to show a bookmark in: its own session while that is a chat, else one on its conversation. */
export function sessionFor<S extends { id: string; chat: boolean; agent_session_id: string | null }>(b: ChatBookmark, sessions: Record<string, S>): S | undefined {
  const own = sessions[b.sessionId];
  if (own?.chat) return own;
  return b.conversationId ? Object.values(sessions).find((s) => s.chat && s.agent_session_id === b.conversationId) : undefined;
}

const isBookmark = (v: any): v is ChatBookmark =>
  !!v && [v.id, v.projectId, v.sessionId, v.sessionName, v.anchor, v.text].every((f) => typeof f === "string") && typeof v.at === "number" && (v.truncated === undefined || typeof v.truncated === "boolean") && (v.conversationId === undefined || typeof v.conversationId === "string");

/** A real conversation id: a Codex chat starts out on the placeholder `latest`. */
const conversationOf = (id: string | null | undefined) => (id && id !== "latest" ? id : undefined);
const sessionKey = (id: string, anchor: string) => `session\n${id}\n${anchor}`;
const conversationKey = (id: string, anchor: string) => `conversation\n${id}\n${anchor}`;

export interface BookmarkDeps {
  load: () => Promise<unknown>;
  apply: (change: BookmarkChange) => Promise<{ items: unknown[]; at_limit: number | null }>;
  notify: (text: string, action?: Toast["action"]) => void;
  now?: () => number;
  newId?: () => string;
}

export function createBookmarks(deps: BookmarkDeps) {
  // With its keys, so that every reply of a long transcript can ask without scanning the list.
  const [saved, setSaved] = createSignal({ items: [] as ChatBookmark[], keys: new Set<string>() });
  const items = () => saved().items;
  /** What this build understands of a list. The rest stays in the file: only the native side writes it. */
  const setItems = (list: unknown[]) => {
    const known = list.filter(isBookmark);
    const keys = new Set(known.flatMap((b) => [sessionKey(b.sessionId, b.anchor), ...(b.conversationId ? [conversationKey(b.conversationId, b.anchor)] : [])]));
    setSaved({ items: known, keys });
  };
  const isSaved = (chat: ChatRef, anchor: string) => {
    const { keys } = saved();
    const conversation = conversationOf(chat.conversationId);
    return keys.has(sessionKey(chat.id, anchor)) || (!!conversation && keys.has(conversationKey(conversation, anchor)));
  };
  const [status, setStatus] = createSignal<"unread" | "ready" | "failed">("unread");

  /** One change at a time, in the order asked: each is decided on the list the one before returned, and
   *  a click during the first read waits for it. */
  let queue = Promise.resolve();
  const inTurn = (op: () => Promise<void>) => (queue = queue.then(op));

  /** Whether the change was made. */
  const apply = async (change: BookmarkChange) => {
    try {
      const done = await deps.apply(change);
      setItems(done.items);
      if (done.at_limit == null) return true;
      deps.notify(t("This project already has {n} saved replies: remove one first.", { n: done.at_limit }));
    } catch (e) {
      deps.notify(t("Could not update saved replies: {error}", { error: errorText(e) }));
    }
    return false;
  };

  /** Runs `change`, only if the file was read. The user has been told if it was not. */
  const changing = (change: () => Promise<unknown>) =>
    inTurn(async () => {
      if (status() === "ready") await change();
    });

  const load = () =>
    inTurn(async () => {
      try {
        const list = await deps.load();
        if (!Array.isArray(list)) throw new Error(t("the file has an unexpected shape"));
        setItems(list);
        setStatus("ready");
      } catch (e) {
        setStatus("failed");
        deps.notify(t("Saved replies could not be read: {error}. chat-bookmarks.json was left as it is, and nothing can be saved until it is fixed and Kelpie restarted.", { error: errorText(e) }));
      }
    });

  const toggle = (item: { text: string }, chat: ChatRef & { name: string }, project: { id: string }) =>
    changing(async () => {
      if (!item.text.trim()) return;
      const anchor = anchorOf(item.text);
      const conversationId = conversationOf(chat.conversationId);
      const found = items().find((b) => b.anchor === anchor && (b.sessionId === chat.id || (!!conversationId && b.conversationId === conversationId)));
      if (found) return apply({ remove: found.id });
      const at = (deps.now ?? Date.now)();
      return apply({ add: { id: (deps.newId ?? uid)(), projectId: project.id, sessionId: chat.id, ...(conversationId ? { conversationId } : {}), sessionName: chat.name, anchor, ...cut(item.text), at } });
    });

  /** From the list, where it may be the only copy left: it can be put back. */
  const remove = (b: ChatBookmark) =>
    changing(async () => {
      if (await apply({ remove: b.id })) deps.notify(t("Removed from saved"), { label: t("Undo"), run: () => inTurn(() => apply({ add: b }).then(() => {})) });
    });

  // Clicking a saved reply. The chat that should show it may not be mounted yet, so the request waits
  // here instead of going out as an event, and whoever finds nothing falls back to the saved text.
  const [pendingReveal, setPendingReveal] = createSignal<{ bookmark: ChatBookmark; sessionId: string } | null>(null);
  const [shown, setShown] = createSignal<ChatBookmark | null>(null);
  let revealTimer: ReturnType<typeof setTimeout> | undefined;

  const endReveal = () => {
    clearTimeout(revealTimer);
    setPendingReveal(null);
  };
  const showSaved = (b: ChatBookmark) => {
    endReveal();
    setShown(b);
  };
  const settleReveal = (asked: { bookmark: ChatBookmark; sessionId: string }, found: boolean) => {
    if (pendingReveal() !== asked) return;
    if (found) endReveal();
    else showSaved(asked.bookmark);
  };

  return {
    load,
    toggle,
    remove,
    readOnly: () => status() === "failed",
    isSaved,
    /** A project's bookmarks, newest first. */
    inProject: (projectId: string) => items().filter((b) => b.projectId === projectId).reverse().sort((a, b) => b.at - a.at),
    pendingReveal,
    askReveal: (bookmark: ChatBookmark, sessionId: string) => {
      clearTimeout(revealTimer);
      const asked = { bookmark, sessionId };
      setPendingReveal(asked);
      revealTimer = setTimeout(() => settleReveal(asked, false), REVEAL_WAIT_MS);
    },
    settleReveal,
    /** The bookmark whose saved text the dialog shows. */
    shown,
    showSaved,
    closeSaved: () => setShown(null),
  };
}

export const bookmarks = createBookmarks({
  load: loadBookmarks,
  apply: applyBookmarks,
  notify: (text, action) => store.notify(text, action, 8000),
});
