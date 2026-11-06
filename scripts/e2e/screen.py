"""The computer's screen from a phone, against the real app, its screen helper and a daemon of this script's own (Windows).

    python -X utf8 scripts/e2e/screen.py <scratch-dir> <app>

<app> is a debug build of the app (target/debug/kelpie.exe with its frontend embedded) with the debug kelpied and kelpie-screen next to it
(`cargo build -p kelpied -p kelpie -p kelpie-screen`): only a debug daemon takes this script for the Kelpie app (KELPIE_TRUSTED_APP) and
answers the OS verification from KELPIE_TEST_USER_PRESENCE, and only the kelpie.exe next to the daemon is the verified app that may provide
the screen. Needs a GPU or the inbox software H.264 encoder (any Windows 10/11 desktop), tauri-driver (PATH or ~/.cargo/bin), msedgedriver
matching WebView2 (PATH or MSEDGEDRIVER), Python's `websockets`, `cryptography` and tkinter.

Everything is set in this process before anything starts: its own endpoint and data folder (phone_support.isolate), a fake Tailscale CLI
(never the real one), and the app started by this script's tauri-driver with the same environment. The user's daemon, data, Tailscale and
keyring are never reached; the script stops only the processes it started, by PID. A small window of this script's own (tkinter) is what the
phone sees and clicks: it writes what happened to it into a file of this script's folder. Nothing typed is printed.

The phone here is emulated: it gets the binary media messages and acknowledges each video frame as a decoder would, without decoding.

1. The app registers as the screen's provider; a second registration is refused.
2. Two phones paired on the Tailscale origin with passkeys, unlocked, are given `control`; a phone that may only read and one that may type are
   refused the screen.
3. The phone lists the test window, sees it (a keyframe with SPS and PPS first, numbered frames after), and the desktop shows who sees it.
4. A click on the window's button, in the picture's pixels, reaches the window; a click with a stale clock does not; text goes into its field;
   with another window of the script's in front, text goes to the shared window or is refused, never into the other one.
5. Stop with the hotkey, Stop on the phone and Stop on the desktop each end it; a revoke ends it within a second, and input after it does nothing;
   the phone locking (leaving the app) ends it within a second too.
6. With the app closed, the phone is told to open Kelpie on the computer.
7. The audit log has each start (with the program shared, never the window's title) and end, and nothing typed; the typed text is in no file of
   the daemon's data folder.
"""
import asyncio
import ctypes
import ctypes.wintypes as wt
import json
import os
import secrets
import shutil
import socket
import struct
import subprocess
import sys
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import phone_support as ps  # noqa: E402

if len(sys.argv) < 3 or os.name != "nt":
    raise SystemExit(__doc__)
scratch = os.path.abspath(sys.argv[1])
app = os.path.abspath(sys.argv[2])
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe")
helper = os.path.join(os.path.dirname(app), "kelpie-screen.exe")
os.makedirs(scratch, exist_ok=True)
WORK = os.path.join(scratch, f"screen-{os.getpid()}")
os.makedirs(WORK)
DATA = ps.isolate("screen")
os.environ["KELPIE_DAEMON_PATH"] = kelpied
os.environ["KELPIE_SCREEN_PATH"] = helper
os.environ["KELPIE_E2E"] = "1"
for k in ("KELPIE_ALLOW_REAL_DAEMON",):
    os.environ.pop(k, None)

from smoke_support import Run, daemon as app_daemon, native_driver, native_driver_hint, tauri_driver  # noqa: E402

NATIVE, TAURI_DRIVER = native_driver(), tauri_driver()
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit(f"needs tauri-driver (PATH or ~/.cargo/bin) and {native_driver_hint()}")
for what, path in (("app", app), ("daemon next to the app", kelpied), ("screen helper next to the app", helper)):
    if not os.path.exists(path):
        raise SystemExit(f"no {what}: {path}")

