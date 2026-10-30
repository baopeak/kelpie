import type { ChatItem } from "./chat-model";

type UserItem = Extract<ChatItem, { kind: "user" }>;

/** A user item that shows a row: one with neither text nor pictures renders nothing to measure or jump to. */
const isPrompt = (it: ChatItem): it is UserItem => it.kind === "user" && (!!it.text || !!it.images?.length);

export const renderedPrompts = (items: readonly ChatItem[]) => items.filter(isPrompt);

/** A prompt within this many pixels of the top shows just under where the bar would be (a jump lands
 *  it there), so the bar would only name the turn before it. */
export const PINNED_REACH = 56;

/** Index of the last prompt that ended above `scrollTop`; -1 when none did, or when the next
 *  prompt reaches the top (`top <= scrollTop + reach`), so it is still on screen. Ascending. */
export function pinnedIndex(prompts: readonly { top: number; bottom: number }[], scrollTop: number, reach = 0): number {
  let lo = 0;
  let hi = prompts.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if (prompts[mid].bottom <= scrollTop) lo = mid + 1;
    else hi = mid;
  }
  const next = prompts[lo];
  return next && next.top <= scrollTop + reach ? -1 : lo - 1;
}

/** The prompt owning the top of the viewport, once it has scrolled off. `earlierId` is the closest
 *  prompt before the rendered window, which has no element: it only owns what lies above the first
 *  rendered prompt, not the top of one that is still on screen. */
export function pinnedPromptId(prompts: readonly { id: string; top: number; bottom: number }[], scrollTop: number, earlierId: string | null, reach = 0): string | null {
  const i = pinnedIndex(prompts, scrollTop, reach);
  if (i >= 0) return prompts[i].id;
  return !prompts.length || prompts[0].top > scrollTop + reach ? earlierId : null;
}

/** The last prompt before index `end`, or null. */
export function lastPromptBefore(items: readonly ChatItem[], end: number): string | null {
  for (let i = end - 1; i >= 0; i--) if (isPrompt(items[i])) return items[i].id;
  return null;
}
