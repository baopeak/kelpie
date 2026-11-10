// Checks a running relay end to end, the way Kelpie and a phone use it: health, a new room, the desktop's signature and the protocol
// version its 101 says, the page origins the room takes phones from, a phone's token, 100 frames each way, a revoke, and the room
// dropped again. For a deploy (CI) or a self-hosted relay.
//   RELAY_URL         https://<relay> (or http://localhost:8787 for workerd on this machine)
//   REGISTRATION_KEY  the relay's registration key; leave it unset for a relay with open registration
//   PAGE=1            also check the phone app served beside the relay: `/` is HTML with its security headers, `/version.json`
//                     parses, its hashed assets are cached for good, and an unknown path is a 404
// These come from the environment, and nothing secret is printed. Exit code 0 when everything held.
import http from "node:http";
import https from "node:https";

const base = (process.env.RELAY_URL ?? "").replace(/\/+$/, "");
const registrationKey = process.env.REGISTRATION_KEY ?? "";
const page = process.env.PAGE === "1";
if (!/^https?:\/\//.test(base)) {
  console.error("set RELAY_URL (and REGISTRATION_KEY for a relay that needs one)");
  process.exit(2);
}
const wsBase = base.replace(/^http/, "ws");
const b64url = (bytes) => Buffer.from(bytes).toString("base64url");
const hex = (bytes) => Buffer.from(bytes).toString("hex");
const sha256 = async (bytes) => hex(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)));
/** Origins a phone's page may come from in this test; nothing is served there. */
const LISTED = "https://phone.smoke.invalid";
const UNLISTED = "https://elsewhere.smoke.invalid";

function step(name, ok, detail = "") {
  console.log(`${ok ? "ok  " : "FAIL"} ${name}${detail ? `: ${detail}` : ""}`);
  if (!ok) process.exit(1);
}

/** A socket and a way to wait for its next message or its close. */
function open(url) {
  const ws = new WebSocket(url);
  ws.binaryType = "arraybuffer";
  const inbox = [];
  const waiting = [];
  let closed = null;
  const closeWaiters = [];
  ws.onmessage = (e) => (waiting.length ? waiting.shift()(e.data) : inbox.push(e.data));
  ws.onclose = (e) => {
    closed = e.code;
    for (const w of closeWaiters.splice(0)) w(e.code);
  };
  const opened = new Promise((resolve, reject) => {
    ws.onopen = () => resolve();
    ws.onerror = () => reject(new Error(`could not open ${url.split("?")[0]}`));
  });
  const next = (ms = 5000) =>
    new Promise((resolve, reject) => {
      if (inbox.length) return resolve(inbox.shift());
      const timer = setTimeout(() => reject(new Error("nothing within 5 s")), ms);
      waiting.push((d) => {
        clearTimeout(timer);
        resolve(d);
      });
    });
  const closedWith = () => (closed !== null ? Promise.resolve(closed) : new Promise((resolve) => closeWaiters.push(resolve)));
  return { ws, opened, next, closedWith };
}

/** A WebSocket upgrade by hand, for what a WebSocket in Node does not show or send: the answer's status and headers, and an `Origin`.
 *  An accepted socket is closed again with `close()`. */
function upgrade(url, headers = {}) {
  const target = new URL(url.replace(/^ws/, "http"));
  const transport = target.protocol === "https:" ? https : http;
  return new Promise((resolve, reject) => {
    const request = transport.request(target, {
      headers: {
        connection: "Upgrade",
        upgrade: "websocket",
        "sec-websocket-version": "13",
        "sec-websocket-key": Buffer.from(crypto.getRandomValues(new Uint8Array(16))).toString("base64"),
        ...headers,
      },
    });
    request.on("upgrade", (response, socket) => resolve({ status: response.statusCode, headers: response.headers, close: () => socket.destroy() }));
    request.on("response", (response) => {
      response.resume();
      resolve({ status: response.statusCode, headers: response.headers, close: () => {} });
    });
    request.on("error", reject);
    request.setTimeout(10_000, () => request.destroy(new Error("no answer within 10 s")));
    request.end();
  });
}

const health = await fetch(`${base}/v1/health`);
step("health", health.status === 200 && (await health.text()) === "ok\n");

const keys = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, ["sign", "verify"]);
const pk = b64url(new Uint8Array(await crypto.subtle.exportKey("raw", keys.publicKey)));
const makeRoom = (headers = {}) =>
  fetch(`${base}/v1/rooms`, { method: "POST", headers: { "content-type": "application/json", ...headers }, body: JSON.stringify({ pk }) });
const fromPage = await makeRoom({ origin: LISTED, ...(registrationKey ? { authorization: `Bearer ${registrationKey}` } : {}) });
step("a page makes no room", fromPage.status === 403, `HTTP ${fromPage.status}`);
let made;
if (registrationKey) {
  const refused = await makeRoom({ authorization: "Bearer not-the-key" });
  step("a wrong registration key makes none", refused.status === 401, `HTTP ${refused.status}`);
  made = await makeRoom({ authorization: `Bearer ${registrationKey}` });
  step("a room is made with the registration key", made.status === 201, `HTTP ${made.status}`);
} else {
  made = await makeRoom();
  step("a room is made without a key (open registration)", made.status === 201, `HTTP ${made.status} ${made.headers.get("x-kelpie-reason") ?? ""}`);
}
step("the answer is not sniffed", made.headers.get("x-content-type-options") === "nosniff");
const { room, ticket } = await made.json();