PORT = 48900 + os.getpid() % 400
BASE = f"http://127.0.0.1:{PORT}/"
FQDN = ps.FAKE_TAILNET_NAME
ORIGIN = f"{FQDN}:8443"
TITLE = "Kelpie screen test " + secrets.token_hex(3)
TYPED = "typed-" + secrets.token_hex(6)
TYPED2 = "again-" + secrets.token_hex(6)
EVENTS = os.path.join(WORK, "window-events.txt")

results = []


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


# ---------- the window the phone sees ----------

WINDOW = r'''
import ctypes, sys, tkinter as tk
ctypes.windll.shcore.SetProcessDpiAwareness(2)
title, events = sys.argv[1], sys.argv[2]
def note(line):
    with open(events, "a", encoding="utf-8") as f:
        f.write(line + "\n")
root = tk.Tk()
root.title(title)
root.geometry("520x320+160+160")
button = tk.Button(root, text="Press me", command=lambda: note("clicked"))
button.place(x=40, y=40, width=200, height=80)
field = tk.Entry(root)
field.place(x=40, y=160, width=300, height=40)
field.bind("<KeyRelease>", lambda e: note("field:" + field.get()))
import os
# A bar that moves while asked to (`.animate`): a picture that changes every frame, for what encoding costs.
bar = tk.Frame(root, bg="#d04020")
def poll():

    if os.path.exists(events + ".animate"):
        bar.place(x=(bar.winfo_x() + 9) % 480, y=240, width=40, height=40)
        root.after(16, poll)
        return
    root.after(100, poll)
root.after(100, poll)
def where():
    root.update_idletasks()
    note("button:%d,%d,%d,%d" % (button.winfo_rootx(), button.winfo_rooty(), button.winfo_width(), button.winfo_height()))
    note("field:@%d,%d,%d,%d" % (field.winfo_rootx(), field.winfo_rooty(), field.winfo_width(), field.winfo_height()))
root.after(500, where)
root.mainloop()
'''

# Another program's window that takes the focus, as a chat window or a toast would: what the phone types must never land in it. A program of
# its own: two windows of one Tk program share Tk's own idea of the focus, which sends keys to the other one while Windows says the first has them.
DECOY = r'''
import ctypes, sys, tkinter as tk
ctypes.windll.shcore.SetProcessDpiAwareness(2)
title, events = sys.argv[1], sys.argv[2]
def note(line):
    with open(events, "a", encoding="utf-8") as f:
        f.write(line + "\n")
root = tk.Tk()
root.title(title)
root.geometry("300x120+760+160")
field = tk.Entry(root)
field.place(x=10, y=10, width=280, height=40)
field.bind("<KeyRelease>", lambda e: note("decoy:" + field.get()))
root.bind("<FocusIn>", lambda e: note("decoy-focused"))
root.after(200, field.focus_set)
root.mainloop()
'''


def events():
    try:
        with open(EVENTS, encoding="utf-8") as f:
            return [line.rstrip("\n") for line in f]
    except OSError:
        return []


def geometry(name):
    for line in events():
        if line.startswith(name):
            x, y, w, h = (int(v) for v in line.split(":", 1)[1].lstrip("@").split(","))
            return x, y, w, h
    return None


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


def helper_cpu(seconds):
    """The screen helper's CPU over `seconds`, as a share of one core, or None without psutil."""
    try:
        import psutil
    except ImportError:
        return None
    for p in psutil.process_iter(["exe"]):
        if (p.info.get("exe") or "").lower() == helper.lower():
            p.cpu_percent(None)
            time.sleep(seconds)
            return p.cpu_percent(None)
    return None


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


def why_not(phone):
    """What the phone was told, where the pointer is and what is in front: for a click or text that did not land."""
    u = ctypes.windll.user32
    buf = ctypes.create_unicode_buffer(120)
    u.GetWindowTextW(u.GetForegroundWindow(), buf, 120)
    states = [m.get("state") for m in phone.log if m["type"] == "sc_state"]
    front = "the test window" if buf.value == TITLE else "the decoy" if buf.value == "Decoy " + TITLE else "Kelpie" if buf.value.startswith("Kelpie") else "another window"
    return f"states {states[-6:]}, pointer {cursor()}, in front: {front}"


