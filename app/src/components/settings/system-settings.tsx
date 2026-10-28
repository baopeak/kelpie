// Session Daemon and About sections.
import { Show, createResource } from "solid-js";
import { getVersion } from "@tauri-apps/api/app";
import { store } from "../../lib/store";
import { t } from "../../lib/i18n";
import { daemon, platformInfo, revealEntry } from "../../lib/ipc";
import { SettingRow, Toggle } from "./setting-row";
import { BrandMark, BrandWordmark, Kip } from "../brand";
import { confirmDialog } from "../../lib/confirm-dialog";
import { checkForUpdate } from "../../lib/updater";

interface DaemonInfo {
  version: string;
  protocol: number;
  api?: number;
  pid: number;
  data_dir: string;
  endpoint: string;
  sessions: number;
}

const [platform] = createResource(() => platformInfo().catch(() => null));

export function DaemonSettings() {
  const [info, { refetch }] = createResource(() => store.connected(), () => daemon<DaemonInfo>("daemon.info").catch(() => null));
  const stopAll = async () => {
    if (!(await confirmDialog(t("Stop the session daemon and every running session?")))) return;
    await daemon("daemon.shutdown", { kill_sessions: true }).catch(() => {});
    setTimeout(() => void refetch(), 1500);
  };
  return (
    <>
      <SettingRow id="daemon-info">
        <Show when={info()} fallback={<span class="muted-inline">{t("Not connected")}</span>}>
          {(d) => (
            <div class="kv">
              <span>{t("Version")}</span><span>{d().version}</span>
              <span>{t("Process")}</span><span>{d().sessions === 1 ? t("pid {pid} · {n} session", { pid: d().pid, n: d().sessions }) : t("pid {pid} · {n} sessions", { pid: d().pid, n: d().sessions })}</span>
              <span>{t("Endpoint")}</span><span class="mono">{d().endpoint}</span>
            </div>
          )}
        </Show>
      </SettingRow>
      <SettingRow id="daemon-data">
        <div class="kv">
          <span>{t("Folder")}</span><span class="mono">{info()?.data_dir ?? platform()?.data_dir ?? ""}</span>
        </div>
        <div class="inline-fields">
          <button class="btn" onClick={() => void revealEntry(info()?.data_dir ?? platform()!.data_dir, "").catch((e) => store.notify(String(e)))}>{t("Open folder")}</button>
          <button class="btn" onClick={() => void revealEntry(info()?.data_dir ?? platform()!.data_dir, "logs/kelpied.log").catch((e) => store.notify(String(e)))}>{t("Show log file")}</button>
        </div>
      </SettingRow>
      <SettingRow id="daemon-stop">
        <button class="btn danger" onClick={() => void stopAll()}>{t("Stop daemon & all sessions")}</button>
      </SettingRow>
    </>
  );
}

export function AboutSettings() {
  const [version] = createResource(() => getVersion().catch(() => "?"));
  const checkNow = async () => {
    try {
      const found = await checkForUpdate(true);
      if (found) store.setOverlay("update");
      else store.notify(t("Kelpie is up to date"));
    } catch (e) {
      store.notify(t("Could not check for updates: {error}", { error: String(e) }));
    }
  };
  return (
    <>
    <SettingRow id="update-checks">
      <div style={{ display: "flex", "align-items": "center", gap: "10px" }}>
        <Toggle on={store.state.settings.updateChecks ?? true} onChange={(on) => store.setSettings({ updateChecks: on })} />
        <button class="btn" onClick={() => void checkNow()}>{t("Check now")}</button>
      </div>
    </SettingRow>
    <SettingRow id="version">
      <div style={{ display: "flex", "align-items": "center", gap: "12px", "margin-bottom": "12px" }}>
        <Kip mood="peek" size={72} />
        <div style={{ display: "flex", "align-items": "center", gap: "8px" }}>
          <BrandMark />
          <BrandWordmark height={18} />
        </div>
      </div>
      <div class="kv">
        <span>Kelpie</span><span>{version()}</span>
        <span>{t("Platform")}</span><span>{platform()?.os ?? ""}</span>
        <span>{t("Stack")}</span><span>Tauri 2 · SolidJS · xterm.js · Rust session daemon</span> {/* i18n-ignore: product names */}
      </div>
      <div class="muted-inline" style={{ "font-size": "11.5px", "margin-top": "10px" }}>
        {t("Agent names and logos are trademarks of their owners, shown only to identify the tools Kelpie launches. Kelpie is not affiliated with or endorsed by them.")}
      </div>
    </SettingRow>
    </>
  );
}
