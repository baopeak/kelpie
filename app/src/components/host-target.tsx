// Where a new pane runs: the project's own machine or a saved SSH host (the "Run on" row of the
// empty-pane launcher and the new session dialog), and the chips that mark panes on a host.
import { For, Show, createEffect, createSignal, on } from "solid-js";
import { store } from "../lib/store";
import { agentFolder, agentsAt, cachedCheck, checkHost, hostAgents, hostById, hostOf, hostTarget, isHostKeyError, osBadge, projectHosts, vault, type SshHost } from "../lib/ssh-hosts";
import { hostLabel, projectTarget } from "../lib/remote";
import { isAgentKind, projectAgents, kindInfo } from "../lib/agents";
import { openContextMenu, type MenuItem } from "./context-menu";
import { openHosts } from "./hosts-page";
import { t, errorText } from "../lib/i18n";
import type { Project, SessionTarget } from "../lib/types";

/** The machine chosen for a new pane: `hostId` null is the project's own (this computer for a
 *  local project); `path` overrides the folder on the host. */
export type RunOn = { hostId: string | null; path: string };

/** The choice a pane's launcher starts with: the host it was split from, if saved. */
export function initialRunOn(project: Project, target: SessionTarget | undefined): RunOn {
  const host = target && target.dest !== projectTarget(project)?.dest ? hostOf(target) : undefined;
  return host ? { hostId: host.id, path: target!.path } : { hostId: null, path: "" };
}

/** The target for a choice (undefined: the project's own). A host is checked first; a new host
 *  key opens its fingerprint check on the Hosts page instead of starting anything. An agent needs
 *  the project's folder on the host; the first one typed is linked to the project. */
export async function resolveRunOn(project: Project, run: RunOn, kind: string): Promise<SessionTarget | undefined | null> {
  const host = run.hostId ? hostById(run.hostId) : undefined;
  if (!host) return undefined;
  if (lacksFolder(project, run, kind)) return null;
  const agent = isAgentKind(kind);
  try {
    const target = await hostTarget(host, project, run.path);
    if (agent && !agentFolder(host, project)) store.linkHost(project.id, { hostId: host.id, path: target.path });
    return target;
  } catch (e) {
    if (isHostKeyError(e)) {
      store.notify(t("Check the fingerprint of {name} first", { name: host.label }));
      openHosts("hosts", host.id, { trust: true });
    } else store.notify(t("Could not connect to {list}", { list: host.label }) + ": " + errorText(String(e)));
    return null;
  }
}

/** True (and says so) when an agent would start on a host without the project's folder there. */
export function lacksFolder(project: Project, run: RunOn, kind: string): boolean {
  const host = run.hostId ? hostById(run.hostId) : undefined;
  if (!host || !isAgentKind(kind) || agentFolder(host, project, run.path)) return false;
  store.notify(t("Type the folder of {project} on {host} first: agents do not start in the login folder", { project: project.name, host: host.label }));
  return true;
}

/** Checks a host once in the background when it gets chosen, so the launcher offers the agents it
 *  has (and a new host key shows before anything starts). */
export function prefetchHost(hostId: string | null, onError?: (message: string) => void) {
  const h = hostId ? hostById(hostId) : undefined;
  if (h && !cachedCheck(h.id)) void checkHost(h).catch((e) => onError?.(isHostKeyError(e) ? t("New host key: check its fingerprint before connecting") : errorText(String(e))));
}

/** Agent kinds offered for a choice: the host's own, or the project's. */
export const runOnAgents = (project: Project, run: RunOn, hidden: string[]) =>
  run.hostId ? hostAgents(run.hostId).filter((k) => !hidden.includes(k)) : projectAgents(project, hidden);

/** Choices in launcher order: the project's own machine, then its linked hosts, then the chosen
 *  host when it is not linked. */
export function runOnChoices(project: Project, run: RunOn): (string | null)[] {
  const linked = projectHosts(project).map((l) => l.host.id);
  return [null, ...linked, ...(run.hostId && !linked.includes(run.hostId) ? [run.hostId] : [])];
}

/** Coloured OS badge ("Ub" on Ubuntu orange); a server glyph when the OS is not known yet. */
export function OsBadge(props: { os: string; size?: number }) {
  const b = () => osBadge(props.os);
  const size = () => props.size ?? 28;
  return (
    <span class="os-badge" title={b()?.label ?? ""} style={{ width: `${size()}px`, height: `${size()}px`, background: b()?.color ?? "var(--bg-elev-2)", "font-size": `${Math.round(size() * 0.4)}px` }}>
      {b()?.short ?? "▤" /* i18n-ignore */}
    </span>
  );
}

