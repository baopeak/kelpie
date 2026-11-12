"""The relay against a real daemon and a real relay on this machine: pairing and working through it with no direct path, padding, what
the relay's capture holds, a revoke made while the relay was away, and off.

    python -X utf8 scripts/e2e/mobile_relay.py <debug kelpied.exe>

Before it: `cd relay && pnpm install --frozen-lockfile && pnpm build:selfhost` (the self-hosted bundle this runs on workerd).

Daemon only, no app and no browser: its own endpoint and data folder (phone_support.isolate), a fake Tailscale CLI (never the real one) so
that the daemon has a Tailscale origin for the relay to let phones in from, and workerd from relay/node_modules with secrets made for this
run and held by its process alone, once open to every install and once with a registration key. TCP taps sit between the daemon
and the relay and between the phone and the relay: they keep what passes, so the test can look for plaintext in it (and read the
control messages), and the daemon's can be cut to play a relay that went away. The phone speaks kelpie-link (link_noise.py) and
connects only to the relay: it is never given the daemon's own address. Nothing here reaches Cloudflare.

1. On a relay open to every install (no registration key) the daemon makes its room with no key; the relay says it is of the second
   version, the daemon tells it the pages it lets in (`origins`, never the single `origin`), the relay names the page a phone came from
   (`o` in `open`), a phone pairs there, and a phone whose page is elsewhere is refused. The same phone paired again is a record of its own (what
   the desktop lists as one phone), and removing the first, as Replace that pairing does, leaves the second paired and working.
2. The daemon makes its room with the registration key and connects; the relay holds the Tailscale origin.
3. Three phones pair through the relay with the QR code's single-use token, from the Tailscale origin; the request names "the relay";
   the pairing gives each a relay token. A second use of a pairing token is refused.
4. Reconnected with its token, a phone types into a shell and reads the result; every frame after the hello is one of the padded sizes.
5. Neither tap's capture holds what was typed, what came back, or the phone's name.
6. The same phone connects again while its old socket is still there: the new session takes over and keeps working.
7. A new room: connected phones hear its ticket, the old room is removed (4004, and 404 for its ticket), and the token works there.
8. With the relay cut off from the daemon, a phone is removed; a phone socket the relay still admits hears `desktop_offline`; once the
   daemon is back, the relay closes it (4002) before it reaches anything, and the token is refused from then on.
9. A phone removed while connected through the relay hears `revoked` in its session, and is closed by the computer (4401).
10. `mobile.status` lists a phone as connected through the relay, then through Tailscale once it connects again through Tailscale Serve's
    listener, then as not connected. Its hello says why it may not take control: `not_granted` while its record may type, and `via_relay`
    on the relay once it was given control (its passkey set up over Tailscale, the lock on); `ts` says the computer is on Tailscale.
11. The kill switch tells a phone on the relay it was removed, and the relay refuses its token after.
12. A relay whose answer names no version (the first version's, played by a tap that strips `x-kelpie-relay-version`) is told the single
    origin and never the list, and phones pair and connect through it as before.
13. Turning the relay off closes the daemon's socket and dials nothing again.
"""
import asyncio
import glob
import hashlib
import json
import os
import secrets
import socket
import subprocess
import sys
import tempfile
import time

import websockets

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import link_noise as ln  # noqa: E402
import phone_support as ps  # noqa: E402

if len(sys.argv) < 2:
    raise SystemExit(__doc__)
exe = os.path.abspath(sys.argv[1])
ps.isolate("phone-relay")
PORT = 47000 + os.getpid() % 1000
BASE = f"http://127.0.0.1:{PORT}/"
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
RELAY = os.path.join(ROOT, "relay")
ORIGIN = f"https://{ps.FAKE_TAILNET_NAME}:8443"
MARKER = "relay-marker-" + secrets.token_hex(6)
PHONE_NAME = "Relay phone " + secrets.token_hex(3)
SIZES = {b + 16 for b in ln.PAD_BUCKETS}

