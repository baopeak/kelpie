// Settings → Agent setup: the MCP servers, instruction files, skills and registry agents of the
// coding agents on this computer. Loaded lazily; everything is read once when the page opens
// (Refresh reads again) and nothing runs in the background. Every change goes through the same
// panel: preview (the diff, secrets masked) → backup → write, with the agent's file checked
// afterwards and put back at once if it no longer parses (app/src-tauri/src/agent_config.rs).
import { For, Show, createSignal, onMount, type JSX } from "solid-js";
import { store } from "../lib/store";
import { t, errorText, formatDateTime } from "../lib/i18n";
import { presetFor } from "../lib/agents";
import { syncPluginAgents } from "../lib/plugins";
import {
  SETUP_AGENTS, agentLabel, agentRegistryFetch, agentSetupApply, agentSetupBackups, agentSetupOverview, agentSetupPreview, agentSetupReveal,
  commandText, copyNeedsConfirm, quoteArg, registryProblem, warningText, copyTargets, diffCounts, diffLines, envOptIn, foldUnchanged, maskedEnv, provenance, registryCommandLine, savedFromRegistry, sizeText, splitArgs,
  type BackupInfo, type Edit, type FileDiff, type Untrusted, type UntrustedValue, type InstructionFile, type McpFile, type McpView, type Overview, type Preview, type RegistryAgent, type Scope, type SkillItem, type Target,
} from "../lib/agent-setup";
import { Toggle } from "./settings/setting-row";
import { Icon } from "./icons";
import "./agent-setup.css";
import { LoadBar } from "./load-bar";

type Tab = "mcp" | "instructions" | "skills" | "find";

interface Pending {
  edit: Edit;
  preview: Preview | null;
  busy: boolean;
  error: string | null;
  confirmed: boolean;
  /** What the change is, in a few words (for the panel's title). */
  title: string;
}

const scopeLabel = (s: Scope) => (s === "project" ? t("project · untrusted") : t("user"));
const tgtKey = (x: Target) => `${x.agent}:${x.scope}`;

/** The scope chip: colour and text, so it never rests on colour alone. */
function ScopeChip(props: { scope: Scope }) {
  return <span class={`as-chip ${provenance(props.scope).tag}`} data-scope={props.scope}>{provenance(props.scope).untrusted ? "▲ " : "● "}{scopeLabel(props.scope)}</span>;
}

/** One file's change as a diff; secrets were masked before they reached the page. */
function DiffBlock(props: { file: FileDiff }) {
  const lines = () => foldUnchanged(diffLines(props.file.before, props.file.after));
  const counts = () => diffCounts(lines());
  return (
    <div class="as-diff" data-diff-file={props.file.path}>
      <div class="as-diff-head">
        <code>{props.file.path}</code>
        <Show when={props.file.real_path}><span class="as-tag" title={props.file.real_path!} data-real-path>{t("is a link to {path}", { path: props.file.real_path! })}</span></Show>
        <Show when={props.file.created}><span class="as-tag">{t("new file")}</span></Show>
        <Show when={props.file.removed}><span class="as-tag">{t("file removed")}</span></Show>
        <span class="as-add">+{counts().added}</span> <span class="as-del">−{counts().removed}</span>
      </div>
      <pre class="as-lines">
        <Show when={props.file.skipped_before > 0}><div class="as-line fold">{t("{n} earlier lines unchanged", { n: props.file.skipped_before })}</div></Show>
        <For each={lines()}>
          {(l) => (l.op === "…"
            ? <div class="as-line fold">{t("{n} lines unchanged", { n: l.text })}</div>
            : <div class={`as-line ${l.op === "+" ? "add" : l.op === "-" ? "del" : ""}`}>{l.op}{" "}{l.text}</div>)}
        </For>
        <Show when={props.file.skipped_after > 0}><div class="as-line fold">{t("{n} later lines unchanged", { n: props.file.skipped_after })}</div></Show>
      </pre>
      <Show when={props.file.truncated}><div class="muted-inline">{t("Only the first lines of a very large change are shown.")}</div></Show>
    </div>
  );
}

