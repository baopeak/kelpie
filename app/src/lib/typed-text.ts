/** Pasted or prepared text minus control characters (tabs and line breaks stay). It may quote
 *  files, diffs, the clipboard or other sessions' output: an ESC left in would let `\x1b[201~`
 *  end the bracketed paste early and the rest would arrive as keystrokes. Same rule as the
 *  daemon's `typed_text`. */
export function typedText(text: string): string {
  // eslint-disable-next-line no-control-regex
  return text.replace(/\r\n?/g, "\n").replace(/[\x00-\x08\x0b-\x1f\x7f-\x9f]/g, "");
}

/** What goes to the program when a paste cannot go through the terminal view (it is detached for a
 *  moment): the cleaned text, in the paste markers when the program asked for them, so that line
 *  breaks stay part of the paste instead of becoming Enter keys. Without markers, line breaks are Enter. */
export function pasteInput(raw: string, bracketed: boolean): string {
  const body = typedText(raw).replace(/\n/g, "\r");
  return bracketed ? `\x1b[200~${body}\x1b[201~` : body;
}
