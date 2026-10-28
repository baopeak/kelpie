// The update offer: release notes and when to install. Sessions keep running either way.
import { Show, createResource, createSignal } from "solid-js";
import { store } from "../lib/store";
import { t } from "../lib/i18n";
import { daemon } from "../lib/ipc";
import { isMac } from "../lib/platform";
import { available, daemonSurvivesUpdate, installUpdate, installWhenIdle, progress, skipUpdate, whenIdle } from "../lib/updater";

export default function UpdateDialog() {
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  // A daemon started from the install folder (Kelpie 0.1.x, or a copy that could not be made)
  // has to stop for this one update.
  const [survives] = createResource(() => daemon<{ exe?: string }>("daemon.info").then((i) => daemonSurvivesUpdate(i.exe, isMac)).catch(() => true));
  const close = () => store.setOverlay(null);
  const install = async () => {
    setBusy(true);
    setError(null);
    try {
      await installUpdate(); // Windows: the installer takes over and the app exits
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };
  const percent = () => {
    const p = progress();
    return p?.downloaded && p.total ? Math.round((p.downloaded / p.total) * 100) : null;
  };
  return (
    <div class="overlay" onPointerDown={(e) => e.target === e.currentTarget && !busy() && close()}>
      <div class="modal update-dialog" onKeyDown={(e) => e.key === "Escape" && !busy() && close()}>
        <Show when={available()} fallback={<p>{t("Kelpie is up to date")}</p>}>
          {(u) => (
            <>
              <h2>{t("Kelpie {version} is available", { version: u().version })}</h2>
              <span class="muted-inline">{t("You have {version}", { version: u().current_version })}</span>
              <Show when={u().notes}><pre class="update-notes">{u().notes}</pre></Show>
              <Show
                when={survives() !== false}
                fallback={<p class="update-warning">{t("This update has to stop running sessions once: the session daemon still runs from the install folder. Agents that can resume continue their conversation; from the next update on, sessions keep running.")}</p>}
              >
                <p class="muted-inline">{t("Kelpie restarts to install it. Your sessions keep running meanwhile and are there when it reopens.")}</p>
              </Show>
              <Show when={busy()}>
                <p class="muted-inline">{percent() != null ? t("Downloading… {percent}%", { percent: percent()! }) : t("Preparing the update…")}</p>
              </Show>
              <Show when={whenIdle()}><p class="muted-inline">{t("Installs as soon as no session is working.")}</p></Show>
              <Show when={error()}><p class="update-error">{error()}</p></Show>
              <div class="footer">
                <button class="btn" disabled={busy()} onClick={() => { skipUpdate(); close(); }}>{t("Skip this version")}</button>
                <button class="btn" disabled={busy() || whenIdle()} onClick={() => { installWhenIdle(); close(); }}>{t("Install when idle")}</button>
                <button class="btn primary" disabled={busy()} onClick={() => void install()}>{t("Install now")}</button>
              </div>
            </>
          )}
        </Show>
      </div>
    </div>
  );
}
