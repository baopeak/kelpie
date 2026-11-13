"""What the screen scripts share (screen.py, screen_net.py): this process's own daemon, data folder and fake Tailscale, the app started by its
own tauri-driver, the phones given control, and an emulated phone that reads the screen's binary messages.

    import screen_support as ss
    setup = ss.prepare("screen", __doc__)     # first: before anything starts, everything this process starts is isolated
    d, run = ss.start(setup)                  # the daemon, the driver and the app; checks the app reached this daemon
    ...
    ss.finish(setup, d, run, "window")        # stops only what this script started, by PID

The user's daemon, data, Tailscale and keyring are never reached: `prepare` sets this process's own endpoint and data folder
(phone_support.isolate) before anything is started, and `finish` checks the endpoint is still this script's before stopping anything.
"""
import asyncio
import ctypes
import ctypes.wintypes as wt
import json
import os
import shutil
import socket
import struct
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import phone_support as ps  # noqa: E402

# Filled by `prepare`, once this process is isolated.
Run = app_daemon = None
results = []
procs = {}


class Setup:
    """Where this run's files are, and the address the phones reach the daemon at."""

    def __init__(self, tag, scratch, app):
        self.tag = tag
        self.scratch = scratch
        self.app = app
        self.kelpied = os.path.join(os.path.dirname(app), "kelpied.exe")
        self.helper = os.path.join(os.path.dirname(app), "kelpie-screen.exe")
        self.work = os.path.join(scratch, f"{tag}-{os.getpid()}")
        self.port = 48900 + os.getpid() % 400
        self.base = f"http://127.0.0.1:{self.port}/"
        self.origin = f"{ps.FAKE_TAILNET_NAME}:8443"
        self.data = None
        self.ts_bin = self.ts_state = self.ts_log = None
        self.native = self.tauri_driver = None


def prepare(tag, doc):
    """Reads `<scratch-dir> <app>` from the command line, isolates this process (own endpoint `kelpie-<tag>-…` and data folder) and checks
    the tools are there. Nothing has started yet."""
    global Run, app_daemon
    if len(sys.argv) < 3 or os.name != "nt":
        raise SystemExit(doc)
    s = Setup(tag, os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2]))
    os.makedirs(s.scratch, exist_ok=True)
    os.makedirs(s.work)
    s.data = ps.isolate(tag)
    os.environ["KELPIE_DAEMON_PATH"] = s.kelpied
    os.environ["KELPIE_SCREEN_PATH"] = s.helper
    os.environ["KELPIE_E2E"] = "1"
    os.environ.pop("KELPIE_ALLOW_REAL_DAEMON", None)
    import smoke_support  # noqa: E402 - only once this process is isolated

    Run, app_daemon = smoke_support.Run, smoke_support.daemon
    s.native, s.tauri_driver = smoke_support.native_driver(), smoke_support.tauri_driver()
    if not s.native or not s.tauri_driver:
        raise SystemExit(f"needs tauri-driver (PATH or ~/.cargo/bin) and {smoke_support.native_driver_hint()}")
    for what, path in (("app", s.app), ("daemon next to the app", s.kelpied), ("screen helper next to the app", s.helper)):
        if not os.path.exists(path):
            raise SystemExit(f"no {what}: {path}")
    return s


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail[:300]})" if detail and not ok else ""), flush=True)


def port_open(port):
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", port)) == 0


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def until(fn, timeout=30, step=0.2):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(step)
    return None


def now_ms():
    return int(time.time() * 1000)


class RECT(ctypes.Structure):
    _fields_ = [("left", wt.LONG), ("top", wt.LONG), ("right", wt.LONG), ("bottom", wt.LONG)]


def frame_bounds(hwnd):
    """The window as DWM draws it: what the screen helper captures, so the picture's pixel (0, 0) is its corner."""
    r = RECT()
    ctypes.windll.dwmapi.DwmGetWindowAttribute(wt.HWND(hwnd), 9, ctypes.byref(r), ctypes.sizeof(r))  # DWMWA_EXTENDED_FRAME_BOUNDS
    return r.left, r.top, r.right - r.left, r.bottom - r.top


def cursor():
    """Where the pointer is, in physical pixels whatever this process's DPI awareness."""
    p = wt.POINT()
    ctypes.windll.user32.GetPhysicalCursorPos(ctypes.byref(p))
    return p.x, p.y


