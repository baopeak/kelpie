"""Shared by the phone protocol e2e scripts (mobile_protocol.py, mobile_perf.py, mobile_resilience.py,
stress.py): an isolated daemon, pairing a phone, and a phone-like kelpie-link client (link_noise.py) that
applies screen frames, acknowledges them and answers heartbeats the way the page does.

Everything runs in the caller's own process with its own KELPIE_ENDPOINT and data folder, so it
never reaches the user's real daemon. The daemon must be a debug build: only that build trusts this script
as the Kelpie app (`KELPIE_TRUSTED_APP`), which approving a phone needs, and only that build answers the OS verification
from `KELPIE_TEST_USER_PRESENCE` instead of raising a real prompt (`approve` here, unless a script's `env` says `deny`). Its
phone keys live in the OS credential store under the run's own endpoint name, and are deleted when the script ends.
"""
import asyncio
import atexit
import ctypes
import importlib.util
import json
import os
import subprocess
import sys
import tempfile
import threading
import time

import websockets

import link_noise as ln

HERE = os.path.dirname(os.path.abspath(__file__))


def load_call():
    spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(HERE, "..", "kelpie-call.py"))
    kc = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(kc)
    return kc


kc = None
MADE = set()  # the endpoints isolate() made here


def isolate(tag):
    """Points this process at its own daemon endpoint and data folder."""
    os.environ["KELPIE_ENDPOINT"] = f"kelpie-{tag}-{os.getpid()}"
    os.environ["KELPIE_DATA_DIR"] = tempfile.mkdtemp(prefix=f"kelpie-{tag}-")
    for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT"):
        os.environ.pop(k, None)
    global kc
    kc = load_call()
    MADE.add(os.environ["KELPIE_ENDPOINT"])
    atexit.register(forget_secrets, os.environ["KELPIE_ENDPOINT"])
    atexit.register(forget_token, os.environ["KELPIE_ENDPOINT"])
    return os.environ["KELPIE_DATA_DIR"]


def forget_token(endpoint):
    """Deletes the token file a test daemon wrote for `endpoint`, which goes in the real Kelpie folder's `auth` whatever the data folder
    is. Only an endpoint isolate() made here, and only that exact name."""
    if endpoint not in MADE or os.environ.get("KELPIE_ENDPOINT") != endpoint:
        raise ValueError(f"not an endpoint of this run: {endpoint!r}")
    path = kc.token_path()
    if os.path.basename(path) != endpoint + ".token":
        raise ValueError(f"not this endpoint's token: {path!r}")
    try:
        os.remove(path)
    except FileNotFoundError:
        pass


def forget_secrets(endpoint):
    """Deletes the phone keys a test daemon on `endpoint` left in the OS credential store. Only ever a test endpoint's:
    the service name carries the endpoint, and the real daemon's has none."""
    if not endpoint or endpoint.startswith("kelpie-" + os.environ.get("USERNAME", os.environ.get("USER", "")) + "-v"):
        raise ValueError(f"not a test endpoint: {endpoint!r}")
    service = f"Kelpie ({endpoint})"
    for key in ("mobile-link-key", "mobile-device-key"):
        if os.name == "nt":
            ctypes.windll.advapi32.CredDeleteW(f"{key}.{service}", 1, 0)  # CRED_TYPE_GENERIC
        else:
            subprocess.run(["security", "delete-generic-password", "-s", service, "-a", key], capture_output=True, check=False)


