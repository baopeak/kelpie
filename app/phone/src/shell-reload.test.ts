import { describe, expect, it } from "vitest";
import { relayRefused, reloadOnce, ShellReload, SHELL_RELOAD_GAP_MS, type ReloadEnv } from "./shell-reload";

const RELAY = "wss://relay.example/v1/phone?t=abc&k=def";

/** A tab: its `sessionStorage` survives each reload, as a browser's does. */
function tab(opts: { storage?: boolean } = {}) {
  let kept: string | null = null;
  let clock = 1_000_000;
  const env: ReloadEnv & { reloads: number; advance(ms: number): void } = {
    reloads: 0,
    now: () => clock,
    read: () => {
      if (opts.storage === false) throw new Error("no storage");
      return kept;
    },
    write: (value) => {
      if (opts.storage === false) throw new Error("no storage");
      kept = value;
    },
    reload: () => void env.reloads++,
    advance: (ms) => void (clock += ms),
  };
  return env;
}

describe("a page that keeps an old CSP", () => {
  it("loads again once when it refuses the relay, and never again in that tab however often it refuses it", () => {
    const env = tab();
    // Each reload starts a new page, with a new ShellReload: only the tab's storage carries over.
    for (let i = 0; i < 5; i++) {
      new ShellReload(env).violation("wss://relay.example", RELAY);
      env.advance(60 * 60 * 1000);
    }
    expect(env.reloads).toBe(1);
  });

  it("leaves alone what it refuses that is not the relay", () => {
    const env = tab();
    const page = new ShellReload(env);
    page.violation("https://elsewhere.example/x.js", RELAY);
    page.violation("inline", RELAY);
    page.violation("", RELAY);
    page.violation("wss://relay.example", null);
    expect(env.reloads).toBe(0);
  });

  it("reloads for a relay that moved, once more", () => {
    const env = tab();
    new ShellReload(env).violation("wss://relay.example", RELAY);
    new ShellReload(env).violation("wss://other-relay.example/v1/phone", "wss://other-relay.example/v1/phone?t=x");
    expect(env.reloads).toBe(2);
  });

  it("waits for the person to come back once the worker kept a shell with another CSP", () => {
    const env = tab();
    const page = new ShellReload(env);
    page.visible();
    expect(env.reloads).toBe(0);
    page.shellChanged();
    expect(env.reloads).toBe(0);
    page.visible();
    expect(env.reloads).toBe(1);
    // Done: coming back again does nothing more.
    page.visible();
    expect(env.reloads).toBe(1);
  });

  it("never reloads for a changed shell twice within a minute, so a worker that kept saying so would not make a loop", () => {
    const env = tab();
    for (let i = 0; i < 3; i++) {
      const page = new ShellReload(env);
      page.shellChanged();
      page.visible();
    }
    expect(env.reloads).toBe(1);
    env.advance(SHELL_RELOAD_GAP_MS);
    const later = new ShellReload(env);
    later.shellChanged();
    later.visible();
    expect(env.reloads).toBe(2);
  });

  it("never reloads where it cannot write down that it did", () => {
    const env = tab({ storage: false });
    new ShellReload(env).violation("wss://relay.example", RELAY);
    expect(reloadOnce(env, "shell", SHELL_RELOAD_GAP_MS)).toBe(false);
    expect(env.reloads).toBe(0);
  });

  it("knows the relay by its host, whole address or origin", () => {
    expect(relayRefused("wss://relay.example", RELAY)).toBe("relay.example");
    expect(relayRefused("wss://relay.example/v1/phone", RELAY)).toBe("relay.example");
    expect(relayRefused("wss://relay.example:444", RELAY)).toBeNull();
    expect(relayRefused("not a url", RELAY)).toBeNull();
  });
});