def press_hotkey():
    """Ctrl+Alt+Shift+K from this process, as a person would press it."""
    user32 = ctypes.windll.user32
    keys = [0x11, 0x12, 0x10, 0x4B]
    for k in keys:
        user32.keybd_event(k, 0, 0, 0)
    for k in reversed(keys):
        user32.keybd_event(k, 0, 2, 0)


# ---------- processes this script starts ----------

procs = {}


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


def media_header(b):
    """What the screen's binary message says (crates/kelpie-screen/src/wire.rs), without its picture."""
    kind = b[1]
    if kind == 1:
        n = b[3]
        seq, capture = struct.unpack_from("<II", b, 4)
        src_w, src_h, coded_w, coded_h = struct.unpack_from("<HHHH", b, 12)
        au = bytes(b[20 + n * 8:])
        nals = [au[i + 3] & 0x1F for i in range(len(au) - 3) if au[i:i + 3] == b"\x00\x00\x01"]
        return {"type": "sc_media", "kind": "video", "key": bool(b[2] & 1), "seq": seq, "src": (src_w, src_h), "coded": (coded_w, coded_h), "nals": nals, "_size": len(b)}
    if kind == 2:
        return {"type": "sc_media", "kind": "tile", "format": b[2], "seq": struct.unpack_from("<I", b, 4)[0], "_size": len(b)}
    if kind == 3:
        return {"type": "sc_media", "kind": "cursor", "visible": b[3] == 1}
    return {"type": "sc_media", "kind": "unknown"}


class ScreenPhone(ps.Phone):
    """A phone that reads the screen's binary messages and acknowledges each video frame as a decoder would, and keeps its lock's window open."""

    async def _read(self):
        try:
            async for raw in self.ws:
                whole = self.session.open(raw)
                if whole is None:
                    continue
                if whole[:1] == b"\x01":
                    m = media_header(whole)
                    m["_t"] = time.monotonic()
                    self.log.append(m)
                    if m["kind"] == "video" and getattr(self, "sc", None):
                        await self.send(type="sc_ack", sc=self.sc, seq=m["seq"])
                    elif m["kind"] == "video":
                        # Before the start's answer (the picture can overtake it): acknowledged once the screen is known, as the page does.
                        self.early = max(getattr(self, "early", 0), m["seq"])
                    continue
                m = json.loads(whole)
                m["_t"] = time.monotonic()
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


def refused(d, method, params):
    try:
        d.call(method, params)
        return None
    except SystemExit as e:  # kelpie-call's way of reporting an error
        return str(e)


TS_BIN = TS_STATE = TS_LOG = None


async def control_phone(d, call, serve_port, name):
    """A phone paired on the Tailscale origin, its passkey set up, given control on the desktop, connected again unlocked."""
    made = await call("mobile.pair_start")
    through = dict(host="127.0.0.1", port=serve_port, origin=f"https://{ORIGIN}", additional_headers={"X-Forwarded-For": "100.101.102.103"})
    asking = await ps.PairAttempt(f"http://{ORIGIN}/", made["url"], name, "write", **through).start()
    request = await asyncio.to_thread(ps.pending_request, d, name)
    await call("mobile.approve", {"id": request["id"], "allow": True, "code": asking.code(), "permission": "write"})
    told = await asking.outcome(15)
    device = ps.Device(asking.priv, asking.pub, asking.dpk, (told or {}).get("device", {}).get("id"))
    held = dict(host="127.0.0.1", port=serve_port, additional_headers={"X-Forwarded-For": "100.101.102.103"})
    phone = ScreenPhone(f"http://{ORIGIN}/", device, origin=f"https://{ORIGIN}", **held)
    await phone.open()
    passkey = ps.SoftPasskey(f"https://{ORIGIN}")
    challenge = (await phone.ask(type="lock_challenge"))["lock"]["challenge"]
    enrolled = await phone.ask(type="lock_enrol", **passkey.create(challenge))
    await call("mobile.set_permission", {"id": device.id, "permission": "control"})
    await phone.close()
    phone = ScreenPhone(f"http://{ORIGIN}/", device, origin=f"https://{ORIGIN}", resume=enrolled["resume"], **held)
    hello = await phone.open()
    return phone, device, hello, passkey


