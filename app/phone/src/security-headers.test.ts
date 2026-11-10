// The hosted app's headers (app/phone/security-headers.ts) against the daemon's own: one policy for both copies of the app.
import { describe, expect, it } from "vitest";
import mobileRs from "../../../crates/kelpied/src/mobile.rs?raw";
import { csp, DEFAULT_HOSTED_ORIGIN, headersFile, hostedCsp, hostedOrigin, IMMUTABLE, NO_STORE_PATHS, NOT_FOUND_PAGE } from "../security-headers";

/** The daemon's `ONLY_SELF`, as its source says it. */
const onlySelf = () => {
  const m = mobileRs.match(/const ONLY_SELF: &str = "([^"]+)";/);
  if (!m) throw new Error("ONLY_SELF not found in crates/kelpied/src/mobile.rs");
  return m[1];
};

const directives = (policy: string) => new Map(policy.split(";").map((d) => d.trim()).filter(Boolean).map((d) => [d.split(" ")[0], d.slice(d.indexOf(" ") + 1)] as const));

/** `_headers` as Cloudflare reads it: a path, then its indented headers. */
function rules(file: string): Map<string, Record<string, string>> {
  const out = new Map<string, Record<string, string>>();
  let current: Record<string, string> | null = null;
  for (const line of file.split("\n")) {
    if (!line.trim()) continue;
    if (!line.startsWith(" ")) out.set(line, (current = {}));
    else {
      const at = line.indexOf(":");
      current![line.slice(0, at).trim()] = line.slice(at + 1).trim();
    }
  }
  return out;
}

const ORIGIN = "https://kelpie-link-relay.example.workers.dev";

describe("the hosted app's headers", () => {
  it("hold the daemon's policy exactly", () => {
    expect(csp()).toBe(onlySelf());
  });

  it("differ from the daemon's policy only in connecting to the relay's own socket, named as wss:", () => {
    const daemon = directives(onlySelf());
    const hosted = directives(hostedCsp(ORIGIN));
    expect([...hosted.keys()]).toEqual([...daemon.keys()]);
    for (const [name, value] of daemon) {
      if (name === "connect-src") expect(hosted.get(name)).toBe("'self' wss://kelpie-link-relay.example.workers.dev");
      else expect(hosted.get(name)).toBe(value);
    }
    expect(hostedCsp("https://relay.example:8443")).toContain("connect-src 'self' wss://relay.example:8443;");
  });

  it("give every file the policy and the daemon's other headers, the shell no cache, and the hashed assets a year", () => {
    const r = rules(headersFile(ORIGIN));
    expect(r.get("/*")).toEqual({ "Content-Security-Policy": hostedCsp(ORIGIN), "X-Content-Type-Options": "nosniff", "Referrer-Policy": "no-referrer", "X-Frame-Options": "DENY" });
    for (const path of ["/", "/index.html", "/sw.js", "/manifest.webmanifest", "/version.json", "/404.html"]) expect(r.get(path)).toEqual({ "Cache-Control": "no-store" });
    expect(NO_STORE_PATHS).toHaveLength(6);
    expect(r.get("/assets/*")).toEqual({ "Cache-Control": "public, max-age=31536000, immutable" });
    expect(IMMUTABLE).toBe("public, max-age=31536000, immutable");
    expect([...r.keys()]).toEqual(["/*", ...NO_STORE_PATHS, "/assets/*"]);
    expect(headersFile(ORIGIN)).not.toContain("\r");
    expect(headersFile(ORIGIN)).toBe(headersFile(ORIGIN));
  });

  it("are for an https origin with a name and maybe a port, nothing else", () => {
    expect(hostedOrigin(undefined)).toBe(DEFAULT_HOSTED_ORIGIN);
    expect(hostedOrigin("")).toBe(DEFAULT_HOSTED_ORIGIN);
    expect(DEFAULT_HOSTED_ORIGIN).toBe("https://kelpie-link-relay.baopeak.workers.dev");
    expect(hostedOrigin("https://localhost:8787")).toBe("https://localhost:8787");
    for (const bad of ["http://relay.example", "https://relay.example/", "https://relay.example/app", "https://user@relay.example", "https://relay.example:port", "wss://relay.example", "https://-bad.example", "https://Relay.Example"]) {
      expect(() => hostedOrigin(bad)).toThrow(/KELPIE_HOSTED_ORIGIN/);
    }
  });

  it("send unknown paths a page with no script and no style of its own", () => {
    expect(NOT_FOUND_PAGE).not.toMatch(/<script|<style|\sstyle=|\son\w+=/i);
    expect(NOT_FOUND_PAGE).toContain('href="/"');
  });
});