/** "● prod-api · /srv/app" on a pane or tab whose session runs on a host. */
export function HostChip(props: { target: SessionTarget; path?: boolean }) {
  const host = () => hostOf(props.target);
  return (
    <span class="host-chip" title={`${t("Runs on {host} over SSH", { host: host()?.label ?? props.target.dest })}${props.target.path ? ` · ${props.target.path}` : ""}`} style={{ "--host-color": host()?.color || "var(--accent)" }}>
      <span class="host-dot" />
      <span class="host-chip-name">{host()?.label ?? props.target.dest}</span>
      <Show when={props.path}><span class="host-chip-path">{props.target.path}</span></Show>
    </span>
  );
}

/** The "Run on" row: one chip per machine, "Other host…", then the folder for a host. */
export function RunOnPicker(props: { project: Project; value: RunOn; onChange: (v: RunOn) => void }) {
  const [error, setError] = createSignal<string | null>(null);
  const host = () => (props.value.hostId ? hostById(props.value.hostId) : undefined);
  const linked = () => projectHosts(props.project).find((l) => l.host.id === props.value.hostId);
  const pick = (hostId: string | null) => {
    setError(null);
    const link = projectHosts(props.project).find((l) => l.host.id === hostId);
    props.onChange({ hostId, path: link?.path ?? "" });
  };
  // Learn the chosen host's agents (and catch a new host key) before the user starts anything,
  // however it got chosen (a chip, Tab, the pane it was split from).
  createEffect(on(() => props.value.hostId, (id) => {
    setError(null);
    prefetchHost(id, setError);
  }));
  const others = (e: MouseEvent) => {
    const shown = new Set(runOnChoices(props.project, props.value));
    const items: MenuItem[] = vault().hosts.filter((h) => !shown.has(h.id)).map((h) => ({ label: h.group ? `${h.label} · ${h.group}` : h.label, run: () => pick(h.id) }));
    if (items.length) items.push(null);
    items.push({ label: t("Manage SSH hosts…"), run: () => openHosts() });
    openContextMenu(e, items);
  };
  const ownLabel = () => (props.project.remote ? hostOf(projectTarget(props.project)!)?.label ?? hostLabel(props.project) : t("This computer"));
  const label = (id: string | null) => (id ? hostById(id)?.label ?? "?" : ownLabel());
  const color = (id: string | null) => (id ? hostById(id)?.color : props.project.remote ? hostOf(projectTarget(props.project)!)?.color : "") || "";
  /** Where panes on the chosen host start, as far as known before connecting. */
  const folder = (h: SshHost) => props.value.path.trim() || linked()?.path || h.folder || cachedCheck(h.id)?.home || "";
  const busy = () => {
    const h = host();
    const path = h && folder(h);
    return h && path ? agentsAt({ dest: h.alias, path, tmux: h.tmux }, props.project.id) : [];
  };
  return (
    <div class="run-on" data-run-on>
      <div class="run-on-row">
        <span class="run-on-label">{t("Run on")}</span>
        <For each={runOnChoices(props.project, props.value)}>
          {(id) => (
            <button class={`toggle run-on-chip${props.value.hostId === id ? " on" : ""}`} data-run-on-host={id ?? "own"} onClick={() => pick(id)}>
              <span class="host-dot" style={{ background: color(id) || "var(--text-faint)" }} />
              {label(id)}
            </button>
          )}
        </For>
        <button class="toggle" data-action="other-host" onClick={others}>{t("Other host…")}</button>
      </div>
      <Show when={host()}>
        {(h) => (
          <div class="run-on-row">
            <span class="run-on-label">{t("Folder")}</span>
            <input class="text-input mono run-on-path" data-field="run-on-path" value={props.value.path} placeholder={linked()?.path || h().folder || t("the login folder")}
              onInput={(e) => props.onChange({ ...props.value, path: e.currentTarget.value })}
              // Digits are text here, not the launcher's "start kind n"; Tab moves focus as usual.
              onKeyDown={(e) => (/^\d$/.test(e.key) || e.key === "Tab") && e.stopPropagation()} />
            <Show when={!linked() || (props.value.path.trim() && props.value.path.trim() !== linked()!.path)}>
              <button class="toggle" data-action="link-host" title={t("Offer this host and folder first in this project's launchers")}
                onClick={() => store.linkHost(props.project.id, { hostId: h().id, path: props.value.path.trim() || linked()?.path || "" })}>
                {linked() ? t("Save folder") : t("Link to this project")}
              </button>
            </Show>
          </div>
        )}
      </Show>
      <Show when={host() && !agentFolder(host()!, props.project, props.value.path)}>
        <div class="form-note" data-run-on-no-folder>{t("Agents need this project's folder on the host; terminals start in {folder}.", { folder: host()!.folder || t("the login folder") })}</div>
      </Show>
      <Show when={busy().length}>
        <div class="form-note warn" data-run-on-busy>
          {t("{n} agent(s) of another project already run in this folder: {list}", { n: busy().length, list: busy().map((s) => `${kindInfo(s.kind).label} · ${store.state.projects.find((p) => p.id === s.project_id)?.name ?? "?"}`).join(", ") })}
        </div>
      </Show>
      <Show when={error()}><div class="form-note error">{error()}</div></Show>
    </div>
  );
}
