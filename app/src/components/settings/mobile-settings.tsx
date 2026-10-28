// Settings → Mobile: the daemon's web app for phones (crates/kelpied/src/mobile.rs). Off by default;
// pairing shows a one-time QR code and each phone must be approved here.
import { For, Show, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { store } from "../../lib/store";
import { daemon, onEvent } from "../../lib/ipc";
import { formatRelative, t, errorText } from "../../lib/i18n";
import { Icon } from "../icons";
import { SettingRow, Toggle } from "./setting-row";
import { confirmDialog } from "../../lib/confirm-dialog";

export interface MobileStatus {
  enabled: boolean;
  running: boolean;
  url: string | null;
  error: string | null;
  ip: string | null;
  port: number;
  tls: boolean;
  cert: string | null;
  key: string | null;
  ips: { ip: string; name: string }[];
  devices: { id: string; name: string; can_write: boolean; created_at: number; last_seen: number }[];
  pairing_expires_in: number | null;
  pending: { id: string; name: string }[];
}

export const mobileStatus = () => daemon<MobileStatus>("mobile.status");

export function MobileSettings() {
  const [status, { refetch }] = createResource(() => store.connected(), () => mobileStatus().catch(() => null));
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [qr, setQr] = createSignal<{ url: string; qr: string; until: number } | null>(null);
  const [now, setNow] = createSignal(Date.now());
  onMount(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    const un = onEvent<{ event: string }>("daemon-event", (m) => {
      if (m.event === "mobile.updated" || m.event === "mobile.pair_request") void refetch();
      if (m.event === "mobile.updated") setQr(null);
    });
    onCleanup(() => {
      clearInterval(timer);
      void un.then((f) => f());
    });
  });
  const run = async (fn: () => Promise<unknown>) => {
    setBusy(true);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
      void refetch();
    }
  };
  const enable = (patch: Record<string, unknown> = {}) => run(() => daemon("mobile.enable", patch));
  const s = () => status();
  return (
    <>
      <SettingRow id="mobile-access">
        <Toggle on={!!s()?.enabled} onChange={(on) => void (on ? enable() : run(() => daemon("mobile.disable")))} />
      </SettingRow>
      <Show when={error() || s()?.error}><div class="form-note error">{errorText(error() ?? s()?.error)}</div></Show>
      <Show when={s()?.enabled}>
        <SettingRow id="mobile-network" wide>
          <div class="inline-fields">
            <select class="text-input" value={s()!.ip ?? ""} disabled={busy()} onChange={(e) => void enable({ ip: e.currentTarget.value })}>
              <option value="">{t("Automatic")}</option>
              <For each={s()!.ips}>{(a) => <option value={a.ip}>{a.ip} ({a.name})</option>}</For>
              <Show when={s()!.ip && !s()!.ips.some((a) => a.ip === s()!.ip)}><option value={s()!.ip!}>{s()!.ip}</option></Show>
            </select>
            <input class="text-input" type="number" min="1024" max="65535" style={{ width: "100px" }} value={s()!.port} disabled={busy()}
              onChange={(e) => void enable({ port: +e.currentTarget.value })} />
            <Show when={s()!.url}><span class="mono mobile-url">{s()!.url}</span></Show>
          </div>
          <Show when={!s()!.tls}>
            <div class="form-note warn-note">{t("Plain HTTP: only on a network you trust (your home Wi-Fi, a Tailscale network). Anyone who can watch that network can read what the phone shows.")}</div>
          </Show>
        </SettingRow>
        <SettingRow id="mobile-tls" wide>
          <div class="inline-fields">
            <input class="text-input mono" placeholder={t("certificate (.crt / .pem)")} value={s()!.cert ?? ""} onChange={(e) => void enable({ cert: e.currentTarget.value })} />
            <input class="text-input mono" placeholder={t("private key (.key / .pem)")} value={s()!.key ?? ""} onChange={(e) => void enable({ key: e.currentTarget.value })} />
          </div>
        </SettingRow>
        <SettingRow id="mobile-pair" wide>
          <Show when={qr() && qr()!.until > now()} fallback={
            <button class="btn primary" disabled={!s()!.running || busy()} onClick={() => void run(async () => {
              const r = await daemon<{ url: string; qr: string; expires_in: number }>("mobile.pair_start");
              setQr({ ...r, until: Date.now() + r.expires_in * 1000 });
            })}><Icon name="plus" size={13} /> {t("Pair a phone")}</button>
          }>
            <div class="mobile-qr">
              <div class="mobile-qr-code" innerHTML={qr()!.qr} data-link={qr()!.url} />
              <div>
                <div>{t("Scan with the phone's camera, then approve it here.")}</div>
                <div class="muted-inline">{t("The code works once and expires in {n} s.", { n: Math.max(0, Math.round((qr()!.until - now()) / 1000)) })}</div>
                <div class="mono mobile-qr-url">{qr()!.url.split("#")[0]}</div>
                <button class="btn" onClick={() => setQr(null)}>{t("Cancel")}</button>
              </div>
            </div>
          </Show>
        </SettingRow>
        <SettingRow id="mobile-devices" wide>
          <div class="saved-layouts">
            <For each={s()!.devices} fallback={<div class="muted-inline">{t("No paired devices.")}</div>}>
              {(d) => (
                <div class="saved-layout" data-device={d.id}>
                  <Icon name="server" size={16} />
                  <div class="saved-layout-main">
                    <div>{d.name}</div>
                    <div class="muted-inline" style={{ "font-size": "11.5px" }}>{t("paired {when} · last seen {seen}", { when: formatRelative(d.created_at), seen: formatRelative(d.last_seen) })}</div>
                  </div>
                  <label class="check-label"><input type="checkbox" checked={d.can_write} onChange={(e) => void run(() => daemon("mobile.set_write", { id: d.id, can_write: e.currentTarget.checked }))} /> {t("May type")}</label>
                  <button class="btn" onClick={async () => (await confirmDialog(t("Unpair “{name}”? It is disconnected at once.", { name: d.name }))) && void run(() => daemon("mobile.revoke", { id: d.id }))}>{t("Unpair")}</button>
                </div>
              )}
            </For>
          </div>
        </SettingRow>
      </Show>
    </>
  );
}

