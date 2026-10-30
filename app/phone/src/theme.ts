// Light and dark, following the phone unless the person picks one.
import { createSignal } from "solid-js";
import { store } from "./storage";

export type ThemeSetting = "auto" | "light" | "dark";

const KEY = "kelpie-theme";
const initial = (): ThemeSetting => {
  const v = store.get(KEY);
  return v === "light" || v === "dark" ? v : "auto";
};

const [theme, setThemeSignal] = createSignal<ThemeSetting>(initial());
export { theme };

export function applyTheme() {
  const t = theme();
  const root = document.documentElement;
  if (t === "auto") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", t);
  const dark = t === "dark" || (t === "auto" && matchMedia("(prefers-color-scheme: dark)").matches);
  document.querySelector('meta[name="theme-color"]')?.setAttribute("content", dark ? "#10131c" : "#f6f6f3");
}

export function setTheme(next: ThemeSetting) {
  if (next === "auto") store.del(KEY);
  else store.set(KEY, next);
  setThemeSignal(next);
  applyTheme();
}