let lastTs = 0;
async function desktopUrl() {
  const ts = (lastTs = Math.max(Date.now(), lastTs + 1));
  const statement = new TextEncoder().encode(`kelpie-relay/1 desktop ${room} ${ts}`);
  const sig = b64url(new Uint8Array(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, keys.privateKey, statement)));
  return `${wsBase}/v1/desktop?t=${encodeURIComponent(ticket)}&ts=${ts}&sig=${sig}`;
}

const probe = await upgrade(await desktopUrl());
probe.close();
step("the desktop's 101 says relay version 2", probe.status === 101 && probe.headers["x-kelpie-relay-version"] === "2", `HTTP ${probe.status}, version ${probe.headers["x-kelpie-relay-version"] ?? "none"}`);

const desktop = open(await desktopUrl());
await desktop.opened;
const token = crypto.getRandomValues(new Uint8Array(32));
const pageToken = crypto.getRandomValues(new Uint8Array(32));
desktop.ws.send(JSON.stringify({ t: "allow", h: [await sha256(token), await sha256(pageToken)] }));
desktop.ws.send(JSON.stringify({ t: "origins", o: [LISTED] }));
desktop.ws.send("info");
await desktop.next();
step("the desktop is in, with its signature, and lists a page origin", true);

const forged = open(`${wsBase}/v1/phone?t=${"a".repeat(32)}.${"A".repeat(43)}&k=${b64url(token)}`);
step("a forged ticket is refused", await forged.opened.then(() => false, () => true));

const phoneUrl = (t) => `${wsBase}/v1/phone?t=${encodeURIComponent(ticket)}&k=${b64url(t)}`;
const unlisted = await upgrade(phoneUrl(pageToken), { origin: UNLISTED });
unlisted.close();
step("a phone from an origin the desktop did not list is refused", unlisted.status === 403, `HTTP ${unlisted.status}`);
const listed = await upgrade(phoneUrl(pageToken), { origin: LISTED });
step("a phone from a listed origin is in", listed.status === 101, `HTTP ${listed.status}`);
const fromPageOpened = JSON.parse(await desktop.next());
step("the desktop hears the origin it came from", fromPageOpened.t === "open" && fromPageOpened.o === LISTED, JSON.stringify(fromPageOpened));
listed.close();
const gone = JSON.parse(await desktop.next());
step("and hears it go", gone.t === "close" && gone.s === fromPageOpened.s, JSON.stringify(gone));

const phone = open(phoneUrl(token));
await phone.opened;
const opened = JSON.parse(await desktop.next());
step("the phone is in, and the desktop hears of its slot, with no origin", opened.t === "open" && !("o" in opened), JSON.stringify(opened));

const began = performance.now();
for (let i = 0; i < 100; i++) {
  const payload = crypto.getRandomValues(new Uint8Array(1024));
  phone.ws.send(payload);
  const framed = new Uint8Array(await desktop.next());
  if (framed[0] * 256 + framed[1] !== opened.s || hex(framed.subarray(2)) !== hex(payload)) step(`frame ${i} to the desktop`, false);
  desktop.ws.send(framed);
  const back = new Uint8Array(await phone.next());
  if (hex(back) !== hex(payload)) step(`frame ${i} back to the phone`, false);
}
step("100 frames each way, unchanged", true, `${((performance.now() - began) / 100).toFixed(1)} ms per round trip`);

desktop.ws.send(JSON.stringify({ t: "allow", h: [] }));
step("a revoked token's socket is closed", (await phone.closedWith()) === 4002);

desktop.ws.send(JSON.stringify({ t: "drop" }));
step("the room is dropped", (await desktop.closedWith()) === 4004);

if (page) {
  const index = await fetch(`${base}/`);
  const html = await index.text();
  const h = index.headers;
  step("/ is the phone app's HTML", index.status === 200 && /^text\/html/.test(h.get("content-type") ?? ""), `HTTP ${index.status} ${h.get("content-type")}`);
  step("with a Content-Security-Policy", /default-src/.test(h.get("content-security-policy") ?? ""), h.get("content-security-policy") ?? "none");
  step("not sniffed", h.get("x-content-type-options") === "nosniff");
  step("no referrer", h.get("referrer-policy") === "no-referrer", h.get("referrer-policy") ?? "none");
  step("never framed", h.get("x-frame-options")?.toUpperCase() === "DENY", h.get("x-frame-options") ?? "none");
  const asset = /["'](\/assets\/[^"']+)["']/.exec(html)?.[1];
  step("/ names a hashed asset", Boolean(asset), asset ?? "none");
  const assetAnswer = await fetch(`${base}${asset}`);
  await assetAnswer.arrayBuffer();
  step("which is cached for good", assetAnswer.status === 200 && /immutable/.test(assetAnswer.headers.get("cache-control") ?? ""), assetAnswer.headers.get("cache-control") ?? "none");
  const version = await fetch(`${base}/version.json`);
  const parsed = await version.json().catch(() => null);
  step("/version.json parses", version.status === 200 && parsed !== null && typeof parsed === "object", JSON.stringify(parsed));
  const missing = await fetch(`${base}/no-such-file`);
  await missing.arrayBuffer();
  step("an unknown path is a 404", missing.status === 404, `HTTP ${missing.status}`);
}
process.exit(0);
