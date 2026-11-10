import { describe, expect, it } from "vitest";
import { APP_VERSION, HOSTED, onHost, replaceQuestion, servedVersion, trustText } from "./hosted";

const fill = (text: string, vars?: Record<string, string | number>) => (vars ? text.replace(/\{(\w+)\}/g, (m, k: string) => (k in vars ? String(vars[k]) : m)) : text);
const HASH = "a".repeat(64);

describe("the hosted app's build facts", () => {
  it("is off where the build did not turn it on, as in the daemon's bundle", () => {
    expect(HOSTED).toBe(false);
    expect(APP_VERSION).toBe("dev");
  });
});

describe("what the origin says it serves", () => {
  const answer = (body: unknown, init: ResponseInit = {}) => async () => new Response(JSON.stringify(body), init);

  it("reads the version and the bundle's fingerprint, from the network only", async () => {
    const asked: [string, RequestInit][] = [];
    const v = await servedVersion(async (url, init) => {
      asked.push([url, init]);
      return new Response(JSON.stringify({ app: "kelpie-phone", version: "v0.1.16", commit: "abc", api_min: 34, bundle_sha256: HASH }));
    });
    expect(v).toEqual({ version: "v0.1.16", bundle: HASH });
    expect(asked).toEqual([["/version.json", { cache: "no-store", redirect: "error" }]]);
  });

  it("says nothing it cannot read as the phone app's", async () => {
    for (const fetcher of [
      answer({ app: "something-else", version: "1", bundle_sha256: HASH }),
      answer({ app: "kelpie-phone", version: "1", bundle_sha256: "not a hash" }),
      answer({ app: "kelpie-phone", bundle_sha256: HASH }),
      answer({ app: "kelpie-phone", version: "1", bundle_sha256: HASH }, { status: 404 }),
      async () => new Response("<html>"),
      async () => {
        throw new TypeError("offline");
      },
    ]) {
      expect(await servedVersion(fetcher)).toBeNull();
    }
  });
});

describe("the words", () => {
  it("say where the app comes from, who could change it, how to check, what this screen cannot prove, and the way without it", () => {
    const text = trustText(fill).join("\n");
    for (const part of [/relay on Cloudflare/, /Cloudflare account/, /release deploy/, /read and type/, /while the app is unlocked/, /fingerprint/, /cannot prove/, /Tailscale/]) expect(text).toMatch(part);
  });

  it("ask before another computer takes this app, naming both", () => {
    expect(replaceQuestion(fill, "Desk", "Laptop")).toBe("This app is paired with Desk. Pair with Laptop instead? Desk loses this phone.");
  });

  it("know a relay by its host", () => {
    expect(onHost("wss://relay.example/v1/phone?t=x", "relay.example")).toBe(true);
    expect(onHost("wss://relay.example:8443/v1/phone", "relay.example")).toBe(false);
    expect(onHost("wss://relay.other.example", "relay.example")).toBe(false);
    expect(onHost("not a url", "relay.example")).toBe(false);
  });
});
