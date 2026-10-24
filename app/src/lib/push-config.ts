// The shape of the daemon's notification settings (`notify.get`, crates/kelpied/src/notify.rs) and
// the plain-language summary under the settings: what is on, and what is missing for it to work.
import { t } from "./i18n";

export interface PushConfig {
  ntfy: { enabled: boolean; server: string };
  telegram: { enabled: boolean; chat_id: string };
  webhook: { enabled: boolean };
  events: { needs_you: boolean; done: boolean; failed: boolean; automation: boolean; approval: boolean; cost_cap: boolean };
  /** A finished turn shorter than this many seconds is not sent. */
  min_turn_s: number;
  away_only: boolean;
  include_screen: boolean;
  /** A link to the session in the phone app: it holds this computer's address on the network. */
  phone_link: boolean;
}

/** Which secrets the daemon holds: values never leave it. */
export interface PushSecrets {
  ntfy_topic: boolean;
  ntfy_token: boolean;
  telegram_token: boolean;
  webhook_url: boolean;
}

export interface PushState {
  config: PushConfig;
  secrets: PushSecrets;
  status?: Partial<Record<"ntfy" | "telegram" | "webhook", { ok_at: number | null; error: string | null }>>;
  /** Messages not sent because they queued up behind a server that did not answer. */
  dropped?: number;
}

/** What the daemon starts with (the same defaults as `Config::default`). */
export function emptyPushConfig(): PushConfig {
  return {
    ntfy: { enabled: false, server: "https://ntfy.sh" },
    telegram: { enabled: false, chat_id: "" },
    webhook: { enabled: false },
    events: { needs_you: true, done: true, failed: true, automation: true, approval: true, cost_cap: true },
    min_turn_s: 30,
    away_only: false,
    include_screen: false,
    phone_link: true,
  };
}

/** What a channel that is switched on still lacks to send anything. */
export function pushProblems(cfg: PushConfig, secrets: PushSecrets): string[] {
  const out: string[] = [];
  if (cfg.telegram.enabled && !secrets.telegram_token) out.push(t("Telegram needs a bot token."));
  if (cfg.telegram.enabled && !cfg.telegram.chat_id) out.push(t("Telegram needs a chat id."));
  if (cfg.webhook.enabled && !secrets.webhook_url) out.push(t("The webhook needs an address."));
  return out;
}

export function pushSummary(cfg: PushConfig, secrets: PushSecrets): string {
  const channels = [cfg.ntfy.enabled && "ntfy", cfg.telegram.enabled && "Telegram", cfg.webhook.enabled && t("a webhook")].filter(Boolean) as string[];
  if (!channels.length) return t("Nothing is sent off this computer.");
  const events = Object.values(cfg.events).some(Boolean) ? "" : " " + t("No event is selected, so nothing will be sent.");
  return [t("Sending to {channels}.", { channels: channels.join(", ") }) + events, ...pushProblems(cfg, secrets)].join(" ");
}
