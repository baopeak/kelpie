// Settings → Layouts: saved pane arrangements (rename, delete, open in the current project).
import { For, Show } from "solid-js";
import { store } from "../../lib/store";
import { t } from "../../lib/i18n";
import { templateKinds } from "../../lib/layout";
import { kindInfo } from "../../lib/agents";
import { TemplateThumb } from "../layout-menu";
import { Icon } from "../icons";
import { SettingRow } from "./setting-row";
import { confirmDialog } from "../../lib/confirm-dialog";

export function LayoutSettings() {
  const layouts = () => store.state.settings.layouts ?? [];
  return (
    <SettingRow id="saved-layouts" wide>
      <Show when={layouts().length} fallback={<div class="muted-inline">{t("No saved layouts yet. Arrange a tab, then Layout menu → Save this layout.")}</div>}>
        <div class="saved-layouts">
          <For each={layouts()}>
            {(l) => (
              <div class="saved-layout">
                <TemplateThumb template={l.template} size={44} />
                <div class="saved-layout-main">
                  <input class="text-input" value={l.name} onChange={(e) => store.renameLayout(l.id, e.currentTarget.value)} />
                  <div class="muted-inline" style={{ "font-size": "11.5px" }}>{templateKinds(l.template).map((k) => (k ? kindInfo(k).label : t("empty"))).join(" · ")}</div>
                </div>
                <button class="btn" disabled={!store.activeProject()} title={t("Open in a new tab of the current project")}
                  onClick={() => { const p = store.activeProject(); if (p) { store.setOverlay(null); void store.applyLayout(p.id, l.template); } }}>{t("Open")}</button>
                <button class="icon-btn" title={t("Delete layout")} onClick={async () => (await confirmDialog(t("Delete layout “{name}”?", { name: l.name }))) && store.deleteLayout(l.id)}><Icon name="close" size={13} /></button>
              </div>
            )}
          </For>
        </div>
      </Show>
    </SettingRow>
  );
}
