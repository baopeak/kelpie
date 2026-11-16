// What each key of a menu prompt does, read from the screen the phone already shows: the daemon sends the keys only, and a card of bare
// "1" and "3" makes the person map digits to options by reading the terminal. A label is given only when the screen says it plainly: the
// last lines that list the keys as "1. Yes" (a marker such as ❯ before it allowed), each key once, in the same order as the keys.
// Anything else (no such lines, a key listed twice, a key missing) gives no labels at all, so the card never names an option wrongly.

/** How far up from the bottom a menu is looked for. */
const LOOK_BACK = 30;
const OPTION = /^\s*(?:[❯›>»▶●○◉*]\s*)?(\S{1,3})[.)]\s+(.+?)\s*$/;

/** Labels by key for `keys`, or an empty map when the screen does not show them plainly. */
export function promptLabels(rows: string[], keys: string[]): Record<string, string> {
  if (!keys.length) return {};
  const tail = rows.slice(-LOOK_BACK);
  // The menu is the last run of option lines: walk up from the bottom to its last line, then take the run.
  let end = tail.length - 1;
  while (end >= 0 && !OPTION.test(tail[end])) end--;
  if (end < 0) return {};
  const found: { key: string; label: string }[] = [];
  for (let i = end; i >= 0; i--) {
    const m = OPTION.exec(tail[i]);
    if (!m) {
      // A wrapped option's second line is indented text: it belongs to the option above and ends nothing.
      if (/^\s{3,}\S/.test(tail[i]) && found.length) continue;
      break;
    }
    found.unshift({ key: m[1], label: m[2] });
  }
  const out: Record<string, string> = {};
  for (const k of keys) {
    const hits = found.filter((f) => f.key.toLowerCase() === k.toLowerCase());
    if (hits.length !== 1) return {};
    out[k] = hits[0].label;
  }
  // The keys come in screen order: labels in another order would be another menu.
  const order = keys.map((k) => found.findIndex((f) => f.key.toLowerCase() === k.toLowerCase()));
  if (order.some((n, i) => i > 0 && n <= order[i - 1])) return {};
  return out;
}
