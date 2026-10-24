// Several Claude Code / Codex sign-ins side by side (backend: app/src-tauri/src/accounts.rs).
// An account is a config home; sessions started on it get `CLAUDE_CONFIG_DIR` / `CODEX_HOME`,
// so logins, settings and transcripts stay apart. The active account per agent is only a
// pointer: switching never touches running sessions, which keep the home they started with.
import { invoke } from "@tauri-apps/api/core";
import { store, sessionLabel } from "./store";
import { daemon } from "./ipc";
import { confirmDialog } from "./confirm-dialog";
import { errorText, t } from "./i18n";
import { uid } from "./platform";
import type { AgentAccount, SessionInfo } from "./types";

export type AccountAgent = "claude" | "codex";
export const ACCOUNT_AGENTS: AccountAgent[] = ["claude", "codex"];
export const HOME_VAR: Record<AccountAgent, string> = { claude: "CLAUDE_CONFIG_DIR", codex: "CODEX_HOME" };
export const isAccountAgent = (kind: string): kind is AccountAgent => kind === "claude" || kind === "codex";

export interface AccountStatus {
  signed_in: boolean;
  email: string | null;
  plan: string | null;
  home: string;
}

export const accountStatus = (agent: AccountAgent, dir: string | null) => invoke<AccountStatus>("account_status", { agent, dir });

export const accountsOf = (agent: AccountAgent): AgentAccount[] => (store.state.settings.accounts ?? []).filter((a) => a.agent === agent);

/** The account new sessions of `agent` use; `null` = the agent's own default sign-in. */
export function activeAccount(agent: AccountAgent): AgentAccount | null {
  const id = store.state.settings.activeAccounts?.[agent];
  return (id && accountsOf(agent).find((a) => a.id === id)) || null;
}

/** Environment that puts a session on an account: the chosen one (`id`; "default" = the
 *  agent's own), else the active one. */
export function accountEnv(kind: string, id?: string | null): Record<string, string> | undefined {
  if (!isAccountAgent(kind)) return undefined;
  const acct = id === "default" ? null : id ? accountsOf(kind).find((a) => a.id === id) ?? null : activeAccount(kind);
  return acct ? { [HOME_VAR[kind]]: acct.dir } : undefined;
}

/** The account a running session belongs to (by its environment); `null` = default. */
export function sessionAccount(s: Pick<SessionInfo, "kind" | "env">): AgentAccount | null {
  if (!isAccountAgent(s.kind)) return null;
  const dir = s.env?.[HOME_VAR[s.kind]];
  return dir ? accountsOf(s.kind).find((a) => samePath(a.dir, dir)) ?? null : null;
}

/** A session's config home (for transcript lookups), if it runs on a non-default account. */
export const sessionHome = (s: Pick<SessionInfo, "kind" | "env">): string | undefined =>
  isAccountAgent(s.kind) ? s.env?.[HOME_VAR[s.kind]] || undefined : undefined;

/** Every non-default home, so past conversations of all accounts are listed. */
export function accountHomes(): { claude: string[]; codex: string[] } {
  return { claude: accountsOf("claude").map((a) => a.dir), codex: accountsOf("codex").map((a) => a.dir) };
}

export function accountLabel(agent: AccountAgent, dir: string | null | undefined): string {
  if (!dir) return t("Default");
  return accountsOf(agent).find((a) => samePath(a.dir, dir))?.label ?? t("Other account");
}

const samePath = (a: string, b: string) => a.replace(/\\/g, "/").toLowerCase() === b.replace(/\\/g, "/").toLowerCase();

export async function addAccount(agent: AccountAgent, label: string): Promise<AgentAccount> {
  const id = `${label.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "").slice(0, 24) || agent}-${uid().slice(0, 6)}`;
  const dir = await invoke<string>("account_create", { agent, id });
  const acct: AgentAccount = { id, agent, label: label.trim() || "Account", dir };
  store.setSettings({ accounts: [...(store.state.settings.accounts ?? []), acct] });
  return acct;
}

export async function removeAccount(acct: AgentAccount) {
  await invoke("account_remove", { agent: acct.agent, id: acct.id });
  const active = { ...(store.state.settings.activeAccounts ?? {}) };
  if (active[acct.agent] === acct.id) delete active[acct.agent];
  store.setSettings({ accounts: (store.state.settings.accounts ?? []).filter((a) => a.id !== acct.id), activeAccounts: active });
}