/** One value of a project server: shown as the repository wrote it, except that something reading
 *  the user's own variables is named, never expanded. */
function ValueRow(props: { v: UntrustedValue }) {
  return (
    <div class="as-uv" data-uv={props.v.key}>
      <code>{props.v.key}</code>
      <span>{props.v.reference ? (props.v.var ? t("reads your variable {name}", { name: props.v.var }) : t("is built from your variables")) : props.v.value === "" ? t("(empty)") : props.v.value}</span>
      <span class={`as-tag${props.v.carried ? "" : " off"}`}>{props.v.carried ? t("will be written") : t("not copied")}</span>
    </div>
  );
}

/** What one copy of a project server would run, in full. */
function UntrustedBlock(props: { u: Untrusted }) {
  return (
    <div class="as-run" data-untrusted-for={props.u.agent}>
      <span>{t("{name} for {agent}", { name: props.u.name, agent: agentLabel(props.u.agent) })}</span>
      <Show when={props.u.argv.length}><code data-argv>{props.u.argv.map(quoteArg).join(" ")}</code></Show>
      <Show when={props.u.url}><code data-argv>{props.u.url}</code></Show>
      <Show when={props.u.arg_vars.length}><div class="as-warn" data-arg-vars>{t("The command or URL reads your variables: {names}", { names: props.u.arg_vars.join(", ") })}</div></Show>
      <Show when={props.u.cwd}><div>{t("Working folder: {path}", { path: props.u.cwd! })}</div></Show>
      <For each={props.u.resolved}>{(r) => <div class="muted-inline" data-resolved>{t("“{from}” becomes {to}", { from: r.from, to: r.to })}</div>}</For>
      <Show when={props.u.env.length}><div class="muted-inline">{t("Environment:")}</div></Show>
      <For each={props.u.env}>{(v) => <ValueRow v={v} />}</For>
      <Show when={props.u.headers.length}><div class="muted-inline">{t("Headers:")}</div></Show>
      <For each={props.u.headers}>{(v) => <ValueRow v={v} />}</For>
    </div>
  );
}

/** The pending change: the diff, what to know before it is written, and the confirmation a
 *  project server needs before it reaches the user's own settings. */
function ChangePanel(props: { pending: Pending; onConfirm: (v: boolean) => void; onApply: () => void; onCancel: () => void }) {
  const p = () => props.pending;
  const pv = () => p().preview;
  const blocked = () => p().busy || !pv() || pv()!.noop || (pv()!.needs_confirm && !p().confirmed);
  return (
    <div class="as-change" data-change-panel role="group" aria-label={p().title}>
      <div class="as-change-title">{p().title}</div>
      <Show when={p().busy && !pv()}><div class="muted-inline">{t("Preparing the preview…")}</div></Show>
      <Show when={pv()}>
        {(v) => (
          <>
            <Show when={v().noop}><div class="muted-inline" data-noop>{t("Nothing would change.")}</div></Show>
            <For each={v().warnings}>{(w) => <div class="as-warn" data-warning={w.code}>{warningText(w)}</div>}</For>
            <Show when={v().needs_confirm}>
              <div class="as-untrusted" data-untrusted>
                <div class="as-untrusted-head">{t("This server comes from the project, and the project is not yours alone. Once copied to your user settings it is available in every project, and the agent will not ask again. Everything it would run:")}</div>
                <For each={v().untrusted}>{(u) => <UntrustedBlock u={u} />}</For>
                <label class="as-check">
                  <input type="checkbox" data-confirm-untrusted checked={p().confirmed} onChange={(e) => props.onConfirm(e.currentTarget.checked)} />
                  {t("I have read all of this and trust it")}
                </label>
              </div>
            </Show>
            <For each={v().files}>{(f) => <DiffBlock file={f} />}</For>
          </>
        )}
      </Show>
      <Show when={p().error}><div class="form-note error" data-change-error>{errorText(p().error)}</div></Show>
      <div class="inline-fields">
        <button class="btn primary" data-apply disabled={blocked()} onClick={props.onApply}>{p().busy && pv() ? t("Saving…") : t("Back up and save")}</button>
        <button class="btn" data-cancel onClick={props.onCancel}>{t("Cancel")}</button>
      </div>
    </div>
  );
}

