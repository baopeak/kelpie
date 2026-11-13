// Agents: permissions, which agent CLIs launchers show, per-agent arguments / permission mode /
// environment, the agent for AI actions, hibernation and keep-awake.
import { For, Show, createEffect, createSignal } from "solid-js";
import { openUrl } from "@tauri-apps/plugin-opener";
import { sessionLabel, store } from "../../lib/store";
import { browserAudit, browserPolicySet, policyGet, subPolicySet } from "../../lib/ipc";
import { SUB_MAX_LIMIT, SUB_SESSIONS_API, parseSubMax, setSubPolicy, subPolicy, toSubPolicy } from "../../lib/sub-sessions";
import { formatDateTime, msg, t } from "../../lib/i18n";
import { BROWSER_AGENT_API } from "../../lib/attention";
import { browserPolicy, refreshBrowserPolicy, setBrowserPolicy } from "../../lib/agent-browser";
import { MAX_SITES, addSite, toAuditEntries, toBrowserPolicy, type AuditEntry, type BrowserMode, type SiteError } from "../../lib/agent-browser-panes";
import { LoadBar } from "../load-bar";
import { CORE_AGENTS, agentPresets, kindInfo, loadAgentPresets } from "../../lib/agents";
import { Icon, KindGlyph } from "../icons";
import { Choice, SettingRow, Toggle } from "./setting-row";
import type { AgentProfile } from "../../lib/types";
import { confirmDialog } from "../../lib/confirm-dialog";

/** Permission flags each agent understands; "" keeps the agent's own default. */
const PERMISSIONS: Record<string, [string, string][]> = {
  claude: [["", msg("Default")], ["acceptEdits", msg("Accept edits")], ["plan", msg("Plan only")], ["bypassPermissions", msg("Bypass (no prompts)")]],
  codex: [["", msg("Default")], ["untrusted", msg("Untrusted")], ["on-request", msg("On request")], ["never", msg("Never ask")]],
};

const parseArgs = (v: string) => v.match(/"[^"]*"|\S+/g)?.map((a) => a.replace(/^"|"$/g, "")) ?? [];
const formatArgs = (a: string[]) => a.map((x) => (/\s/.test(x) ? `"${x}"` : x)).join(" ");

function parseEnv(text: string): Record<string, string> {
  const env: Record<string, string> = {};
  for (const line of text.split(/\r?\n/)) {
    const m = line.match(/^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=(.*)$/);
    if (m) env[m[1]] = m[2].trim();
  }
  return env;
}

const s = () => store.state.settings;
const profileOf = (kind: string): AgentProfile => s().profiles.find((p) => p.kind === kind) ?? { kind, label: kindInfo(kind).label, args: [] };

/** Arguments, permission mode and environment of one agent. */
function AgentFields(props: { kind: string }) {
  const p = () => profileOf(props.kind);
  const update = (patch: Partial<AgentProfile>) => store.setProfile(props.kind, patch);
  return (
    <div class="agent-card">
      <label class="field">
        <span>{t("Arguments")}</span>
        <input class="text-input" value={formatArgs(p().args)} placeholder={props.kind === "codex" ? t("e.g. --model gpt-5") : t("e.g. --model opus")}
          onChange={(e) => update({ args: parseArgs(e.currentTarget.value) })} />
      </label>
      <Show when={PERMISSIONS[props.kind]}>
        <label class="field">
          <span>{props.kind === "codex" ? t("Approval policy") : t("Permission mode")}</span>
          <select class="text-input" value={p().permission ?? ""} onChange={(e) => update({ permission: e.currentTarget.value || undefined })}>
            <For each={PERMISSIONS[props.kind]}>{([v, label]) => <option value={v}>{t(label)}</option>}</For>
          </select>
        </label>
      </Show>
      <label class="field">
        <span>{t("Environment")}</span>
        <textarea class="text-input" rows={2} placeholder={t("KEY=value, one per line (stored in Kelpie's settings file as plain text)")}
          value={Object.entries(p().env ?? {}).map(([k, v]) => `${k}=${v}`).join("\n")}
          onChange={(e) => update({ env: parseEnv(e.currentTarget.value) })} />
      </label>
    </div>
  );
}