export function setActiveAccount(agent: AccountAgent, id: string | null) {
  const active = { ...(store.state.settings.activeAccounts ?? {}) };
  if (id) active[agent] = id;
  else delete active[agent];
  store.setSettings({ activeAccounts: active });
}

export function renameAccount(id: string, label: string) {
  if (!label.trim()) return;
  store.setSettings({ accounts: (store.state.settings.accounts ?? []).map((a) => (a.id === id ? { ...a, label: label.trim() } : a)) });
}

/** Why a session cannot move to another account (`null`: it can). `target`: an account id; null =
 *  the agent's own default sign-in. */
export function switchRefusal(s: Pick<SessionInfo, "kind" | "chat" | "remote" | "agent_session_id" | "env">, target: AgentAccount | null): string | null {
  if (!isAccountAgent(s.kind)) return t("Only Claude Code and Codex sessions can change account.");
  if (s.chat) return t("A chat view session cannot change account: open it in the terminal first.");
  if (s.remote) return t("A session on another machine keeps its account.");
  if (!s.agent_session_id) return t("This session has no conversation yet, so there is nothing to carry over.");
  const current = sessionAccount(s);
  if ((current?.id ?? null) === (target?.id ?? null)) return t("The session is already on that account.");
  return null;
}

interface TranscriptMove {
  refusal: string | null;
  files: number;
  bytes: number;
}

const accountTranscript = (s: SessionInfo, target: AgentAccount | null, apply: boolean) =>
  invoke<TranscriptMove>("account_transcript", { agent: s.kind, from: sessionHome(s) ?? null, to: target?.dir ?? null, cwd: s.cwd, conversation: s.agent_session_id, apply });

/** Moves a terminal session to another account: copies its conversation into that account's home,
 *  stops the agent and starts it again there on the same conversation. Refuses, with the reason,
 *  when the conversation cannot be carried over; nothing is stopped before that is known. */
export async function switchSessionAccount(s: SessionInfo, target: AgentAccount | null) {
  const name = sessionLabel(s);
  const refusal = switchRefusal(s, target);
  if (refusal) return store.notify(refusal, undefined, 8000);
  const agent = s.kind as AccountAgent;
  try {
    const plan = await accountTranscript(s, target, false);
    if (plan.refusal) return store.notify(t(plan.refusal), undefined, 10000);
    const who = target?.label ?? t("Default");
    if (!(await confirmDialog(t("Switch {name} to the account “{account}”? The agent restarts and continues the same conversation there.", { name, account: who })))) return;
    if (s.alive) {
      await daemon("session.kill", { handle: s.handle });
      const end = Date.now() + 8000;
      while (store.sessions[s.id]?.alive && Date.now() < end) await new Promise((r) => setTimeout(r, 100));
      if (store.sessions[s.id]?.alive) return store.notify(t("{name} did not stop.", { name }));
    }
    try {
      await accountTranscript(s, target, true);
      await daemon("session.set_env_restart", { handle: s.handle, env: { [HOME_VAR[agent]]: target?.dir ?? null } });
    } catch (e) {
      // The session goes on where it was.
      await store.resumeSession(s.id);
      return store.notify(t("Could not switch {name}: {error}", { name, error: errorText(e) }), undefined, 10000);
    }
    store.notify(t("{name} now runs on the account “{account}”.", { name, account: who }));
  } catch (e) {
    store.notify(t("Could not switch {name}: {error}", { name, error: errorText(e) }), undefined, 10000);
  }
}

/** Signs in in the floating terminal: the agent's own login flow, on that account's home. */
export function signIn(agent: AccountAgent, acct: AgentAccount | null) {
  const args = agent === "claude" ? ["claude", "auth", "login"] : ["codex", "login"];
  const env = acct ? { [HOME_VAR[agent]]: acct.dir } : undefined;
  if (!store.state.settings.floatingTerminal) store.setSettings({ floatingTerminal: true });
  void store.newFloating("custom", { args, env, name: t("Sign in · {name}", { name: acct?.label ?? t("Default") }) });
}
