// The phone's words, untranslated (the harness needs no dictionary).
export function t(text: string, vars?: Record<string, string | number>): string {
  return vars ? text.replace(/\{(\w+)\}/g, (_, k: string) => String(vars[k] ?? "")) : text;
}
/** A word to translate later, as the page marks it. */
export const msg = (text: string) => text;
