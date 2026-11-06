// Files open in an external editor: the host, the path, the last upload, auto-upload, and Stop (which stops watching
// and deletes the local copy).
import { For, Show } from "solid-js";
import { t } from "../../lib/i18n";
import { relTimeShort } from "../../lib/platform";
import { editSessions, setAutoUpload, stopEdit, uploadEdit } from "../../lib/remote-edit";

export function EditSessions(props: { label: (alias: string) => string }) {
  const stateText = (s: ReturnType<typeof editSessions>[number]) => {
    switch (s.state) {
      case "downloading":
        return t("Downloading…");
      case "uploading":
        return s.dirty ? t("Uploading… (a newer save follows)") : t("Uploading…");
      case "asking":
        return s.dirty ? t("Waiting for your answer (a newer save follows)") : t("Waiting for your answer");
      case "pending":
        return t("Changed: not uploaded yet");
      case "error":
        return s.error ?? t("Failed");
      default:
        return s.lastUpload ? t("uploaded {when}", { when: relTimeShort(s.lastUpload) }) : t("open in your editor");
    }
  };
  return (
    <Show when={editSessions().length}>
      <div class="edit-sessions" data-edit-sessions>
        <span class="section-title">{t("Open in an external editor")}</span>
        <For each={editSessions()}>
          {(s) => (
            <div class={`edit-session ${s.state}`} data-edit={s.id}>
              <span class="mono" title={s.remotePath}>{s.name}</span>
              <span class="muted-inline">{props.label(s.dest)}</span>
              <span class={`transfer-state ${s.state === "error" ? "failed" : ""}`}>{stateText(s)}</span>
              <span class="spacer" />
              <label class="check-inline" title={t("Upload on every save; off: ask each time")}>
                <input type="checkbox" checked={s.auto} onChange={(e) => setAutoUpload(s.id, e.currentTarget.checked)} /> {t("Upload on save")}
              </label>
              <Show when={!s.auto || s.state === "pending" || s.state === "error"}><button class="btn" onClick={() => void uploadEdit(s.id)}>{t("Upload now")}</button></Show>
              <button class="btn" data-action="stop-edit" onClick={() => void stopEdit(s.id)}>{t("Stop")}</button>
            </div>
          )}
        </For>
      </div>
    </Show>
  );
}