results = []


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail and not ok else ""), flush=True)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Tap:
    """A TCP forwarder that keeps what passes both ways, and can be cut (its connections dropped, new ones refused) and restored.
    `streams` keeps each connection's two directions apart, for reading the WebSocket frames in them. `strip_version`: the
    relay's answers lose `x-kelpie-relay-version`, as a relay of the first version sends them."""

    def __init__(self, target, strip_version=False):
        self.target, self.up, self.seen, self.accepted, self.open = target, True, bytearray(), 0, set()
        self.strip_version = strip_version
        self.streams = []

    async def start(self):
        self.server = await asyncio.start_server(self._client, "127.0.0.1", 0)
        self.port = self.server.sockets[0].getsockname()[1]
        return self

    async def _client(self, reader, writer):
        self.accepted += 1
        if not self.up:
            writer.transport.abort()
            return
        try:
            up_reader, up_writer = await asyncio.open_connection("127.0.0.1", self.target)
        except OSError:
            writer.transport.abort()
            return
        pair = (writer, up_writer)
        self.open.add(pair)
        stream = {"up": bytearray(), "down": bytearray()}
        self.streams.append(stream)

        async def pipe(src, dst, kept, answers):
            head = bytearray()  # the HTTP answer's head, until it is whole: the header goes from it as one piece
            try:
                while data := await src.read(65536):
                    if answers and self.strip_version and head is not None:
                        head += data
                        if b"\r\n\r\n" not in head:
                            continue
                        top, rest = bytes(head).split(b"\r\n\r\n", 1)
                        top = b"\r\n".join(line for line in top.split(b"\r\n") if not line.lower().startswith(b"x-kelpie-relay-version:"))
                        data, head = top + b"\r\n\r\n" + rest, None
                    if len(self.seen) < 16 << 20:
                        self.seen += data
                    if len(kept) < 4 << 20:
                        kept += data
                    dst.write(data)
                    await dst.drain()
            except (OSError, asyncio.IncompleteReadError):
                pass
            finally:
                dst.transport.abort()

        await asyncio.gather(pipe(reader, up_writer, stream["up"], False), pipe(up_reader, writer, stream["down"], True))
        self.open.discard(pair)

    def cut(self):
        self.up = False
        for a, b in list(self.open):
            a.transport.abort()
            b.transport.abort()

    def restore(self):
        self.up = True

    def close(self):
        self.cut()
        self.server.close()

    def texts(self, direction):
        """The JSON text frames of every connection in one direction (`up`: what the daemon sent, unmasked; `down`: what the relay sent)."""
        out = []
        for stream in self.streams:
            data = bytes(stream[direction])
            if b"\r\n\r\n" not in data:
                continue
            out += [json.loads(t) for t in ws_texts(data.split(b"\r\n\r\n", 1)[1]) if t.startswith("{")]
        return out


def ws_texts(data):
    """The text frames in a run of WebSocket frames (RFC 6455), unmasked."""
    texts, i = [], 0
    while i + 2 <= len(data):
        opcode, length, masked = data[i] & 0x0F, data[i + 1] & 0x7F, data[i + 1] & 0x80
        i += 2
        if length == 126:
            length, i = int.from_bytes(data[i:i + 2], "big"), i + 2
        elif length == 127:
            length, i = int.from_bytes(data[i:i + 8], "big"), i + 8
        mask = data[i:i + 4] if masked else b""
        i += 4 if masked else 0
        payload = data[i:i + length]
        i += length
        if len(payload) < length:
            break
        if masked:
            payload = bytes(b ^ mask[n % 4] for n, b in enumerate(payload))
        if opcode == 1:
            texts.append(payload.decode("utf-8", "replace"))
    return texts


