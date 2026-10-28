// Agents: permissions, which agent CLIs launchers show, per-agent arguments / permission mode /
// environment, the agent for AI actions, hibernation and keep-awake.
import { For, Show, createSignal } from "solid-js";
import { store } from "../../lib/store";
import { msg, t } from "../../lib/i18n";
import { CORE_AGENTS, agentPresets, kindInfo, loadAgentPresets } from "../../lib/agents";
import { KindGlyph } from "../icons";
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
