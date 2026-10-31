import { beforeAll, describe, expect, it, vi } from "vitest";
import { applyLocale, errorText, locale } from "./i18n";

const SOURCES = import.meta.glob<string>("../../../crates/kelpied/src/{user_presence,mobile}.rs", { query: "?raw", import: "default", eager: true });

/** What the daemon tells a person about a phone-access decision: its own fixed English, which the app translates. */
const MESSAGES: [string, string][] = [
  ["user_presence.rs", "Kelpie is already asking you to verify: answer that prompt first."],
  ["user_presence.rs", "An earlier verification dialog is still open: close the earlier dialog first. If there is no such dialog, restart Kelpie's background service."],
  ["user_presence.rs", "The verification was not answered in time."],
  ["user_presence.rs", "The verification failed: please try again."],
  ["user_presence.rs", "You refused or did not answer the last 3 verifications, so Kelpie will not ask again for a few minutes. If you did not start them, turn off phone access and revoke all (that needs no verification)."],
  ["user_presence.rs", "Verification was cancelled."],
  ["user_presence.rs", "That is not the password of the account signed in to this computer."],
  ["user_presence.rs", "That is not the account signed in to this computer."],
  ["user_presence.rs", "Windows would not sign that account in here, so it could not confirm you. Set up Windows Hello or check the account."],
  ["user_presence.rs", "Windows could not check what was entered: please try again."],
  ["mobile.rs", "Removed for now, but this could not be saved: it will come back after a restart unless this is fixed."],
  ["mobile.rs", "Changed for now, but this could not be saved: the phone will have its old permission after a restart unless this is fixed."],
  ["mobile.rs", "Turned off for now, but this could not be saved: it will come back on after a restart unless this is fixed."],
];

describe("what the daemon says about phone access, in the app's language", () => {
  beforeAll(() => {
    vi.stubGlobal("document", { documentElement: {} });
  });

  it.each(MESSAGES)("%s: %s", (file, message) => {
    expect(SOURCES[`../../../crates/kelpied/src/${file}`], "the daemon still says it").toContain(message);
    applyLocale("en");
    expect(locale()).toBe("en");
    expect(errorText(message)).toBe(message);
    applyLocale("vi");
    const shown = errorText(`Error: ${message}`);
    expect(shown).not.toContain(message);
    expect(shown).toMatch(/[^\x00-\x7f]/);
    applyLocale("en");
  });
});
