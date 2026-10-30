"""Shared by the phone protocol e2e scripts (mobile_protocol.py, mobile_perf.py, mobile_resilience.py,
stress.py): an isolated daemon, pairing a phone, and a phone-like WebSocket client that applies screen
frames, acknowledges them and answers heartbeats the way the page does.

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

    def __init__(self, exe, env=None):
        assert os.environ.get("KELPIE_ENDPOINT"), "call isolate() first"
        self.exe = exe
        self.env = env
        self.proc = None
        self.conn = None
        self.lock = threading.Lock()

    def start(self):
        env = {**os.environ, **self.env} if self.env else None
        self.proc = subprocess.Popen([self.exe, "serve"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env)
        self.conn = None
        for _ in range(200):
            try:
                self.conn = kc.Conn()
                return self
            except OSError:
                time.sleep(0.1)
        raise RuntimeError("the test daemon did not start")

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


def pair(d, base, name, can_write):
    """Pairs a phone with the daemon's phone access at `base` (http://host:port/); returns (token, device id).
    A busy machine paces pairing (503): it is tried again a few seconds later."""
    for attempt in range(8):
        try:
            return _pair_once(d, base, name, can_write)
        except RuntimeError as e:
            if "503" not in str(e) or attempt == 7:
                raise
            time.sleep(6)


def _pair_once(d, base, name, can_write):
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
    """A phone: connects to /ws, keeps every message it was sent, applies screen frames to the lines it
    holds (a frame that follows one it does not hold is recorded in `lost`), acknowledges them (every
    second frame, or 250 ms after the first one not acknowledged) and answers the daemon's heartbeats.
    `auto_ack` and `auto_hb` switch those off for tests of what a silent phone gets."""

    def __init__(self, base, token, auto_ack=True, auto_hb=True, caps=True):
        self.url = base.replace("http://", "ws://").replace("https://", "wss://") + "ws"
        self.token = token
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

    async def open(self):
        self.ws = await websockets.connect(self.url, max_size=None, open_timeout=10, ping_interval=None)
        await self.ws.send(json.dumps({"token": self.token, **({"caps": self.caps} if self.caps else {})}))
        self.reader = asyncio.ensure_future(self._read())
        self.acker = asyncio.ensure_future(self._flush_acks())
        await self.wait(lambda m: m["type"] == "hello", 10)
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
                m = json.loads(raw)
                m["_t"] = time.monotonic()
                m["_w"] = time.time()
                m["_size"] = len(raw)
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
        # The daemon builds on `base` from now on: older screens are not needed. A whole frame says
        # nothing of that (an acknowledgement of ours may still be on its way), so only what we have
        # acknowledged ourselves is let go.
        keep = self.acked.get(sid, 0) if m["base"] is None else m["base"]
        for rev in [r for r in held if r < keep]:
            del held[rev]
        held[m["rev"]] = lines
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
        self.acker.cancel()


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
