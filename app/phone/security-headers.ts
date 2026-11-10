// The headers the phone app is served with from the relay's origin, in one place. The daemon serves its own copy of the app with the
// same policy (crates/kelpied/src/mobile.rs, `ONLY_SELF`); a test holds the two together. Cloudflare reads them from the bundle's
// `_headers` file, which the hosted build writes from here (vite.phone.config.ts). Nothing here may import from Node: the tests and
// the build both read it.

/** Where the hosted app is served from when the build is not told otherwise: the relay worker's own origin. */
export const DEFAULT_HOSTED_ORIGIN = "https://kelpie-link-relay.baopeak.workers.dev";

/** The daemon's `ONLY_SELF`, directive by directive: the page loads only from itself and connects only to itself. */
export const CSP_DIRECTIVES: readonly (readonly [string, string])[] = [
  ["default-src", "'self'"],
  ["connect-src", "'self'"],
  ["img-src", "'self' data:"],
  ["style-src", "'self'"],
  ["script-src", "'self'"],
  ["frame-ancestors", "'none'"],
  ["base-uri", "'none'"],
  ["form-action", "'none'"],
];

/** The policy, with `connect` added to what the page may connect to. */
export function csp(connect: readonly string[] = []): string {
  return CSP_DIRECTIVES.map(([name, value]) => `${name} ${name === "connect-src" ? [value, ...connect].join(" ") : value}`).join("; ");
}

/** Sent with every file besides the policy. */
export const COMMON_HEADERS: Readonly<Record<string, string>> = {
  "X-Content-Type-Options": "nosniff",
  "Referrer-Policy": "no-referrer",
  "X-Frame-Options": "DENY",
};

/** Files that must always come from the network: the shell, the worker, and what says which version is live. */
export const NO_STORE_PATHS: readonly string[] = ["/", "/index.html", "/sw.js", "/manifest.webmanifest", "/version.json", "/404.html"];
/** Named after their content, so they never change. */
export const IMMUTABLE = "public, max-age=31536000, immutable";

/** The hosted origin the build was given: `https://<name>[:port]`, nothing else (no path, no user, no plain http). Throws otherwise. */
export function hostedOrigin(value: string | undefined): string {
  const origin = value?.trim() || DEFAULT_HOSTED_ORIGIN;
  if (!/^https:\/\/[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)*(?::\d{1,5})?$/.test(origin)) {
    throw new Error(`KELPIE_HOSTED_ORIGIN must be https://<name>[:port], got ${JSON.stringify(origin)}`);
  }
  return origin;
}

/** The policy of the hosted page: its relay socket is on its own origin, named as `wss:` so that no browser's reading of `'self'` matters. */
export function hostedCsp(origin: string): string {
  return csp([`wss://${new URL(origin).host}`]);
}

/** Cloudflare's `_headers` for the hosted bundle. */
export function headersFile(origin: string): string {
  const block = (path: string, headers: Record<string, string>) => [path, ...Object.entries(headers).map(([k, v]) => `  ${k}: ${v}`)].join("\n");
  return [
    block("/*", { "Content-Security-Policy": hostedCsp(origin), ...COMMON_HEADERS }),
    ...NO_STORE_PATHS.map((p) => block(p, { "Cache-Control": "no-store" })),
    block("/assets/*", { "Cache-Control": IMMUTABLE }),
  ].join("\n") + "\n";
}

/** What an unknown path gets: a page with no script, which Cloudflare serves without running the Worker. */
export const NOT_FOUND_PAGE = `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>Not found · Kelpie</title>
</head>
<body>
  <h1>Not found</h1>
  <p>There is nothing here. <a href="/">Open Kelpie</a>.</p>
</body>
</html>
`;