/** Asks whether a phone that scanned the pairing code may connect (shown over everything). */
export function MobileApprovals() {
  const [requests, setRequests] = createSignal<{ id: string; name: string }[]>([]);
  onMount(() => {
    const un = onEvent<{ event: string; params: any }>("daemon-event", (m) => {
      if (m.event === "mobile.pair_request") setRequests((r) => [...r, { id: m.params.id, name: m.params.name }]);
      if (m.event === "mobile.updated") void mobileStatus().then((s) => setRequests((r) => r.filter((x) => s.pending.some((p) => p.id === x.id)))).catch(() => {});
    });
    onCleanup(() => void un.then((f) => f()));
  });
  const answer = (id: string, allow: boolean, canWrite = false) => {
    setRequests((r) => r.filter((x) => x.id !== id));
    void daemon("mobile.approve", { id, allow, can_write: canWrite }).catch((e) => store.notify(String(e)));
  };
  return (
    <For each={requests()}>
      {(r) => (
        <div class="overlay mobile-approve">
          <div class="modal" style={{ width: "460px" }}>
            <h2>{t("Pair “{name}”?", { name: r.name })}</h2>
            <div style={{ color: "var(--text-dim)" }}>{t("A device that scanned your pairing code wants to see your sessions. Only approve a device you are holding.")}</div>
            <div class="footer">
              <button class="btn" onClick={() => answer(r.id, false)}>{t("Deny")}</button>
              <span style={{ flex: 1 }} />
              <button class="btn" onClick={() => answer(r.id, true, false)}>{t("Allow reading")}</button>
              <button class="btn primary" onClick={() => answer(r.id, true, true)}>{t("Allow reading and typing")}</button>
            </div>
          </div>
        </div>
      )}
    </For>
  );
}
