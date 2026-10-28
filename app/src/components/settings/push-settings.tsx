// Settings → Notifications → "Off this computer": ntfy, Telegram and a webhook, kept by the session
// daemon (crates/kelpied/src/notify.rs) so they work with the window closed. The daemon holds the
// secrets in the OS credential store; this page only learns whether each one is set.
import { For, Show, createEffect, createSignal } from "solid-js";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { store } from "../../lib/store";
import { daemon } from "../../lib/ipc";
import { confirmDialog } from "../../lib/confirm-dialog";
import { errorText, t } from "../../lib/i18n";
import { SettingRow, Toggle } from "./setting-row";
import { emptyPushConfig, pushSummary, type PushConfig, type PushState } from "../../lib/push-config";
import "./push-settings.css";

type Channel = "ntfy" | "telegram" | "webhook";

/** A secret field: nothing is ever read back, only "set" or "not set". */
function SecretField(props: { set: boolean; placeholder: string; onSave: (value: string | null) => void }) {
  const [value, setValue] = createSignal("");
  const save = () => {
    if (!value().trim()) return;
    props.onSave(value().trim());
    setValue("");
  };
  return (
    <Show
      when={!props.set}
      fallback={
        <span class="inline-fields">
          <span class="muted-inline">{t("Saved in the system credential store")}</span>
          <button class="btn mini" onClick={() => props.onSave(null)}>{t("Remove")}</button>
        </span>
      }
    >
      <input class="text-input" type="password" autocomplete="off" placeholder={props.placeholder} value={value()} onInput={(e) => setValue(e.currentTarget.value)} onChange={save} />
    </Show>
  );
}