/** Every known agent CLI: installed or not, shown in launchers or hidden, configurable. */
function AgentList() {
  const [open, setOpen] = createSignal<string | null>(null);
  const hidden = () => s().hiddenAgents ?? [];
  const toggle = (kind: string, show: boolean) => store.setSettings({ hiddenAgents: show ? hidden().filter((k) => k !== kind) : [...hidden(), kind] });
  const rows = () => {
    const list = agentPresets().filter((p) => !CORE_AGENTS.includes(p.id));
    // Installed first, then alphabetical.
    return list.slice().sort((a, b) => Number(b.installed) - Number(a.installed) || a.label.localeCompare(b.label));
  };
  return (
    <div class="agent-list">
      <LoadBar when={!rows().length} />
      <For each={rows()} fallback={<div class="muted">{t("Loading the agent list from the session daemon…")}</div>}>
        {(p) => (
          <div class={`agent-item${p.installed ? "" : " missing"}`}>
            <div class="agent-item-head">
              <KindGlyph kind={p.id} size={18} />
              <span class="agent-name">{p.label}</span>
              <span class="mono muted-inline">{p.bin}</span>
              <span style={{ flex: 1 }} />
              <Show when={p.installed} fallback={<span class="chip">{t("not installed")}</span>}>
                <button class="link-btn" onClick={() => setOpen(open() === p.id ? null : p.id)}>{open() === p.id ? t("Done") : t("Configure")}</button>
                <Toggle on={!hidden().includes(p.id)} onChange={(on) => toggle(p.id, on)} label={t("Show {name}", { name: p.label })} />
              </Show>
            </div>
            <Show when={open() === p.id}><AgentFields kind={p.id} /></Show>
          </div>
        )}
      </For>
      <button class="btn" style={{ "align-self": "flex-start" }} onClick={() => void loadAgentPresets()}>{t("Rescan")}</button>
    </div>
  );
}

/** The daemon's sub-session policy (it keeps it in policy.json, so it holds with the window closed). Every change is sent, then
 *  shown as the daemon answers. On a daemon below the API gate there is nothing to set: the note offers the restart. */
function SubSessionSettings() {
  const supported = () => store.daemonApi() >= SUB_SESSIONS_API;
  const [busy, setBusy] = createSignal(false);
  createEffect(() => {
    if (store.connected() && supported()) void policyGet().then((r) => setSubPolicy(toSubPolicy(r)), () => {});
  });
  const send = async (patch: { sub_sessions?: "off" | "ask" | "allow"; sub_max?: number }) => {
    if (busy()) return;
    setBusy(true);
    try {
      setSubPolicy(toSubPolicy(await subPolicySet(patch)));
    } catch (e) {
      store.notify(String(e).replace(/^Error: /, ""), undefined, 8000);
    } finally {
      setBusy(false);
    }
  };
  const mode = async (v: "off" | "ask" | "allow") => {
    if (v === "allow" && !(await confirmDialog(t("Let agents open sub-sessions without asking you? Each one is another agent that uses tokens and works in your folders, and one opened by an agent that skips permission checks does too.")))) return;
    await send({ sub_sessions: v });
  };
  const fixed = () => (subPolicy()?.sub_max ?? 0) > 0;
  return (
    <>
      <SettingRow id="sub-sessions" wide>
        <Show when={supported()} fallback={<Show when={store.connected() && store.daemonApi() > 0} fallback={<span class="muted">{t("Connect to the session daemon to change this.")}</span>}><DaemonTooOld /></Show>}>
          <div class="inline-fields" style={{ "align-items": "center" }}>
            <Choice value={subPolicy()?.sub_sessions ?? "ask"} options={[["off", t("Off")], ["ask", t("Ask")], ["allow", t("Allow")]]} onChange={(v) => void mode(v)} />
            <button class="link-btn" onClick={() => void openUrl("https://github.com/baopeak/kelpie/blob/master/docs/user-guide.md#sub-sessions")}>{t("How sub-sessions work")} <Icon name="external" size={11} /></button>
          </div>
        </Show>
      </SettingRow>
      <Show when={supported()}>
        <SettingRow id="sub-sessions-max">
          <div class="inline-fields" style={{ "align-items": "center" }}>
            <Choice value={fixed() ? "set" : "auto"} options={[["auto", t("Auto")], ["set", t("Set")]]} onChange={(v) => void send({ sub_max: v === "auto" ? 0 : subPolicy()?.sub_max || subPolicy()?.sub_auto_max || 2 })} />
            <Show when={fixed()} fallback={<span class="muted-inline">{t("{n} on this computer", { n: subPolicy()?.sub_auto_max ?? "…" })}</span>}>
              <input class="text-input" type="number" min="1" max={SUB_MAX_LIMIT} style={{ width: "80px" }} value={subPolicy()?.sub_max ?? 1}
                onChange={(e) => {
                  const n = parseSubMax(e.currentTarget.value);
                  if (n === null) e.currentTarget.value = String(subPolicy()?.sub_max ?? 1);
                  else void send({ sub_max: n });
                }} />
            </Show>
          </div>
        </SettingRow>
      </Show>
    </>
  );
}

