// Checks a running relay end to end, the way Kelpie and a phone use it: health, a new room, the desktop's signature, a phone's token,
// 100 frames each way, a revoke, and the room dropped again. For a deploy (CI) or a self-hosted relay.
//   RELAY_URL         https://<relay> (or http://localhost:8787 for workerd on this machine)
//   REGISTRATION_KEY  the relay's registration key
// Both come from the environment, and nothing secret is printed. Exit code 0 when everything held.
const base = (process.env.RELAY_URL ?? "").replace(/\/+$/, "");
const registrationKey = process.env.REGISTRATION_KEY ?? "";
if (!/^https?:\/\//.test(base) || !registrationKey) {
  console.error("set RELAY_URL and REGISTRATION_KEY");
  process.exit(2);
}
const wsBase = base.replace(/^http/, "ws");
const b64url = (bytes) => Buffer.from(bytes).toString("base64url");
const hex = (bytes) => Buffer.from(bytes).toString("hex");
const sha256 = async (bytes) => hex(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)));

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

const health = await fetch(`${base}/v1/health`);
step("health", health.status === 200 && (await health.text()) === "ok\n");

const keys = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, ["sign", "verify"]);
const pk = b64url(new Uint8Array(await crypto.subtle.exportKey("raw", keys.publicKey)));
const made = await fetch(`${base}/v1/rooms`, { method: "POST", headers: { authorization: `Bearer ${registrationKey}`, "content-type": "application/json" }, body: JSON.stringify({ pk }) });
step("a room is made with the registration key", made.status === 201, `HTTP ${made.status}`);
const { room, ticket } = await made.json();
const refused = await fetch(`${base}/v1/rooms`, { method: "POST", headers: { authorization: "Bearer not-the-key", "content-type": "application/json" }, body: JSON.stringify({ pk }) });
step("a wrong registration key makes none", refused.status === 401, `HTTP ${refused.status}`);

const ts = Date.now();
const sig = b64url(new Uint8Array(await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, keys.privateKey, new TextEncoder().encode(`kelpie-relay/1 desktop ${room} ${ts}`))));
const desktop = open(`${wsBase}/v1/desktop?t=${encodeURIComponent(ticket)}&ts=${ts}&sig=${sig}`);
await desktop.opened;
const token = crypto.getRandomValues(new Uint8Array(32));
desktop.ws.send(JSON.stringify({ t: "allow", h: [await sha256(token)] }));
desktop.ws.send("info");
await desktop.next();
step("the desktop is in, with its signature", true);

const forged = open(`${wsBase}/v1/phone?t=${"a".repeat(32)}.${"A".repeat(43)}&k=${b64url(token)}`);
step("a forged ticket is refused", await forged.opened.then(() => false, () => true));

const phone = open(`${wsBase}/v1/phone?t=${encodeURIComponent(ticket)}&k=${b64url(token)}`);
await phone.opened;
const opened = JSON.parse(await desktop.next());
step("the phone is in, and the desktop hears of its slot", opened.t === "open", JSON.stringify(opened));

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
process.exit(0);
