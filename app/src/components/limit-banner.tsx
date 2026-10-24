// The strip above a pane whose agent the subscription's usage limit stopped: continue by itself
// when the limit lifts, move to another account, or hand the work to another agent. Shown only
// then (the daemon sets `limited`), and loaded on first use.
import { Show, createSignal, onCleanup, onMount } from "solid-js";
import { store, sessionLabel } from "../lib/store";
import { accountsOf, isAccountAgent, switchSessionAccount, sessionAccount } from "../lib/accounts";
import { cancelContinue, scheduleContinue, scheduled, usageFor } from "../lib/limits";
import { nextClockTime, resetTime } from "../lib/limit-time";
import { handOffSession } from "../lib/agent-actions";
import { isAgentKind, kindInfo, launchableAgents } from "../lib/agents";
import { openContextMenu, type MenuItem } from "./context-menu";
import { Icon } from "./icons";
import { formatTime, t } from "../lib/i18n";
import type { SessionInfo } from "../lib/types";
import "./limit-banner.css";

export default function LimitBanner(props: { s: SessionInfo }) {
  /** undefined: still asking; null: the meter does not say when the limit lifts. */
  const [reset, setReset] = createSignal<number | null | undefined>(undefined);
  const [clock, setClock] = createSignal("");
  const timer = () => scheduled[props.s.id];

  const check = async () => {
    setReset(undefined);
    setReset(await usageFor(props.s).then(resetTime, () => null));
  };
  onMount(() => {
    void check();
    // The meter moves on: a limit reached later in the day gets its time when it is next asked.
    const every = window.setInterval(() => reset() == null && void check(), 60_000);
    onCleanup(() => clearInterval(every));
  });

  const switchMenu = (e: MouseEvent) => {
    const s = props.s;
    const current = sessionAccount(s)?.id ?? null;
    const items: MenuItem[] = [];
    if (isAccountAgent(s.kind)) {
      if (current !== null) items.push({ label: t("Default"), run: () => void switchSessionAccount(s, null) });
      for (const a of accountsOf(s.kind).filter((x) => x.id !== current)) items.push({ label: a.label, run: () => void switchSessionAccount(s, a) });
    }
    if (!items.length) items.push({ label: t("No other account: add one in Settings → Accounts"), run: () => store.openSettings("accounts") });
    openContextMenu(e, items);
  };

  const handOffMenu = (e: MouseEvent) => {
    const s = props.s;
    const items: MenuItem[] = launchableAgents(store.state.settings.hiddenAgents)
      .filter((k) => k !== s.kind)
      .map((k) => ({ label: kindInfo(k).label, run: () => void handOffSession(s, k) }));
    openContextMenu(e, items);
  };

  const confirmed = () => {
    const [hh, mm] = clock().split(":").map(Number);
    if (!Number.isInteger(hh) || !Number.isInteger(mm)) return;
    void scheduleContinue(props.s, nextClockTime(hh, mm), "confirmed");
  };

  return (
    <div class="limit-banner" role="status" data-limit-banner={props.s.id}>
      <Icon name="alert" size={13} />
      <span class="limit-text">
        <Show when={timer()} fallback={t("{name} hit a usage or rate limit.", { name: sessionLabel(props.s) })}>
          {t("Will type “continue” at {time}.", { time: formatTime(timer()!) })}
        </Show>
      </span>
      <span class="limit-actions">
        <Show
          when={timer()}
          fallback={
            <Show
              when={reset()}
              fallback={
                <Show when={reset() === null}>
                  <input class="text-input limit-time" type="time" value={clock()} title={t("The time the limit lifts (your local time)")} onInput={(e) => setClock(e.currentTarget.value)} />
                  <button class="btn mini" disabled={!clock()} onClick={confirmed}>{t("Continue at this time")}</button>
                </Show>
              }
            >
              {(at) => <button class="btn mini primary" data-continue-at-reset onClick={() => void scheduleContinue(props.s, at(), "meter")}>{t("Continue at reset ({time})", { time: formatTime(at()) })}</button>}
            </Show>
          }
        >
          <button class="btn mini" data-cancel-continue onClick={() => void cancelContinue(props.s)}>{t("Cancel")}</button>
        </Show>
        <Show when={isAccountAgent(props.s.kind) && !props.s.remote}>
          <button class="btn mini" onClick={switchMenu}>{t("Switch account…")}</button>
        </Show>
        <Show when={props.s.project_id && isAgentKind(props.s.kind)}>
          <button class="btn mini" onClick={handOffMenu}>{t("Hand off to…")}</button>
        </Show>
      </span>
    </div>
  );
}
