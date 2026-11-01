import { render } from "solid-js/web";
import { createEffect } from "solid-js";
import { App } from "./app";
import { locale } from "./i18n";
import { watchInstall } from "./install-guide";
import { needsCount } from "./inbox-order";
import { sessions, startApp } from "./phone-state";
import { applyTheme } from "./theme";
import "./phone.css";

applyTheme();
watchInstall();
matchMedia("(prefers-color-scheme: dark)").addEventListener("change", applyTheme);

render(() => {
  // The language follows the setting; the page title and the app icon badge say how many sessions need you.
  createEffect(() => {
    document.documentElement.lang = locale();
  });
  createEffect(() => {
    const n = needsCount(sessions());
    document.title = n ? `(${n}) Kelpie` : "Kelpie";
    const nav = navigator as Navigator & { setAppBadge?: (n?: number) => Promise<void>; clearAppBadge?: () => Promise<void> };
    void (n ? nav.setAppBadge?.(n) : nav.clearAppBadge?.())?.catch(() => {});
  });
  return <App />;
}, document.getElementById("root")!);

startApp();

// The app shell is kept for offline starts, on secure origins only (HTTPS, Tailscale's HTTPS, localhost):
// browsers refuse a service worker on plain http, where the page simply loads from the computer each time.
if (window.isSecureContext && "serviceWorker" in navigator) {
  navigator.serviceWorker.register("/sw.js", { scope: "/" }).catch(() => {});
}
