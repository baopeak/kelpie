// What stands in front of the app while it is locked, the cover that goes up when the page is hidden, and the sheet that asks for the passkey again over a dangerous approval.
import { Show } from "solid-js";
import { CommandBlock } from "./command-block";
import { t } from "./i18n";
import { connected, lockGo, lockShown, recheckAsked } from "./phone-state";

function words(face: string): { title: string; text: string } {
  switch (face) {
    case "enrol":
      return { title: t("Set up the app lock"), text: t("Kelpie opens only after Face ID, a fingerprint or your phone's passcode. They stay on this phone: your computer only checks the signature.") };
    case "unlock":
      return { title: t("Kelpie is locked"), text: t("Verify that it is you to see your sessions.") };
    case "desktop":
      return { title: t("Press the button on the computer"), text: t("Setting up the app lock starts on your computer, so a copied phone key cannot do it. Press \"Reset this phone's lock\" next to this phone in Kelpie → Settings → Mobile, then set it up here within five minutes.") };
    case "closed":
      return { title: t("Reading without a passkey is off"), text: t("This computer does not allow reading on a page that cannot have a passkey. Open Kelpie through Tailscale, or change this in Kelpie → Settings → Mobile.") };
    case "no_name":
      return { title: t("A passkey needs a name"), text: t("This phone was paired at an address that is a number, and a passkey needs a name. Pair it again from Kelpie → Settings → Mobile, or turn the app lock off there.") };
    case "broken":
      return { title: t("Reset this phone's lock"), text: t("Your computer cannot check this phone's passkey. Press \"Reset this phone's lock\" in Kelpie → Settings → Mobile on the computer, then set up a new one here.") };
    default:
      return { title: t("Kelpie is locked"), text: t("Reaching your computer. It decides when this opens.") };
  }
}

export function LockScreen() {
  const s = () => lockShown();
  const can = () => s().face === "unlock" || s().face === "enrol";
  return (
    <div class="lock-screen" data-lock-screen={s().face}>
      <h2>{words(s().face).title}</h2>
      <p class="muted">{words(s().face).text}</p>
      <Show when={can()}>
        <button class="btn primary" data-lock-go disabled={s().asking || !connected()} onClick={() => void lockGo()}>
          {s().asking ? t("Asking…") : s().face === "enrol" ? t("Set up") : t("Unlock")}
        </button>
      </Show>
      <Show when={can() && !connected()}><p class="muted" role="status">{t("Waiting for the connection to your computer.")}</p></Show>
      <Show when={s().error}><p class="muted err" role="status" data-lock-error>{s().error}</p></Show>
      <Show when={s().face === "unlock"}>
        <p class="muted lock-help">{t("Face ID was reset, or this is a new phone? Press \"Reset this phone's lock\" in Kelpie → Settings → Mobile on the computer.")}</p>
      </Show>
    </div>
  );
}

/** Opaque, and up the moment the page is hidden: what the app switcher takes a picture of. */
export function LockCover() {
  return <Show when={lockShown().covered}><div class="lock-cover" data-lock-cover /></Show>;
}

/** The computer wants the passkey for this approval. The button is a tap of the person's own, which a ceremony needs. */
export function RecheckSheet() {
  return (
    <Show when={recheckAsked()}>
      {(asked) => (
        <div class="sheet-back">
          <div class="sheet" role="dialog" aria-label={t("Confirm this approval")} data-recheck>
            <h2>{t("Confirm this approval")}</h2>
            <p class="muted">{t("You asked to verify again for dangerous commands. This is what you would approve:")}</p>
            <CommandBlock command={asked().command} risk={{ level: "dangerous", tags: [] }} />
            <Show when={asked().error}><p class="muted err" role="status">{asked().error}</p></Show>
            <div class="actions">
              <button class="btn primary" data-recheck-go onClick={() => asked().go()}>{t("Verify")}</button>
              <button class="btn" data-recheck-cancel onClick={() => asked().cancel()}>{t("Cancel")}</button>
            </div>
          </div>
        </div>
      )}
    </Show>
  );
}
