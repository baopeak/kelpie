// Automations: start an agent, type a saved prompt or run a command on a schedule or when something
// happens. Stored and run by the session daemon (`automation.*`); the ones set off by an agent's
// turn or a commit also run while the window is closed.
import { For, Show, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { store } from "../../lib/store";
import { daemon } from "../../lib/ipc";
import { kindInfo, launchableAgents } from "../../lib/agents";
import { formatDateTime, formatRelative, msg, t } from "../../lib/i18n";
import { KindGlyph } from "../icons";
import { SettingRow, Toggle } from "./setting-row";
import type { Automation, AutomationEventKind } from "../../lib/types";
import { HOME_VAR, accountsOf, activeAccount, isAccountAgent, type AccountAgent } from "../../lib/accounts";
import { confirmDialog } from "../../lib/confirm-dialog";
import { REVIEW_API } from "../../lib/review-queue";
import { AUTOMATION_API, automationEvents } from "../../lib/automation-events";

const DAYS = [msg("Sunday"), msg("Monday"), msg("Tuesday"), msg("Wednesday"), msg("Thursday"), msg("Friday"), msg("Saturday")];

const EVENTS: [AutomationEventKind, string][] = [
  ["file", msg("A file changes")],
  ["turn", msg("An agent finishes a turn")],
  ["commit", msg("A commit is made")],
  ["pr_checks_failed", msg("Pull request checks fail")],
  ["finished", msg("A worktree is finished")],
];
/** Reported by the window: they fire only while it is open. */
const FROM_WINDOW: AutomationEventKind[] = ["file", "pr_checks_failed", "finished"];
const TOKENS = "{file}, {files}, {commit}, {branch}";

export function scheduleLabel(a: Pick<Automation, "every" | "time" | "weekday">): string {
  if (a.every === "hourly") return t("hourly at :{minute}", { minute: a.time.split(":")[1] ?? "00" });
  if (a.every === "weekly") return t("{day}s at {time}", { day: t(DAYS[a.weekday % 7]), time: a.time });
  return a.every === "weekdays" ? t("weekdays at {time}", { time: a.time }) : t("daily at {time}", { time: a.time });
}

/** What starts an automation, in words. */
export function whenLabel(a: Pick<Automation, "every" | "time" | "weekday" | "trigger">): string {
  const tr = a.trigger;
  if (!tr) return scheduleLabel(a);
  switch (tr.event) {
    case "file": return t("when {glob} changes", { glob: tr.glob });
    case "turn": return t("when an agent finishes a turn");
    case "commit": return t("when a commit is made");
    case "pr_checks_failed": return t("when pull request checks fail");
    case "finished": return t("when a worktree is finished");
  }
}

/** The command as the user will approve it: program and arguments, one per word, quoted when needed. */
export const commandText = (a: Pick<Automation, "shell" | "kind" | "args" | "prompt">) =>
  a.shell
    ? [a.shell.program, ...a.shell.argv].map((p) => (/[\s"']/.test(p) || !p ? JSON.stringify(p) : p)).join(" ")
    : `${[a.kind, ...a.args].join(" ")} \u2190 ${a.prompt}`;

const blank = (): Automation => ({
  id: "", name: "", enabled: true, kind: "claude", prompt: "", cwd: store.activeProject()?.path ?? "", project_id: store.activeProject()?.id ?? null,
  args: [], every: "daily", time: "09:00", weekday: 1, last_run: 0, last_session: null, last_error: null, next_run: 0, action: "agent",
});

function Editor(props: { initial: Automation; onDone: () => void }) {
  const [a, setA] = createSignal<Automation>({ ...props.initial });
  const set = (patch: Partial<Automation>) => setA({ ...a(), ...patch });
  const events = () => store.daemonApi() >= AUTOMATION_API;
  const action = () => a().action ?? "agent";
  const [account, setAccount] = createSignal<string>(
    (isAccountAgent(props.initial.kind) && accountsOf(props.initial.kind).find((x) => props.initial.env?.[HOME_VAR[props.initial.kind as AccountAgent]] === x.dir)?.id) || (props.initial.id ? "default" : activeAccount(props.initial.kind as AccountAgent)?.id ?? "default"),
  );
  const quickChoices = () => (store.state.settings.quickCommands ?? []).filter((c) => c.action === "agent" && c.text.trim());
  const ready = () => {
    const x = a();
    if (!x.name.trim() || !x.cwd) return false;
    if (x.trigger?.event === "file" && !x.trigger.glob.trim()) return false;
    return action() === "shell" ? !!x.shell?.program.trim() : !!x.prompt.trim();
  };
  const save = async () => {
    try {
      const x = a();
      // Agent runs use the agent profile's environment on the chosen account.
      const env = action() === "shell" ? {} : store.profileLaunch(x.kind, { account: account() }).env ?? {};
      await daemon("automation.save", { ...x, env, kind: action() === "shell" ? "" : x.kind });
      props.onDone();
    } catch (e) {
      store.notify(String(e), undefined, 8000);
    }
  };
  const pickProject = (id: string) => {
    const p = store.state.projects.find((x) => x.id === id);
    if (p) set({ project_id: p.id, cwd: p.path });
  };
  const setTrigger = (patch: Partial<NonNullable<Automation["trigger"]>>) => set({ trigger: { event: "file", glob: "**/*.md", daily_cap: 10, ...a().trigger, ...patch } });
  const setAction = (next: NonNullable<Automation["action"]>) =>
    set({ action: next, shell: next === "shell" ? a().shell ?? { program: "", argv: [] } : null, deliver: next === "shell" ? null : a().deliver });
  const pickQuick = (id: string) => {
    const c = quickChoices().find((x) => x.id === id);
    if (c) set({ prompt: c.text, quick_label: c.label });
  };
  return (
    <div class="automation-editor">
      <label class="field"><span>{t("Name")}</span><input class="text-input" value={a().name} placeholder={t("Weekday triage")} onInput={(e) => set({ name: e.currentTarget.value })} /></label>
      <label class="field" style={{ flex: 2 }}>
        <span>{t("Project")}</span>
        <select class="text-input" value={a().project_id ?? ""} onChange={(e) => pickProject(e.currentTarget.value)}>
          <Show when={!a().project_id}><option value="">{a().cwd || t("Choose a project")}</option></Show>
          <For each={store.state.projects}>{(p) => <option value={p.id}>{p.name}</option>}</For>
        </select>
      </label>
      <div class="inline-fields">
        <label class="field">
          <span>{t("Starts")}</span>
          <select class="text-input" value={a().trigger ? "event" : "clock"} onChange={(e) => set({ trigger: e.currentTarget.value === "event" ? { event: "file", glob: "**/*.md", daily_cap: 10 } : null })}>
            <option value="clock">{t("On a schedule")}</option>
            <option value="event" disabled={!events()}>{t("When something happens")}</option>
          </select>
        </label>
        <Show when={a().trigger}>
          {(tr) => (
            <>
              <label class="field">
                <span>{t("When")}</span>
                <select class="text-input" value={tr().event} onChange={(e) => setTrigger({ event: e.currentTarget.value as AutomationEventKind })}>
                  <For each={EVENTS}>{([v, label]) => <option value={v}>{t(label)}</option>}</For>
                </select>
              </label>
              <Show when={tr().event === "file"}>
                <label class="field" style={{ flex: 1 }}>
                  <span>{t("Files")}</span>
                  <input class="text-input" style={{ width: "100%" }} value={tr().glob} placeholder="**/*.md" /* i18n-ignore */ onInput={(e) => setTrigger({ glob: e.currentTarget.value })} />
                </label>
              </Show>
              <label class="field">
                <span>{t("At most per day")}</span>
                <input class="text-input" type="number" min={1} max={1000} style={{ width: "80px" }} value={tr().daily_cap} onInput={(e) => setTrigger({ daily_cap: Math.max(1, Math.min(1000, +e.currentTarget.value || 1)) })} />
              </label>
            </>
          )}
        </Show>
      </div>
      <Show when={!events()}><div class="muted">{t("Restart the session daemon (Settings → Session Daemon) to use triggers and commands.")}</div></Show>
      <Show when={a().trigger}>
        <div class="muted">
          {FROM_WINDOW.includes(a().trigger!.event) ? t("This trigger comes from the Kelpie window: it fires only while the window is open.") : t("This trigger is seen by the session daemon: it also fires while the window is closed.")}
          {" "}{t("It runs at most once a minute and not while a session it started is working. Changes in that time are ignored.")}
        </div>
      </Show>
      <Show when={!a().trigger}>
        <div class="inline-fields">
          <label class="field">
            <span>{t("Every")}</span>
            <select class="text-input" value={a().every} onChange={(e) => set({ every: e.currentTarget.value as Automation["every"] })}>
              <option value="hourly">{t("Hour")}</option><option value="daily">{t("Day")}</option><option value="weekdays">{t("Weekday (Mon–Fri)")}</option><option value="weekly">{t("Week")}</option>
            </select>
          </label>
          <Show when={a().every === "weekly"}>
            <label class="field">
              <span>{t("On")}</span>
              <select class="text-input" value={a().weekday} onChange={(e) => set({ weekday: +e.currentTarget.value })}>
                <For each={DAYS}>{(d, i) => <option value={i()}>{t(d)}</option>}</For>
              </select>
            </label>
          </Show>
          <label class="field">
            <span>{a().every === "hourly" ? t("At minute") : t("At")}</span>
            <input class="text-input" type="time" value={a().time} onInput={(e) => set({ time: e.currentTarget.value || "09:00" })} />
          </label>
        </div>
      </Show>
      <label class="field">
        <span>{t("Does")}</span>
        <select class="text-input" value={action()} onChange={(e) => setAction(e.currentTarget.value as NonNullable<Automation["action"]>)}>
          <option value="agent">{t("Start an agent session with a prompt")}</option>
          <option value="quick" disabled={!events()}>{t("Type a saved prompt into an idle agent")}</option>
          <option value="shell" disabled={!events()}>{t("Run a command")}</option>
        </select>
      </label>
      <Show when={action() !== "shell"}>
        <div class="inline-fields">
          <label class="field" style={{ flex: 1 }}>
            <span>{t("Agent")}</span>
            <select class="text-input" value={a().kind} onChange={(e) => set({ kind: e.currentTarget.value })}>
              <For each={launchableAgents(store.state.settings.hiddenAgents)}>{(k) => <option value={k}>{kindInfo(k).label}</option>}</For>
            </select>
          </label>
          <Show when={isAccountAgent(a().kind) && accountsOf(a().kind as AccountAgent).length}>
            <label class="field" style={{ flex: 1 }}>
              <span>{t("Account")}</span>
              <select class="text-input" value={account()} onChange={(e) => setAccount(e.currentTarget.value)}>
                <option value="default">{t("Default")}</option>
                <For each={accountsOf(a().kind as AccountAgent)}>{(x) => <option value={x.id}>{x.label}</option>}</For>
              </select>
            </label>
          </Show>
        </div>
        <Show when={store.daemonApi() >= REVIEW_API && !!store.state.projects.find((p) => p.id === a().project_id)?.parentId}>
          <label class="field">
            <span>{t("When it finishes")}</span>
            <select class="text-input" value={a().deliver ?? ""} onChange={(e) => set({ deliver: (e.currentTarget.value || null) as Automation["deliver"] })}>
              <option value="">{t("Nothing")}</option><option value="review">{t("Move the folder's card to Review")}</option>
            </select>
          </label>
        </Show>
      </Show>
      <Show when={action() === "quick"}>
        <label class="field">
          <span>{t("Saved prompt")}</span>
          <select class="text-input" value={quickChoices().find((c) => c.label === a().quick_label && c.text === a().prompt)?.id ?? ""} onChange={(e) => pickQuick(e.currentTarget.value)}>
            <option value="">{a().prompt ? (a().quick_label ?? t("Prompt")) : t("Choose a quick command")}</option>
            <For each={quickChoices()}>{(c) => <option value={c.id}>{c.label}</option>}</For>
          </select>
        </label>
        <Show when={!quickChoices().length}><div class="muted">{t("Save an agent prompt under Quick commands first.")}</div></Show>
        <Show when={a().prompt}><div class="muted automation-preview">{a().prompt}</div></Show>
      </Show>
      <Show when={action() === "agent"}>
        <label class="field"><span>{t("Prompt")}</span><textarea class="text-input" rows={4} value={a().prompt} placeholder={t("Triage new issues and summarize blockers")} onInput={(e) => set({ prompt: e.currentTarget.value })} /></label>
      </Show>
      <Show when={action() === "shell"}>
        <label class="field"><span>{t("Program")}</span><input class="text-input" value={a().shell?.program ?? ""} placeholder="python" /* i18n-ignore */ onInput={(e) => set({ shell: { ...a().shell!, argv: a().shell!.argv, program: e.currentTarget.value } })} /></label>
        <label class="field">
          <span>{t("Arguments (one per line)")}</span>
          <textarea class="text-input" rows={3} value={(a().shell?.argv ?? []).join("\n")} placeholder={"scripts/update.py\n{file}"} onInput={(e) => set({ shell: { ...a().shell!, argv: e.currentTarget.value.split("\n").filter((l) => l.length) } })} />
        </label>
        <div class="muted">
          {t("No shell is used: the program and each line are passed exactly as written. A line that is only one of {tokens} is replaced by that value from the event; the same data is in KELPIE_* environment variables. An interpreter (PowerShell, bash, python, node…) is given it only to run a script file in the project, such as powershell -File script.ps1 {file}. You approve the command, and the contents of its scripts, after saving it.", { tokens: TOKENS })}
        </div>
      </Show>
      {/* An older daemon ignores the field and would run for real. */}
      <label class="check-label"><input type="checkbox" disabled={!events()} checked={!!a().dry_run} onChange={(e) => set({ dry_run: e.currentTarget.checked })} /> {t("Dry run: only note what it would do")}</label>
      <div class="inline-fields" style={{ "justify-content": "flex-end" }}>
        <button class="btn" onClick={props.onDone}>{t("Cancel")}</button>
        <button class="btn primary" disabled={!ready()} onClick={() => void save()}>{t("Save")}</button>
      </div>
    </div>
  );
}

export function AutomationSettings() {
  const [list, { refetch }] = createResource(() => store.connected(), () => daemon<Automation[]>("automation.list").catch(() => [] as Automation[]));
  const [editing, setEditing] = createSignal<Automation | null>(null);
  onMount(() => {
    const on = () => void refetch();
    window.addEventListener("kelpie:automations", on);
    onCleanup(() => window.removeEventListener("kelpie:automations", on));
  });
  const toggle = (a: Automation, enabled: boolean) => void daemon("automation.save", { ...a, enabled }).catch((e) => store.notify(String(e)));
  const run = (a: Automation) => void daemon("automation.run", { id: a.id }).then(() => store.notify(t("Started {name}", { name: a.name })), (e) => store.notify(String(e)));
  const dryRun = async (a: Automation) => {
    try {
      const r = await daemon<{ note: string }>("automation.run", { id: a.id, dry_run: true });
      store.notify(r.note, undefined, 15000);
    } catch (e) {
      store.notify(String(e), undefined, 8000);
    }
  };
  const remove = async (a: Automation) => (await confirmDialog(t("Delete the automation “{name}”?", { name: a.name }))) && void daemon("automation.delete", { id: a.id }).catch((e) => store.notify(String(e)));
  const approve = async (a: Automation & { command_hash?: string; command_program?: string | null; command_files?: string[]; command_unvetted?: boolean }) => {
    const hash = a.command_hash;
    if (!hash) return;
    const program = a.action === "shell" ? (a.command_program ?? t("not found")) : a.kind;
    const files = a.command_files?.length ? a.command_files.join("\n") : t("none");
    const warning = a.command_unvetted ? `${t("This program will receive file names chosen by agents as its arguments. Approve only if it never runs them as commands.")}\n\n` : "";
    const ok = await confirmDialog(
      warning + t("Run this command whenever “{name}” fires?\n\n{command}\n\nProgram that will run: {program}\nScripts and programs from the project, covered by their contents (an edit asks you again):\n{files}\n\nIn the folder {cwd}\nCommand hash {hash}", { name: a.name, command: commandText(a), program, files, cwd: a.cwd, hash: hash.slice(0, 16) }),
    );
    if (ok) void daemon("automation.approve", { id: a.id, hash }).catch((e) => store.notify(String(e), undefined, 8000));
  };
  const state = automationEvents.state;

  return (
    <SettingRow id="automations" wide>
      <div class="automations">
        <Show when={state().error}>
          <div class="automation-error">{state().error}</div>
          <Show when={state().fix === "dismiss"}>
            <button class="btn" style={{ "align-self": "flex-start" }} onClick={() => void automationEvents.resolve("dismiss")}>{t("Dismiss")}</button>
          </Show>
          <Show when={state().fix === "restore"}>
            <div class="inline-fields">
              <button class="btn primary" onClick={() => void automationEvents.resolve("restore")}>{t("Restore the newer list")}</button>
              <button class="btn" onClick={() => void automationEvents.resolve("keep")}>{t("Keep the current list")}</button>
            </div>
          </Show>
        </Show>
        <Show when={store.daemonApi() >= AUTOMATION_API}>
          <div class="inline-fields">
            <Toggle on={!state().paused} onChange={(on) => void automationEvents.setPaused(!on)} label={t("Automations run")} />
            <span class="muted-inline">{state().paused ? t("Paused: nothing starts by itself. “Run now” still works.") : t("On")}</span>
          </div>
        </Show>
        <Show when={editing()} keyed fallback={<button class="btn" style={{ "align-self": "flex-start" }} disabled={!!state().error} onClick={() => setEditing(blank())}>{t("New automation")}</button>}>
          {(a) => <Editor initial={a} onDone={() => { setEditing(null); void refetch(); }} />}
        </Show>
        <For each={list() ?? []} fallback={<div class="muted">{t("No automations yet.")}</div>}>
          {(a: Automation & { command_hash?: string; command_approved?: boolean }) => (
            <div class={`automation-item${a.enabled ? "" : " paused"}`}>
              <KindGlyph kind={a.action === "shell" ? "shell" : a.kind} size={18} />
              <div class="automation-text">
                <div class="automation-name">{a.name}{a.dry_run ? ` · ${t("dry run")}` : ""}</div>
                <div class="muted-inline">
                  {whenLabel(a)} · {store.state.projects.find((p) => p.id === a.project_id)?.name ?? a.cwd}
                  {a.enabled && a.next_run ? ` · ${t("next {when}", { when: formatDateTime(a.next_run, { weekday: "short", hour: "2-digit", minute: "2-digit" }) })}` : ""}
                  {a.last_run ? ` · ${t("last run {when}", { when: formatRelative(a.last_run) })}${a.last_error ? ` ${t("(failed)")}` : ""}` : ""}
                  {a.trigger && a.cap_count ? ` · ${t("{n} of {max} today", { n: a.cap_count, max: a.trigger.daily_cap })}` : ""}
                </div>
                <Show when={a.command_hash}><div class="muted-inline automation-command">{commandText(a)}</div></Show>
                <Show when={a.last_note && !a.last_error}><div class="muted-inline">{a.last_note}</div></Show>
                <Show when={a.last_error}><div class="automation-error">{a.last_error}</div></Show>
              </div>
              <Show when={a.command_hash && !a.command_approved}>
                <button class="link-btn" onClick={() => void approve(a)}>{t("Approve command")}</button>
              </Show>
              <Show when={store.daemonApi() >= AUTOMATION_API}><button class="link-btn" onClick={() => void dryRun(a)}>{t("Dry run")}</button></Show>
              <button class="link-btn" onClick={() => run(a)}>{t("Run now")}</button>
              <button class="link-btn" onClick={() => setEditing(a)}>{t("Edit")}</button>
              <button class="link-btn danger" onClick={() => remove(a)}>{t("Delete")}</button>
              <Toggle on={a.enabled} onChange={(on) => toggle(a, on)} label={t("Enabled")} />
            </div>
          )}
        </For>
      </div>
    </SettingRow>
  );
}
