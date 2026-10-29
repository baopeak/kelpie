// Words for the connection state. "Your connection is slow" and "the computer is busy" are different
// things and read differently; so are "no network" and "the computer does not answer".
import type { ConnState } from "./conn-state";
import { intlLocale, t } from "./i18n";

export type Tone = "ok" | "warn" | "bad" | "idle";

export const clock = (ms: number, seconds = false) =>
  new Date(ms).toLocaleTimeString(intlLocale(), { hour: "2-digit", minute: "2-digit", ...(seconds ? { second: "2-digit" } : {}) });

/** "2 min ago" for a time in the past. */
export function ago(ms: number, at = Date.now()): string {
  const s = Math.max(0, Math.round((at - ms) / 1000));
  if (s < 10) return t("just now");
  if (s < 60) return t("{n} s ago", { n: s });
  if (s < 3600) return t("{n} min ago", { n: Math.floor(s / 60) });
  return t("{n} h ago", { n: Math.floor(s / 3600) });
}

/** Why the computer is busy, in a sentence. */
export function busyText(reasons: string[]): string {
  if (reasons.includes("memory")) return t("The computer is low on memory, so updates are slower. Typing still works.");
  if (reasons.includes("thermal") || reasons.includes("battery_saver")) return t("The computer is hot or in battery saver, so updates are slower. Typing still works.");
  return t("The computer is busy, so updates are slower. Typing still works.");
}

export function describeConn(s: ConnState): { label: string; detail: string; tone: Tone } {
  switch (s.kind) {
    case "connecting":
      return { label: t("Connecting…"), detail: t("Reaching Kelpie on your computer."), tone: "idle" };
    case "live":
      return { label: t("Live"), detail: t("Connected to Kelpie on your computer."), tone: "ok" };
    case "degraded":
      return s.why === "slow"
        ? { label: t("Slow connection"), detail: t("Your connection is slow, so what you see may be a little behind."), tone: "warn" }
        : { label: t("Waiting for an answer"), detail: t("Your computer has not answered yet. Checking the connection."), tone: "warn" };
    case "reconnecting":
      return {
        label: t("Reconnecting (try {n})", { n: s.attempt }),
        detail: s.nextInS === null ? t("Trying now. What you see is the last state.") : t("Next try in {s} s. What you see is the last state.", { s: s.nextInS }),
        tone: "bad",
      };
    case "no_network":
      return { label: t("No network"), detail: t("Your phone is offline. What you see is the last state; this reconnects by itself when the network is back."), tone: "bad" };
    case "sign_in":
      return { label: t("Sign-in needed"), detail: t("This network wants you to sign in first (a Wi-Fi login page). Do that, then come back."), tone: "bad" };
    case "computer_offline":
      return {
        label: s.since ? t("Computer offline since {time}", { time: clock(s.since) }) : t("Computer offline"),
        detail: t("Your computer does not answer. It may be asleep, off, or on another network. What you see is the last state."),
        tone: "bad",
      };
    case "relay_unreachable":
      return { label: t("Relay unreachable"), detail: t("The relay cannot be reached. What you see is the last state."), tone: "bad" };
    case "restarted":
      return { label: t("Computer restarted"), detail: t("Kelpie restarted on your computer. Sessions were loaded again."), tone: "warn" };
    case "busy":
      return { label: t("Computer busy"), detail: busyText(s.reasons), tone: "warn" };
    case "replaced":
      return { label: t("Open in another window"), detail: t("This device is connected in another window. Tap to use it here."), tone: "idle" };
    case "unpaired":
      return { label: t("Not paired"), detail: t("This device is no longer paired."), tone: "bad" };
    case "outdated":
      return { label: t("Update Kelpie"), detail: t("Kelpie on your computer is older than this app needs. Update it, then reload."), tone: "bad" };
  }
}
