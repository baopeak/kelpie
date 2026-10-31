// The steps before pairing on a secure origin: install the app, then scan the pairing code inside it (or paste the link).
import { createSignal, For, onCleanup, Show } from "solid-js";
import { t } from "./i18n";
import { firstScreen, platformOf, readCode, type CodeReading, type FirstScreen } from "./install-state";
import { startScan, type Scanner } from "./qr-scan";
import { store } from "./storage";

interface InstallPromptEvent extends Event {
  prompt(): Promise<void>;
  userChoice: Promise<{ outcome: "accepted" | "dismissed" }>;
}

const STAY = "kelpie-stay-in-browser";
const [installPrompt, setInstallPrompt] = createSignal<InstallPromptEvent | null>(null);
const [installed, setInstalled] = createSignal(false);
const [stayInBrowser, setStayInBrowser] = createSignal(store.get(STAY) === "1");

/** Chrome offers installing once, early: kept from the start of the page until the person asks. */
export function watchInstall() {
  window.addEventListener("beforeinstallprompt", (e) => {
    e.preventDefault();
    setInstallPrompt(e as InstallPromptEvent);
  });
  window.addEventListener("appinstalled", () => {
    setInstallPrompt(null);
    setInstalled(true);
  });
}

function runsInstalled(): boolean {
  return matchMedia("(display-mode: standalone)").matches || (navigator as Navigator & { standalone?: boolean }).standalone === true;
}

export function screenNow(paired: boolean, hasCode: boolean): FirstScreen {
  return firstScreen({ https: location.protocol === "https:", installed: runsInstalled(), paired, hasCode, stayInBrowser: stayInBrowser() });
}

function codeProblem(r: CodeReading & { ok: false }): string {
  if (r.why === "install_code") return t("That is the code of the first step. Press Step 2: Pair in Kelpie on your computer and scan the code it shows then.");
  if (r.why === "other_address") return t("This code is for {host}, and this app belongs to another address. Show a new code in Kelpie on your computer.", { host: r.host ?? "" });
  return t("That is not a Kelpie pairing code. Show a new one in Kelpie on your computer.");
}

/** How to install, for this kind of phone. */
export function InstallGuide() {
  const platform = platformOf(navigator.userAgent, navigator.maxTouchPoints ?? 0);
  const install = async () => {
    const e = installPrompt();
    if (!e) return;
    await e.prompt();
    const { outcome } = await e.userChoice;
    setInstallPrompt(null);
    if (outcome === "accepted") setInstalled(true);
  };
  const stay = () => {
    store.set(STAY, "1");
    setStayInBrowser(true);
  };
  const iosSteps = () => [
    t("Tap the Share button in Safari."),
    t("Tap Add to Home Screen, then Add."),
    t("Open Kelpie from your Home Screen and pair there."),
  ];
  return (
    <div class="card" data-install={platform}>
      <h2>{t("Install Kelpie on this phone")}</h2>
      <p class="muted">{t("Pairing happens inside the installed app. Your phone keeps what the app stores apart from this browser, so a pairing made here would not be there.")}</p>
      <Show when={platform === "ios"}>
        <ol class="steps"><For each={iosSteps()}>{(s) => <li>{s}</li>}</For></ol>
      </Show>
      <Show when={platform !== "ios"}>
        <Show when={!installed()} fallback={<p data-installed>{t("Installed. Open Kelpie from your home screen and pair there.")}</p>}>
          <Show when={installPrompt()} fallback={<p class="muted">{t("In your browser's menu choose Install app or Add to Home screen, then open Kelpie from your home screen.")}</p>}>
            <p><button class="btn primary" data-install-now onClick={() => void install()}>{t("Install")}</button></p>
          </Show>
        </Show>
      </Show>
      <div class="actions"><button class="btn" data-stay-in-browser onClick={stay}>{t("Continue in this browser")}</button></div>
    </div>
  );
}

/** The pairing code: from the camera, or pasted. `done` gets the code to pair with. */
export function ScanCode(props: { done: (code: string) => void; message: string | null }) {
  let video!: HTMLVideoElement;
  let scanner: Scanner | null = null;
  const [scanning, setScanning] = createSignal(false);
  const [problem, setProblem] = createSignal<string | null>(null);
  const [text, setText] = createSignal("");
  const stop = () => {
    scanner?.stop();
    scanner = null;
    setScanning(false);
  };
  onCleanup(stop);
  const take = (value: string) => {
    const r = readCode(value, location.host);
    if (r.ok) {
      stop();
      props.done(r.code);
      return true;
    }
    setProblem(codeProblem(r));
    return false;
  };
  const scan = async () => {
    setProblem(null);
    setScanning(true);
    const started = await startScan(video, (seen) => void take(seen));
    if (!scanning()) {
      started?.stop();
      return;
    }
    scanner = started;
    if (!started) {
      setScanning(false);
      setProblem(t("The camera could not be used here. Paste the pairing link instead: copy it from Kelpie on your computer."));
    }
  };
  return (
    <div class="card" data-scan>
      <h2>{t("Pair this device")}</h2>
      <p class="muted">{props.message ?? t("In Kelpie on your computer open Settings → Mobile, press Step 2: Pair, and scan the code it shows.")}</p>
      <video ref={video} class="scan-view" classList={{ on: scanning() }} playsinline muted />
      <div class="actions">
        <Show when={!scanning()} fallback={<button class="btn" data-scan-stop onClick={stop}>{t("Stop the camera")}</button>}>
          <button class="btn primary" data-scan-start onClick={() => void scan()}>{t("Scan the code")}</button>
        </Show>
      </div>
      <p class="muted">{t("Or paste the pairing link:")}</p>
      <input class="field" data-paste autocomplete="off" autocapitalize="off" spellcheck={false} value={text()} onInput={(e) => setText(e.currentTarget.value)} />
      <div class="actions"><button class="btn" data-paste-go disabled={!text().trim()} onClick={() => void take(text())}>{t("Use this link")}</button></div>
      <Show when={problem()}><p class="err" role="status" data-scan-problem>{problem()}</p></Show>
    </div>
  );
}