const siteErrors: Record<SiteError, string> = {
  scheme: msg("Type the site with http:// or https:// in front, such as https://example.com."),
  invalid: msg("That is not a site address."),
  path: msg("Type only the site, without a path, such as https://example.com."),
  duplicate: msg("That site is already in the list."),
  full: msg("The list holds {n} sites at most."),
};

/** The daemon's browser policy (kept in policy.json, like the sub-session policy): what agents may do in browser panes, whether they may
 *  run scripts, the sites they may open, and what they did lately. Shown only by a daemon that has it. */
function BrowserAgentSettings() {
  const supported = () => store.daemonApi() >= BROWSER_AGENT_API;
  const [busy, setBusy] = createSignal(false);
  const [site, setSite] = createSignal("");
  const [siteError, setSiteError] = createSignal<string | null>(null);
  const [audit, setAudit] = createSignal<AuditEntry[] | null>(null);
  const loadAudit = () => void browserAudit(50).then((r) => setAudit(toAuditEntries(r)), () => setAudit([]));
  createEffect(() => {
    if (!store.connected() || !supported()) return;
    void refreshBrowserPolicy();
    loadAudit();
  });
  const send = async (patch: { browser_agents?: BrowserMode; browser_eval?: boolean; browser_sites?: string[] }) => {
    if (busy()) return false;
    setBusy(true);
    try {
      setBrowserPolicy(toBrowserPolicy(await browserPolicySet(patch)));
      return true;
    } catch (e) {
      store.notify(String(e).replace(/^Error: /, ""), undefined, 8000);
      return false;
    } finally {
      setBusy(false);
    }
  };
  const p = () => browserPolicy();
  const sites = () => p()?.sites ?? [];
  const add = async () => {
    const next = addSite(sites(), site());
    if ("error" in next) return setSiteError(t(siteErrors[next.error], { n: MAX_SITES }));
    setSiteError(null);
    if (await send({ browser_sites: next.sites })) setSite("");
  };
  const who = (id: string) => (store.sessions[id] ? sessionLabel(store.sessions[id]) : id.slice(0, 8));
  return (
    <Show when={supported()}>
      <SettingRow id="browser-agents" wide>
        <div class="inline-fields" style={{ "align-items": "center", "flex-wrap": "wrap" }}>
          <Choice value={p()?.mode ?? "off"} options={[["off", t("Off")], ["read", t("Read only")], ["act", t("Read and act")]]} disabled={busy()} onChange={(v) => void send({ browser_agents: v })} />
          <span class="muted-inline">{t("Applies to agents started after the change.")}</span>
        </div>
      </SettingRow>
      <SettingRow id="browser-agents-eval">
        <Toggle on={p()?.eval ?? false} disabled={busy() || p()?.mode !== "act"} onChange={(on) => void send({ browser_eval: on })} label={t("Agents may run scripts in pages")} />
      </SettingRow>
      <SettingRow id="browser-agents-sites" wide>
        <div class="browser-sites">
          <For each={sites()}>
            {(s) => (
              <span class="chip browser-site">
                <span class="mono">{s}</span>
                <button class="icon-btn" title={t("Remove")} disabled={busy()} onClick={() => void send({ browser_sites: sites().filter((x) => x !== s) })}><Icon name="close" size={10} /></button>
              </span>
            )}
          </For>
        </div>
        <form class="inline-fields" onSubmit={(e) => { e.preventDefault(); void add(); }}>
          <input class="text-input" style={{ width: "260px" }} value={site()} placeholder="https://example.com" spellcheck={false}
            onInput={(e) => { setSite(e.currentTarget.value); setSiteError(null); }} />
          <button class="btn" type="submit" disabled={busy() || !site().trim()}>{t("Add")}</button>
        </form>
        <Show when={siteError()}><div class="form-note error">{siteError()}</div></Show>
      </SettingRow>
      <SettingRow id="browser-agents-audit" wide>
        <div class="browser-audit">
          <For each={audit() ?? []} fallback={<div class="muted">{audit() ? t("No agent has acted in a browser pane yet.") : t("Loading…")}</div>}>
            {(e) => (
              <div class="browser-audit-row" data-ok={e.ok}>
                <span class="muted-inline">{formatDateTime(e.at)}</span>
                <span>{who(e.session)}</span>
                <span class="mono">{e.tool}</span>
                <span class="mono muted-inline">{e.host}</span>
                <span class="browser-audit-target">{e.target}</span>
                <Show when={!e.ok}><span class="form-note error">{e.error ? t("Not done: {error}", { error: e.error }) : t("Not done")}</span></Show>
              </div>
            )}
          </For>
        </div>
        <button class="btn" style={{ "align-self": "flex-start" }} onClick={loadAudit}>{t("Refresh")}</button>
      </SettingRow>
    </Show>
  );
}

