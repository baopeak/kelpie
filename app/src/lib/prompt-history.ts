// Prompts sent from the composer, remembered per project (newest first): ↑ / ↓ in the composer
// walks them and the command palette searches them.
export const MAX_HISTORY = 100;

/** `list` with `text` first; an earlier copy of it is dropped. Blank text changes nothing. */
export function remember(list: readonly string[] | undefined, text: string): string[] {
  const entry = text.trim();
  const rest = (list ?? []).filter((x) => x !== entry);
  return entry ? [entry, ...rest].slice(0, MAX_HISTORY) : [...rest];
}

/** The position after ↑ (`dir` = 1, older) or ↓ (-1, newer) from `index` (-1 is the draft being
 *  typed); stays put at either end. */
export function stepHistory(length: number, index: number, dir: 1 | -1): number {
  return Math.min(length - 1, Math.max(-1, index + dir));
}

/** Entries holding every word of `query`, newest first (all of them for an empty query). */
export function searchHistory(list: readonly string[], query: string, limit = 50): string[] {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  return list.filter((x) => words.every((w) => x.toLowerCase().includes(w))).slice(0, limit);
}
