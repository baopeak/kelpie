/** A button title as hover-hints.tsx shows it: "Split right" → label only; "Compose (Ctrl+Shift+I)"
 *  (a title with commandHint() appended) → label + keys. */
export function splitHint(text: string): { label: string; keys: string } {
  const m = /^(.*\S)\s+\(([^()]+)\)$/.exec(text);
  return m ? { label: m[1], keys: m[2] } : { label: text, keys: "" };
}
