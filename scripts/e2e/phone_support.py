"""Shared by the phone protocol e2e scripts (mobile_protocol.py, mobile_perf.py): an isolated daemon,
pairing a phone, and a phone-like WebSocket client that applies screen frames the way the page does.

Everything runs in the caller's own process with its own KELPIE_ENDPOINT and data folder, so it
never reaches the user's real daemon.
"""
import asyncio
import importlib.util
import json
import os
import subprocess
import tempfile
import threading
import time
import urllib.request

import websockets

HERE = os.path.dirname(os.path.abspath(__file__))


def load_call():
    spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(HERE, "..", "kelpie-call.py"))
    kc = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(kc)
    return kc


kc = None


def isolate(tag):
    """Points this process at its own daemon endpoint and data folder."""
    os.environ["KELPIE_ENDPOINT"] = f"kelpie-{tag}-{os.getpid()}"
    os.environ["KELPIE_DATA_DIR"] = tempfile.mkdtemp(prefix=f"kelpie-{tag}-")
    for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT"):
        os.environ.pop(k, None)
    global kc
    kc = load_call()
    return os.environ["KELPIE_DATA_DIR"]


class Daemon:
    """A kelpied of this script's own: `Daemon(exe).start()`, `.call(method, params)`, `.stop()`."""

    def __init__(self, exe):
        assert os.environ.get("KELPIE_ENDPOINT"), "call isolate() first"
        self.exe = exe
        self.proc = None
        self.conn = None

    def start(self):
        self.proc = subprocess.Popen([self.exe, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.conn = None
        for _ in range(200):
            try:
                self.conn = kc.Conn()
                return self
            except OSError:
                time.sleep(0.1)
        raise RuntimeError("the test daemon did not start")

    def call(self, method, params=None):
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


def pair(d, base, name, can_write):
    """Pairs a phone with the daemon's phone access at `base` (http://host:port/); returns (token, device id)."""
    link = d.call("mobile.pair_start")["url"]
    code = link.split("#p=")[1]
    got = {}

    def post():
        req = urllib.request.Request(base + "pair", data=json.dumps({"code": code, "name": name}).encode(), method="POST", headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                got["token"] = json.loads(r.read())["token"]
        except Exception as e:  # noqa: BLE001
            got["error"] = str(e)

    t = threading.Thread(target=post, daemon=True)
    t.start()
    end = time.time() + 20
    pending = []
    while not pending and time.time() < end:
        pending = d.call("mobile.status")["pending"]
        time.sleep(0.1)
    if not pending:
        raise RuntimeError("no pairing request arrived: " + str(got))
    d.call("mobile.approve", {"id": pending[0]["id"], "allow": True, "can_write": can_write})
    t.join(30)
    if "token" not in got:
        raise RuntimeError("pairing failed: " + str(got))
    device = next(x for x in d.call("mobile.status")["devices"] if x["name"] == name)
    return got["token"], device["id"]


class Phone:
    """A phone: connects to /ws, keeps every message it was sent, and applies screen frames to the
    lines it holds (a frame that follows one it does not hold is recorded in `lost`)."""

    def __init__(self, base, token):
        self.url = base.replace("http://", "ws://").replace("https://", "wss://") + "ws"
        self.token = token
        self.log = []
        self.lines = {}
        self.rev = {}
        self.lost = []
        self.closed = None
        self._next = 0

    async def open(self):
        self.ws = await websockets.connect(self.url, max_size=None, open_timeout=10, ping_interval=None)
        await self.ws.send(json.dumps({"token": self.token}))
        self.reader = asyncio.ensure_future(self._read())
        await self.wait(lambda m: m["type"] == "hello", 10)
        return self.hello()

    def hello(self):
        return next(m for m in self.log if m["type"] == "hello")

    async def _read(self):
        try:
            async for raw in self.ws:
                m = json.loads(raw)
                m["_t"] = time.monotonic()
                m["_w"] = time.time()
                self.log.append(m)
                if m["type"] == "screen":
                    self._apply(m)
        except websockets.ConnectionClosed as e:
            self.closed = e.rcvd.code if e.rcvd else 1006
        finally:
            self.closed = self.closed or 1006

    def _apply(self, m):
        sid = m["id"]
        if m["base"] is not None and m["base"] != self.rev.get(sid):
            self.lost.append(m)
            return
        old = [] if m["base"] is None else self.lines.get(sid, [])
        lines = (old[m["shift"]:] + [""] * m["height"])[: m["height"]]
        for i, text in m["rows"]:
            lines[i] = text
        self.lines[sid], self.rev[sid] = lines, m["rev"]

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
        await self.ws.send(json.dumps(msg))

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
        self.reader.cancel()


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