def to_front(hwnd):
    """Brings a window to the front from this process, as the screen helper does: attached for a moment to the thread in front."""
    u = ctypes.windll.user32
    fg = u.GetForegroundWindow()
    them, me = u.GetWindowThreadProcessId(fg, None), ctypes.windll.kernel32.GetCurrentThreadId()
    attached = bool(them) and u.AttachThreadInput(me, them, True)
    u.BringWindowToTop(hwnd)
    u.SetForegroundWindow(hwnd)
    if attached:
        u.AttachThreadInput(me, them, False)
    return True


# ---------- processes this script starts ----------


def spawn(name, argv):
    procs[name] = subprocess.Popen(argv, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def stop(name):
    p = procs.get(name)
    if p is not None and p.poll() is None:
        subprocess.run(["taskkill", "/PID", str(p.pid), "/T", "/F"], capture_output=True)
        try:
            p.wait(timeout=10)
        except subprocess.TimeoutExpired:
            print(f"{name} (pid {p.pid}) did not stop", flush=True)


# ---------- the phone ----------


def media_header(b):
    """What the screen's binary message says (crates/kelpie-screen/src/wire.rs), without its picture."""
    kind = b[1]
    if kind == 1:
        n = b[3]
        seq, capture = struct.unpack_from("<II", b, 4)
        src_w, src_h, coded_w, coded_h = struct.unpack_from("<HHHH", b, 12)
        au = bytes(b[20 + n * 8:])
        nals = [au[i + 3] & 0x1F for i in range(len(au) - 3) if au[i:i + 3] == b"\x00\x00\x01"]
        return {"type": "sc_media", "kind": "video", "key": bool(b[2] & 1), "seq": seq, "capture_ms": capture, "src": (src_w, src_h), "coded": (coded_w, coded_h), "nals": nals, "_size": len(b)}
    if kind == 2:
        return {"type": "sc_media", "kind": "tile", "format": b[2], "seq": struct.unpack_from("<I", b, 4)[0], "_size": len(b)}
    if kind == 3:
        x, y = struct.unpack_from("<ii", b, 4) if len(b) >= 12 else (None, None)
        return {"type": "sc_media", "kind": "cursor", "visible": b[3] == 1, "x": x, "y": y, "_size": len(b)}
    return {"type": "sc_media", "kind": "unknown", "_size": len(b)}


class ScreenPhone(ps.Phone):
    """A phone that reads the screen's binary messages and acknowledges each video frame as a decoder would, and keeps its lock's window open.
    `ack_delay` (s) stands in for decoding: each frame is acknowledged that long after it arrived. Each message keeps when it arrived (`_t`,
    monotonic; `_w`, wall ms)."""

    ack_delay = 0.0

    async def _ack_later(self, sc, seq):
        await asyncio.sleep(self.ack_delay)
        if self.closed is None and getattr(self, "sc", None) == sc:
            try:
                await self.send(type="sc_ack", sc=sc, seq=seq)
            except Exception:  # noqa: BLE001 - the connection closed meanwhile: nothing to acknowledge on
                pass

    async def _read(self):
        try:
            async for raw in self.ws:
                whole = self.session.open(raw)
                if whole is None:
                    continue
                if whole[:1] == b"\x01":
                    m = media_header(whole)
                    m["_t"] = time.monotonic()
                    m["_w"] = time.time() * 1000
                    self.log.append(m)
                    if m["kind"] == "video" and getattr(self, "sc", None):
                        if self.ack_delay:
                            asyncio.ensure_future(self._ack_later(self.sc, m["seq"]))
                        else:
                            await self.send(type="sc_ack", sc=self.sc, seq=m["seq"])
                    elif m["kind"] == "video":
                        # Before the start's answer (the picture can overtake it): acknowledged once the screen is known, as the page does.
                        self.early = max(getattr(self, "early", 0), m["seq"])
                    continue
                m = json.loads(whole)
                m["_t"] = time.monotonic()
                m["_w"] = time.time() * 1000
                self.log.append(m)
                if m["type"] == "hb" and not m.get("echo") and self.auto_hb:
                    await self.send(type="hb", echo=True, n=m.get("n"))
        except Exception as e:  # noqa: BLE001 - the socket closed under it
            rcvd = getattr(e, "rcvd", None)
            self.closed = rcvd.code if rcvd else 1006
        finally:
            self.closed = self.closed or 1006

    async def touch(self):
        await self.send(type="lock_touch")

    async def click(self, x, y, st=None):
        await self.send(type="sc_input", sc=self.sc, ops=[{"k": "click", "x": x, "y": y, "b": "left"}], st=st if st is not None else now_ms())

    async def media(self, since, pred=lambda m: True, timeout=10):
        return await self.wait(lambda m: m["type"] == "sc_media" and pred(m), timeout, since)


async def control_phone(s, d, call, serve_port, name, via_port=None):
    """A phone paired on the Tailscale origin, its passkey set up, given control on the desktop, connected again unlocked (through `via_port`,
    a fault proxy in front of `serve_port`, when given). Returns the phone, its device, its hello, its passkey and how to connect it again."""
    made = await call("mobile.pair_start")
    through = dict(host="127.0.0.1", port=serve_port, origin=f"https://{s.origin}", additional_headers={"X-Forwarded-For": "100.101.102.103"})
    asking = await ps.PairAttempt(f"http://{s.origin}/", made["url"], name, "write", **through).start()
    request = await asyncio.to_thread(ps.pending_request, d, name)
    await call("mobile.approve", {"id": request["id"], "allow": True, "code": asking.code(), "permission": "write"})
    told = await asking.outcome(15)
    device = ps.Device(asking.priv, asking.pub, asking.dpk, (told or {}).get("device", {}).get("id"))
    held = dict(host="127.0.0.1", port=serve_port, additional_headers={"X-Forwarded-For": "100.101.102.103"})
    phone = ScreenPhone(f"http://{s.origin}/", device, origin=f"https://{s.origin}", **held)
    await phone.open()
    passkey = ps.SoftPasskey(f"https://{s.origin}")
    challenge = (await phone.ask(type="lock_challenge"))["lock"]["challenge"]
    enrolled = await phone.ask(type="lock_enrol", **passkey.create(challenge))
    await call("mobile.set_permission", {"id": device.id, "permission": "control"})
    await phone.close()
    again = dict(held, port=via_port or serve_port)

    def connect():
        return ScreenPhone(f"http://{s.origin}/", device, origin=f"https://{s.origin}", resume=enrolled["resume"], **again)

    phone = connect()
    hello = await phone.open()
    return phone, device, hello, passkey, connect


async def phones_on(s, call):
    """Phone access on, Tailscale (the fake one) serving it: the port its serve forwards to, which the phones connect to."""
    await call("mobile.enable", {"ip": "127.0.0.1", "port": s.port})
    ps.fake_tailscale_says(s.ts_state, status=ps.FAKE_TAILSCALE_READY)
    await call("mobile.set_tailscale", {"on": True})
    changes = [c for c in ps.fake_tailscale_calls(s.ts_log) if c.startswith("serve --bg")]
    return int(changes[-1].rsplit(":", 1)[1])


def start(s, daemon_env=None):
    """This script's daemon (with a fake Tailscale CLI), its tauri-driver and the app; checks the app is connected to this daemon."""
    s.ts_bin, s.ts_state, s.ts_log = ps.write_fake_tailscale(s.work)
    env = {"KELPIE_LOAD_FORCE": "L0:calm", "KELPIE_E2E": "1", "KELPIE_TAILSCALE_BIN": s.ts_bin}
    env.update(daemon_env or {})
    d = ps.Daemon(s.kelpied, env=env).start()
    procs["daemon"] = d.proc
    try:
        driver_port = free_port()
        os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{driver_port}"
        spawn("driver", [s.tauri_driver, "--port", str(driver_port), "--native-port", str(free_port()), "--native-driver", s.native])
        if not until(lambda: port_open(driver_port), 30):
            raise SystemExit("tauri-driver did not open its port")
        run = Run(os.path.join(s.scratch, "shots"), app=s.app)
    except BaseException:
        # Nothing of this script's may outlive it: the driver and this script's own daemon stop before the error goes on.
        finish(s, d, None)
        raise
    info = app_daemon(run.d, "daemon.info") if until(lambda: run.d.js("return !!(window.__kelpie && window.__kelpie.store.connected())"), 60) else {}
    check("the app is connected to this script's daemon", info.get("pid") == d.pid, info.get("pid"))
    return d, run


def finish(s, d, run, *names):
    """Stops the app, the processes named and the driver, then this script's daemon (never another), and removes the work folder."""
    if run is not None and run.d is not None:
        try:
            run.d.quit()
        except Exception:  # noqa: BLE001
            pass
    for name in names:
        stop(name)
    stop("driver")
    assert os.environ["KELPIE_ENDPOINT"].startswith(f"kelpie-{s.tag}-")
    if d is not None:
        d.stop()
    shutil.rmtree(s.work, ignore_errors=True)