export default function AgentSetup() {
  const project = () => {
    const p = store.activeProject();
    return p && !p.remote ? p : undefined;
  };
  const [tab, setTab] = createSignal<Tab>("mcp");
  const [data, setData] = createSignal<Overview | null>(null);
  const [loadError, setLoadError] = createSignal<string | null>(null);
  const [pending, setPending] = createSignal<Pending | null>(null);
  const [message, setMessage] = createSignal<{ text: string; error?: boolean } | null>(null);
  const [backups, setBackups] = createSignal<BackupInfo[] | null>(null);

  const [loading, setLoading] = createSignal(false);
  const load = async () => {
    setLoadError(null);
    setLoading(true);
    try {
      setData(await agentSetupOverview(project()?.path ?? null));
    } catch (e) {
      setLoadError(errorText(e));
    } finally {
      setLoading(false);
    }
  };
  // Read once, when the page opens.
  onMount(() => void load());

  const loadBackups = async () => {
    try {
      setBackups(await agentSetupBackups());
    } catch (e) {
      setMessage({ text: errorText(e), error: true });
    }
  };

  /** Shows what `edit` would do. Nothing is written until the user applies it. */
  const propose = async (edit: Edit, title: string) => {
    setMessage(null);
    setPending({ edit, preview: null, busy: true, error: null, confirmed: false, title });
    try {
      const preview = await agentSetupPreview(project()?.path ?? null, edit);
      setPending({ edit, preview, busy: false, error: null, confirmed: false, title });
    } catch (e) {
      setPending(null);
      setMessage({ text: errorText(e), error: true });
    }
  };

  const apply = async () => {
    const p = pending();
    if (!p?.preview) return;
    // The user's confirmation travels with the edit; the app refuses a project server without it.
    const edit: Edit = p.edit.kind === "copy" ? { ...p.edit, confirm_untrusted: p.confirmed } : p.edit;
    setPending({ ...p, busy: true, error: null });
    try {
      const done = await agentSetupApply(project()?.path ?? null, edit, p.preview.token);
      setPending(null);
      setMessage({ text: t("Saved {n} file(s). Each was backed up first (see Backups below).", { n: done.files.length }) });
      await load();
      if (backups()) await loadBackups();
    } catch (e) {
      setPending({ ...p, busy: false, error: String(e) });
      await load();
    }
  };

  const files = () => data()?.mcp ?? [];
  const reveal = (path: string) => void agentSetupReveal(project()?.path ?? null, path).catch((e) => setMessage({ text: errorText(e), error: true }));

  // ---------- MCP servers ----------

  const [copyFor, setCopyFor] = createSignal<string | null>(null);
  const [copyTo, setCopyTo] = createSignal<string[]>([]);
  const [withEnv, setWithEnv] = createSignal(false);

  const openCopy = (key: string) => {
    setCopyFor(copyFor() === key ? null : key);
    setCopyTo([]);
    setWithEnv(false);
  };
  const destScopes = (): Scope[] => copyTo().map((k) => k.split(":")[1] as Scope);

  function CopyForm(props: { file: McpFile; server: McpView }) {
    const from: Target = { agent: props.file.agent, scope: props.file.scope };
    const choices = () => copyTargets(files(), from);
    const toggle = (k: string, on: boolean) => setCopyTo(on ? [...copyTo(), k] : copyTo().filter((x) => x !== k));
    return (
      <div class="as-copy" data-copy-form>
        <div class="muted-inline">{t("Copy “{name}” to:", { name: props.server.name })}</div>
        <div class="as-targets">
          <For each={choices()}>
            {(f) => (
              <label class="as-check">
                <input type="checkbox" data-copy-target={tgtKey(f)} checked={copyTo().includes(tgtKey(f))} onChange={(e) => toggle(tgtKey(f), e.currentTarget.checked)} />
                {agentLabel(f.agent)} <ScopeChip scope={f.scope} />
              </label>
            )}
          </For>
        </div>
        <Show when={props.server.env_keys.length || props.server.header_keys.length}>
          <label class="as-check" classList={{ dim: !envOptIn(destScopes()) }}>
            <input type="checkbox" data-copy-env disabled={!envOptIn(destScopes())} checked={withEnv() && envOptIn(destScopes())} onChange={(e) => setWithEnv(e.currentTarget.checked)} />
            {from.scope === "project"
              ? t("Also copy the project server's environment and header entries to the files of yours ticked above (the next step lists every one). Otherwise none of them is copied.")
              : t("Also write the environment values (your own settings files only). Otherwise they become references to variables of the same name; project files never get values.")}
          </label>
        </Show>
        <Show when={copyNeedsConfirm(from.scope, destScopes())}>
          <div class="as-warn">{t("This server comes from the project. Copying it to your user settings needs your confirmation on the next step.")}</div>
        </Show>
        <div class="inline-fields">
          <button class="btn primary" data-copy-preview disabled={!copyTo().length}
            onClick={() => {
              const to = copyTo().map((k) => ({ agent: k.split(":")[0], scope: k.split(":")[1] as Scope }));
              void propose({ kind: "copy", from, name: props.server.name, to, include_env: withEnv() && envOptIn(destScopes()), confirm_untrusted: false }, t("Copy “{name}”", { name: props.server.name }));
              setCopyFor(null);
            }}>{t("Preview")}</button>
          <button class="btn" onClick={() => setCopyFor(null)}>{t("Cancel")}</button>
        </div>
      </div>
    );
  }

  function ServerRow(props: { file: McpFile; server: McpView }) {
    const target: Target = { agent: props.file.agent, scope: props.file.scope };
    const key = () => `${tgtKey(target)}:${props.server.name}`;
    return (
      <div class="as-server" classList={{ off: !props.server.enabled }} data-server={props.server.name} data-server-file={tgtKey(target)}>
        <div class="as-server-line">
          <div class="as-server-main">
            <div class="as-server-name">{props.server.name} <span class="as-tag">{props.server.transport}</span>
              <Show when={!props.server.enabled}><span class="as-tag off">{t("off")}</span></Show>
            </div>
            <code class="as-cmd">{commandText(props.server)}</code>
            <Show when={props.server.env_keys.length}>
              <div class="as-env" data-env>{maskedEnv(props.server.env_keys).join("  ")}</div>
            </Show>
            <Show when={props.server.header_keys.length}>
              <div class="as-env" data-headers>{t("Headers: {names}", { names: props.server.header_keys.join(", ") })}</div>
            </Show>
          </div>
          <Show when={props.file.can_toggle}>
            <Toggle on={props.server.enabled} label={t("Enabled")} onChange={(on) => void propose({ kind: "toggle", target, name: props.server.name, enabled: on }, on ? t("Turn on “{name}”", { name: props.server.name }) : t("Turn off “{name}”", { name: props.server.name }))} />
          </Show>
          <button class="btn" data-copy-open={props.server.name} onClick={() => openCopy(key())}>{t("Copy to…")}</button>
          <button class="icon-btn" title={t("Remove server")} aria-label={t("Remove server")} data-remove={props.server.name}
            onClick={() => void propose({ kind: "remove", target, name: props.server.name }, t("Remove “{name}”", { name: props.server.name }))}><Icon name="close" size={13} /></button>
        </div>
        <Show when={copyFor() === key()}><CopyForm file={props.file} server={props.server} /></Show>
      </div>
    );
  }

  const [newName, setNewName] = createSignal("");
  const [newKind, setNewKind] = createSignal<"stdio" | "http" | "sse">("stdio");
  const [newCommand, setNewCommand] = createSignal("");
  const [newUrl, setNewUrl] = createSignal("");
  const [newTargets, setNewTargets] = createSignal<string[]>([]);
  const addEdit = (): Edit => {
    const line = newCommand().trim();
    const [command, ...args] = newKind() === "stdio" ? splitArgs(line) : [""];
    return {
      kind: "add",
      targets: newTargets().map((k) => ({ agent: k.split(":")[0], scope: k.split(":")[1] as Scope })),
      server: { name: newName().trim(), transport: newKind(), command: command ?? "", args, url: newUrl().trim() },
    };
  };
  const canAdd = () => !!newName().trim() && newTargets().length > 0 && (newKind() === "stdio" ? !!newCommand().trim() : !!newUrl().trim());

  const McpTab = () => (
    <>
      <For each={SETUP_AGENTS}>
        {(a) => (
          <Show when={files().some((f) => f.agent === a.id)}>
            <div class="as-agent" data-mcp-agent={a.id}>
              <div class="as-agent-name">{a.label}</div>
              <For each={files().filter((f) => f.agent === a.id)}>
                {(f) => (
                  <div class="as-file" data-mcp-file={tgtKey(f)}>
                    <div class="as-file-head">
                      <ScopeChip scope={f.scope} />
                      <code class="as-path" title={f.path}>{f.path}</code>
                      <button class="icon-btn" title={t("Show file")} aria-label={t("Show file")} onClick={() => reveal(f.path)}><Icon name="folder" size={13} /></button>
                    </div>
                    <Show when={f.error}><div class="form-note error" data-file-error>{errorText(f.error)}</div></Show>
                    <For each={f.servers} fallback={<Show when={!f.error}><div class="muted-inline">{f.exists ? t("No MCP servers.") : t("No file yet.")}</div></Show>}>
                      {(s) => <ServerRow file={f} server={s} />}
                    </For>
                  </div>
                )}
              </For>
            </div>
          </Show>
        )}
      </For>
      <details class="as-add-server" data-add-server>
        <summary>{t("Add an MCP server")}</summary>
        <div class="as-add-body">
          <div class="inline-fields">
            <input class="text-input" data-new-name placeholder={t("Name")} value={newName()} onInput={(e) => setNewName(e.currentTarget.value)} />
            <select class="text-input" data-new-kind value={newKind()} onChange={(e) => setNewKind(e.currentTarget.value as "stdio" | "http" | "sse")}>
              <option value="stdio">{t("Program")}</option>
              <option value="http">HTTP</option>
              <option value="sse">SSE</option>
            </select>
          </div>
          <Show when={newKind() === "stdio"} fallback={<input class="text-input" data-new-url placeholder="https://example.com/mcp" value={newUrl()} onInput={(e) => setNewUrl(e.currentTarget.value)} />}>
            <input class="text-input" data-new-command placeholder={t("npx -y @scope/server --option")} value={newCommand()} onInput={(e) => setNewCommand(e.currentTarget.value)} />
          </Show>
          <div class="muted-inline">{t("Add it to:")}</div>
          <div class="as-targets">
            <For each={files()}>
              {(f) => (
                <label class="as-check">
                  <input type="checkbox" data-new-target={tgtKey(f)} checked={newTargets().includes(tgtKey(f))}
                    onChange={(e) => setNewTargets(e.currentTarget.checked ? [...newTargets(), tgtKey(f)] : newTargets().filter((x) => x !== tgtKey(f)))} />
                  {agentLabel(f.agent)} <ScopeChip scope={f.scope} />
                </label>
              )}
            </For>
          </div>
          <div class="inline-fields">
            <button class="btn primary" data-add-preview disabled={!canAdd()} onClick={() => void propose(addEdit(), t("Add “{name}”", { name: newName().trim() }))}>{t("Preview")}</button>
          </div>
        </div>
      </details>
    </>
  );

  // ---------- instruction files ----------

  const [geminiMode, setGeminiMode] = createSignal<"setting" | "copy">("setting");
  const ins = () => data()?.instructions;
  const insName = (f: InstructionFile) => ({ claude: "CLAUDE.md", agents: "AGENTS.md", gemini: "GEMINI.md", copilot: ".github/copilot-instructions.md", kiro: ".kiro/steering" })[f.agent] ?? f.agent;

  const InstructionsTab = () => (
    <>
      <div class="as-list" data-instruction-list>
        <For each={ins()?.files ?? []}>
          {(f) => (
            <div class="as-row" classList={{ dim: !f.exists }} data-instruction={f.agent} data-exists={f.exists}>
              <ScopeChip scope={f.scope} />
              <div class="as-row-main"><span class="as-server-name">{insName(f)}</span> <code class="as-path" title={f.path}>{f.path}</code></div>
              <span class="muted-inline">{f.exists ? sizeText(f.bytes) : t("missing")}</span>
              <button class="icon-btn" title={t("Show file")} aria-label={t("Show file")} onClick={() => reveal(f.path)}><Icon name="folder" size={13} /></button>
            </div>
          )}
        </For>
      </div>
      <div class="as-one-file" data-one-file>
        <div class="as-agent-name">{t("One file for all agents")}</div>
        <div class="muted-inline">{t("AGENTS.md becomes the one place for the project's instructions: Codex, Cursor, OpenCode, Copilot and Kiro read it themselves, CLAUDE.md imports it with @AGENTS.md, and Gemini reads it through its settings (or gets a copy). Nothing is deleted; you see every change first.")}</div>
        <div class="as-status">
          <span data-claude-imports>{ins()?.claude_imports_agents ? t("● CLAUDE.md imports AGENTS.md") : t("○ CLAUDE.md does not import AGENTS.md")}</span>
          <span data-gemini-reads>{ins()?.gemini_reads_agents ? t("● Gemini reads AGENTS.md") : t("○ Gemini does not read AGENTS.md")}</span>
        </div>
        <div class="inline-fields">
          <select class="text-input" data-gemini-mode value={geminiMode()} onChange={(e) => setGeminiMode(e.currentTarget.value as "setting" | "copy")}>
            <option value="setting">{t("Gemini: list AGENTS.md in its settings")}</option>
            <option value="copy">{t("Gemini: keep a copy as GEMINI.md")}</option>
          </select>
          <button class="btn primary" data-unify disabled={!project()} onClick={() => void propose({ kind: "instructions", gemini: geminiMode() }, t("Use AGENTS.md for every agent"))}>{t("Preview")}</button>
        </div>
        <Show when={!project()}><div class="muted-inline">{t("Open a project on this computer to set up its instruction files.")}</div></Show>
      </div>
    </>
  );

  // ---------- skills, commands, subagents ----------

  const [skillFilter, setSkillFilter] = createSignal("");
  const skills = (kind: SkillItem["kind"]) => {
    const q = skillFilter().trim().toLowerCase();
    return (data()?.skills ?? []).filter((s) => s.kind === kind && (!q || `${s.name} ${s.description} ${s.agent}`.toLowerCase().includes(q)));
  };
  const KINDS: { kind: SkillItem["kind"]; title: () => string }[] = [
    { kind: "skill", title: () => t("Skills") },
    { kind: "command", title: () => t("Commands") },
    { kind: "subagent", title: () => t("Subagents") },
  ];

  const SkillsTab = () => (
    <>
      <input class="text-input" data-skill-filter placeholder={t("Filter")} value={skillFilter()} onInput={(e) => setSkillFilter(e.currentTarget.value)} />
      <For each={KINDS}>
        {(k) => (
          <div class="as-agent" data-skill-kind={k.kind}>
            <div class="as-agent-name">{k.title()} <span class="muted-inline">{skills(k.kind).length}</span></div>
            <For each={skills(k.kind)} fallback={<div class="muted-inline">{t("None found.")}</div>}>
              {(s) => (
                <div class="as-row" data-skill={s.name}>
                  <ScopeChip scope={s.scope} />
                  <div class="as-row-main">
                    <div><span class="as-server-name">{s.name}</span> <span class="as-tag">{agentLabel(s.agent)}</span></div>
                    <Show when={s.description}><div class="muted-inline">{s.description}</div></Show>
                  </div>
                  <button class="icon-btn" title={t("Show file")} aria-label={t("Show file")} onClick={() => reveal(s.path)}><Icon name="folder" size={13} /></button>
                </div>
              )}
            </For>
          </div>
        )}
      </For>
    </>
  );

  // ---------- find more agents ----------

  const [registry, setRegistry] = createSignal<RegistryAgent[] | null>(null);
  const [registryBusy, setRegistryBusy] = createSignal(false);
  const [registryError, setRegistryError] = createSignal<string | null>(null);
  const [registryFilter, setRegistryFilter] = createSignal("");
  const [confirming, setConfirming] = createSignal<string | null>(null);
  const saved = () => store.state.settings.registryAgents ?? [];

  /** One request, and only because the button was pressed. */
  const search = async () => {
    setRegistryBusy(true);
    setRegistryError(null);
    try {
      setRegistry(await agentRegistryFetch());
    } catch (e) {
      setRegistryError(errorText(e));
    } finally {
      setRegistryBusy(false);
    }
  };

  const save = async (entry: RegistryAgent) => {
    const agent = savedFromRegistry(entry);
    if (!agent) return;
    const before = saved();
    store.setSettings({ registryAgents: [...before.filter((a) => a.id !== agent.id), agent] });
    await syncPluginAgents(true);
    setConfirming(null);
    if (!presetFor(agent.id)) {
      store.setSettings({ registryAgents: before });
      await syncPluginAgents(true);
      setMessage({ text: t("The session daemon did not accept “{name}”.", { name: agent.label }), error: true });
      return;
    }
    setMessage({ text: t("Saved “{name}”. It does not run until you start a session with it.", { name: agent.label }) });
  };

  const forget = async (id: string) => {
    store.setSettings({ registryAgents: saved().filter((a) => a.id !== id) });
    await syncPluginAgents(true);
  };

  const shownRegistry = () => {
    const q = registryFilter().trim().toLowerCase();
    return (registry() ?? []).filter((a) => !q || `${a.id} ${a.name} ${a.description}`.toLowerCase().includes(q));
  };

  const FindTab = () => (
    <>
      <div class="muted-inline">{t("Looks up the Agent Client Protocol registry: one request, only when you press the button. Nothing is installed or run; you see the full command and confirm before an agent is saved.")}</div>
      <div class="inline-fields">
        <button class="btn primary" data-registry-search disabled={registryBusy()} onClick={() => void search()}>{registryBusy() ? t("Searching…") : registry() ? t("Search again") : t("Search the registry")}</button>
        <Show when={registry()}><input class="text-input" data-registry-filter placeholder={t("Filter")} value={registryFilter()} onInput={(e) => setRegistryFilter(e.currentTarget.value)} /></Show>
      </div>
      <Show when={registryError()}><div class="form-note error" data-registry-error>{registryError()}</div></Show>
      <Show when={saved().length}>
        <div class="as-agent" data-saved-agents>
          <div class="as-agent-name">{t("Saved from the registry")}</div>
          <For each={saved()}>
            {(a) => (
              <div class="as-row" data-saved-agent={a.id}>
                <div class="as-row-main">
                  <div><span class="as-server-name">{a.label}</span> <span class="muted-inline">v{a.version}</span></div>
                  <code class="as-cmd">{[a.bin, ...a.acp].join(" ")}</code>
                </div>
                <button class="btn" data-forget={a.id} onClick={() => void forget(a.id)}>{t("Remove")}</button>
              </div>
            )}
          </For>
        </div>
      </Show>
      <For each={shownRegistry()}>
        {(a) => (
          <div class="as-card" data-registry-agent={a.id}>
            <div class="as-server-line">
              <div class="as-server-main">
                <div class="as-server-name">{a.name} <span class="muted-inline">v{a.version}</span> <span class="as-tag">{a.distribution}</span></div>
                <Show when={a.description}><div class="muted-inline">{a.description}</div></Show>
                <Show when={a.command.length}><code class="as-cmd">{registryCommandLine(a)}</code></Show>
                <Show when={a.env_keys.length}><div class="muted-inline">{t("Asks for environment variables: {names}", { names: a.env_keys.join(", ") })}</div></Show>
                <Show when={a.problem}><div class="as-warn" data-problem={a.problem!}>{registryProblem(a.problem!)}</div></Show>
                <Show when={a.pinned}><div class="muted-inline" data-pinned>{t("Version pinned to {version}", { version: a.pinned! })}</div></Show>
              </div>
              <Show when={a.def}>
                <button class="btn" data-registry-save={a.id} disabled={saved().some((s) => s.id === a.def!.id)} onClick={() => setConfirming(a.id)}>
                  {saved().some((s) => s.id === a.def!.id) ? t("Saved") : t("Save as custom agent…")}
                </button>
              </Show>
            </div>
            <Show when={confirming() === a.id}>
              <div class="as-untrusted" data-registry-confirm>
                <div class="as-untrusted-head">{t("Kelpie will save “{name}” as an agent. Starting a session with it will run exactly this, from the registry, at this version:", { name: a.name })}</div>
                <code class="as-cmd big">{registryCommandLine(a)}</code>
                <div class="inline-fields">
                  <button class="btn primary" data-registry-confirm-yes onClick={() => void save(a)}>{t("Save agent")}</button>
                  <button class="btn" onClick={() => setConfirming(null)}>{t("Cancel")}</button>
                </div>
              </div>
            </Show>
          </div>
        )}
      </For>
    </>
  );

  // ---------- backups ----------

  const BackupsList = () => (
    <details class="as-backups" data-backups onToggle={(e) => e.currentTarget.open && void loadBackups()}>
      <summary>{t("Backups")}</summary>
      <div class="muted-inline">{t("Each file is copied here just before Kelpie changes it. Restoring shows the difference first and backs up what is there now.")}</div>
      <For each={backups() ?? []} fallback={<div class="muted-inline">{backups() ? t("No backups yet.") : t("Loading…")}</div>}>
        {(b) => (
          <div class="as-row" data-backup={b.id}>
            <div class="as-row-main"><code class="as-path" title={b.path}>{b.path}</code><div class="muted-inline">{formatDateTime(b.at * 1000)}{b.existed ? "" : ` · ${t("the file did not exist")}`}</div></div>
            <button class="btn" data-restore={b.id} onClick={() => void propose({ kind: "restore", backup: b.id }, t("Restore this backup"))}>{t("Restore…")}</button>
          </div>
        )}
      </For>
    </details>
  );

  const TABS: { id: Tab; label: () => string }[] = [
    { id: "mcp", label: () => t("MCP servers") },
    { id: "instructions", label: () => t("Instructions") },
    { id: "skills", label: () => t("Skills & commands") },
    { id: "find", label: () => t("Find more agents") },
  ];
  const view: Record<Tab, () => JSX.Element> = { mcp: McpTab, instructions: InstructionsTab, skills: SkillsTab, find: FindTab };

  return (
    <div class="as-page" data-agent-setup>
      <LoadBar when={loading() || pending()?.busy} />
      <div class="as-tabs" role="tablist">
        <For each={TABS}>
          {(x) => <button class={`as-tab${tab() === x.id ? " on" : ""}`} role="tab" aria-selected={tab() === x.id} data-tab={x.id} onClick={() => setTab(x.id)}>{x.label()}</button>}
        </For>
        <span class="as-spacer" />
        <Show when={data()}><span class="muted-inline" data-read-ms>{t("Read in {ms} ms", { ms: Math.round(data()!.elapsed_ms) })}</span></Show>
        <button class="btn" data-refresh onClick={() => void load()}><Icon name="refresh" size={13} /> {t("Refresh")}</button>
      </div>
      <Show when={!project()}>
        <div class="muted-inline" data-no-project>{store.activeProject()?.remote ? t("Project files of a remote project are on its host: only your own settings are shown.") : t("No project open: only your own settings are shown.")}</div>
      </Show>
      <Show when={data() && !data()!.writable}><div class="as-warn">{t("Writes are off in this run.")}</div></Show>
      <Show when={loadError()}><div class="form-note error">{loadError()}</div></Show>
      <Show when={message()}>{(m) => <div class={m().error ? "form-note error" : "form-note ok"} data-message>{m().text}</div>}</Show>
      <Show when={pending()}>
        {(p) => <ChangePanel pending={p()} onConfirm={(v) => setPending({ ...p(), confirmed: v })} onApply={() => void apply()} onCancel={() => setPending(null)} />}
      </Show>
      <Show when={data()} fallback={<Show when={!loadError()}><div class="muted-inline">{t("Loading…")}</div></Show>}>
        <div class="as-body">{view[tab()]()}</div>
        <Show when={tab() === "mcp"}><BackupsList /></Show>
      </Show>
    </div>
  );
}