/** A daemon that predates sub-sessions: the controls would do nothing, so they are not shown. */
function DaemonTooOld() {
  return (
    <div class="inline-fields" style={{ "align-items": "center" }}>
      <span class="muted">{t("The running session daemon is older and has no sub-sessions. Restart it (it keeps your sessions' conversations) to use them.")}</span>
      <Show when={store.connected()}><button class="btn" onClick={() => void store.switchDaemon()}>{t("Restart the session daemon")}</button></Show>
    </div>
  );
}

export function AgentSettings() {
  return (
    <>
      <SettingRow id="agent-permissions">
        <Choice value={s().agentPermissions ?? "manual"} options={[["manual", t("Manual")], ["yolo", t("Yolo")]]} onChange={async (v) => {
          if (v === "yolo" && !(await confirmDialog(t("Start agents with their skip-permission flags? They can then run commands and edit files without asking. A worktree is not a sandbox: the agent can still reach anything your account can.")))) return;
          store.setSettings({ agentPermissions: v });
        }} />
      </SettingRow>
      <For each={CORE_AGENTS}>
        {(kind) => (
          <SettingRow id={`agent-${kind}`} wide>
            <AgentFields kind={kind} />
          </SettingRow>
        )}
      </For>
      <SettingRow id="agent-list" wide>
        <AgentList />
      </SettingRow>
      <SubSessionSettings />
      <BrowserAgentSettings />
      <SettingRow id="ai-agent">
        <Choice value={s().aiAgent ?? "claude"} options={[["claude", "Claude Code"], ["codex", "Codex"]]} onChange={(v) => store.setSettings({ aiAgent: v })} />
      </SettingRow>
      <SettingRow id="auto-worktree">
        <Choice value={s().autoWorktree ?? "ask"} options={[["ask", t("Ask")], ["always", t("Always")], ["never", t("Never")]]} onChange={(v) => store.setSettings({ autoWorktree: v })} />
      </SettingRow>
      <SettingRow id="hibernate">
        <div class="inline-fields">
          <Toggle on={s().hibernate} onChange={(on) => store.setSettings({ hibernate: on })} />
          <input class="text-input" type="number" min="1" max="1440" style={{ width: "80px" }} value={s().hibernateMinutes ?? 30} disabled={!s().hibernate}
            onChange={(e) => store.setSettings({ hibernateMinutes: Math.min(1440, Math.max(1, +e.currentTarget.value || 30)) })} />
          <span class="muted-inline">{t("minutes idle")}</span>
        </div>
      </SettingRow>
      <SettingRow id="resume-sessions">
        <Choice value={s().resumeSessions ?? "shown"} options={[["shown", t("When shown")], ["all", t("All at once")], ["off", t("Off")]]} onChange={(v) => store.setSettings({ resumeSessions: v })} />
      </SettingRow>
      <SettingRow id="prioritize-visible">
        <Toggle on={s().prioritizeVisible ?? true} onChange={(on) => store.setSettings({ prioritizeVisible: on })} />
      </SettingRow>
      <SettingRow id="turn-checkpoints">
        <Toggle on={s().checkpoints ?? false} onChange={(on) => store.setSettings({ checkpoints: on })} />
      </SettingRow>
      <SettingRow id="agents-scripting">
        <Toggle on={s().agentsScripting ?? true} onChange={(on) => store.setSettings({ agentsScripting: on })} />
      </SettingRow>
      <SettingRow id="keep-awake">
        <Choice value={s().keepAwake ?? "off"} options={[["off", t("Off")], ["agents", t("While agents work")], ["always", t("Always")]]} onChange={(v) => store.setSettings({ keepAwake: v })} />
      </SettingRow>
    </>
  );
}
