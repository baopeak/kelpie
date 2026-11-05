"""The relay against a real daemon and a real relay on this machine: pairing and working through it with no direct path, padding, what
the relay's capture holds, a revoke made while the relay was away, and off.

    python -X utf8 scripts/e2e/mobile_relay.py <debug kelpied.exe>

Before it: `cd relay && pnpm install --frozen-lockfile && pnpm build:selfhost` (the self-hosted bundle this runs on workerd).

Daemon only, no app and no browser: its own endpoint and data folder (phone_support.isolate), a fake Tailscale CLI (never the real one) so
that the daemon has a Tailscale origin for the relay to let phones in from, and workerd from relay/node_modules with secrets made for this
run and held by its process alone. Two TCP taps sit between the daemon and the relay and between the phone and the relay: they keep what
passes, so the test can look for plaintext in it, and the daemon's can be cut to play a relay that went away. The phone speaks kelpie-link
(link_noise.py) and connects only to the relay: it is never given the daemon's own address. Nothing here reaches Cloudflare.

1. The daemon makes its room with the registration key and connects; the relay holds the Tailscale origin.
2. A phone pairs through the relay with the QR code's single-use token, from the Tailscale origin; the request names "the relay"; the
   pairing gives it a relay token. A second use of the pairing token is refused.
3. Reconnected with its token, the phone types into a shell and reads the result; every frame after the hello is one of the padded sizes.
4. Neither tap's capture holds what was typed, what came back, or the phone's name.
5. With the relay cut off from the daemon, the phone is removed; a phone socket the relay still admits hears `desktop_offline`; once the
   daemon is back, the relay closes it (4002) before it reaches anything, and the token is refused from then on.
6. Turning the relay off closes the daemon's socket and dials nothing again.
"""
import asyncio
import base64
import glob
import hashlib
import hmac
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


def hkdf_sha256(ikm, info, n=32):
    prk = hmac.new(b"\0" * 32, ikm, hashlib.sha256).digest()
    return hmac.new(prk, info + b"\x01", hashlib.sha256).digest()[:n]


def b64(data):
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


