// A reply that is still arriving: which part of its text is final, and how to show the rest.

const FENCE = /^\s*(```|~~~)/;
/** A trimmed delimiter row. Tested on every delta, so it must stay linear on long runs of spaces. */
const DELIMITER_ROW = /^\|?\s*:?-+:?\s*(?:\|\s*:?-+:?\s*)*\|?$/;

/** Where streamed text can be cut: after the last blank line outside a code fence. Blocks
 *  before it are final, so they are parsed once, not again on every delta. */
export function stableEnd(text: string): number {
  let end = 0;
  let fence = false;
  let i = 0;
  for (const line of text.split("\n")) {
    const next = i + line.length + 1;
    if (FENCE.test(line)) fence = !fence;
    else if (!fence && line.trim() === "" && next <= text.length) end = next;
    i = next;
  }
  return end;
}

/** A table row's cells as marked counts them: split at unescaped pipes, outer pipes not counted. */
function cellCount(row: string): number {
  const cells = row.replace(/\\[\s\S]/g, "").split("|");
  if (!cells[0].trim()) cells.shift();
  if (cells.length && !cells[cells.length - 1].trim()) cells.pop();
  return cells.length;
}

/** Whether the open block (the text after `stableEnd`) is a table whose delimiter row has
 *  arrived, so it can already render as one. Follows marked's rules: until the row has a cell for
 *  every column, and a `|` or `:` to tell it from a heading's underline, marked sees no table. */
export function openTable(tail: string): boolean {
  const [header, second] = tail.split("\n", 2);
  if (second === undefined || !header.includes("|") || FENCE.test(header)) return false;
  const delimiter = second.trim();
  return DELIMITER_ROW.test(delimiter) && /[|:]/.test(delimiter) && cellCount(header) === cellCount(delimiter);
}
