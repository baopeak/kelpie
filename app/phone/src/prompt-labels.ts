// What each key of a menu prompt does, read from the screen the phone already shows: the daemon sends the keys only, and a card of bare
// "1" and "3" makes the person map digits to options by reading the terminal. The menu is read the way the daemon finds it
// (`MENU_ITEM` and `menu_labels` in crates/kelpied/src/choices.rs; keep the two in step): numbered lines among the last non-empty lines,
// a "1." starting the menu afresh, a box border or a marker before the number allowed. A label is given only when that menu ends the
// screen (a few hint lines below it at most) and its numbers are exactly the prompt's keys, in order. Anything else gives no labels at all,
// so a card never names an option from some other list on the screen.

/** How far up a menu may start, in non-empty lines from the bottom (the daemon's `MENU_LINES`). */
const MENU_LINES = 12;
/** Lines allowed under the last option: a wrapped label, a hint such as "Esc to cancel", a box's bottom edge. */
const FOOTER_LINES = 3;
const OPTION = /^\s*(?:[│|]\s*)?(?:[❯>›»▶●○◉◯*]\s*)?(\d)[.)]\s+(\S.*?)\s*[│|]?\s*$/;

/** Labels by key for `keys`, or an empty map when the screen does not show them plainly. */
export function promptLabels(rows: string[], keys: string[]): Record<string, string> {
  if (!keys.length) return {};
  const lines = rows.filter((r) => r.trim()).slice(-MENU_LINES);
  let items: { key: string; label: string }[] = [];
  let last = -1;
  lines.forEach((line, i) => {
    const m = OPTION.exec(line);
    if (!m) return;
    if (m[1] === "1") items = [];
    items.push({ key: m[1], label: m[2] });
    last = i;
  });
  if (!items.length || lines.length - 1 - last > FOOTER_LINES) return {};
  if (items.length !== keys.length || items.some((it, i) => it.key !== keys[i])) return {};
  return Object.fromEntries(items.map((it) => [it.key, it.label]));
}
