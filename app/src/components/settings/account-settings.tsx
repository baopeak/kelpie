// Settings → Accounts: several Claude Code / Codex sign-ins. Each account has its own config
// home; the active one is used by new sessions, running sessions keep theirs.
import { For, Show, createResource, createSignal, onCleanup, onMount } from "solid-js";
import { store } from "../../lib/store";
import { t } from "../../lib/i18n";
import { ACCOUNT_AGENTS, accountStatus, accountsOf, activeAccount, addAccount, removeAccount, renameAccount, setActiveAccount, signIn, type AccountAgent } from "../../lib/accounts";
import { kindInfo } from "../../lib/agents";
import type { AgentAccount } from "../../lib/types";
import { Icon, KindGlyph } from "../icons";
import { SettingRow } from "./setting-row";
import { confirmDialog } from "../../lib/confirm-dialog";

/** Re-reads sign-in state every few seconds while the page is open (a sign-in may be running
 *  in the floating terminal). */
const [tick, setTick] = createSignal(0);

function AccountRow(props: { agent: AccountAgent; account: AgentAccount | null }) {
  const [status] = createResource(() => [props.account?.dir ?? null, tick()] as const, ([dir]) => accountStatus(props.agent, dir).catch(() => null));
  const active = () => (activeAccount(props.agent)?.id ?? null) === (props.account?.id ?? null);
  const running = () => Object.values(store.sessions).filter((s) => s.alive && s.kind === props.agent && (s.env?.[props.agent === "claude" ? "CLAUDE_CONFIG_DIR" : "CODEX_HOME"] ?? null) === (props.account?.dir ?? null)).length;
  return (
    <div class={`account-row${active() ? " active" : ""}`} data-account={props.account?.id ?? "default"}>
      <div class="account-main">
        <div class="account-name">
          <Show when={props.account} fallback={<span>{t("Default")}</span>}>
            {(a) => <input class="text-input" value={a().label} onChange={(e) => renameAccount(a().id, e.currentTarget.value)} />}
          </Show>
          <Show when={active()}><span class="badge account-active">{t("Active")}</span></Show>
        </div>
        <div class="account-sub">
          <Show when={status()} fallback={<span>{t("Checking…")}</span>}>
            {(st) => (
              <Show when={st().signed_in} fallback={<span class="account-out">{t("Not signed in")}</span>}>
                <span class="account-in">{st().email ? t("Signed in as {email}", { email: st().email! }) : t("Signed in")}{st().plan ? ` · ${st().plan}` : ""}</span>
              </Show>
            )}
          </Show>
          <Show when={running()}><span> · {t("{n} running", { n: running() })}</span></Show>
          <span class="account-home" title={status()?.home}>{props.account ? props.account.dir : props.agent === "claude" ? "~/.claude" : "~/.codex"}</span>
        </div>
      </div>
      <button class="btn" onClick={() => signIn(props.agent, props.account)}>{status()?.signed_in ? t("Sign in again") : t("Sign in")}</button>
      <button class="btn" disabled={active()} onClick={() => setActiveAccount(props.agent, props.account?.id ?? null)}>{t("Use for new sessions")}</button>
      <Show when={props.account}>
        {(a) => (
          <button class="icon-btn" title={t("Remove account (its sign-in and history go to the trash)")}
            onClick={async () => {
              if (!(await confirmDialog(t("Remove the {agent} account “{name}”? Its sign-in, settings and conversation history move to the trash.", { agent: kindInfo(props.agent).label, name: a().label })))) return;
              void removeAccount(a()).catch((e) => store.notify(t("Could not remove the account: {error}", { error: String(e) })));
            }}>
            <Icon name="close" size={13} />
          </button>
        )}
      </Show>
    </div>
  );
}

function AgentAccounts(props: { agent: AccountAgent }) {
  const [name, setName] = createSignal<string | null>(null);
  const add = async () => {
    const label = name()?.trim();
    if (!label) return;
    setName(null);
    try {
      const acct = await addAccount(props.agent, label);
      signIn(props.agent, acct);
    } catch (e) {
      store.notify(t("Could not add the account: {error}", { error: String(e) }));
    }
  };
  return (
    <div class="accounts-agent" data-agent={props.agent}>
      <div class="accounts-head"><KindGlyph kind={props.agent} size={16} /> {kindInfo(props.agent).label}</div>
      <AccountRow agent={props.agent} account={null} />
      <For each={accountsOf(props.agent)}>{(a) => <AccountRow agent={props.agent} account={a} />}</For>
      <Show
        when={name() !== null}
        fallback={<button class="btn accounts-add" onClick={() => setName("")}><Icon name="plus" size={13} /> {t("Add {agent} account", { agent: kindInfo(props.agent).label })}</button>}
      >
        <div class="layout-custom">
          <input class="text-input" placeholder={t("Name, e.g. Work")} value={name()!} ref={(el) => queueMicrotask(() => el.focus())}
            onInput={(e) => setName(e.currentTarget.value)} onKeyDown={(e) => { if (e.key === "Enter") void add(); if (e.key === "Escape") setName(null); }} />
          <button class="btn primary" disabled={!name()!.trim()} onClick={() => void add()}>{t("Add and sign in")}</button>
        </div>
      </Show>
    </div>
  );
}

export function AccountSettings() {
  onMount(() => {
    const timer = setInterval(() => setTick((n) => n + 1), 3000);
    onCleanup(() => clearInterval(timer));
  });
  return (
    <SettingRow id="accounts" wide>
      <div class="accounts">
        <For each={ACCOUNT_AGENTS}>{(agent) => <AgentAccounts agent={agent} />}</For>
      </div>
    </SettingRow>
  );
}