def start_workerd(port, secrets_env):
    """workerd with the self-hosted bundle on `port`, its rooms in a folder of this run. Secrets only in its own environment."""
    bundle = os.path.join(RELAY, "workerd", "dist", "entry.js")
    if not os.path.exists(bundle):
        raise SystemExit("build the relay first: cd relay && pnpm install --frozen-lockfile && pnpm build:selfhost")
    found = glob.glob(os.path.join(RELAY, "node_modules", ".pnpm", "@cloudflare+workerd-*", "node_modules", "@cloudflare", "workerd-*", "bin", "workerd*"))
    if not found:
        raise SystemExit("no workerd binary in relay/node_modules: run pnpm install there")
    run = tempfile.mkdtemp(prefix="kelpie-relay-workerd-")
    os.makedirs(os.path.join(run, "dist"))
    os.makedirs(os.path.join(run, "rooms"))
    with open(os.path.join(RELAY, "workerd", "config.capnp"), encoding="utf-8") as f:
        config = f.read().replace('address = "*:8787"', f'address = "127.0.0.1:{port}"')
    with open(os.path.join(run, "config.capnp"), "w", encoding="utf-8") as f:
        f.write(config)
    with open(bundle, "rb") as src, open(os.path.join(run, "dist", "entry.js"), "wb") as dst:
        dst.write(src.read())
    env = {k: v for k, v in os.environ.items() if k not in ("REGISTRATION_KEY", "OPEN_REGISTRATION", "TICKET_SECRET")}
    proc = subprocess.Popen([found[0], "serve", "config.capnp"], cwd=run, env={**env, **secrets_env}, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    end = time.time() + 20
    while time.time() < end:
        try:
            with socket.create_connection(("127.0.0.1", port), 0.5):
                return proc
        except OSError:
            time.sleep(0.2)
    proc.kill()
    raise SystemExit("workerd did not start")


async def until(fn, timeout, step=0.2):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        got = await fn()
        if got:
            return got
        await asyncio.sleep(step)
    return None


async def main(d, phone_tap, desk_tap, registration_key, open_tap, open_port, v1_tap):
    async def call(method, params=None):
        return await asyncio.to_thread(d.call, method, params or {})

    async def relay_status():
        return (await call("mobile.status"))["relay"]

    # ---------- the room ----------
    await call("mobile.enable", {"ip": "127.0.0.1", "port": PORT})
    ps.fake_tailscale_says(TS[1], status=ps.FAKE_TAILSCALE_READY)
    told = await call("mobile.set_tailscale", {"on": True})
    check("Tailscale mode gives the daemon its origin", told.get("origin") == ORIGIN.removeprefix("https://"), told.get("origin"))
    await call("mobile.set_lock", {"on": False})
    url = f"ws://127.0.0.1:{desk_tap.port}"

    vectors = json.load(open(os.path.join(RELAY, "test", "vectors.json"), encoding="utf-8"))
    check("the pairing token is derived as the relay's vectors say", ps.relay_pair_token(bytes.fromhex(vectors["psk_hex"])) == vectors["pair_token"])

    async def pair_through_relay(name, first=False, via=None, relay_url=None):
        """A phone pairs through the relay with the QR code's token: (device, its relay token, the room's ticket), or None. `via`: the port the phone
        reaches the relay at (the phone side's tap), `relay_url`: the relay the computer is on."""
        link = (await call("mobile.pair_start"))["url"]
        payload = json.loads(ln.unb64(link.split("#p=")[1]))
        relay_ep = next((e for e in payload["ep"] if e["t"] == "relay"), None)
        if first:
            check("the QR code names the relay and the room's ticket", relay_ep and relay_ep["a"] == (relay_url or url) and "." in relay_ep["r"], payload["ep"])
        if not relay_ep:
            return None
        pair_token = ps.relay_pair_token(ln.unb64(payload["psk"]))
        base = f"ws://127.0.0.1:{via or phone_tap.port}/v1/phone?t={relay_ep['r']}"
        attempt = ps.PairAttempt(BASE, link, name, "write", origin=ORIGIN)
        attempt.url = f"{base}&p={pair_token}"
        await attempt.start()
        if first:
            check("the pairing handshake goes through the relay", attempt.session is not None, f"closed {attempt.closed}, silent {attempt.silent}")
        if attempt.session is None:
            return None
        request = await asyncio.to_thread(ps.pending_request, d, name)
        if first:
            check("the request says it came through the relay", request["address"] == "the relay", request["address"])
        approve = asyncio.ensure_future(call("mobile.approve", {"id": request["id"], "allow": True, "code": attempt.code(), "permission": "write"}))
        paired = await attempt.outcome()
        await approve
        if first:
            check("paired, with a relay token", paired and paired["type"] == "paired" and paired.get("relay", {}).get("token"), paired)
            try:
                again = await websockets.connect(f"{base}&p={pair_token}", origin=ORIGIN, open_timeout=5)
                await again.close()
                check("a pairing token is let in once", False)
            except websockets.exceptions.InvalidHandshake as e:
                check("a pairing token is let in once", ps.status_of(e) == 403, ps.status_of(e))
        if not (paired and paired.get("relay")):
            return None
        return ps.Device(attempt.priv, attempt.pub, attempt.dpk, paired["device"]["id"]), paired["relay"]["token"], relay_ep["r"]

    def phone_url(ticket, token):
        return f"ws://127.0.0.1:{phone_tap.port}/v1/phone?t={ticket}&k={token}"

    async def through_relay(device, ticket, token, port=None):
        phone = ps.Phone(BASE, device, origin=ORIGIN)
        phone.url = f"ws://127.0.0.1:{port}/v1/phone?t={ticket}&k={token}" if port else phone_url(ticket, token)
        phone.caps = ["ack", "hb", "relay", "pad"]
        # As the app does: which token it holds, so that the computer does not rotate the one in use.
        phone.greeting = {"relay_token": hashlib.sha256(ln.unb64(token)).hexdigest()}
        hello = await phone.open()
        if hello.get("pad"):
            phone.session.pad = True
        return phone, hello

    async def answers(phone, n):
        """The computer still answers this phone: a heartbeat of its own comes back."""
        await phone.send(type="hb", n=n)
        return bool(await phone.wait(lambda m: m["type"] == "hb" and m.get("echo") and m.get("n") == n, 5))

    # ---------- a relay open to every install ----------
    open_url = f"ws://127.0.0.1:{open_tap.port}"
    opened = await asyncio.to_thread(lambda: _refused(d, "mobile.set_relay", {"on": True, "url": open_url}))
    status = await until(lambda: _connected_at(relay_status, open_url, open_tap), 20)
    check("a relay open to every install makes a room with no registration key, and the daemon connects", opened is None and bool(status), opened or status)
    check("…a relay that says it is of the second version", status and status.get("version") == 2, status)
    open_paired = await pair_through_relay(PHONE_NAME + " open", via=open_port, relay_url=open_url)
    check("a phone pairs through it", bool(open_paired))
    sent, heard = open_tap.texts("up"), open_tap.texts("down")
    check("the computer tells a relay of the second version the pages it lets in, never the single origin", any(m.get("t") == "origins" and ORIGIN in m.get("o", []) for m in sent) and not any(m.get("t") == "origin" for m in sent), [m.get("t") for m in sent][:12])
    check("…and the relay says which page a phone came from (`o` in `open`)", any(m.get("t") == "open" and m.get("o") == ORIGIN for m in heard), [m for m in heard if m.get("t") == "open"][:3])
    elsewhere = json.loads(ln.unb64((await call("mobile.pair_start"))["url"].split("#p=")[1]))
    relay_ep = next(e for e in elsewhere["ep"] if e["t"] == "relay")
    try:
        foreign = await websockets.connect(f"ws://127.0.0.1:{open_port}/v1/phone?t={relay_ep['r']}&p={ps.relay_pair_token(ln.unb64(elsewhere['psk']))}", origin="https://not-a-page-it-lets-in.example", open_timeout=5)
        await foreign.close()
        check("a phone from a page the room does not let in is refused", False)
    except websockets.exceptions.InvalidHandshake as e:
        check("a phone from a page the room does not let in is refused", ps.status_of(e) == 403, ps.status_of(e))
    if open_paired:
        # The same phone paired again (another way, in the app): a record of its own, which the desktop lists with the first under "Same phone as".
        # "Replace that pairing" then removes the old record with `mobile.revoke`, only once the new one is approved, and the new one keeps working.
        again = await pair_through_relay(PHONE_NAME + " open", via=open_port, relay_url=open_url)
        same_name = [x["id"] for x in (await call("mobile.status"))["devices"] if x["name"] == PHONE_NAME + " open"]
        check("the same phone paired again is a record of its own, beside the first", bool(again) and len(same_name) == 2 and again[0].id in same_name and open_paired[0].id in same_name, same_name)
        await call("mobile.revoke", {"id": open_paired[0].id})
        if again:
            kept, kept_hello = await through_relay(again[0], again[2], again[1], port=open_port)
            check("…and removing the old one leaves the new one paired and working", kept_hello.get("type") == "hello" and await answers(kept, 31)
                  and [x["id"] for x in (await call("mobile.status"))["devices"] if x["name"] == PHONE_NAME + " open"] == [again[0].id])
            await kept.close()
            await call("mobile.revoke", {"id": again[0].id})

    # ---------- a relay that asks for a registration key ----------
    wrong = await asyncio.to_thread(lambda: _refused(d, "mobile.set_relay", {"on": True, "url": url, "operator_key": "not-the-key"}))
    check("a wrong registration key makes no room", "did not take that registration key" in (wrong or ""), wrong)
    plain = await asyncio.to_thread(lambda: _refused(ps.kc.Conn(), "mobile.set_relay", {"on": True, "url": url, "operator_key": registration_key}))
    check("the plain user token cannot turn the relay on", "needs the Kelpie app" in (plain or ""), plain)
    await call("mobile.set_relay", {"on": True, "url": url, "operator_key": registration_key})
    status = await until(lambda: _connected_at(relay_status, url, desk_tap), 20)
    check("the daemon makes its room and connects", bool(status), status)
    check("…with the Tailscale origin for phones", status and status["origin"] == ORIGIN, status)

    # ---------- pairing through the relay ----------
    paired_a = await pair_through_relay(PHONE_NAME, first=True)
    if not paired_a:
        return
    device, token, ticket = paired_a
    paired_b = await pair_through_relay(PHONE_NAME + " B")
    paired_c = await pair_through_relay(PHONE_NAME + " C")
    check("more phones pair through the relay the same way", paired_b and paired_c)
    if not (paired_b and paired_c):
        return
    check("the device is on a secure origin", next(x for x in (await call("mobile.status"))["devices"] if x["id"] == device.id)["secure_origin"])

    # ---------- working through it ----------
    phone, hello = await through_relay(device, ticket, token)
    check("the phone reconnects through the relay, and may type", hello["device"]["permission"] == "write", hello["device"])
    check("…and the computer says it reads padded chunks", hello.get("pad") is True)
    sizes = []
    opener = phone.session.open
    phone.session.open = lambda frame: (sizes.append(len(frame)), opener(frame))[1]
    work = tempfile.mkdtemp(prefix="kelpie-phone-relay-cwd-")
    shell = await call("session.create", {"kind": "shell", "cwd": work, "name": "relay-shell"})
    sid = shell["id"]
    await phone.ask(type="sub", id=sid, height=40, have_rev=None, focus=True)
    await asyncio.sleep(1.5)
    typed = await phone.ask(type="input", id=sid, text=f"echo {MARKER[::-1]}", enter=True)
    check("typing goes through", typed and typed.get("type") == "ack", typed)
    shown = await until(lambda: _shows(phone, sid, MARKER[::-1]), 15)
    check("…and what the shell printed comes back", bool(shown))
    check("every frame after the hello is one of the padded sizes", sizes and all(n in SIZES for n in sizes), sorted(set(sizes))[:8])

    # ---------- what the relay saw ----------
    for name, tap in (("phone side", phone_tap), ("computer side", desk_tap)):
        seen = bytes(tap.seen)
        check(f"the {name} of the relay carried traffic", len(seen) > 1000, len(seen))
        check("…and none of it is what was typed, printed or named", MARKER[::-1].encode() not in seen and PHONE_NAME.encode() not in seen and b"relay-shell" not in seen)

    # ---------- the same phone again, while its old socket is still there ----------
    again, _ = await through_relay(device, ticket, token)
    await until(lambda: _closed(phone), 10)
    check("a phone that connects again takes over from its old socket", phone.closed in (4001, 4409), phone.closed)
    await asyncio.sleep(3)
    check("…and nothing late of the old socket touches the new session", again.closed is None and await answers(again, 77), again.closed)

    # ---------- a new room ----------
    await call("mobile.relay_recreate")
    moved = await again.wait(lambda m: m["type"] == "addrs" and any(e["t"] == "relay" and e.get("r") != ticket for e in m.get("endpoints", [])), 15)
    check("a new room: connected phones hear its ticket", bool(moved))
    await until(lambda: _closed(again), 15)
    check("…and the old room is removed (4004)", again.closed == 4004, again.closed)
    try:
        old_room = await websockets.connect(phone_url(ticket, token), origin=ORIGIN, open_timeout=5)
        await old_room.close()
        check("the old ticket finds no room", False)
    except websockets.exceptions.InvalidHandshake as e:
        check("the old ticket finds no room", ps.status_of(e) == 404, ps.status_of(e))
    ticket = next(e for e in moved["endpoints"] if e["t"] == "relay")["r"] if moved else ticket
    await until(lambda: _connected(relay_status), 20)
    phone, hello = await through_relay(device, ticket, token)
    check("the phone works in the new room with the token it had", hello["type"] == "hello")

    # ---------- a revoke while the relay is away ----------
    device_b, token_b, _ = paired_b
    desk_tap.cut()
    down = await until(lambda: _not_connected(relay_status), 60)
    check("the daemon sees the relay gone", bool(down), down)
    await call("mobile.revoke", {"id": device_b.id})
    stale = await websockets.connect(phone_url(ticket, token_b), origin=ORIGIN, open_timeout=5)
    first = await asyncio.wait_for(stale.recv(), 5)
    check("the relay still lets the old token in, and says the computer is away", first == json.dumps({"t": "desktop_offline"}, separators=(",", ":")), first)
    desk_tap.restore()
    got_binary = False
    code = None
    try:
        while True:
            msg = await asyncio.wait_for(stale.recv(), 90)
            got_binary = got_binary or isinstance(msg, bytes)
    except websockets.ConnectionClosed as e:
        code = e.rcvd.code if e.rcvd else None
    except asyncio.TimeoutError:
        pass
    check("once the daemon is back, the removed phone is closed first (4002)", code == 4002, code)
    check("…having reached nothing", not got_binary)
    try:
        late = await websockets.connect(phone_url(ticket, token_b), origin=ORIGIN, open_timeout=5)
        await late.close()
        check("its token is refused from then on", False)
    except websockets.exceptions.InvalidHandshake as e:
        check("its token is refused from then on", ps.status_of(e) == 403, ps.status_of(e))
    await until(lambda: _connected(relay_status), 60)

    # ---------- a revoke while the phone is connected through the relay ----------
    device_c, token_c, _ = paired_c
    phone_c, _ = await through_relay(device_c, ticket, token_c)
    await call("mobile.revoke", {"id": device_c.id})
    await until(lambda: _closed(phone_c), 10)
    check("a phone removed while connected through the relay is told so inside its session", any(m["type"] == "revoked" for m in phone_c.log), [m["type"] for m in phone_c.log][-4:])
    check("…before its socket is closed by the computer (4401), not by the relay (4002)", phone_c.closed == 4401, phone_c.closed)

    # ---------- how the desktop sees a phone connected, and why the phone may not take control ----------
    # A fresh connection, as below: the one from before the relay was cut was told `desktop_offline`.
    await phone.close()
    phone, hello = await through_relay(device, ticket, token)
    check("the desktop lists the phone as connected through the relay", await until(lambda: _route_is(call, device.id, "relay"), 10), await _route_of(call, device.id))
    check("…and the phone hears that its record is below control, and that this computer is on Tailscale", hello["device"].get("control") == "not_granted" and hello.get("ts") is True, (hello["device"], hello.get("ts")))
    # The direct route returns: Tailscale Serve's listener, as serve calls it. The newer connection takes over from the relay's.
    serve_port = int([c for c in ps.fake_tailscale_calls(TS[2]) if c.startswith("serve --bg")][-1].rsplit(":", 1)[1])
    held = dict(host="127.0.0.1", port=serve_port, additional_headers={"X-Forwarded-For": "100.101.102.103"})
    direct = ps.Phone(f"http://{ORIGIN.removeprefix('https://')}/", device, origin=ORIGIN, **held)
    await direct.open()
    check("…then through Tailscale once the direct route is back", await until(lambda: _route_is(call, device.id, "tailscale"), 10), await _route_of(call, device.id))
    # Given control (its passkey set up over Tailscale, with the lock on), the relay is what keeps the screen from it.
    await call("mobile.set_lock", {"on": True})
    passkey = ps.SoftPasskey(ORIGIN)
    enrolled = await direct.ask(type="lock_enrol", **passkey.create((await direct.ask(type="lock_challenge"))["lock"]["challenge"]))
    check("the phone sets up its passkey over Tailscale", (enrolled or {}).get("result") == "ok", enrolled)
    await call("mobile.set_permission", {"id": device.id, "permission": "control"})
    await direct.close()
    phone, hello = await through_relay(device, ticket, token)
    check("a phone given control hears through the relay that the relay is what keeps the screen from it", hello["device"].get("control") == "via_relay" and hello["device"]["permission"] != "control", hello["device"])
    await phone.close()
    check("…and once it goes, the desktop lists it as not connected", await until(lambda: _route_is(call, device.id, None), 10), await _route_of(call, device.id))
    await call("mobile.set_permission", {"id": device.id, "permission": "write"})
    await call("mobile.set_lock", {"on": False})

    # ---------- the kill switch ----------
    # A fresh connection: the one from before the relay was cut was told `desktop_offline`, which the page acts on by connecting again.
    await phone.close()
    phone, _ = await through_relay(device, ticket, token)
    await call("mobile.revoke_all")
    await until(lambda: _closed(phone), 10)
    check("the kill switch tells a phone on the relay it was removed", any(m["type"] == "revoked" for m in phone.log), [m["type"] for m in phone.log][-4:])

    async def refused():
        try:
            after = await websockets.connect(phone_url(ticket, token), origin=ORIGIN, open_timeout=5)
            await after.close()
            return None
        except websockets.exceptions.InvalidHandshake as e:
            return ps.status_of(e)

    # The relay hears last, a second after the sessions had their say.
    check("…and the relay lets its token in no more", await until(refused, 10, step=0.5) == 403)
    await call("mobile.enable", {"ip": "127.0.0.1", "port": PORT})
    await until(lambda: _connected(relay_status), 30)
    # ---------- a relay of the first version: its answer names no version ----------
    v1_url = f"ws://127.0.0.1:{v1_tap.port}"
    moved = await asyncio.to_thread(lambda: _refused(d, "mobile.set_relay", {"on": True, "url": v1_url}))
    status = await until(lambda: _connected_at(relay_status, v1_url, v1_tap), 30)
    check("a relay whose answer names no version is taken for the first one, and the daemon connects", moved is None and status and status.get("version") == 1, moved or status)
    sent = v1_tap.texts("up")
    check("…and is told the single origin, never the list of pages", any(m.get("t") == "origin" and m.get("o") == ORIGIN for m in sent) and not any(m.get("t") == "origins" for m in sent), [m.get("t") for m in sent][:12])
    v1_paired = await pair_through_relay(PHONE_NAME + " v1", relay_url=v1_url)
    check("phones pair through it from the Tailscale origin", bool(v1_paired))
    if v1_paired:
        v1_phone, v1_hello = await through_relay(v1_paired[0], v1_paired[2], v1_paired[1])
        check("…and connect through it again with their token", v1_hello.get("type") == "hello" and await answers(v1_phone, 91))
        await v1_phone.close()

    # ---------- off ----------
    await call("mobile.set_relay", {"on": False})
    before = v1_tap.accepted
    await asyncio.sleep(3)
    status = await relay_status()
    check("off: no connection, and none made again", not status["on"] and not status["connected"] and v1_tap.accepted == before, (status, v1_tap.accepted - before))
    after = json.loads(ln.unb64((await call("mobile.pair_start"))["url"].split("#p=")[1]))
    check("…and a QR code names no relay", not any(e["t"] == "relay" for e in after["ep"]), after["ep"])
    await call("session.close", {"handle": next(s for s in await call("session.list") if s["id"] == sid)["handle"]})


async def _connected(status):
    s = await status()
    return s if s["on"] and s["connected"] else None


async def _connected_at(status, url, tap):
    """Connected to the relay at `url`, through its tap: a connection to the relay it was on before does not count."""
    s = await status()
    return s if s["on"] and s["connected"] and s["url"] == url and tap.accepted else None


async def _closed(phone):
    return phone.closed is not None


async def _route_of(call, device_id):
    """How the desktop lists a device as connected now (`connection.route` in `mobile.status`), None when it is not."""
    listed = next((x for x in (await call("mobile.status"))["devices"] if x["id"] == device_id), {})
    return (listed.get("connection") or {}).get("route")


async def _route_is(call, device_id, route):
    return await _route_of(call, device_id) == route


async def _not_connected(status):
    s = await status()
    return s if not s["connected"] else None


async def _shows(phone, sid, text):
    return text in phone.text(sid)


def _refused(conn, method, params):
    try:
        conn.call(method, params)
        return None
    except SystemExit as e:  # kelpie-call's way of reporting an error
        return str(e)


TS = ps.write_fake_tailscale(tempfile.mkdtemp(prefix="kelpie-relay-ts-"))


async def run():
    relay_port, open_port = free_port(), free_port()
    registration_key = secrets.token_urlsafe(32)
    workerd = start_workerd(relay_port, {"TICKET_SECRET": secrets.token_urlsafe(32), "REGISTRATION_KEY": registration_key})
    open_workerd = start_workerd(open_port, {"TICKET_SECRET": secrets.token_urlsafe(32), "OPEN_REGISTRATION": "1"})
    phone_tap = await Tap(relay_port).start()
    desk_tap = await Tap(relay_port).start()
    open_tap = await Tap(open_port).start()
    v1_tap = await Tap(relay_port, strip_version=True).start()
    d = ps.Daemon(exe, env={"KELPIE_E2E": "1", "KELPIE_TAILSCALE_BIN": TS[0]})
    await asyncio.to_thread(d.start)
    try:
        await main(d, phone_tap, desk_tap, registration_key, open_tap, open_port, v1_tap)
    finally:
        await asyncio.to_thread(d.stop)
        for tap in (phone_tap, desk_tap, open_tap, v1_tap):
            tap.close()
        for proc in (workerd, open_workerd):
            proc.terminate()
            try:
                proc.wait(10)
            except subprocess.TimeoutExpired:
                proc.kill()


asyncio.run(run())
failed = [n for n, ok in results if not ok]
print(f"\n{len(results) - len(failed)} of {len(results)} passed")
sys.exit(1 if failed else 0)
