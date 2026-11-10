// Settings → Mobile, the relay's row: where the relay is (Kelpie's own, or another), the registration key that makes this computer's room
// there once when the relay asks for one, how the connection stands, whether it serves the phone app, a new room when the relay stays slow,
// and what the relay can see. The switch is the row's (`OptionRow`); turning it on asks the person to verify, at the daemon
// (crates/kelpied/src/mobile/relay.rs).
import { Show, createSignal } from "solid-js";
import { mobileSecure } from "../../lib/ipc";
import { errorText, t } from "../../lib/i18n";
import { HOSTED_API } from "../../lib/attention";
import { confirmDialog } from "../../lib/confirm-dialog";
import { asksForKey, hostedLine, hostedReason, movingRelayWarning, relayServesApp, roomNeedsNothing } from "../../lib/mobile-options";
import type { MobileRelayHosted } from "../../lib/types";

/** `relay` in `mobile.status` (absent below `RELAY_API`; the hosted app's fields absent below `HOSTED_API`). */
export interface RelayStatus extends Partial<MobileRelayHosted> {
  on: boolean;
  url: string;
  default_url: string;
  /** This computer has a room there. */
  room: boolean;
  /** The Tailscale page origin phones installed over Tailscale are let in from. */
  origin: string | null;
  /** The registration key is kept, so a new room needs nothing typed. */
  operator_key: boolean;
  connected: boolean;
  error: string | null;
  rtt_ms: number | null;
  /** The round trip has stayed above the limit for a day. */
  slow: boolean;
  /** Phones that hold a token. */
  phones: number;
}

export type RelayForm = ReturnType<typeof createRelayForm>;

/** What the row's fields hold, and what its switch and button do. `run` is the page's: busy, errors, then the status again. */
export function createRelayForm(run: (fn: () => Promise<unknown>) => Promise<void>) {
  // null: the address as the daemon holds it.
  const [url, setUrl] = createSignal<string | null>(null);
  const [key, setKey] = createSignal("");
  // The relay answered that it needs a registration key: its field shows from then on.
  const [keyAsked, setKeyAsked] = createSignal(false);
  const turn = async (on: boolean, status: RelayStatus | undefined) => {
    if (!on) return run(() => mobileSecure("mobile.set_relay", { on: false }));
    const address = (url() ?? status?.url ?? "").trim();
    // Phones on the hosted app reach this computer only through this relay: the person hears so before verifying.
    const moving = movingRelayWarning(status, address);
    if (moving && !(await confirmDialog(`${moving} ${t("Use this relay anyway?")}`))) return;
    await run(async () => {
      const operator = key().trim();
      try {
        await mobileSecure("mobile.set_relay", { on: true, url: address, ...(operator ? { operator_key: operator } : {}) });
      } catch (e) {
        if (asksForKey(e)) setKeyAsked(true);
        throw e;
      }
      setKey("");
      setUrl(null);
    });
  };
  const recreate = () => run(() => mobileSecure("mobile.relay_recreate", {}));
  return { url, setUrl, key, setKey, keyAsked, turn, recreate };
}

