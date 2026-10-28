import { describe, expect, it } from "vitest";
import { emptyPushConfig, pushProblems, pushSummary, type PushSecrets } from "./push-config";

const none: PushSecrets = { ntfy_topic: false, ntfy_token: false, telegram_token: false, webhook_url: false };

describe("pushSummary", () => {
  it("says nothing is sent until a channel is on", () => {
    expect(pushSummary(emptyPushConfig(), none)).toBe("Nothing is sent off this computer.");
  });

  it("lists the channels that are on", () => {
    const c = emptyPushConfig();
    c.ntfy.enabled = true;
    expect(pushSummary(c, none)).toBe("Sending to ntfy.");
    c.telegram = { enabled: true, chat_id: "-100" };
    expect(pushSummary(c, { ...none, telegram_token: true })).toBe("Sending to ntfy, Telegram.");
  });

  it("says what a channel still lacks", () => {
    const c = emptyPushConfig();
    c.telegram.enabled = true;
    c.webhook.enabled = true;
    expect(pushProblems(c, none)).toEqual(["Telegram needs a bot token.", "Telegram needs a chat id.", "The webhook needs an address."]);
    expect(pushProblems(c, { ...none, telegram_token: true, webhook_url: true })).toEqual(["Telegram needs a chat id."]);
    expect(pushSummary(c, none)).toContain("Telegram needs a bot token.");
  });

  it("warns when no event is selected", () => {
    const c = emptyPushConfig();
    c.ntfy.enabled = true;
    c.events = { needs_you: false, done: false, failed: false, automation: false, approval: false, cost_cap: false };
    expect(pushSummary(c, none)).toContain("No event is selected");
  });
});

describe("emptyPushConfig", () => {
  it("starts quiet: no channel, no screen text, short turns skipped", () => {
    const c = emptyPushConfig();
    expect([c.ntfy.enabled, c.telegram.enabled, c.webhook.enabled, c.include_screen, c.away_only]).toEqual([false, false, false, false, false]);
    expect(c.phone_link).toBe(true);
    expect(c.min_turn_s).toBe(30);
    expect(c.ntfy.server).toBe("https://ntfy.sh");
  });
});