class Tap:
    """A TCP forwarder that keeps what passes both ways, and can be cut (its connections dropped, new ones refused) and restored."""

    def __init__(self, target):
        self.target, self.up, self.seen, self.accepted, self.open = target, True, bytearray(), 0, set()

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

        async def pipe(src, dst):
            try:
                while data := await src.read(65536):
                    if len(self.seen) < 16 << 20:
                        self.seen += data
                    dst.write(data)
                    await dst.drain()
            except (OSError, asyncio.IncompleteReadError):
                pass
            finally:
                dst.transport.abort()

        await asyncio.gather(pipe(reader, up_writer), pipe(up_reader, writer))
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
    proc = subprocess.Popen([found[0], "serve", "config.capnp"], cwd=run, env={**os.environ, **secrets_env}, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
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


async def main(d, phone_tap, desk_tap, registration_key):
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
    wrong = await asyncio.to_thread(lambda: _refused(d, "mobile.set_relay", {"on": True, "url": url, "operator_key": "not-the-key"}))
    check("a wrong registration key makes no room", "did not take that registration key" in (wrong or ""), wrong)
    plain = await asyncio.to_thread(lambda: _refused(ps.kc.Conn(), "mobile.set_relay", {"on": True, "url": url, "operator_key": registration_key}))
    check("the plain user token cannot turn the relay on", "needs the Kelpie app" in (plain or ""), plain)
    await call("mobile.set_relay", {"on": True, "url": url, "operator_key": registration_key})
    status = await until(lambda: _connected(relay_status), 20)
    check("the daemon makes its room and connects", bool(status), status)
    check("…with the Tailscale origin for phones", status and status["origin"] == ORIGIN, status)

    # ---------- pairing through the relay ----------
    link = (await call("mobile.pair_start"))["url"]
    payload = json.loads(ln.unb64(link.split("#p=")[1]))
    relay_ep = next((e for e in payload["ep"] if e["t"] == "relay"), None)
    check("the QR code names the relay and the room's ticket", relay_ep and relay_ep["a"] == url and "." in relay_ep["r"], payload["ep"])
    if not relay_ep:
        return
    ticket = relay_ep["r"]
    vectors = json.load(open(os.path.join(RELAY, "test", "vectors.json"), encoding="utf-8"))
    check("the pairing token is derived as the relay's vectors say", b64(hkdf_sha256(bytes.fromhex(vectors["psk_hex"]), b"kelpie-relay-pair")) == vectors["pair_token"])
    pair_token = b64(hkdf_sha256(ln.unb64(payload["psk"]), b"kelpie-relay-pair"))
    phone_base = f"ws://127.0.0.1:{phone_tap.port}/v1/phone?t={ticket}"
    attempt = ps.PairAttempt(BASE, link, PHONE_NAME, "write", origin=ORIGIN)
    attempt.url = f"{phone_base}&p={pair_token}"
    await attempt.start()
    check("the pairing handshake goes through the relay", attempt.session is not None, f"closed {attempt.closed}, silent {attempt.silent}")
    if attempt.session is None:
        return
    request = await asyncio.to_thread(ps.pending_request, d, PHONE_NAME)
    check("the request says it came through the relay", request["address"] == "the relay", request["address"])
    approve = asyncio.ensure_future(call("mobile.approve", {"id": request["id"], "allow": True, "code": attempt.code(), "permission": "write"}))
    paired = await attempt.outcome()
    await approve
    check("paired, with a relay token", paired and paired["type"] == "paired" and paired.get("relay", {}).get("token"), paired)
    if not (paired and paired.get("relay")):
        return
    try:
        again = await websockets.connect(f"{phone_base}&p={pair_token}", origin=ORIGIN, open_timeout=5)
        await again.close()
        check("a pairing token is let in once", False)
    except websockets.exceptions.InvalidHandshake as e:
        check("a pairing token is let in once", status_of(e) == 403, status_of(e))
    token = paired["relay"]["token"]
    device_id = paired["device"]["id"]
    device = ps.Device(attempt.priv, attempt.pub, attempt.dpk, device_id)
    check("the device is on a secure origin", next(x for x in (await call("mobile.status"))["devices"] if x["id"] == device_id)["secure_origin"])

    # ---------- working through it ----------
    phone = ps.Phone(BASE, device, origin=ORIGIN)
    phone.url = f"{phone_base}&k={token}"
    phone.caps = ["ack", "hb", "relay", "pad"]
    hello = await phone.open()
    check("the phone reconnects through the relay, and may type", hello["device"]["permission"] == "write", hello["device"])
    check("…and the computer says it reads padded chunks", hello.get("pad") is True)
    phone.session.pad = True
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
    await phone.close()

    # ---------- what the relay saw ----------
    for name, tap in (("phone side", phone_tap), ("computer side", desk_tap)):
        seen = bytes(tap.seen)
        check(f"the {name} of the relay carried traffic", len(seen) > 1000, len(seen))
        check(f"…and none of it is what was typed, printed or named", MARKER[::-1].encode() not in seen and PHONE_NAME.encode() not in seen and b"relay-shell" not in seen)

    # ---------- a revoke while the relay is away ----------
    desk_tap.cut()
    down = await until(lambda: _not_connected(relay_status), 60)
    check("the daemon sees the relay gone", bool(down), down)
    await call("mobile.revoke", {"id": device_id})
    stale = await websockets.connect(f"{phone_base}&k={token}", origin=ORIGIN, open_timeout=5)
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
        late = await websockets.connect(f"{phone_base}&k={token}", origin=ORIGIN, open_timeout=5)
        await late.close()
        check("its token is refused from then on", False)
    except websockets.exceptions.InvalidHandshake as e:
        check("its token is refused from then on", status_of(e) == 403, status_of(e))

    # ---------- off ----------
    await call("mobile.set_relay", {"on": False})
    before = desk_tap.accepted
    await asyncio.sleep(3)
    status = await relay_status()
    check("off: no connection, and none made again", not status["on"] and not status["connected"] and desk_tap.accepted == before, (status, desk_tap.accepted - before))
    after = json.loads(ln.unb64((await call("mobile.pair_start"))["url"].split("#p=")[1]))
    check("…and a QR code names no relay", not any(e["t"] == "relay" for e in after["ep"]), after["ep"])
    await call("session.close", {"handle": next(s for s in await call("session.list") if s["id"] == sid)["handle"]})


async def _connected(status):
    s = await status()
    return s if s["on"] and s["connected"] else None


async def _not_connected(status):
    s = await status()
    return s if not s["connected"] else None


async def _shows(phone, sid, text):
    return text in phone.text(sid)


def status_of(e):
    """The HTTP status a refused upgrade answered, from either of websockets' client implementations."""
    return getattr(getattr(e, "response", None), "status_code", None) or getattr(e, "status_code", None)


def _refused(conn, method, params):
    try:
        conn.call(method, params)
        return None
    except Exception as e:  # noqa: BLE001
        return str(e)


TS = ps.write_fake_tailscale(tempfile.mkdtemp(prefix="kelpie-relay-ts-"))


async def run():
    relay_port = free_port()
    registration_key = secrets.token_urlsafe(32)
    workerd = start_workerd(relay_port, {"TICKET_SECRET": secrets.token_urlsafe(32), "REGISTRATION_KEY": registration_key})
    phone_tap = await Tap(relay_port).start()
    desk_tap = await Tap(relay_port).start()
    d = ps.Daemon(exe, env={"KELPIE_E2E": "1", "KELPIE_TAILSCALE_BIN": TS[0]})
    await asyncio.to_thread(d.start)
    try:
        await main(d, phone_tap, desk_tap, registration_key)
    finally:
        await asyncio.to_thread(d.stop)
        phone_tap.close()
        desk_tap.close()
        workerd.terminate()
        try:
            workerd.wait(10)
        except subprocess.TimeoutExpired:
            workerd.kill()


asyncio.run(run())
failed = [n for n, ok in results if not ok]
print(f"\n{len(results) - len(failed)} of {len(results)} passed")
sys.exit(1 if failed else 0)