export function RelayDetails(props: { status: RelayStatus; form: RelayForm; busy: boolean; api: number }) {
  const s = () => props.status;
  const address = () => props.form.url() ?? s().url;
  const custom = () => address().trim() !== s().default_url;
  const hosted = () => props.api >= HOSTED_API && !!s().hosted;
  // A room is made when there is none at this address. It needs the key unless the key is kept; a daemon that makes rooms without one asks for it only
  // once the relay said it needs one.
  const needsKey = () => (!hosted() || props.form.keyAsked()) && !s().operator_key && (!s().room || address().trim() !== s().url);
  const phones = () =>
    s().phones === 0
      ? hosted()
        ? t("No phone has a token yet: a phone gets one when it pairs on the hosted app, or the next time it connects over Tailscale.")
        : t("No phone has a token yet: each gets one the next time it connects over Tailscale.")
      : s().phones === 1
        ? t("1 phone can use it.")
        : t("{n} phones can use it.", { n: s().phones });
  // Phones installed over Tailscale are let in from its origin only. With the phone app served or being checked, that is not needed to use the relay.
  const noOrigin = () => !s().origin && (!hosted() || (!s().hosted!.available && hostedReason(s()) !== null));
  return (
    <div class="relay-details" data-relay={s().on ? "on" : "off"}>
      <Show when={s().on} fallback={
        <>
          <div class="inline-fields relay-field">
            <label class="muted-inline" for="relay-url">{t("Relay address")}</label>
            <input id="relay-url" class="text-input mono" style={{ width: "340px" }} data-relay-url value={address()} placeholder={s().default_url} spellcheck={false} onInput={(e) => props.form.setUrl(e.currentTarget.value)} />
            <Show when={custom()}><button class="link-btn" data-relay-default onClick={() => props.form.setUrl(s().default_url)}>{t("Use Kelpie's relay")}</button></Show>
          </div>
          <Show when={custom()}><div class="form-note">{t("Another relay: one you run yourself, or someone you trust runs. It sees what Kelpie's relay would, and nothing more.")}</div></Show>
          <Show when={needsKey()}>
            <div class="inline-fields relay-field">
              <label class="muted-inline" for="relay-key">{t("Registration key")}</label>
              <input id="relay-key" class="text-input" style={{ width: "340px" }} type="password" autocomplete="off" data-relay-key value={props.form.key()} onInput={(e) => props.form.setKey(e.currentTarget.value)} />
            </div>
            <div class="form-note">{t("Whoever runs the relay gives you this key. It is needed once, to make this computer's room there, and is kept in this computer's credential store.")}</div>
          </Show>
        </>
      }>
        <div class="muted-inline" data-relay-state={s().connected ? "connected" : "down"}>
          {s().connected ? (s().rtt_ms !== null ? t("Connected to the relay ({ms} ms there and back).", { ms: s().rtt_ms! }) : t("Connected to the relay.")) : t("Not connected to the relay right now. Kelpie keeps trying.")}
          {" "}{phones()}
        </div>
        <Show when={s().error}><div class="form-note error" data-relay-error>{errorText(s().error)}</div></Show>
        <Show when={hosted()}>
          <div class="muted-inline" data-relay-hosted={s().hosted!.available ? "available" : "not"}>{hostedLine(s())}</div>
          <Show when={hostedReason(s())}><div class="muted-inline" data-relay-hosted-reason>{errorText(hostedReason(s()))}</div></Show>
        </Show>
        <Show when={noOrigin()}><div class="form-note warn-note" data-relay-no-origin>{t("Turn on Tailscale mode once, so the relay knows the app's address: phones are let in from there only.")}</div></Show>
        <Show when={s().slow}><div class="form-note warn-note" data-relay-slow>{t("The relay has been slow from this computer for a day. A new room is made near where this computer is now, and phones move to it by themselves.")}</div></Show>
        <div class="inline-fields">
          <button class="btn" data-relay-recreate disabled={props.busy || !roomNeedsNothing(s(), props.api)} onClick={() => void props.form.recreate()}>{t("Make a new room")}</button>
        </div>
      </Show>
      <details class="relay-sees" data-relay-sees>
        <summary>{t("What the relay can see")}</summary>
        <ul class="form-note">
          <li>{t("That a phone and this computer are connected, and when.")}</li>
          <li>{t("Their internet addresses, and how much they send.")}</li>
          <li>{t("The number of this computer's room.")}</li>
          <Show when={hosted() && relayServesApp(s())} fallback={<li>{t("Never what is said: it is encrypted between the phone and this computer, and the relay holds no key to it. It keeps no logs of what passes, and serves no page.")}</li>}>
            <li>{t("Never what is said: it is encrypted between the phone and this computer, and the relay holds no key to it. It keeps no logs of what passes, and serves the phone app: see What this trusts, under Hosted phone app.")}</li>
          </Show>
        </ul>
      </details>
    </div>
  );
}

/** Asked after a phone was removed with the relay on. No is the answer unless the person makes a room: "Not now" comes first and has the focus. */
export function NewRoomOffer(props: { name: string; busy: boolean; onMake: () => void; onSkip: () => void }) {
  return (
    <div class="form-note warn-note" data-relay-room-offer>
      <div>{t("“{name}” was removed. It still knows this computer's room at the relay, though the relay no longer lets it in. Make a new room too?", { name: props.name })}</div>
      <div>{t("Phones connected now move to the new room by themselves, over their encrypted link. A phone not connected now cannot use the relay until it connects once over Tailscale, and may need to be paired again. Making a room asks you to verify.")}</div>
      <div class="inline-fields" style={{ "margin-top": "6px" }}>
        <button class="btn" data-relay-room-skip autofocus ref={(b) => queueMicrotask(() => b.focus())} onClick={() => props.onSkip()}>{t("Not now")}</button>
        <button class="btn" data-relay-room-make disabled={props.busy} onClick={() => props.onMake()}>{t("Make a new room")}</button>
      </div>
    </div>
  );
}
