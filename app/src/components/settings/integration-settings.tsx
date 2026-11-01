// Settings → Integrations: Linear (API key), Jira (site + e-mail + API token, or a Server /
// Data Center personal access token) and GitLab (personal access tokens per host). Tokens go
// to the OS credential store; this page never shows them again.
import { For, Show, createResource, createSignal } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { store } from "../../lib/store";
import { t, errorText } from "../../lib/i18n";
import { LoadBar } from "../load-bar";
import { gitlabKey, gitlabUser, jiraKey, jiraMyself, linearViewer, secretClear, secretHas, secretSet } from "../../lib/integrations";
import { Icon } from "../icons";
import { SettingRow } from "./setting-row";

const integrations = () => store.state.settings.integrations ?? { linear: false, jira: null, gitlabHosts: [] };
const setIntegrations = (patch: Partial<ReturnType<typeof integrations>>) => store.setSettings({ integrations: { ...integrations(), ...patch } });

/** A token field: saves to the credential store, then checks it with `check`. */
function TokenField(props: { secretKey: string; placeholder: string; check: () => Promise<string>; onSaved?: () => void; onCleared?: () => void }) {
  const [value, setValue] = createSignal("");
  const [has, { refetch }] = createResource(() => props.secretKey, (k) => secretHas(k).catch(() => false));
  const [who, setWho] = createSignal<string | null>(null);
  const [error, setError] = createSignal<string | null>(null);
  const [busy, setBusy] = createSignal(false);
  const test = async () => {
    setBusy(true);
    setError(null);
    try {
      setWho(await props.check());
    } catch (e) {
      setWho(null);
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };
  const save = async () => {
    if (!value().trim()) return;
    try {
      await secretSet(props.secretKey, value());
      setValue("");
      await refetch();
      props.onSaved?.();
      await test();
    } catch (e) {
      setError(String(e));
    }
  };
  const clear = async () => {
    await secretClear(props.secretKey).catch((e) => setError(String(e)));
    setWho(null);
    await refetch();
    props.onCleared?.();
  };
  return (
    <div class="token-field">
      <LoadBar when={busy()} />
      <div class="inline-fields">
        <input class="text-input" type="password" autocomplete="off" placeholder={has() ? t("•••••••• (saved — paste a new one to replace)") : props.placeholder} value={value()}
          onInput={(e) => setValue(e.currentTarget.value)} onKeyDown={(e) => e.key === "Enter" && void save()} />
        <button class="btn primary" disabled={!value().trim()} onClick={() => void save()}>{t("Save")}</button>
        <Show when={has()}>
          <button class="btn" disabled={busy()} onClick={() => void test()}>{busy() ? t("Checking…") : t("Test")}</button>
          <button class="btn" onClick={() => void clear()}>{t("Remove")}</button>
        </Show>
      </div>
      <Show when={who()}><div class="form-note ok">{t("Connected as {name}", { name: who()! })}</div></Show>
      <Show when={error()}><div class="form-note error">{errorText(error())}</div></Show>
    </div>
  );
}

export function IntegrationSettings() {
  const [jiraUrl, setJiraUrl] = createSignal(integrations().jira?.url ?? "");
  const [jiraEmail, setJiraEmail] = createSignal(integrations().jira?.email ?? "");
  const [newHost, setNewHost] = createSignal("");
  const jiraSite = () => ({ url: jiraUrl().trim().replace(/\/+$/, ""), email: jiraEmail().trim() });
  const saveJiraSite = () => setIntegrations({ jira: jiraSite().url ? jiraSite() : null });
  const hosts = () => ["gitlab.com", ...integrations().gitlabHosts.filter((h) => h !== "gitlab.com")];
  return (
    <>
      <SettingRow id="integration-linear" wide>
        <TokenField secretKey="linear" placeholder={t("Linear personal API key (lin_api_…)")} check={linearViewer}
          onSaved={() => setIntegrations({ linear: true })} onCleared={() => setIntegrations({ linear: false })} />
        <button class="link-btn" onClick={() => void openUrl("https://linear.app/settings/account/security")}>{t("Create a key in Linear")} <Icon name="external" size={11} /></button>
      </SettingRow>
      <SettingRow id="integration-jira" wide>
        <div class="inline-fields">
          <input class="text-input" style={{ flex: 2 }} placeholder="https://your-team.atlassian.net" value={jiraUrl()} onInput={(e) => setJiraUrl(e.currentTarget.value)} onChange={saveJiraSite} />
          <input class="text-input" style={{ flex: 1.4 }} placeholder={t("E-mail (Cloud; empty for Server / Data Center)")} value={jiraEmail()} onInput={(e) => setJiraEmail(e.currentTarget.value)} onChange={saveJiraSite} />
        </div>
        <Show when={jiraSite().url}>
          <TokenField secretKey={jiraKey(jiraSite())} placeholder={jiraSite().email ? t("Atlassian API token") : t("Personal access token")} check={() => jiraMyself(jiraSite())} onSaved={saveJiraSite} />
        </Show>
        <button class="link-btn" onClick={() => void openUrl("https://id.atlassian.com/manage-profile/security/api-tokens")}>{t("Create an Atlassian API token")} <Icon name="external" size={11} /></button>
      </SettingRow>
      <SettingRow id="integration-gitlab" wide>
        <For each={hosts()}>
          {(host) => (
            <div class="gitlab-host">
              <div class="inline-fields" style={{ "align-items": "center" }}>
                <span class="mono" style={{ flex: 1 }}>{host}</span>
                <Show when={host !== "gitlab.com"}>
                  <button class="icon-btn" title={t("Forget this host")} onClick={() => setIntegrations({ gitlabHosts: integrations().gitlabHosts.filter((h) => h !== host) })}><Icon name="close" size={12} /></button>
                </Show>
              </div>
              <TokenField secretKey={gitlabKey(host)} placeholder={t("Personal access token for {host} (scope: api)", { host })} check={() => gitlabUser(host)} />
            </div>
          )}
        </For>
        <div class="inline-fields">
          <input class="text-input" placeholder={t("Self-hosted GitLab host, e.g. gitlab.example.com")} value={newHost()} onInput={(e) => setNewHost(e.currentTarget.value)} />
          <button class="btn" disabled={!newHost().trim()} onClick={() => {
            // https is the default; an explicit http:// (a local instance) is kept.
            const h = newHost().trim().replace(/^https:\/\//, "").replace(/\/+$/, "").toLowerCase();
            if (h && !integrations().gitlabHosts.includes(h)) setIntegrations({ gitlabHosts: [...integrations().gitlabHosts, h] });
            setNewHost("");
          }}>{t("Add host")}</button>
        </div>
      </SettingRow>
    </>
  );
}