async def scenario(run, d):
    w = run.d

    async def js(script, *args):
        return await asyncio.to_thread(w.js, script, *args)

    async def call(method, params=None):
        return await asyncio.to_thread(d.call, method, params or {})

    # ---------- 1. the provider ----------
    run.boot()
    await asyncio.sleep(1)
    second = await asyncio.to_thread(lambda: refused(d, "provider.register", {"kind": "screen"}))
    check("a second screen provider is refused", "already provides" in (second or ""), second)

    # ---------- 2. phones ----------
    await call("mobile.enable", {"ip": "127.0.0.1", "port": PORT})
    ps.fake_tailscale_says(TS_STATE, status=ps.FAKE_TAILSCALE_READY)
    await call("mobile.set_tailscale", {"on": True})
    changes = [c for c in ps.fake_tailscale_calls(TS_LOG) if c.startswith("serve --bg")]
    serve_port = int(changes[-1].rsplit(":", 1)[1])
    phone, device, hello, _ = await control_phone(d, call, serve_port, "Screen phone")
    check("the phone comes back unlocked, with control", hello.get("locked") is False and hello["device"]["permission"] == "control", hello.get("device"))
    spare, spare_device, _, spare_key = await control_phone(d, call, serve_port, "Spare phone")
    for name, can_write in (("Reader", False), ("Typist", True)):
        other_device, _ = await asyncio.to_thread(ps.pair, d, BASE, name, can_write)
        other = ScreenPhone(BASE, other_device)
        await other.open()
        for verb in ({"type": "screens"}, {"type": "sc_start", "target": "w:1:2"}):
            reply = await other.ask(**verb)
            check(f"{name}: {verb['type']} is refused", (reply or {}).get("code") == "no_control", reply)
        await other.close()

    # ---------- 3. see the window ----------
    spawn("window", [sys.executable, "-c", WINDOW, TITLE, EVENTS])
    check("the test window is up", until(lambda: geometry("button") and geometry("field:@"), 20))
    listed = await phone.ask(type="screens", timeout=20)
    rows = (listed or {}).get("targets", [])
    target = next((r for r in rows if r["kind"] == "window" and r["title"] == TITLE), None)
    check("the phone lists the screens and the test window", target is not None and any(r["kind"] == "monitor" for r in rows), [r.get("title") for r in rows][:10])
    hwnd = int(target["id"].split(":")[1]) if target else 0

    async def see(p=None):
        p = p or phone
        mark = p.mark()
        p.sc, p.early = None, 0
        started = await p.ask(type="sc_start", target=target["id"], timeout=20)
        p.sc = (started or {}).get("sc")
        if p.sc and getattr(p, "early", 0):
            await p.send(type="sc_ack", sc=p.sc, seq=p.early)
        p.early = 0
        first = await p.media(mark, lambda m: m["kind"] == "video", 15)
        return started, first, mark

    started, first, mark = await see()
    check("it starts, and says what it shows", bool(phone.sc) and started.get("kind") == "window" and started.get("title") == TITLE, started)
    check("the first frame is a keyframe with its parameter sets", first is not None and first["key"] and {7, 8, 5} <= set(first["nals"]), first)
    shared = await asyncio.to_thread(until, lambda: w.js("return document.querySelector('[data-screen-shared]')?.textContent || ''"), 5)
    check("the desktop shows who sees the screen", "Screen phone" in (shared or ""), shared)

    # ---------- 4. click and type ----------
    fx, fy, fw, fh = frame_bounds(hwnd)
    sw, sh = started.get("w", fw), started.get("h", fh)

    def to_source(x, y):
        return round((x - fx) * sw / max(fw, 1)), round((y - fy) * sh / max(fh, 1))

    bx, by, bw, bh = geometry("button")
    cx, cy = to_source(bx + bw // 2, by + bh // 2)
    await phone.click(cx, cy, st=now_ms() - 1500)
    await asyncio.sleep(1.0)
    check("a click that came too late clicks nothing", "clicked" not in events())
    await phone.click(cx, cy)
    check("a click on the button in the picture's pixels presses it", until(lambda: "clicked" in events(), 5), why_not(phone))
    ex, ey, ew, eh = geometry("field:@")
    await phone.click(*to_source(ex + ew // 2, ey + eh // 2))
    await asyncio.sleep(0.3)
    typed = await phone.ask(type="sc_text", sc=phone.sc, text=TYPED, st=now_ms())
    check("text goes with its own answer", (typed or {}).get("result") == "sent", typed and typed.get("type"))
    check("…and lands in the window's field", until(lambda: f"field:{TYPED}" in events(), 5), why_not(phone))
    # Another window takes the focus (a chat, a toast): typed text goes to the shared window, brought to the front, or is refused, never elsewhere.
    spawn("decoy", [sys.executable, "-c", DECOY, "Decoy " + TITLE, EVENTS])
    decoy = until(lambda: ctypes.windll.user32.FindWindowW(None, "Decoy " + TITLE), 10)
    in_front = until(lambda: to_front(decoy) and ctypes.windll.user32.GetForegroundWindow() == decoy, 5)
    check("the other window has taken the focus", in_front)
    again = await phone.ask(type="sc_text", sc=phone.sc, text=TYPED2, st=now_ms())
    await asyncio.to_thread(until, lambda: any(e.startswith("field:") and e.endswith(TYPED2) for e in events()), 2)
    # The field already holds the first text: the second goes after it.
    landed = any(e.startswith("field:") and e.endswith(TYPED2) for e in events())
    refused_front = (again or {}).get("code") == "not_front"
    check("with another window in front, text goes to the shared window or is refused", (landed and (again or {}).get("result") == "sent") or (refused_front and not landed), (again or {}).get("code") or (again or {}).get("result"))
    check("…and never into the window that took the focus", not any(e.startswith("decoy:") and TYPED2 in e for e in events()))
    # The window changed (the button, the field): its frames followed the first, numbered in order.
    seqs = [m["seq"] for m in phone.log[mark:] if m["type"] == "sc_media" and m["kind"] == "video"]
    check("frames follow the first, numbered in order", len(seqs) >= 2 and seqs == sorted(seqs) and len(set(seqs)) == len(seqs), seqs[:20])

    # The picture's first and last pixel are the window's corners on the screen: no rounding off its edge, whatever the scaling.
    src = first["src"] if first else (sw, sh)
    print(f"SPIKE picture {src[0]}x{src[1]} (coded {first['coded'][0]}x{first['coded'][1]}), start said {sw}x{sh}, DWM frame {fw}x{fh}" if first else "SPIKE no first frame", flush=True)
    for sx, sy in ((0, 0), (sw - 1, sh - 1)):
        await phone.send(type="sc_input", sc=phone.sc, ops=[{"k": "move", "x": sx, "y": sy}], st=now_ms())
        want = (fx + round(sx * fw / max(sw, 1)), fy + round(sy * fh / max(sh, 1)))
        got = await asyncio.to_thread(until, lambda: (lambda c: c if abs(c[0] - want[0]) <= 1 and abs(c[1] - want[1]) <= 1 else None)(cursor()), 3, 0.05)
        check(f"the picture's pixel {sx},{sy} is the window's corner on the screen", got is not None, f"want {want}, pointer at {cursor()}")

    # What encoding a picture that changes every frame costs (printed, not checked: it depends on the machine).
    mark = phone.mark()
    with open(EVENTS + ".animate", "w") as f:
        f.write("1")
    await asyncio.sleep(1.5)
    await phone.send(type="sc_key", sc=phone.sc)
    cpu = await asyncio.to_thread(helper_cpu, 5)
    os.remove(EVENTS + ".animate")
    moving = [m for m in phone.log[mark:] if m["type"] == "sc_media" and m["kind"] == "video"]
    keys = [m["_size"] for m in moving if m["key"]]
    deltas = sorted(m["_size"] for m in moving if not m["key"])
    span = (moving[-1]["_t"] - moving[0]["_t"]) if len(moving) > 1 else 0
    stats = [m for m in phone.log[mark:] if m["type"] == "sc_stats"]
    print(f"SPIKE moving picture: {len(moving)} frames in {span:.1f} s, keyframes {keys} bytes, P-frames median {deltas[len(deltas) // 2] if deltas else 0} max {deltas[-1] if deltas else 0} bytes", flush=True)
    print(f"SPIKE helper CPU {cpu if cpu is None else round(cpu)} % of one core", flush=True)
    if stats:
        s = stats[-1]
        print(f"SPIKE stats: {s.get('fps')} fps, {s.get('w')}x{s.get('h')}, {s.get('kbps')} kbps, encode p95 {s.get('encode_ms_p95')} ms, encoder {s.get('encoder')}, reason {s.get('reason')}, crisp {s.get('crisp')}", flush=True)
        print(f"SPIKE stages: {json.dumps(s.get('stages'))}", flush=True)
    check("the computer reports its stages' timings for the measurement plan", bool(stats) and isinstance(stats[-1].get('stages'), dict) and 'capture_to_send_ms_p95' in stats[-1]['stages'], stats[-1].get('stages') if stats else None)
    told = [(m["type"], m.get("state") or m.get("reason") or m.get("code")) for m in phone.log if m["type"] in ("sc_end", "sc_state", "error")]
    check("a picture that changes keeps coming while it changes", len(moving) >= 20, f"{len(moving)} frames; the phone was told {told[-8:]}")

    # ---------- 5. ending it ----------
    mark = phone.mark()
    press_hotkey()
    ended = await phone.wait(lambda m: m["type"] == "sc_end", 5, mark)
    check("the hotkey ends it for the phone", (ended or {}).get("reason") == "hotkey", ended)
    gone = await asyncio.to_thread(until, lambda: not w.js("return !!document.querySelector('[data-screen-shared]')"), 5)
    check("…and the desktop's indicator goes", bool(gone))

    await see()
    mark = phone.mark()
    stopped = await phone.ask(type="sc_stop", sc=phone.sc)
    gone = await asyncio.to_thread(until, lambda: not w.js("return !!document.querySelector('[data-screen-shared]')"), 5)
    check("Stop on the phone ends it on the desktop", (stopped or {}).get("type") == "ack" and bool(gone), stopped)

    await see()
    await asyncio.to_thread(until, lambda: w.js("return !!document.querySelector('[data-screen-shared]')"), 5)
    mark = phone.mark()
    await asyncio.to_thread(w.click, "[data-screen-stop]")
    ended = await phone.wait(lambda m: m["type"] == "sc_end", 5, mark)
    check("Stop on the desktop ends it for the phone", (ended or {}).get("reason") == "desktop", ended)

    await see()
    clicks = events().count("clicked")
    at = time.time()
    await call("mobile.revoke", {"id": device.id})
    gone = await asyncio.to_thread(until, lambda: not w.js("return !!document.querySelector('[data-screen-shared]')"), 5, 0.05)
    check("a revoke ends it within a second", bool(gone) and time.time() - at < 1.0, f"{time.time() - at:.2f} s")
    try:
        await phone.click(cx, cy)
    except Exception:  # noqa: BLE001 - the revoked phone's connection is closed
        pass
    await asyncio.sleep(1.0)
    check("…and input after it does nothing", events().count("clicked") == clicks)
    await phone.close()

    # Leaving the app on the phone locks it, and that ends the screen at once, with no grace.
    await see(spare)
    await asyncio.to_thread(until, lambda: w.js("return !!document.querySelector('[data-screen-shared]')"), 5)
    at = time.time()
    await spare.send(type="lock_now")
    gone = await asyncio.to_thread(until, lambda: not w.js("return !!document.querySelector('[data-screen-shared]')"), 5, 0.05)
    check("the phone locking ends the screen within a second", bool(gone) and time.time() - at < 1.0, f"{time.time() - at:.2f} s")
    challenge = (await spare.ask(type="lock_challenge"))["lock"]["challenge"]
    unlocked = await spare.ask(type="lock_unlock", **spare_key.get(challenge))
    check("…and it can be unlocked again", (unlocked or {}).get("result") == "ok", unlocked and unlocked.get("type"))

    # ---------- 6. the app closed ----------
    await asyncio.to_thread(w.quit)
    run.d = None
    await asyncio.sleep(2)
    reply = await spare.ask(type="screens", timeout=10)
    check("with the app closed, the phone is told to open Kelpie", (reply or {}).get("code") == "no_app" and "Open Kelpie" in (reply or {}).get("message", ""), reply)
    await spare.close()

    # ---------- 7. what is kept ----------
    entries = json.dumps((await call("mobile.audit", {"limit": 200}))["entries"])
    program = os.path.basename(sys.executable)
    check("the audit log has the screens' starts, with the program shared, and their ends", "screen_start" in entries and program in entries and "screen_stop" in entries, program)
    check("…never the window's title", TITLE not in entries)
    check("…and nothing typed, the second text included", TYPED2 not in entries)
    check("…and nothing typed", TYPED not in entries)
    leaked = []
    for root, _, files in os.walk(DATA):
        for f in files:
            try:
                with open(os.path.join(root, f), "rb") as fh:
                    if TYPED.encode() in fh.read():
                        leaked.append(f)
            except OSError:
                pass
    check("the typed text is in no file of the daemon's data folder", not leaked, leaked)


def main():
    global TS_BIN, TS_STATE, TS_LOG
    TS_BIN, TS_STATE, TS_LOG = ps.write_fake_tailscale(WORK)
    d = ps.Daemon(kelpied, env={"KELPIE_LOAD_FORCE": "L0:calm", "KELPIE_E2E": "1", "KELPIE_TAILSCALE_BIN": TS_BIN}).start()
    procs["daemon"] = d.proc
    driver_port = free_port()
    os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{driver_port}"
    run = None
    try:
        spawn("driver", [TAURI_DRIVER, "--port", str(driver_port), "--native-port", str(free_port()), "--native-driver", NATIVE])
        if not until(lambda: port_open(driver_port), 30):
            raise SystemExit("tauri-driver did not open its port")
        run = Run(os.path.join(scratch, "shots"), app=app)
        info = app_daemon(run.d, "daemon.info") if until(lambda: run.d.js("return !!(window.__kelpie && window.__kelpie.store.connected())"), 60) else {}
        check("the app is connected to this script's daemon", info.get("pid") == d.pid, info.get("pid"))
        asyncio.run(scenario(run, d))
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False)
    finally:
        if run is not None and run.d is not None:
            try:
                run.d.quit()
            except Exception:  # noqa: BLE001
                pass
        stop("window")
        stop("decoy")
        stop("driver")
        assert os.environ["KELPIE_ENDPOINT"].startswith("kelpie-screen-")
        d.stop()
        shutil.rmtree(WORK, ignore_errors=True)
    failed = [r for r in results if not r[1]]
    print(json.dumps({"passed": len(results) - len(failed), "failed": len(failed)}))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
