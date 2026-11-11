// File paths in a terminal's text as xterm links: `src/a.ts:42`, `D:\w\x.py`, a bare `name.py`. A path the terminal wrapped onto the
// next rows is still one link. What a link does is the caller's (the terminal opens it only on Ctrl/Cmd+click).
import type { IBufferLine, ILink, ILinkProvider, Terminal } from "@xterm/xterm";
import { findFileRefs, type FileRef } from "../chat/chat-links";

export interface Cell { chars: string; width: number }

/** At most this many rows make one wrapped line here: a link is looked for on every hover, so a huge wrapped line must stay cheap. */
const MAX_ROWS = 24;

/** The text of rows that make one line, and where each of its UTF-16 units is: the row (from the first) and the 0-based column. The
 *  second cell of a wide character has width 0 and adds nothing; an empty cell reads as a space. */
export function rowsText(rows: Cell[][]): { text: string; at: { row: number; col: number }[] } {
  let text = "";
  const at: { row: number; col: number }[] = [];
  rows.forEach((cells, row) =>
    cells.forEach((cell, col) => {
      if (cell.width === 0) return;
      const chars = cell.chars || " ";
      for (let i = 0; i < chars.length; i++) at.push({ row, col });
      text += chars;
    }),
  );
  return { text, at };
}

function cellsOf(line: IBufferLine, cols: number): Cell[] {
  const cells: Cell[] = [];
  for (let x = 0; x < cols; x++) {
    const cell = line.getCell(x);
    cells.push(cell ? { chars: cell.getChars(), width: cell.getWidth() } : { chars: "", width: 1 });
  }
  return cells;
}

/** The file links on buffer row `y` (1-based, as xterm asks), found in the whole wrapped line it belongs to; both ends 1-based and
 *  inclusive. */
export function fileLinksAt(rows: Cell[][], first: number, y: number): { ref: FileRef; text: string; start: { x: number; y: number }; end: { x: number; y: number } }[] {
  const { text, at } = rowsText(rows);
  return findFileRefs(text)
    .map((r) => {
      const a = at[r.start];
      const b = at[r.end - 1];
      return { ref: { path: r.path, ...(r.line ? { line: r.line } : {}) }, text: r.text, start: { x: a.col + 1, y: first + a.row }, end: { x: b.col + 1, y: first + b.row } };
    })
    .filter((l) => l.start.y <= y && l.end.y >= y);
}

/** An xterm link provider for file paths: `activate` gets each click (the modifier check is the caller's), `hover` the link under the
 *  pointer and then null. */
export function fileLinkProvider(term: Terminal, on: { activate: (e: MouseEvent, ref: FileRef) => void; hover: (ref: FileRef | null) => void }): ILinkProvider {
  return {
    provideLinks(y, callback) {
      const buffer = term.buffer.active;
      let first = y - 1;
      while (first > 0 && y - 1 - first < MAX_ROWS && buffer.getLine(first)?.isWrapped) first--;
      let last = y - 1;
      while (last - first < MAX_ROWS && buffer.getLine(last + 1)?.isWrapped) last++;
      const rows: Cell[][] = [];
      for (let r = first; r <= last; r++) {
        const line = buffer.getLine(r);
        if (line) rows.push(cellsOf(line, term.cols));
      }
      const links: ILink[] = fileLinksAt(rows, first + 1, y).map((l) => ({
        range: { start: l.start, end: l.end },
        text: l.text,
        decorations: { pointerCursor: true, underline: true },
        activate: (e) => on.activate(e, l.ref),
        hover: () => on.hover(l.ref),
        leave: () => on.hover(null),
      }));
      callback(links.length ? links : undefined);
    },
  };
}
