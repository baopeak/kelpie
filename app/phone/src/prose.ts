// The little Markdown an agent's prose leans on, read for the phone: lists as lists, **bold** and `code`. The result is text and a kind
// for each piece; the view puts each piece in an element as a text node, so nothing an agent writes ever becomes markup.

export type Inline = { kind: "text" | "bold" | "code"; text: string };
/** A list keeps the agent's own numbering: `start` is its first number (1 for bullets), and a number that does not follow starts a new list. */
export type ProseBlock = { kind: "para"; text: string } | { kind: "list"; ordered: boolean; start: number; items: string[] };

const BULLET = /^\s{0,3}[-*•]\s+(.*)$/;
const NUMBERED = /^\s{0,3}(\d{1,3})[.)]\s+(.*)$/;

/** Paragraphs and lists: a run of "- " (or "1. ") lines is one list; everything else stays as written, line breaks kept. */
export function proseBlocks(text: string): ProseBlock[] {
  const out: ProseBlock[] = [];
  let para: string[] = [];
  const flush = () => {
    // Blank lines around a list are its spacing, not text of their own.
    while (para.length && !para[para.length - 1].trim()) para.pop();
    while (para.length && !para[0].trim()) para.shift();
    if (para.length) out.push({ kind: "para", text: para.join("\n") });
    para = [];
  };
  for (const line of text.split("\n")) {
    const b = BULLET.exec(line);
    const n = b ? null : NUMBERED.exec(line);
    const item = b?.[1] ?? n?.[2];
    if (item !== undefined) {
      const ordered = !b;
      const number = n ? Number(n[1]) : 1;
      const last = out[out.length - 1];
      const follows = last?.kind === "list" && last.ordered === ordered && (!ordered || last.start + last.items.length === number);
      if (!para.length && follows) (last as { items: string[] }).items.push(item);
      else {
        flush();
        out.push({ kind: "list", ordered, start: number, items: [item] });
      }
    } else if (!line.trim() && !para.length) {
      // A blank line between list items or before a paragraph: spacing only.
    } else para.push(line);
  }
  flush();
  return out;
}

/** `code` and **bold** in a line of prose. An unclosed mark is left as the characters it is. */
export function inlineParts(text: string): Inline[] {
  const out: Inline[] = [];
  const re = /`([^`\n]+)`|\*\*([^*\n]+?)\*\*/g;
  let at = 0;
  for (const m of text.matchAll(re)) {
    if (m.index! > at) out.push({ kind: "text", text: text.slice(at, m.index) });
    out.push(m[1] !== undefined ? { kind: "code", text: m[1] } : { kind: "bold", text: m[2] });
    at = m.index! + m[0].length;
  }
  if (at < text.length) out.push({ kind: "text", text: text.slice(at) });
  return out;
}
