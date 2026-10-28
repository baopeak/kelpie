// Settings → Voice: speech models (download with progress, cancel, delete, choose), the Whisper
// language, the microphone, and a field to try dictation in.
import { For, Show, createResource, createSignal } from "solid-js";
import { store } from "../../lib/store";
import { t, errorText } from "../../lib/i18n";
import {
  WHISPER_LANGUAGES, cancelDownload, clearDownload, deleteModel, downloadModel, micDevices, speechDownloads, speechModels, voiceAvailable, voiceSettings,
  type SpeechModel,
} from "../../lib/voice";
import { MicButton } from "../voice";
import { SettingRow } from "./setting-row";
import { confirmDialog } from "../../lib/confirm-dialog";

const mb = (n: number) => `${Math.round(n / 1e6)} MB`;
const setVoice = (patch: Partial<ReturnType<typeof voiceSettings>>) => store.setSettings({ voice: { ...voiceSettings(), ...patch } });

export function VoiceSettings() {
  const [available] = createResource(() => voiceAvailable().catch(() => false));
  const [models, { refetch }] = createResource(available, () => speechModels().catch(() => [] as SpeechModel[]));
  const [devices] = createResource(available, () => micDevices().catch(() => []));
  const [error, setError] = createSignal<string | null>(null);
  let tryField: HTMLTextAreaElement | undefined;

  const download = async (m: SpeechModel) => {
    setError(null);
    try {
      await downloadModel(m.id);
      // The first model downloaded becomes the one used.
      if (!voiceSettings().model || !models()?.some((x) => x.id === voiceSettings().model && x.installed)) setVoice({ model: m.id });
    } catch (e) {
      if (!String(e).includes("cancelled")) setError(t("{name}: {error}", { name: m.label, error: String(e) }));
    } finally {
      clearDownload(m.id);
      await refetch();
    }
  };
  const remove = async (m: SpeechModel) => {
    if (!(await confirmDialog(t("Delete the {name} model ({size})?", { name: m.label, size: mb(m.size) })))) return;
    await deleteModel(m.id).catch((e) => setError(String(e)));
    if (voiceSettings().model === m.id) setVoice({ model: models()?.find((x) => x.installed && x.id !== m.id)?.id ?? null });
    await refetch();
  };
  const current = () => models()?.find((m) => m.id === voiceSettings().model);

  return (
    <Show when={available() !== false} fallback={<div class="muted">{t("Voice dictation is not included in this build of Kelpie (the kelpie-stt helper is missing).")}</div>}>
      <SettingRow id="voice-models" wide>
        <div class="speech-models">
          <For each={models()}>
            {(m) => {
              const progress = () => speechDownloads()[m.id];
              return (
                <div class="speech-model" data-model={m.id}>
                  <label class="speech-model-main">
                    <input type="radio" name="speech-model" disabled={!m.installed} checked={voiceSettings().model === m.id && m.installed} onChange={() => setVoice({ model: m.id })} />
                    <span>
                      <div>{m.label}</div>
                      <div class="muted-inline" style={{ "font-size": "11.5px" }}>{m.languages === "vi" ? t("Vietnamese") : t("~100 languages")} · {mb(m.size)}</div>
                    </span>
                  </label>
                  <Show when={progress()} fallback={
                    <Show when={m.installed} fallback={<button class="btn" onClick={() => void download(m)}>{t("Download")}</button>}>
                      <span class="plugin-state on">{t("Downloaded")}</span>
                      <button class="btn" onClick={() => void remove(m)}>{t("Delete")}</button>
                    </Show>
                  }>
                    {(p) => (
                      <>
                        <span class="speech-progress"><span style={{ width: `${p().total ? (100 * p().done) / p().total : 0}%` }} /></span>
                        <span class="muted-inline">{p().total ? Math.floor((100 * p().done) / p().total) : 0}%</span>
                        <button class="btn" onClick={() => void cancelDownload(m.id)}>{t("Cancel")}</button>
                      </>
                    )}
                  </Show>
                </div>
              );
            }}
          </For>
          <Show when={error()}><div class="form-note error">{errorText(error())}</div></Show>
        </div>
      </SettingRow>
      <SettingRow id="voice-language">
        <select class="text-input" disabled={current()?.languages === "vi"} value={current()?.languages === "vi" ? "vi" : voiceSettings().language || "auto"}
          onChange={(e) => setVoice({ language: e.currentTarget.value })}>
          <Show when={current()?.languages === "vi"}><option value="vi">{t("Vietnamese")}</option></Show>
          <For each={WHISPER_LANGUAGES}>{([code, name]) => <option value={code}>{t(name)}</option>}</For>
        </select>
      </SettingRow>
      <SettingRow id="voice-mic">
        <select class="text-input" value={voiceSettings().device ?? ""} onChange={(e) => setVoice({ device: e.currentTarget.value || null })}>
          <option value="">{t("System default")}{devices()?.find((d) => d.default) ? ` (${devices()!.find((d) => d.default)!.name})` : ""}</option>
          <For each={devices()}>{(d) => <option value={d.name}>{d.name}</option>}</For>
        </select>
      </SettingRow>
      <SettingRow id="voice-try" wide>
        <div class="voice-try">
          <textarea ref={tryField} class="text-input" rows={2} placeholder={current()?.installed ? t("Click the microphone and speak…") : t("Download a model above first")} />
          <MicButton field={() => tryField} size={16} />
        </div>
      </SettingRow>
    </Show>
  );
}