class Daemon:
    """A kelpied of this script's own: `Daemon(exe).start()`, `.call(method, params)`, `.stop()`."""

    def __init__(self, exe, env=None, as_app=True):
        """`as_app=False`: only the plain user's connection, for a build that takes no script for the app (a release daemon)."""
        assert os.environ.get("KELPIE_ENDPOINT"), "call isolate() first"
        self.exe = exe
        self.env = env
        self.as_app = as_app
        self.proc = None
        self.conn = None
        self.lock = threading.Lock()

    def start(self):
        trusted = os.pathsep.join(dict.fromkeys(p for p in (sys.executable, getattr(sys, "_base_executable", "")) if p))
        env = {**os.environ, "KELPIE_TEST_USER_PRESENCE": "approve", **(self.env or {}), "KELPIE_TRUSTED_APP": trusted}
        self.proc = subprocess.Popen([self.exe, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env)
        self.conn = None
        for _ in range(200):
            try:
                self.conn = kc.Conn()
                break
            except OSError:
                time.sleep(0.1)
        else:
            raise RuntimeError("the test daemon did not start")
        if not self.as_app:
            return self
        # This connection stands for the app (approving a phone, raising what it may do); other kc.Conn() stay the plain user.
        who = self.conn.call("auth", {"token": kc.user_token(), "client": "app"})
        if not who.get("app"):
            raise RuntimeError("the daemon does not take this script for the app: run the phone e2e scripts against a debug build of kelpied")
        return self

    def call(self, method, params=None):
        with self.lock:  # one connection, used from the test's threads too
            return self.conn.call(method, params or {})

    def stop(self, wait=15):
        """Ends the daemon this script started (and its sessions)."""
        try:
            self.conn.call("daemon.shutdown", {"kill_sessions": True})
        except Exception:
            pass
        end = time.time() + wait
        while self.proc.poll() is None and time.time() < end:
            time.sleep(0.2)
        if self.proc.poll() is None:
            self.proc.kill()

    def restart(self):
        """The daemon stops (its sessions are restored by the next one) and starts again."""
        try:
            self.conn.call("daemon.shutdown", {})
        except Exception:
            pass
        end = time.time() + 15
        while self.proc.poll() is None and time.time() < end:
            time.sleep(0.2)
        if self.proc.poll() is None:
            self.proc.kill()
        return self.start()

    @property
    def pid(self):
        return self.proc.pid


class Device:
    """A paired phone's keys: what `Phone` connects with."""

    def __init__(self, priv, pub, dpk, device_id):
        self.priv, self.pub, self.dpk, self.id = priv, pub, dpk, device_id


def link_url(base):
    return base.replace("http://", "ws://").replace("https://", "wss://") + "link"


def pair_secrets(link):
    """What a pairing link holds: the desktop's public key and the pairing secret."""
    payload = json.loads(ln.unb64(link.split("#p=")[1]))
    return ln.unb64(payload["dpk"]), ln.unb64(payload["psk"])


class PairAttempt:
    """One pairing connection from a QR link, as far as `start` takes it: the handshake and the first sealed message, which
    spends the code and asks the person. `closed` is the close code if the daemon refused, or `silent` when it never answered."""

    def __init__(self, base, link, name, want="read", key=None, **connect):
        """`connect`: what else `websockets.connect` is told (another `host` and `port` to reach `base` through, `origin`, headers)."""
        self.url, self.name, self.want, self.connect = link_url(base), name, want, connect
        self.dpk, self.psk = pair_secrets(link)
        self.priv, self.pub = key or ln.keypair()
        self.session = None
        self.closed = None
        self.silent = False

    async def start(self):
        self.ws = await websockets.connect(self.url, max_size=None, open_timeout=10, ping_interval=None, compression=None, **self.connect)
        init = ln.Initiator(ln.PAIR, self.priv, self.pub, self.dpk, self.psk)
        await self.ws.send(init.first)
        try:
            reply = await asyncio.wait_for(self.ws.recv(), 8)
        except asyncio.TimeoutError:
            self.silent = True
            await self.ws.close()
            return self
        except websockets.ConnectionClosed as e:
            self.closed = e.rcvd.code if e.rcvd else 1006
            self.silent = self.closed == 1006 or e.rcvd is None
            return self
        self.session, _ = init.finish(reply)
        for frame in self.session.seal(json.dumps({"type": "pair", "name": self.name, "want": self.want})):
            await self.ws.send(frame)
        return self

    def code(self):
        return self.session.pair_code()

    async def outcome(self, timeout=30):
        """What the phone is told once the person decided: `{"type": "paired", ...}`, `declined`, `timeout`, or None if the socket ended."""
        try:
            return json.loads(self.session.open(await asyncio.wait_for(self.ws.recv(), timeout)))
        except (asyncio.TimeoutError, websockets.ConnectionClosed):
            return None
        finally:
            await self.ws.close()


def pending_request(d, name, timeout=20):
    """The desktop's pending pairing request of the phone called `name`."""
    end = time.time() + timeout
    while time.time() < end:
        found = [p for p in d.call("mobile.status")["pending"] if p["name"] == name]
        if found:
            return found[0]
        time.sleep(0.1)
    raise RuntimeError(f"no pairing request from {name}")


def pair(d, base, name, can_write):
    """Pairs a phone with the daemon's phone access at `base` (http://host:port/), typing its code as the person would; returns
    (its keys, device id). A busy machine paces pairing (close code 4503): it is tried again a few seconds later."""
    for attempt in range(8):
        try:
            return _pair_once(d, base, name, can_write)
        except RuntimeError as e:
            if "4503" not in str(e) or attempt == 7:
                raise
            time.sleep(6)


def _pair_once(d, base, name, can_write):
    # On a loop of its own: callers are plain code and coroutines alike, and approving here takes blocking daemon calls.
    link = d.call("mobile.pair_start")["url"]
    attempt = PairAttempt(base, link, name, "write" if can_write else "read")

    async def talk():
        await attempt.start()
        if attempt.session is None:
            return None
        return await attempt.outcome()

    got = {}
    t = threading.Thread(target=lambda: got.update(told=asyncio.run(talk())), daemon=True)
    t.start()
    end = time.time() + 20
    while attempt.session is None and t.is_alive() and time.time() < end:
        time.sleep(0.02)
    if attempt.session is None:
        t.join(1)
        raise RuntimeError(f"pairing refused: close code {attempt.closed}, silent {attempt.silent}")
    request = pending_request(d, name)
    d.call("mobile.approve", {"id": request["id"], "allow": True, "code": attempt.code(), "permission": "write" if can_write else "read"})
    t.join(30)
    told = got.get("told") or {}
    if told.get("type") != "paired":
        raise RuntimeError("pairing failed: " + str(told))
    return Device(attempt.priv, attempt.pub, attempt.dpk, told["device"]["id"]), told["device"]["id"]


FAKE_TAILNET_NAME = "desk.tail1234.ts.net"
FAKE_TAILSCALE_READY = json.dumps({"BackendState": "Running", "Self": {"DNSName": FAKE_TAILNET_NAME + "."}, "CertDomains": [FAKE_TAILNET_NAME]})


def write_fake_tailscale(folder):
    """A program that answers like the Tailscale CLI, from a file, and keeps a list of what it was asked. For a daemon started with
    `KELPIE_E2E` and `KELPIE_TAILSCALE_BIN` set to the first of what this returns (program, state file, log), which then has no way to reach the
    person's real Tailscale. It starts stopped, with nothing served; `fake_tailscale_says` changes that."""
    state, log = os.path.join(folder, "fake-tailscale-state.json"), os.path.join(folder, "fake-tailscale-calls.log")
    script = os.path.join(folder, "fake_tailscale.py")
    program = os.path.join(folder, "fake-tailscale.cmd" if os.name == "nt" else "fake-tailscale")
    with open(state, "w", encoding="utf-8") as f:
        json.dump({"status": '{"BackendState":"Stopped"}', "serve": "{}"}, f)
    open(log, "w", encoding="utf-8").close()
    with open(script, "w", encoding="utf-8") as f:
        f.write(
            "import json, sys\n"
            f"state_file, log = {state!r}, {log!r}\n"
            "args = ' '.join(sys.argv[1:])\n"
            "open(log, 'a', encoding='utf-8').write(args + '\\n')\n"
            "state = json.load(open(state_file, encoding='utf-8'))\n"
            "if args == 'status --json':\n    print(state['status'])\n"
            "elif args == 'serve status --json':\n    sys.exit(1) if state.get('serve_down') else print(state['serve'])\n"
            "elif args == 'serve --https=8443 off':\n    state['serve'] = '{}'\n"
            "elif args.startswith('serve --bg --https=8443 http://127.0.0.1:'):\n"
            "    port = args.rsplit(':', 1)[1]\n"
            f"    state['serve'] = json.dumps({{'TCP': {{'8443': {{'HTTPS': True}}}}, 'Web': {{'{FAKE_TAILNET_NAME}:8443': {{'Handlers': {{'/': {{'Proxy': 'http://127.0.0.1:' + port}}}}}}}}}})\n"
            "else:\n    sys.exit(3)\n"
            "json.dump(state, open(state_file, 'w', encoding='utf-8'))\n")
    with open(program, "w", encoding="utf-8") as f:
        if os.name == "nt":
            f.write(f'@"{sys.executable}" "{script}" %*\r\n')
        else:
            f.write(f'#!/bin/sh\nexec "{sys.executable}" "{script}" "$@"\n')
    if os.name != "nt":
        os.chmod(program, 0o755)
    return program, state, log


def fake_tailscale_says(state_file, **state):
    with open(state_file, encoding="utf-8") as f:
        now = json.load(f)
    with open(state_file, "w", encoding="utf-8") as f:
        json.dump({**now, **state}, f)


def fake_tailscale_calls(log):
    if not os.path.exists(log):
        return []
    with open(log, encoding="utf-8") as f:
        return f.read().splitlines()


WIRE_KEPT = 1 << 18
HELD_FRAMES = 8  # screens a phone holds for one session (`HELD_FRAMES` in screen-sync.ts)


class Phone:
    """A phone: connects to /link, keeps every message it was sent, applies screen frames to the lines it
    holds (a frame that follows one it does not hold is recorded in `lost`), acknowledges them (every
    second frame, or 250 ms after the first one not acknowledged) and answers the daemon's heartbeats.
    `auto_ack` and `auto_hb` switch those off for tests of what a silent phone gets."""

    def __init__(self, base, device, auto_ack=True, auto_hb=True, caps=True):
        self.url = link_url(base)
        self.device = device
        self.auto_ack, self.auto_hb = auto_ack, auto_hb
        self.caps = ["ack", "hb"] if caps else None  # None: a page from before frames were acknowledged
        self.log = []
        self.lines = {}
        self.rev = {}
        # By session: the screens a frame may still be built on (frame number -> lines), the last frame
        # acknowledged, and how many were applied since.
        self.held = {}
        self.acked = {}
        self.unacked = {}
        self.lost = []
        self.closed = None
        self.close_reason = None
        self._next = 0
        self.session = None
        self.send_lock = asyncio.Lock()
        self.wire = bytearray()  # the first bytes seen on the socket, for checking nothing readable passes
        self.reader = self.acker = None

    async def open(self):
        self.ws = await websockets.connect(self.url, max_size=None, open_timeout=10, ping_interval=None, compression=None)
        init = ln.Initiator(ln.RECONNECT, self.device.priv, self.device.pub, self.device.dpk)
        try:
            await self.ws.send(init.first)
            reply = await asyncio.wait_for(self.ws.recv(), 10)
        except websockets.ConnectionClosed as e:
            self.closed = e.rcvd.code if e.rcvd else 1006
            return None
        self.session, _ = init.finish(reply)
        await self.send(**({"caps": self.caps} if self.caps else {}))
        self.reader = asyncio.ensure_future(self._read())
        self.acker = asyncio.ensure_future(self._flush_acks())
        await self.wait(lambda m: m["type"] == "hello", 10)
        if not any(m["type"] == "hello" for m in self.log):
            raise ConnectionError(f"no hello from the daemon: closed {self.closed}, reason {self.close_reason!r}")
        return self.hello()

    def resume_from(self, other):
        """Carries what `other` (a connection that ended) held over to this one: the screens it can resume from."""
        self.held = {sid: dict(h) for sid, h in other.held.items()}
        self.acked = dict(other.acked)
        for sid, rev in self.acked.items():
            self.lines[sid], self.rev[sid] = self.held[sid][rev], rev

    def hello(self):
        return next(m for m in self.log if m["type"] == "hello")

    async def _read(self):
        try:
            async for raw in self.ws:
                if len(self.wire) < WIRE_KEPT:
                    self.wire += raw[: WIRE_KEPT - len(self.wire)]
                whole = self.session.open(raw)
                if whole is None:
                    continue
                m = json.loads(whole)
                m["_t"] = time.monotonic()
                m["_w"] = time.time()
                m["_size"] = len(whole)
                self.log.append(m)
                if m["type"] == "screen":
                    await self._apply(m)
                elif m["type"] == "hb" and not m.get("echo") and self.auto_hb:
                    await self.send(type="hb", echo=True, n=m.get("n"))
        except websockets.ConnectionClosed as e:
            self.closed = e.rcvd.code if e.rcvd else 1006
            self.close_reason = e.rcvd.reason if e.rcvd else None
        finally:
            self.closed = self.closed or 1006

    async def _apply(self, m):
        sid = m["id"]
        held = self.held.setdefault(sid, {})
        if m["base"] is not None and m["base"] not in held:
            self.lost.append(m)
            return
        old = [] if m["base"] is None else held[m["base"]]
        lines = (old[m["shift"]:] + [""] * m["height"])[: m["height"]]
        for i, text in m["rows"]:
            lines[i] = text
        # The daemon builds on `base` from now on: older screens are not needed. A whole frame says nothing of that: it may have
        # heard only an earlier acknowledgement, so nothing is let go for one, beyond the bound. As the page does (screen-sync.ts).
        if m["base"] is not None:
            for rev in [r for r in held if r < m["base"]]:
                del held[rev]
        held.pop(m["rev"], None)
        held[m["rev"]] = lines
        while len(held) > HELD_FRAMES:
            del held[next(iter(held))]
        self.lines[sid], self.rev[sid] = lines, m["rev"]
        self.unacked[sid] = self.unacked.get(sid, 0) + 1
        if self.auto_ack and self.unacked[sid] >= 2:
            await self.ack(sid)

    async def ack(self, sid):
        """Tells the daemon the last frame applied for `sid` is held."""
        self.unacked[sid] = 0
        self.acked[sid] = self.rev[sid]
        await self.send(type="ack", id=sid, rev=self.rev[sid])

    async def _flush_acks(self):
        """A frame not followed by another is acknowledged within about 250 ms."""
        since = {}
        try:
            while True:
                await asyncio.sleep(0.05)
                now = time.monotonic()
                for sid, n in list(self.unacked.items()):
                    if not n:
                        since.pop(sid, None)
                    elif self.auto_ack and now - since.setdefault(sid, now) >= 0.25 and self.closed is None:
                        since.pop(sid, None)
                        await self.ack(sid)
        except (asyncio.CancelledError, websockets.ConnectionClosed):
            pass

    def text(self, sid):
        return "\n".join(self.lines.get(sid, []))

    def mark(self):
        return len(self.log)

    async def wait(self, pred, timeout=10, since=0):
        """The first message from `since` on that satisfies `pred`, or None."""
        end = time.monotonic() + timeout
        seen = since
        while time.monotonic() < end:
            while seen < len(self.log):
                if pred(self.log[seen]):
                    return self.log[seen]
                seen += 1
            if self.closed is not None:
                return None
            await asyncio.sleep(0.01)
        return None

    async def send(self, **msg):
        async with self.send_lock:  # sealing numbers the frames: they go out in the order they were sealed
            for frame in self.session.seal(json.dumps(msg)):
                await self.ws.send(frame)

    async def ask(self, timeout=10, **msg):
        """Sends a request and returns its ack or error."""
        self._next += 1
        req = f"r{self._next}"
        start = self.mark()
        await self.send(req=req, **msg)
        return await self.wait(lambda m: m.get("req") == req, timeout, start)

    async def close(self):
        try:
            await self.ws.close()
        except Exception:  # noqa: BLE001
            pass
        for task in (self.reader, self.acker):
            if task:
                task.cancel()


# A Claude-style chat agent that takes one turn at a time. It traces when a message arrives (`ARRIVE <text>`) and when its
# turn ends (`END <text>`) in the file given as its first argument, keeps a turn open while the file named by its second
# argument exists, and ends the turn early (`STOP`) on an interrupt. Run it as `python -u turn_agent.py <trace> <hold>`.
TURN_AGENT_SOURCE = """import json, os, queue, sys, threading, time
sys.stdin.reconfigure(encoding='utf-8'); sys.stdout.reconfigure(encoding='utf-8')
trace, hold = sys.argv[1], sys.argv[2]
def out(m):
    sys.stdout.write(json.dumps(m) + '\\n'); sys.stdout.flush()
def note(s):
    open(trace, 'a', encoding='utf-8').write(s + '\\n')
turns, stop = queue.Queue(), threading.Event()
def reader():
    for line in sys.stdin:
        m = json.loads(line)
        if m.get('type') == 'control_request' and m['request']['subtype'] == 'interrupt':
            note('STOP'); stop.set()
            out({'type': 'control_response', 'response': {'subtype': 'success', 'request_id': m['request_id'], 'response': {}}})
        elif m.get('type') == 'user':
            note('ARRIVE ' + m['message']['content'][0]['text']); turns.put(m)
threading.Thread(target=reader, daemon=True).start()
while True:
    m = turns.get(); stop.clear()
    text = m['message']['content'][0]['text']
    out({'type': 'user', 'message': m['message']})
    out({'type': 'assistant', 'message': {'id': 'a-' + text, 'role': 'assistant', 'content': [{'type': 'text', 'text': 'on it: ' + text}]}})
    while os.path.exists(hold) and not stop.is_set():
        time.sleep(0.05)
    note('END ' + text)
    out({'type': 'result', 'subtype': 'success', 'is_error': False, 'result': 'done'})
"""


def write_turn_agent(folder):
    """Writes the turn-taking chat agent into `folder`; returns (script path, trace path, hold path)."""
    script = os.path.join(folder, "turn_agent.py")
    with open(script, "w", encoding="utf-8") as f:
        f.write(TURN_AGENT_SOURCE)
    return script, os.path.join(folder, "turn-agent.trace"), os.path.join(folder, "turn-agent.hold")


def wait_for(fn, timeout):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(0.2)
    return None


async def async_wait_for(fn, timeout):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        await asyncio.sleep(0.1)
    return None


def rss_mb(pid):
    """Resident memory of a process in MiB (psutil)."""
    import psutil
    return psutil.Process(pid).memory_info().rss / (1 << 20)