export default function PushSettings() {
  // A plain signal, not a resource: reloading after each change must not flash the section.
  const [state, setState] = createSignal<PushState | null>(null);
  const refetch = () => daemon<PushState>("notify.get").then(setState, () => {});
  createEffect(() => store.connected() && void refetch());
  const [error, setError] = createSignal<string | null>(null);
  const [topic, setTopic] = createSignal<string | null>(null);
  const [tested, setTested] = createSignal<Partial<Record<Channel, string>>>({});
  const cfg = (): PushConfig => state()?.config ?? emptyPushConfig();
  const secrets = () => state()?.secrets ?? { ntfy_topic: false, ntfy_token: false, telegram_token: false, webhook_url: false };

  // Saves go one after another, and an edit shows at once: two quick changes never undo each other.
  let chain: Promise<unknown> = Promise.resolve();
  let pending = 0;
  const save = (config?: PushConfig, secretChanges?: Record<string, string | null>) => {
    pending++;
    chain = chain.then(async () => {
      setError(null);
      try {
        await daemon("notify.set", { config, secrets: secretChanges });
      } catch (e) {
        setError(errorText(e));
      }
      if (--pending === 0) await refetch();
    });
    return chain;
  };
  const edit = (patch: (c: PushConfig) => void) => {
    const next = structuredClone(cfg());
    patch(next);
    const now = state();
    if (now) setState({ ...now, config: next });
    void save(next);
  };
  /** Screen text leaves the computer: ask before the first message can carry it. */
  const setScreen = async (on: boolean) => {
    if (on && !(await confirmDialog(t("The last lines of the terminal leave this computer, through the server you chose (ntfy.sh is a public service). Credentials Kelpie recognises are masked, but that is a best effort. Turn this on only for servers you trust.")))) return;
    edit((c) => (c.include_screen = on));
  };
  const test = async (ch: Channel) => {
    setTested({ ...tested(), [ch]: t("Sending…") });
    const done = (text: string) => setTested({ ...tested(), [ch]: text });
    await daemon("notify.test", { channel: ch }).then(() => done(t("Sent: check your phone.")), (e) => done(t("Failed: {error}", { error: errorText(e) })));
    void refetch();
  };
  const showTopic = async () => {
    const r = await daemon<{ ntfy_topic: string | null }>("notify.get", { reveal_topic: true }).catch(() => null);
    setTopic(r?.ntfy_topic ?? null);
  };
  const rotate = async () => {
    if (!(await confirmDialog(t("Make a new topic? Phones subscribed to the old one stop receiving notifications until you subscribe to the new one.")))) return;
    await daemon("notify.rotate_topic").then(() => showTopic(), (e) => setError(errorText(e)));
    void refetch();
  };
  const status = (ch: Channel) => state()?.status?.[ch];

  return (
    <Show when={state()}>
      <h3 class="settings-subhead">{t("Off this computer")}</h3>
      <div class="form-note">{t("Also tell your phone, with Kelpie's window closed. The session daemon sends these; nothing is sent until you switch a channel on.")}</div>
      <Show when={error()}><div class="form-note error">{error()}</div></Show>

      <SettingRow id="push-ntfy" wide>
        <div class="push-block">
          <Toggle on={cfg().ntfy.enabled} onChange={(on) => edit((c) => (c.ntfy.enabled = on))} />
          <Show when={cfg().ntfy.enabled}>
            <div class="inline-fields">
              <input class="text-input" placeholder="https://ntfy.sh" value={cfg().ntfy.server} title={t("ntfy server")} onChange={(e) => edit((c) => (c.ntfy.server = e.currentTarget.value.trim()))} />
              <button class="btn mini" onClick={() => void test("ntfy")}>{t("Send a test")}</button>
            </div>
            <div class="inline-fields">
              <Show when={topic()} fallback={<button class="btn mini" onClick={() => void showTopic()}>{t("Show topic")}</button>}>
                <span class="mono" data-ntfy-topic>{topic()}</span>
                <button class="btn mini" onClick={() => void writeText(topic()!).then(() => store.notify(t("Topic copied")))}>{t("Copy")}</button>
              </Show>
              <button class="btn mini" onClick={() => void rotate()}>{t("New topic")}</button>
            </div>
            <div class="form-note">{t("Subscribe to this topic in the ntfy app. It is 128 random bits that only you know; anyone who learns it can read your notifications, so make a new one if it was shared.")}</div>
            <SecretField set={secrets().ntfy_token} placeholder={t("Access token (optional)")} onSave={(v) => void save(undefined, { ntfy_token: v })} />
            <Show when={tested().ntfy ?? status("ntfy")?.error}><div class="form-note">{tested().ntfy ?? status("ntfy")?.error}</div></Show>
          </Show>
        </div>
      </SettingRow>

      <SettingRow id="push-telegram" wide>
        <div class="push-block">
          <Toggle on={cfg().telegram.enabled} onChange={(on) => edit((c) => (c.telegram.enabled = on))} />
          <Show when={cfg().telegram.enabled}>
            <SecretField set={secrets().telegram_token} placeholder={t("Bot token from @BotFather")} onSave={(v) => void save(undefined, { telegram_token: v })} />
            <div class="inline-fields">
              <input class="text-input" placeholder={t("Chat id")} value={cfg().telegram.chat_id} onChange={(e) => edit((c) => (c.telegram.chat_id = e.currentTarget.value.trim()))} />
              <button class="btn mini" disabled={!secrets().telegram_token || !cfg().telegram.chat_id} onClick={() => void test("telegram")}>{t("Send a test")}</button>
            </div>
            <Show when={tested().telegram ?? status("telegram")?.error}><div class="form-note">{tested().telegram ?? status("telegram")?.error}</div></Show>
          </Show>
        </div>
      </SettingRow>

      <SettingRow id="push-webhook" wide>
        <div class="push-block">
          <Toggle on={cfg().webhook.enabled} onChange={(on) => edit((c) => (c.webhook.enabled = on))} />
          <Show when={cfg().webhook.enabled}>
            <div class="inline-fields">
              <SecretField set={secrets().webhook_url} placeholder={t("https://example.com/hook")} onSave={(v) => void save(undefined, { webhook_url: v })} />
              <button class="btn mini" disabled={!secrets().webhook_url} onClick={() => void test("webhook")}>{t("Send a test")}</button>
            </div>
            <Show when={tested().webhook ?? status("webhook")?.error}><div class="form-note">{tested().webhook ?? status("webhook")?.error}</div></Show>
          </Show>
        </div>
      </SettingRow>

      <SettingRow id="push-events" wide>
        <div class="push-events">
          <For each={pushEventChoices()}>
            {([key, label]) => (
              <label class="check-label">
                <input type="checkbox" checked={cfg().events[key]} onChange={(e) => edit((c) => (c.events[key] = e.currentTarget.checked))} /> {label}
              </label>
            )}
          </For>
        </div>
      </SettingRow>

      <SettingRow id="push-filters" wide>
        <div class="push-block">
          <div class="inline-fields">
            <span class="muted-inline">{t("A finished turn must last at least")}</span>
            <input class="text-input" type="number" min="0" max="86400" style={{ width: "80px" }} value={cfg().min_turn_s}
              onChange={(e) => edit((c) => (c.min_turn_s = Math.max(0, Math.round(+e.currentTarget.value || 0))))} />
            <span class="muted-inline">{t("seconds")}</span>
          </div>
          <label class="check-label">
            <input type="checkbox" checked={cfg().away_only} onChange={(e) => edit((c) => (c.away_only = e.currentTarget.checked))} /> {t("Only while Kelpie's window is not in focus or the computer is idle")}
          </label>
        </div>
      </SettingRow>

      <SettingRow id="push-screen" wide>
        <div class="push-block">
          <Toggle on={cfg().include_screen} onChange={(on) => void setScreen(on)} />
          <Show when={cfg().include_screen}>
            <div class="form-note warn-note">{t("The last lines of the terminal leave this computer, through the server you chose (ntfy.sh is a public service). Credentials Kelpie recognises are masked, but that is a best effort. Turn this on only for servers you trust.")}</div>
          </Show>
        </div>
      </SettingRow>
      <SettingRow id="push-link" wide>
        <div class="push-block">
          <Toggle on={cfg().phone_link} onChange={(on) => edit((c) => (c.phone_link = on))} />
          <div class="form-note">{t("With phone access on, the link holds this computer's address on your network (no code, no token), and goes to the server you chose.")}</div>
        </div>
      </SettingRow>
      <div class="form-note">{pushSummary(cfg(), secrets())}</div>
      <Show when={state()?.dropped}><div class="form-note warn-note">{t("{n} messages were not sent: the server did not answer in time and they queued up.", { n: state()!.dropped! })}</div></Show>
    </Show>
  );
}

function pushEventChoices(): [keyof PushConfig["events"], string][] {
  return [
    ["needs_you", t("An agent needs you")],
    ["done", t("An agent finished")],
    ["failed", t("An agent failed")],
    ["automation", t("An automation ran")],
    ["approval", t("A run waits for your approval")],
    ["cost_cap", t("A session passed its cost cap")],
  ];
}
