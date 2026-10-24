import { For, Show, createSignal } from "solid-js";
import { allLayouts } from "../lib/plugins";
import { store } from "../lib/store";
import { pickProjectFolder, chordOf, commands } from "../lib/commands";
import { chordLabel } from "../lib/platform";
import { Icon, KindGlyph } from "./icons";
import { openContextMenu, type MenuItem } from "./context-menu";
import { TemplateThumb } from "./layout-menu";
import { templateKinds } from "../lib/layout";
import { kindInfo, presetFor } from "../lib/agents";
import { hostLabel } from "../lib/remote";
import { hostById, projectHosts, vault } from "../lib/ssh-hosts";
import { RunOnPicker, resolveRunOn, runOnAgents, type RunOn } from "./host-target";
import type { Project, SessionKind } from "../lib/types";
import { t, msg } from "../lib/i18n";
import { openHosts } from "./hosts-page";
import { BrandWordmark, Kip, kipShown } from "./brand";

const kbd = (id: string) => {
  const c = commands.find((x) => x.id === id);
  const k = c && chordOf(c);
  return k ? chordLabel(k) : "";
};

export function Welcome() {
  return (
    <div class="empty">
      <div class="welcome">
        <Show when={kipShown()}><Kip mood="peek" size={150} /></Show>
        <h1><BrandWordmark height={34} /></h1>
        <div style={{ color: "var(--text-dim)" }}>
          {t("Run many coding agents across many projects. Sessions live in a background daemon, so switching projects or closing this window never stops them.")}
        </div>
        <div class="actions">
          <button class="btn primary" onClick={() => void pickProjectFolder()}>{t("Add project folder")}</button>
          <button class="btn" onClick={() => store.setOverlay("remote")}>{t("Add remote project (SSH)")}</button>
          <button class="btn" onClick={() => openHosts()}>{t("SSH hosts")}</button>
          <button class="btn" onClick={() => store.setOverlay("palette")}>{t("Command palette")} <span class="kbd">{kbd("palette")}</span></button>
        </div>
      </div>
    </div>
  );
}

const SUBTITLES: Record<string, string> = {
  claude: "Anthropic Claude Code CLI",
  codex: "OpenAI Codex CLI",
  omp: "oh-my-pi coding agent",
  shell: msg("Your default shell"),
};

/** Core agents, installed preset agents (Settings → Agents can hide them), then the shell; on a
 *  chosen host, the agents that host has. */
const choices = (project: Project, run: RunOn): { kind: SessionKind; sub: string }[] => {
  const host = run.hostId ? hostById(run.hostId)?.label : project.remote ? hostLabel(project) : undefined;
  return [...runOnAgents(project, run, store.state.settings.hiddenAgents), "shell"].map((kind) => ({ kind, sub: host ? t("on {host}", { host }) : SUBTITLES[kind] ? t(SUBTITLES[kind]) : t("{bin} CLI", { bin: presetFor(kind)?.bin ?? kind }) }));
};

/** Shown in a project with no tabs: pick the machine ("Run on"), then one click starts an agent. */
export function Launcher(props: { project: Project }) {
  const [runOn, setRunOn] = createSignal<RunOn>({ hostId: null, path: "" });
  // One start at a time: a double click must not start a second session.
  let starting = false;
  const start = async (kind: SessionKind, run = runOn()) => {
    if (starting) return;
    starting = true;
    const projectId = props.project.id;
    const target = await resolveRunOn(props.project, run, kind);
    if (target !== null) await store.newSession(projectId, kind, { target });
    starting = false;
  };
  /** The SSH card: a shell on a saved host (linked hosts first, in their linked folder). */
  const sshMenu = (e: MouseEvent) => {
    const links = projectHosts(props.project);
    if (!vault().hosts.length) return openHosts();
    const hosts = [...links.map((l) => l.host), ...vault().hosts.filter((h) => !links.some((l) => l.host.id === h.id))];
    const items: MenuItem[] = hosts.map((h) => {
      const path = links.find((l) => l.host.id === h.id)?.path ?? "";
      return { label: h.group ? `${h.label} · ${h.group}` : h.label, hint: path || undefined, run: () => void start("shell", { hostId: h.id, path }) };
    });
    items.push(null, { label: t("Manage SSH hosts…"), run: () => openHosts() });
    openContextMenu(e, items);
  };
  return (
    <div class="empty">
      <div class="welcome">
        <Show when={kipShown()}><Kip mood="peek" size={96} /></Show>
        <h1>{props.project.name}</h1>
        <div style={{ color: "var(--text-faint)", "font-family": "var(--font-mono)", "font-size": "12px" }}>{props.project.path}</div>
        <RunOnPicker project={props.project} value={runOn()} onChange={setRunOn} />
        <div class="launcher">
          <For each={choices(props.project, runOn())}>
            {(c) => (
              <div class="launch-card" onClick={() => void start(c.kind)}>
                <KindGlyph kind={c.kind} size={34} />
                <div>
                  <div>{kindInfo(c.kind).label}</div>
                  <div class="sub">{c.sub}</div>
                </div>
              </div>
            )}
          </For>
          <div class="launch-card" data-launch="ssh" onClick={sshMenu}>
            <span class="ssh-glyph"><Icon name="server" size={18} /></span>
            <div>
              <div>SSH</div>
              <div class="sub">{vault().hosts.length ? t("Terminal on a saved host") : t("Add an SSH host")}</div>
            </div>
          </div>
        </div>
        <Show when={allLayouts().length}>
          <div class="launcher-layouts">
            <div class="layout-menu-head">{t("Saved layouts")}</div>
            <For each={allLayouts()}>
              {(l) => (
                <button class="launch-card" onClick={() => void store.applyLayout(props.project.id, l.template)}>
                  <TemplateThumb template={l.template} size={34} />
                  <div style={{ "text-align": "left" }}>
                    <div>{l.name}</div>
                    <div class="sub">{templateKinds(l.template).map((k) => (k ? kindInfo(k).label : t("empty"))).join(" · ")}</div>
                  </div>
                </button>
              )}
            </For>
          </div>
        </Show>
        <div style={{ color: "var(--text-faint)", "font-size": "12px" }}>
          {t("New terminal")} <span class="kbd">{kbd("new.shell")}</span> · {t("Go to file")} <span class="kbd">{kbd("quickopen")}</span> · {t("Jump")} <span class="kbd">{kbd("jump")}</span>
        </div>
      </div>
    </div>
  );
}
