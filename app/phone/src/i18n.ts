// The phone's language, with the same contract as the desktop's (src/lib/i18n.ts): the English text is the
// key, `{name}` placeholders, a missing translation shows English. Only the strings this bundle uses are
// shipped (see vite-dictionary.ts). The chat model and adapters import the desktop module by path; the
// phone build points that import here.
import { createSignal } from "solid-js";
import vi from "virtual:phone-vi";
import { store } from "./storage";

export type Locale = "en" | "vi";
export type LocaleSetting = "auto" | Locale;

const KEY = "kelpie-lang";

export function systemLocale(): Locale {
  return (navigator.languages?.[0] ?? navigator.language ?? "en").toLowerCase().startsWith("vi") ? "vi" : "en";
}

const saved = (): LocaleSetting => {
  const v = store.get(KEY);
  return v === "en" || v === "vi" ? v : "auto";
};

const [setting, setSettingSignal] = createSignal<LocaleSetting>(saved());
export const localeSetting = setting;
export const locale = (): Locale => (setting() === "auto" ? systemLocale() : (setting() as Locale));

export function setLocale(next: LocaleSetting) {
  if (next === "auto") store.del(KEY);
  else store.set(KEY, next);
  setSettingSignal(next);
  document.documentElement.lang = locale();
}

export const intlLocale = () => (locale() === "vi" ? "vi-VN" : "en-US");

function fill(text: string, vars?: Record<string, string | number>) {
  return vars ? text.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : text;
}

/** The text in the phone's language. Reactive: re-runs when the language changes. */
export function t(text: string, vars?: Record<string, string | number>): string {
  return fill(locale() === "vi" ? (vi as Record<string, string>)[text] ?? text : text, vars);
}

export const msg = (text: string) => text;
