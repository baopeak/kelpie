"""Browser pane takeover from a phone, against the real app and a daemon of this script's own (Windows: the WebView2 DevTools protocol).

    python -X utf8 scripts/e2e/takeover.py <scratch-dir> <app>

<app> is a debug build of the app (target/debug/kelpie.exe with its frontend embedded) with the debug kelpied next to it: only a debug daemon takes
this script for the Kelpie app (KELPIE_TRUSTED_APP) and answers the OS verification from KELPIE_TEST_USER_PRESENCE, and only the kelpie.exe next to
the daemon is the verified app that may provide panes. Needs tauri-driver (PATH or ~/.cargo/bin), msedgedriver matching WebView2 (PATH or
MSEDGEDRIVER), and Python's `websockets` and `cryptography`.

Everything is set in this process before anything starts: its own endpoint and data folder (phone_support.isolate), a fake Tailscale CLI (never the
real one), and the app started by this script's tauri-driver with the same environment, so that it connects to this daemon and nothing else. The
user's daemon, data, Tailscale and keyring are never reached; the script stops only the processes it started, by PID. A local site stands in for a
sign-in page and an OAuth consent window. Nothing typed into the page is printed.

1. The app registers as the provider of browser panes; a second registration (this script's own connection, which the daemon takes for the app too)
   is refused, and the audit log has it.
2. A phone paired on the Tailscale origin, its passkey set up (software keys) and unlocked, is given `control` on the desktop with the person's
   verification; a phone that may only read and one that may type are refused takeover.
3. The phone lists the pane, takes it over and gets frames; the desktop shows the banner. It types a user name and a password into the page and taps
   Sign in: the page sees the text and a trusted click. A tap sent with a stale clock reading clicks nothing.
   The user name field is tapped through the page's own gesture and coordinate code (app/phone/src/takeover.ts, run by Node from
   takeover_gestures.mjs on simulated pointer events against the real frame), the password goes straight after the tap that chose its field and
   lands there, and a pinch sends nothing.
   On a still page captures stand in for the silent screencast and the stream never pauses. With the pane behind another tab, and then with the
   app's window minimised (Win32, on this script's own app only), the phone is told the pane is hidden, and a tap made then never clicks, not even
   once the pane shows again; shown, a tap clicks again.
   `--spike` also measures, past the guard, what WebView2 does with input to a pane or a window of its own it does not show, and what
   `document.visibilityState` says then (`SPIKE ...` lines).
4. The OAuth window (`window.open` with a size) opens as a window of its own: the frames come from it, a tap on Allow reaches it with
   `window.opener` set, the opener hears it, and once it closes the frames come from the pane again, under another window id. A link with
   `target=_blank` still opens in the pane.
   Then through a TCP fault proxy (net_fault_proxy.py): the same phone's new connection keeps the hold; taps sent while the link is frozen for 5 s
   click nothing once it comes back; with 1 s round trips the newest frame stays under two round trips old.
5. Stop on the desktop ends the takeover for the phone; a second one is ended from the phone; a third is ended by revoking the phone, within a second
   or two. The password is in no file of the daemon's data folder and in no audit entry.
"""
import asyncio
import json
import os
import secrets
import shutil
import socket
import subprocess
import sys
import threading
import time
import traceback
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import net_fault_proxy as nfp  # noqa: E402
import phone_support as ps  # noqa: E402

if len(sys.argv) < 3 or os.name != "nt":
    raise SystemExit(__doc__)
scratch = os.path.abspath(sys.argv[1])
app = os.path.abspath(sys.argv[2])
# `--spike`: also measure what the spike asks (input latency, frame age and size, the WebView's CPU with and without a stream, frames from a hidden
# pane) and print it; nothing measured is a check.
SPIKE = "--spike" in sys.argv
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe")
os.makedirs(scratch, exist_ok=True)
WORK = os.path.join(scratch, f"takeover-{os.getpid()}")
os.makedirs(WORK)
DATA = ps.isolate("takeover")
os.environ["KELPIE_DAEMON_PATH"] = kelpied
os.environ["KELPIE_E2E"] = "1"
for k in ("KELPIE_ALLOW_REAL_DAEMON",):
    os.environ.pop(k, None)

from smoke_support import Run, daemon as app_daemon, native_driver, native_driver_hint, tauri_driver  # noqa: E402

NATIVE, TAURI_DRIVER = native_driver(), tauri_driver()
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit(f"needs tauri-driver (PATH or ~/.cargo/bin) and {native_driver_hint()}")
for what, path in (("app", app), ("daemon next to the app", kelpied)):
    if not os.path.exists(path):
        raise SystemExit(f"no {what}: {path}")

PORT = 48500 + os.getpid() % 400
BASE = f"http://127.0.0.1:{PORT}/"
FQDN = ps.FAKE_TAILNET_NAME
ORIGIN = f"{FQDN}:8443"
USER = "alice-" + secrets.token_hex(3)
PASSWORD = "pw-" + secrets.token_hex(8)

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


# ---------- the site: a sign-in page and an OAuth consent window ----------

LOGIN = """<!doctype html><html><head><title>Example Sign In</title>
<style>body{margin:0;font:16px sans-serif} .x{position:absolute;left:40px;width:220px;height:36px;box-sizing:border-box}</style></head><body>
<input id="user" class="x" style="top:40px" autocomplete="off"><input id="pass" class="x" type="password" style="top:100px">
<button id="signin" class="x" style="top:160px">Sign in</button><button id="oauth" class="x" style="top:220px">Continue with Example</button>
<a id="blank" class="x" href="/page2" target="_blank" style="top:280px">Open in a new window</a>
<div id="probe" class="x" style="top:340px;background:#ddd"></div>
<button id="probewin" class="x" style="top:400px">Open a probe window</button>
<div id="spin" style="position:absolute;left:300px;top:40px;width:20px;height:20px;background:#2d7ff9;animation:spin 1s linear infinite"></div>
<style>@keyframes spin{to{transform:rotate(360deg)}}</style>
<script>
const report = (q) => fetch('/report?' + new URLSearchParams(q));
document.getElementById('signin').addEventListener('click', (e) => report({login: document.getElementById('user').value + ':' + document.getElementById('pass').value, trusted: e.isTrusted}));
document.getElementById('oauth').addEventListener('click', () => window.open('/oauth', 'oauth', 'width=480,height=520'));
document.getElementById('probewin').addEventListener('click', () => window.open('/probe', 'probe', 'width=480,height=400'));
window.addEventListener('message', (e) => report({granted: String(e.data)}));
document.addEventListener('visibilitychange', () => report({lvis: document.visibilityState}));
document.getElementById('probe').addEventListener('click', (e) => { e.target.style.background = e.target.style.background === 'red' ? '#ddd' : 'red'; report({probe: Date.now(), trusted: e.isTrusted}); });
</script></body></html>"""
OAUTH = """<!doctype html><html><head><title>Example consent</title><style>body{margin:0} #allow{position:absolute;left:40px;top:40px;width:200px;height:48px}</style></head>
<body><button id="allow">Allow</button><script>
document.getElementById('allow').addEventListener('click', (e) => {
  fetch('/report?' + new URLSearchParams({opener: String(!!window.opener), trusted: e.isTrusted})).finally(() => { if (window.opener) window.opener.postMessage('yes', '*'); window.close(); });
});
</script></body></html>"""
PAGE2 = "<!doctype html><html><head><title>Second Page</title></head><body><h1>Page two</h1></body></html>"
# A window of its own (`window.open` with a size), for what WebView2 does to a window behind another or minimised: it says what the page thinks of
# its visibility, and when it was clicked.
PROBE = """<!doctype html><html><head><title>Probe window</title><style>body{margin:0} .x{position:absolute;left:40px;width:200px;height:48px}</style></head>
<body><button id="hit" class="x" style="top:40px">Hit</button><button id="close" class="x" style="top:120px">Close</button><script>
const report = (q) => fetch('/report?' + new URLSearchParams(q));
report({pvis: document.visibilityState});
document.addEventListener('visibilitychange', () => report({pvis: document.visibilityState}));
document.getElementById('hit').addEventListener('click', (e) => report({pclick: Date.now(), trusted: e.isTrusted}));
document.getElementById('close').addEventListener('click', () => window.close());
</script></body></html>"""
# A page that never changes by itself: no animation, no caret. The screencast goes silent on it.
STATIC = """<!doctype html><html><head><title>Still page</title><style>body{margin:0;font:16px sans-serif} #go{position:absolute;left:40px;top:40px;width:220px;height:40px}</style></head>
<body><button id="go">Continue</button><script>
document.addEventListener('visibilitychange', () => fetch('/report?' + new URLSearchParams({svis: document.visibilityState})));
document.getElementById('go').addEventListener('click', (e) => { e.target.textContent = e.target.textContent === 'Done' ? 'Continue' : 'Done'; fetch('/report?' + new URLSearchParams({still: Date.now(), trusted: e.isTrusted})); });
</script></body></html>"""
reports = []
reports_lock = threading.Lock()


class Site(BaseHTTPRequestHandler):
    def log_message(self, *a):  # what the page reports holds the password
        pass

    def do_GET(self):
        path, _, query = self.path.partition("?")
        if path == "/report":
            with reports_lock:
                reports.append(dict(urllib.parse.parse_qsl(query)))
            body = b"ok"
        else:
            body = (OAUTH if path == "/oauth" else PAGE2 if path == "/page2" else STATIC if path == "/static" else PROBE if path == "/probe" else LOGIN).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def reported(key, since=0):
    with reports_lock:
        return [r for r in reports[since:] if key in r]


def report_count():
    with reports_lock:
        return len(reports)


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


# The phone's screen as the page lays the picture out in it (CSS pixels of a 390 × 844 phone, below its header).
SCREEN = {"left": 0, "top": 96, "width": 390, "height": 560}
GESTURES = os.path.join(HERE, "takeover_gestures.mjs")


def jpeg_size(b64):
    """Width and height of a JPEG from its start-of-frame marker."""
    import base64
    data = base64.b64decode(b64)
    i = 2
    while i + 9 < len(data):
        if data[i] != 0xFF:
            i += 1
            continue
        marker, length = data[i + 1], int.from_bytes(data[i + 2:i + 4], "big")
        if marker in (0xC0, 0xC1, 0xC2):
            return int.from_bytes(data[i + 7:i + 9], "big"), int.from_bytes(data[i + 5:i + 7], "big")
        i += 2 + length
    raise ValueError("no frame size in the JPEG")


def phone_gestures(spec):
    """The ops the phone page's own code (app/phone/src/takeover.ts) makes of pointer events, through Node."""
    # Node strips the types itself from 23.6 on; before that it takes the flag. Either way nothing else of the file changes.
    for flags in ([], ["--experimental-strip-types"]):
        out = subprocess.run(["node", "--no-warnings", *flags, GESTURES], input=json.dumps(spec), capture_output=True, text=True, timeout=30)
        if out.returncode == 0:
            return json.loads(out.stdout)
    raise RuntimeError("takeover_gestures.mjs: " + out.stderr[-500:])


class TkPhone(ps.Phone):
    """A phone that acknowledges takeover frames as it gets them, and keeps its app lock's window open."""

    async def open(self):
        hello = await super().open()
        self.tk_acker = asyncio.ensure_future(self._tk_acks())
        return hello

    async def _tk_acks(self):
        seen, touched = 0, time.time()
        try:
            while self.closed is None:
                while seen < len(self.log):
                    m = self.log[seen]
                    seen += 1
                    if m["type"] == "tk_frame":
                        # As a page that never loads the same picture twice, when asked: such a frame is not acknowledged.
                        same = self.prev_data == m["data"]
                        self.prev_data = m["data"]
                        if same and not self.ack_identical:
                            continue
                        self.acked_seq = m["seq"]
                        await self.send(type="tk_ack", tk=m["tk"], seq=m["seq"])
                    elif m["type"] == "tk_fhb" and m.get("paused") and self.acked_seq:
                        # The page's answer to a paused heartbeat: its last acknowledged frame again.
                        await self.send(type="tk_ack", tk=m["tk"], seq=self.acked_seq)
                if time.time() - touched > 20:
                    touched = time.time()
                    await self.send(type="lock_touch")
                await asyncio.sleep(0.03)
        except Exception:  # noqa: BLE001 - the socket closed under it
            pass

    ack_identical = True
    prev_data = None
    acked_seq = 0

    @property
    def last_frame(self):
        """The newest frame the phone has: what it shows, and what input is mapped against."""
        return next((m for m in reversed(self.log) if m["type"] == "tk_frame"), None)

    @property
    def window(self):
        """The window id of the newest frame: input names it, as the page does."""
        frame = self.last_frame
        return frame["meta"].get("window_id", 0) if frame else 0

    async def tap(self, tk, x, y, st=None):
        await self.send(type="tk_input", tk=tk, ops=[{"k": "tap", "x": x, "y": y}], st=st if st is not None else now_ms(), w=self.window)

    async def type_text(self, tk, text):
        return await self.ask(type="tk_text", tk=tk, text=text, st=now_ms(), w=self.window)

    async def touches(self, tk, events):
        """Runs `events` (pointer events on the phone's screen, `SCREEN`) through the page's own gesture and coordinate code, against the last frame,
        and sends what it made. Returns the ops."""
        frame = self.last_frame
        natural = jpeg_size(frame["data"])
        ops = await asyncio.to_thread(phone_gestures, {"el": SCREEN, "natural": {"w": natural[0], "h": natural[1]}, "meta": frame["meta"], "events": events})
        if ops:
            await self.send(type="tk_input", tk=tk, ops=ops, st=now_ms(), w=self.window)
        return ops

    def screen_point(self, x, y):
        """Where on the phone's screen page point (x, y) is drawn in the last frame (the inverse of the page's `toPage`, for choosing where to touch)."""
        frame = self.last_frame
        nw, nh = jpeg_size(frame["data"])
        scale = min(SCREEN["width"] / nw, SCREEN["height"] / nh)
        bw, bh = nw * scale, nh * scale
        left, top = SCREEN["left"] + (SCREEN["width"] - bw) / 2, SCREEN["top"] + (SCREEN["height"] - bh) / 2
        return left + x / frame["meta"]["deviceWidth"] * bw, top + y / frame["meta"]["deviceHeight"] * bh

    async def frame(self, since, pred=lambda m: True, timeout=10):
        return await self.wait(lambda m: m["type"] == "tk_frame" and pred(m), timeout, since)

    async def close(self):
        if getattr(self, "tk_acker", None):
            self.tk_acker.cancel()
        await super().close()


async def scenario(run, d):
    w = run.d

    async def js(script, *args):
        return await asyncio.to_thread(w.js, script, *args)

    async def call(method, params=None):
        return await asyncio.to_thread(d.call, method, params or {})

    # ---------- 1. the provider ----------
    run.boot()
    await asyncio.sleep(1)
    second = await asyncio.to_thread(lambda: refused(d, "provider.register", {"kind": "browser"}))
    check("a second provider is refused", "already provides" in (second or ""), second)
    audit = await call("mobile.audit", {"limit": 20})
    check("…and the audit log has it", any(e["action"] == "provider_refused" for e in audit["entries"]), audit)
    plain = await asyncio.to_thread(lambda: refused(ps.kc.Conn(), "provider.register", {"kind": "browser"}))
    check("the plain user token cannot provide panes", "Kelpie app" in (plain or ""), plain)

    # ---------- a browser pane with the sign-in page ----------
    proj = os.path.join(WORK, "proj")
    os.makedirs(proj, exist_ok=True)
    pid = await js("return window.__kelpie.store.addProject(arguments[0])", proj)
    await js("window.__kelpie.store.switchProject(arguments[0]); window.__kelpie.browser.openBrowser(arguments[1], 'tab', arguments[0])", pid, f"http://127.0.0.1:{SITE_PORT}/login")
    label = await asyncio.to_thread(until, lambda: w.js(
        "const ws = window.__kelpie.store.workspace(arguments[0]); for (const t of ws.tabs) { const p = window.__kelpie.layout.allPanes(t.root).find((p) => p.kind === 'browser'); if (p) return 'browser-' + p.id; } return null;", pid), 15)
    loaded = await asyncio.to_thread(until, lambda: "/login" in (w.invoke("browser_info", {"label": label}).get("ok") or {}).get("url", ""), 20)
    check("the pane shows the sign-in page", bool(label and loaded), label)

    # ---------- 2. phones: one paired on the Tailscale origin with a passkey, given control; a reader; a typist ----------
    await call("mobile.enable", {"ip": "127.0.0.1", "port": PORT})
    ps.fake_tailscale_says(TS_STATE, status=ps.FAKE_TAILSCALE_READY)
    await call("mobile.set_tailscale", {"on": True})
    changes = [c for c in ps.fake_tailscale_calls(TS_LOG) if c.startswith("serve --bg")]
    serve_port = int(changes[-1].rsplit(":", 1)[1])
    made = await call("mobile.pair_start")
    through = dict(host="127.0.0.1", port=serve_port, origin=f"https://{ORIGIN}", additional_headers={"X-Forwarded-For": "100.101.102.103"})
    asking = await ps.PairAttempt(f"http://{ORIGIN}/", made["url"], "Takeover phone", "write", **through).start()
    request = await asyncio.to_thread(ps.pending_request, d, "Takeover phone")
    await call("mobile.approve", {"id": request["id"], "allow": True, "code": asking.code(), "permission": "write"})
    told = await asking.outcome(15)
    device = ps.Device(asking.priv, asking.pub, asking.dpk, (told or {}).get("device", {}).get("id"))
    held = dict(host="127.0.0.1", port=serve_port, additional_headers={"X-Forwarded-For": "100.101.102.103"})
    phone = TkPhone(f"http://{ORIGIN}/", device, origin=f"https://{ORIGIN}", **held)
    await phone.open()
    passkey = ps.SoftPasskey(f"https://{ORIGIN}")
    challenge = (await phone.ask(type="lock_challenge"))["lock"]["challenge"]
    enrolled = await phone.ask(type="lock_enrol", **passkey.create(challenge))
    check("the phone sets up its passkey", (enrolled or {}).get("result") == "ok", enrolled)
    listed = next(x for x in (await call("mobile.status"))["devices"] if x["id"] == device.id)
    check("…and Settings may now give it control", listed.get("may_control") is True, listed)
    await call("mobile.set_permission", {"id": device.id, "permission": "control"})
    await phone.close()
    phone = TkPhone(f"http://{ORIGIN}/", device, origin=f"https://{ORIGIN}", resume=enrolled["resume"], **held)
    hello = await phone.open()
    check("it comes back unlocked, with control", hello.get("locked") is False and hello["device"]["permission"] == "control", hello.get("device"))

    for name, can_write in (("Reader", False), ("Typist", True)):
        other_device, _ = await asyncio.to_thread(ps.pair, d, BASE, name, can_write)
        other = TkPhone(BASE, other_device)
        await other.open()
        for verb in ({"type": "panes"}, {"type": "tk_start", "pane": label}):
            reply = await other.ask(**verb)
            check(f"{name}: {verb['type']} is refused", (reply or {}).get("code") == "no_control", reply)
        await other.close()

    # ---------- 3. take over, sign in ----------
    if SPIKE:
        print(f"SPIKE webview cpu without a stream: {await asyncio.to_thread(webview_cpu, 5)}% of a core", flush=True)
    panes = await phone.ask(type="panes")
    check("the phone lists the pane", any(p["pane"] == label and p["host"] == f"127.0.0.1:{SITE_PORT}" for p in (panes or {}).get("panes", [])), panes)
    mark = phone.mark()
    started = await phone.ask(type="tk_start", pane=label, timeout=15)
    tk = (started or {}).get("tk")
    check("it takes the pane over", bool(tk), started)
    first = await phone.frame(mark)
    check("…and frames come, with the page's size", first is not None and first["meta"].get("deviceWidth", 0) > 0 and first["meta"].get("window") == "pane", first and first.get("meta"))
    banner = await asyncio.to_thread(until, lambda: w.js("return document.querySelector('[data-takeover]')?.textContent || ''"), 5)
    check("the desktop shows who controls the pane", "Takeover phone" in (banner or ""), banner)

    since = report_count()
    await phone.tap(tk, 150, 178, st=now_ms() - 1500)
    await asyncio.sleep(1.0)
    check("a tap that came too late clicks nothing", not reported("login", since))
    # Through the page's own code: a finger on the phone's screen, mapped to the page by the frame it shows.
    ux, uy = phone.screen_point(150, 58)
    made = await phone.touches(tk, [{"type": "down", "id": 1, "x": ux, "y": uy, "t": 0}, {"type": "up", "id": 1, "x": ux, "y": uy, "t": 80}])
    check("a touch on the user name field is a tap there", len(made) == 1 and made[0]["k"] == "tap" and abs(made[0]["x"] - 150) < 3 and abs(made[0]["y"] - 58) < 3, made)
    typed = await phone.type_text(tk, USER)
    # The password straight after the tap that chose its field: it must not overtake it.
    await phone.tap(tk, 150, 118)
    secret = await phone.type_text(tk, PASSWORD)
    check("text goes with its own answer", (typed or {}).get("result") == "sent" and (secret or {}).get("result") == "sent", (typed, secret and secret.get("type")))
    px, py = phone.screen_point(150, 108)
    pinch = [{"type": "down", "id": 1, "x": px, "y": py, "t": 0}, {"type": "down", "id": 2, "x": px + 40, "y": py + 40, "t": 10}]
    for i in range(1, 12):
        pinch += [{"type": "move", "id": 1, "x": px - i * 4, "y": py - i * 4, "t": 400 + 40 * i}, {"type": "move", "id": 2, "x": px + 40 + i * 4, "y": py + 40 + i * 4, "t": 400 + 40 * i}]
    pinch += [{"type": "up", "id": 2, "x": px + 90, "y": py + 90, "t": 900}, {"type": "up", "id": 1, "x": px - 50, "y": py - 50, "t": 910}]
    check("a pinch on the phone sends nothing to the page", await phone.touches(tk, pinch) == [])
    sx, sy = phone.screen_point(150, 178)
    await phone.touches(tk, [{"type": "down", "id": 3, "x": sx, "y": sy, "t": 0}, {"type": "up", "id": 3, "x": sx, "y": sy, "t": 70}])
    login = await asyncio.to_thread(until, lambda: reported("login", since), 5)
    check("the page signed in with what the phone typed", bool(login) and login[-1]["login"] == f"{USER}:{PASSWORD}")
    check("…with a trusted click", bool(login) and login[-1].get("trusted") == "true", login and login[-1].get("trusted"))

    if SPIKE:
        await spike(phone, tk, w, pid)

    # ---------- a still page: the screencast goes silent, captures stand in, and the stream never locks up ----------
    phone.ack_identical = False
    mark = phone.mark()
    await asyncio.to_thread(w.invoke, "browser_navigate", {"label": label, "url": f"http://127.0.0.1:{SITE_PORT}/static"})
    await asyncio.sleep(8)
    frames = [m for m in phone.log[mark:] if m["type"] == "tk_frame"]
    check("a still page is captured once the stream is silent", any(m["meta"].get("captured") for m in frames), [m["meta"].get("captured") for m in frames][-6:])
    captured = [m["data"] for m in frames if m["meta"].get("captured")]
    # A capture may show what an earlier one did when a screencast frame came between them (the phone showed that one since): only the frame right
    # before a capture counts.
    repeats = [i for i, (a, b) in enumerate(zip(frames, frames[1:])) if b["meta"].get("captured") and a["data"] == b["data"]]
    check("…and a capture never repeats the frame just before it", not repeats, f"{len(captured)} captures, repeats at {repeats}")
    paused = [m for m in phone.log[mark:] if m["type"] == "tk_fhb" and m.get("paused")]
    check("…and the stream never pauses on it, though the phone does not acknowledge a picture it has", not paused, len(paused))
    since = report_count()
    await phone.tap(tk, 150, 60)
    still = await asyncio.to_thread(until, lambda: reported("still", since), 5)
    check("after 8 still seconds a tap still clicks: the stream is not locked", bool(still) and still[-1].get("trusted") == "true", {"paused beats": len(paused)})
    mark = phone.mark()
    changed = await phone.frame(mark, timeout=5)
    check("…and the page's change reaches the phone", changed is not None)
    phone.ack_identical = True
    await hidden_pane(phone, tk, w, pid, label)
    if SPIKE:
        tk = await spike_restart_still(phone, tk, label)
    await asyncio.to_thread(w.invoke, "browser_navigate", {"label": label, "url": f"http://127.0.0.1:{SITE_PORT}/login"})
    await asyncio.to_thread(until, lambda: "/login" in (w.invoke("browser_info", {"label": label}).get("ok") or {}).get("url", ""), 10)
    await asyncio.sleep(1.5)

    # ---------- 4. the OAuth window ----------
    since = report_count()
    mark = phone.mark()
    await phone.tap(tk, 150, 238)
    popup = await phone.frame(mark, lambda m: m["meta"].get("window") == "popup", 15)
    seen = {"popup": w.invoke("browser_info", {"label": label + "-popup-1"}), "pane": (w.invoke("browser_info", {"label": label}).get("ok") or {}).get("url"), "frames": [(m["meta"].get("window"), m["meta"].get("window_id")) for m in phone.log[mark:] if m["type"] == "tk_frame"][-3:]}
    check("the OAuth window opens as its own, and the phone sees it", popup is not None, seen)
    mark = phone.mark()
    await phone.tap(tk, 140, 64)
    opener = await asyncio.to_thread(until, lambda: reported("opener", since), 8)
    check("a tap on Allow reaches it, with window.opener", bool(opener) and opener[-1]["opener"] == "true" and opener[-1].get("trusted") == "true", opener)
    granted = await asyncio.to_thread(until, lambda: reported("granted", since), 8)
    check("…and the page that opened it hears it", bool(granted) and granted[-1]["granted"] == "yes", granted)
    back = await phone.frame(mark, lambda m: m["meta"].get("window") == "pane", 10)
    check("once it closes, the frames come from the pane again", back is not None)
    check("…under another window id, so input meant for the popup cannot land on the pane", back is not None and popup is not None and back["meta"].get("window_id") != popup["meta"].get("window_id"))
    if SPIKE:
        await spike_questions(phone, tk, w, pid, label)

    # ---------- a bad network: the phone's link through a fault proxy ----------
    proxy = nfp.FaultProxy(serve_port)
    proxy_port = await proxy.start()
    slow = TkPhone(f"http://{ORIGIN}/", device, origin=f"https://{ORIGIN}", resume=enrolled["resume"], **{**held, "port": proxy_port})
    await slow.open()
    # The older connection is told it was replaced and ends: the hold waits for this one.
    await ps.async_wait_for(lambda: phone.closed is not None, 5)
    await asyncio.sleep(0.3)
    rebound = await slow.ask(type="tk_rebind", tk=tk)
    check("the same phone's new connection keeps the hold", (rebound or {}).get("type") == "ack", rebound)
    await phone.close()
    phone = slow
    check("…and gets a fresh frame", await phone.frame(0, timeout=10) is not None)
    since = report_count()
    proxy.blackhole(5)
    for _ in range(3):
        await phone.tap(tk, 150, 178)
        await asyncio.sleep(0.5)
    await asyncio.sleep(6)
    check("taps sent while the link froze for 5 s click nothing once it comes back", not reported("login", since))
    proxy.latency = 0.5
    await asyncio.sleep(3)
    mark = phone.mark()
    await asyncio.sleep(4)
    ages = [m["_w"] * 1000 - m["server_ts"] for m in phone.log[mark:] if m["type"] == "tk_frame"]
    check("with 1 s round trips the newest frame is never more than two round trips old: nothing queues", bool(ages) and max(ages) <= 2500, ages[-5:])
    proxy.latency = 0
    await asyncio.sleep(1)
    await phone.tap(tk, 150, 298)
    blank = await asyncio.to_thread(until, lambda: "/page2" in (w.invoke("browser_info", {"label": label}).get("ok") or {}).get("url", ""), 8)
    check("a target=_blank link still opens in the pane", bool(blank))

    # ---------- 5. ending it: on the desktop, on the phone, by revoking ----------
    mark = phone.mark()
    await asyncio.to_thread(w.click, "[data-takeover-stop]")
    ended = await phone.wait(lambda m: m["type"] == "tk_end", 5, mark)
    check("Stop on the desktop ends it for the phone", (ended or {}).get("reason") == "desktop", ended)
    gone = await asyncio.to_thread(until, lambda: not w.js("return !!document.querySelector('[data-takeover]')"), 5)
    check("…and the banner goes", bool(gone))

    again = await phone.ask(type="tk_start", pane=label, timeout=15)
    tk = (again or {}).get("tk")
    await asyncio.to_thread(until, lambda: w.js("return !!document.querySelector('[data-takeover]')"), 5)
    stopped = await phone.ask(type="tk_stop", tk=tk)
    gone = await asyncio.to_thread(until, lambda: not w.js("return !!document.querySelector('[data-takeover]')"), 5)
    check("Stop on the phone ends it on the desktop", (stopped or {}).get("type") == "ack" and bool(gone), stopped)

    third = await phone.ask(type="tk_start", pane=label, timeout=15)
    check("a third takeover starts", bool((third or {}).get("tk")), third)
    await asyncio.to_thread(until, lambda: w.js("return !!document.querySelector('[data-takeover]')"), 5)
    at = time.time()
    await call("mobile.revoke", {"id": device.id})
    gone = await asyncio.to_thread(until, lambda: not w.js("return !!document.querySelector('[data-takeover]')"), 5, 0.05)
    check("a revoke ends it within two seconds", bool(gone) and time.time() - at < 2.0, f"{time.time() - at:.2f} s")
    await phone.close()

    entries = json.dumps((await call("mobile.audit", {"limit": 200}))["entries"])
    check("the audit log has the takeovers' start and end, and nothing typed", "takeover_start" in entries and "takeover_stop" in entries and PASSWORD not in entries and USER not in entries)
    leaked = []
    for root, _, files in os.walk(DATA):
        for f in files:
            try:
                with open(os.path.join(root, f), "rb") as fh:
                    if PASSWORD.encode() in fh.read():
                        leaked.append(f)
            except OSError:
                pass
    check("the password is in no file of the daemon's data folder", not leaked, leaked)


# ---------- the app's own windows (Win32), to minimise one or put one behind another ----------

def _user32():
    import ctypes
    from ctypes import wintypes
    u = ctypes.WinDLL("user32", use_last_error=True)
    u.EnumWindows.argtypes = [ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM), wintypes.LPARAM]
    u.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    u.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
    u.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
    u.SetWindowPos.argtypes = [wintypes.HWND, wintypes.HWND, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int, wintypes.UINT]
    u.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
    return ctypes, wintypes, u


def _exe_of(pid):
    ctypes, wintypes, _ = _user32()
    k32 = ctypes.WinDLL("kernel32", use_last_error=True)
    k32.OpenProcess.restype = wintypes.HANDLE
    h = k32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not h:
        return ""
    try:
        buf, size = ctypes.create_unicode_buffer(1024), wintypes.DWORD(1024)
        return buf.value if k32.QueryFullProcessImageNameW(h, 0, buf, ctypes.byref(size)) else ""
    finally:
        k32.CloseHandle(h)


def app_windows():
    """Visible top-level windows of the app this script started, found by its executable's path (never the person's own Kelpie): [(hwnd, title, area)]."""
    ctypes, wintypes, u = _user32()
    found, exes = [], {}

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def each(hwnd, _):
        if u.IsWindowVisible(hwnd):
            pid = wintypes.DWORD()
            u.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            if pid.value not in exes:
                exes[pid.value] = os.path.normcase(_exe_of(pid.value))
            if exes[pid.value] == os.path.normcase(app):
                title = ctypes.create_unicode_buffer(512)
                u.GetWindowTextW(hwnd, title, 512)
                r = wintypes.RECT()
                u.GetWindowRect(hwnd, ctypes.byref(r))
                found.append((hwnd, title.value, (r.right - r.left) * (r.bottom - r.top)))
        return True

    u.EnumWindows(each, 0)
    return found


def main_window():
    """The app's main window: its largest window that is not a page's own."""
    wins = [x for x in app_windows() if x[1] and "Probe window" not in x[1] and "Example consent" not in x[1]]
    return max(wins, key=lambda x: x[2])[0] if wins else None


def window_titled(text):
    return next((h for h, title, _ in app_windows() if text in title), None)


SW_MINIMIZE, SW_RESTORE = 6, 9


def show_window(hwnd, how):
    _user32()[2].ShowWindow(hwnd, how)


def put_behind(hwnd, other):
    """Puts window `hwnd` right behind `other`, inside its rectangle, without activating either: `other` covers it."""
    ctypes, wintypes, u = _user32()
    r = wintypes.RECT()
    u.GetWindowRect(other, ctypes.byref(r))
    u.SetWindowPos(hwnd, other, r.left + 40, r.top + 80, 0, 0, 0x0001 | 0x0010)  # SWP_NOSIZE | SWP_NOACTIVATE


def bring_up(hwnd):
    _user32()[2].SetWindowPos(hwnd, 0, 0, 0, 0, 0, 0x0001 | 0x0002 | 0x0010)  # HWND_TOP; SWP_NOSIZE | SWP_NOMOVE | SWP_NOACTIVATE


async def hidden_pane(phone, tk, w, pid, label):
    """A pane the computer does not show (another tab in front, the window minimised) takes no input, and the phone is told so. WebView2 keeps input
    sent to such a pane and runs it once the pane shows again, on whatever is there by then: a tap made while it was hidden must never click then."""
    tab = await asyncio.to_thread(w.js, "return window.__kelpie.store.workspace(arguments[0]).activeTab;", pid)
    other = await asyncio.to_thread(w.js, "const ws = window.__kelpie.store.workspace(arguments[0]); return ws.tabs.find((t) => t.id !== arguments[1])?.id;", pid, tab)
    if not other:
        await asyncio.to_thread(w.js, "window.__kelpie.browser.openBrowser('about:blank', 'tab', arguments[0])", pid)
        await asyncio.sleep(1.5)
        other = await asyncio.to_thread(w.js, "const ws = window.__kelpie.store.workspace(arguments[0]); return ws.tabs.find((t) => t.id !== arguments[1])?.id;", pid, tab)
    # The first switch away from a held pane may be undone once (the window brings it back); the next one stays.
    mark = phone.mark()
    for _ in range(3):
        await asyncio.to_thread(w.js, "window.__kelpie.store.selectTab(arguments[0], arguments[1])", pid, other)
        await asyncio.sleep(1.5)
        if await asyncio.to_thread(w.js, "return window.__kelpie.store.workspace(arguments[0]).activeTab;", pid) == other:
            break
    told = await phone.wait(lambda m: m["type"] == "tk_fhb" and m.get("tk") == tk and m.get("hidden") is True, 3, mark)
    check("the phone is told the pane is hidden behind another tab", told is not None)
    since = report_count()
    await phone.tap(tk, 150, 60)
    await asyncio.sleep(1)
    mark = phone.mark()
    await asyncio.to_thread(w.js, "window.__kelpie.store.selectTab(arguments[0], arguments[1])", pid, tab)
    shown = await phone.wait(lambda m: m["type"] == "tk_fhb" and m.get("tk") == tk and m.get("hidden") is False, 4, mark)
    late = await asyncio.to_thread(until, lambda: reported("still", since), 3, 0.05)
    check("a tap made while the pane was hidden does not click, then or once it is shown again", not late, late)
    check("…and the phone hears it is shown again", shown is not None)

    main = main_window()
    check("the app's main window is found", main is not None)
    if main is None:
        return
    mark = phone.mark()
    show_window(main, SW_MINIMIZE)
    told = await phone.wait(lambda m: m["type"] == "tk_fhb" and m.get("tk") == tk and m.get("hidden") is True, 4, mark)
    check("the phone is told the pane is hidden while the window is minimised", told is not None)
    since = report_count()
    await phone.tap(tk, 150, 60)
    await asyncio.sleep(1)
    show_window(main, SW_RESTORE)
    await asyncio.sleep(1.5)
    late = await asyncio.to_thread(until, lambda: reported("still", since), 2, 0.05)
    check("a tap made while the window was minimised does not click once it is restored", not late, late)
    since = report_count()
    await phone.tap(tk, 150, 60)
    again = await asyncio.to_thread(until, lambda: reported("still", since), 5)
    check("…and once shown, a tap clicks again", bool(again))


async def spike_questions(phone, tk, w, pid, label):
    """What WebView2 does with input to a page it does not show, and what that page says of its visibility, measured with the app's debug input
    command (`browser_test_input`, which the takeover's guard does not stand in front of): for a pane behind another tab and in a minimised window,
    and for a window of its own behind the main one and minimised. On the sign-in page, which reports a click on its probe square."""

    def start_input(target, events):
        # Not awaited through WebDriver, which runs one command at a time: a call that waited would hold back the switch that shows the pane.
        w.js("window.__tq = null; const t0 = performance.now(); window.__TAURI_INTERNALS__.invoke('browser_test_input', {label: arguments[0], events: arguments[1]})"
             ".then(() => { window.__tq = performance.now() - t0; }, (e) => { window.__tq = 'error: ' + e; });", target, events)

    def took():
        return w.js("return window.__tq;")

    def click(x, y):
        return [{"method": "Input.dispatchMouseEvent", "params": {"type": t, "x": x, "y": y, "button": "left", "buttons": b, "clickCount": 1}} for t, b in (("mousePressed", 1), ("mouseReleased", 0))]

    def last(key, since):
        r = reported(key, since)
        return [x[key] for x in r]

    async def case(name, target, x, y, key, vis_key, hide, show):
        """Hides (`hide`), clicks `target` at (x, y) past the guard, waits, shows it again (`show`): whether the call returned while hidden, whether the
        click came while hidden or only once shown, what the page said of its visibility, and whether the phone was told."""
        mark = phone.mark()
        since = report_count()
        await hide()
        await asyncio.sleep(1.5)
        start_input(target, click(x, y))
        await asyncio.sleep(2)
        returned = await asyncio.to_thread(took)
        clicked_hidden = bool(reported(key, since))
        await show()
        await asyncio.sleep(2)
        later = await asyncio.to_thread(took)
        clicked_later = bool(reported(key, since)) and not clicked_hidden
        told = any(m["type"] == "tk_fhb" and m.get("hidden") is True for m in phone.log[mark:])
        print(f"SPIKE {name}: input call returned while hidden: {returned is not None} ({returned}); afterwards: {later}; clicked while hidden: {clicked_hidden};"
              f" clicked only once shown: {clicked_later}; visibilityState said: {last(vis_key, since)}; the phone was told hidden: {told}", flush=True)

    tab = await asyncio.to_thread(w.js, "return window.__kelpie.store.workspace(arguments[0]).activeTab;", pid)
    other = await asyncio.to_thread(w.js, "const ws = window.__kelpie.store.workspace(arguments[0]); return ws.tabs.find((t) => t.id !== arguments[1])?.id;", pid, tab)

    async def tab_away():
        # A new hold may bring its pane back once on the first switch away.
        for _ in range(3):
            await asyncio.to_thread(w.js, "window.__kelpie.store.selectTab(arguments[0], arguments[1])", pid, other)
            await asyncio.sleep(1.2)
            if await asyncio.to_thread(w.js, "return window.__kelpie.store.workspace(arguments[0]).activeTab;", pid) == other:
                return

    async def tab_back():
        await asyncio.to_thread(w.js, "window.__kelpie.store.selectTab(arguments[0], arguments[1])", pid, tab)

    main = main_window()

    async def minimise(hwnd):
        show_window(hwnd, SW_MINIMIZE)

    async def restore(hwnd):
        show_window(hwnd, SW_RESTORE)

    if other:
        await case("pane behind another tab", label, 150, 355, "probe", "lvis", tab_away, tab_back)
    if main:
        await case("pane in a minimised window", label, 150, 355, "probe", "lvis", lambda: minimise(main), lambda: restore(main))
    # A window of its own, opened by the page: the phone sees it from now on.
    mark = phone.mark()
    await phone.tap(tk, 150, 418)
    popup = await phone.frame(mark, lambda m: m["meta"].get("window") == "popup", 15)
    await asyncio.sleep(1.5)
    probe = window_titled("Probe window")
    labels = [f"{label}-popup-{i}" for i in range(1, 6)]
    infos = {lb: (await asyncio.to_thread(w.invoke, "browser_info", {"label": lb})).get("ok") for lb in labels}
    popup_label = next((lb for lb, info in infos.items() if info and "/probe" in (info.get("url") or "")), None)
    if popup is None or probe is None or popup_label is None or main is None:
        print(f"SPIKE probe window: not found (frame {popup is not None}, window {probe}, label {popup_label}, main {main})", flush=True)
        return

    async def behind():
        put_behind(probe, main)

    async def up():
        bring_up(probe)

    await case("window behind the main one", popup_label, 140, 64, "pclick", "pvis", behind, up)
    await case("window minimised", popup_label, 140, 64, "pclick", "pvis", lambda: minimise(probe), lambda: restore(probe))
    # Through the phone, with the guard: behind the main window, a tap must not click later.
    since = report_count()
    put_behind(probe, main)
    await asyncio.sleep(1.5)
    await phone.tap(tk, 140, 64)
    await asyncio.sleep(1.5)
    bring_up(probe)
    await asyncio.sleep(2)
    print(f"SPIKE a phone tap on a window behind the main one clicks later: {bool(reported('pclick', since))}", flush=True)
    # Closed from its own page: the phone goes back to the pane.
    mark = phone.mark()
    await asyncio.to_thread(w.invoke, "browser_test_input", {"label": popup_label, "events": click(140, 144)})
    await phone.frame(mark, lambda m: m["meta"].get("window") == "pane", 10)


def webview_cpu(seconds):
    """CPU (percent of one core) of the app's WebView2 processes over `seconds`."""
    import psutil
    procs = []
    for p in psutil.process_iter(["exe", "name"]):
        if p.info["exe"] and os.path.normcase(p.info["exe"]) == os.path.normcase(app):
            procs += [c for c in p.children(recursive=True) if c.name().lower() == "msedgewebview2.exe"]

    def total():
        t = 0.0
        for c in procs:
            try:
                ct = c.cpu_times()
                t += ct.user + ct.system
            except Exception:  # noqa: BLE001 - gone meanwhile
                pass
        return t

    t0, w0 = total(), time.time()
    time.sleep(seconds)
    return round((total() - t0) / (time.time() - w0) * 100, 1)


def pct(values, q):
    v = sorted(values)
    return v[min(len(v) - 1, int(len(v) * q))] if v else None


async def spike(phone, tk, w, pid):
    """What the spike asks, measured on this machine and printed (`SPIKE ...`)."""
    # Tap → the page's click handler, by the one clock both share.
    latencies = []
    for _ in range(20):
        since = report_count()
        sent = time.time() * 1000
        await phone.tap(tk, 90, 355)
        got = await asyncio.to_thread(until, lambda: reported("probe", since), 3, 0.01)
        if got:
            latencies.append(int(got[-1]["probe"]) - sent)
        await asyncio.sleep(0.15)
    print(f"SPIKE tap->click ms: n={len(latencies)} p50={pct(latencies, 0.5):.0f} p95={pct(latencies, 0.95):.0f} trusted={all(r.get('trusted') == 'true' for r in reported('probe'))}", flush=True)
    # Frames as they come: how old the newest is when it arrives, and how large.
    mark = phone.mark()
    await asyncio.sleep(5)
    frames = [m for m in phone.log[mark:] if m["type"] == "tk_frame"]
    ages = [m["_w"] * 1000 - m["server_ts"] for m in frames]
    sizes = [len(m["data"]) * 3 // 4 for m in frames]
    print(f"SPIKE frames in 5 s: {len(frames)}; age ms p50={pct(ages, 0.5)} p95={pct(ages, 0.95)}; jpeg bytes p50={pct(sizes, 0.5)} max={max(sizes) if sizes else None}; meta={frames[-1]['meta'] if frames else None}", flush=True)
    # The WebView's CPU with the stream on, then with the pane hidden behind another tab (and whether frames still come from it).
    on = await asyncio.to_thread(webview_cpu, 5)
    print(f"SPIKE webview cpu with the stream: {on}% of a core", flush=True)
    tab = await asyncio.to_thread(w.js, "const s = window.__kelpie.store; const ws = s.workspace(arguments[0]); return ws.activeTab;", pid)
    await asyncio.to_thread(w.js, "window.__kelpie.browser.openBrowser('about:blank', 'tab', arguments[0])", pid)
    await asyncio.sleep(1.5)
    other = await asyncio.to_thread(w.js, "const ws = window.__kelpie.store.workspace(arguments[0]); return ws.tabs[ws.tabs.length - 1].id;", pid)
    back = await asyncio.to_thread(w.js, "return window.__kelpie.store.workspace(arguments[0]).activeTab;", pid)
    print(f"SPIKE first switch away: the held pane's tab came back: {back == tab}", flush=True)
    # The second switch away stays: the pane is hidden now, and only captures come.
    await asyncio.to_thread(w.js, "window.__kelpie.store.selectTab(arguments[0], arguments[1])", pid, other)
    await asyncio.sleep(2.5)
    mark = phone.mark()
    await asyncio.sleep(3)
    hidden = [m for m in phone.log[mark:] if m["type"] == "tk_frame"]
    print(f"SPIKE frames in 3 s with the pane hidden: {len(hidden)} ({sum(1 for m in hidden if m['meta'].get('captured'))} captures)", flush=True)
    await asyncio.to_thread(w.js, "window.__kelpie.store.selectTab(arguments[0], arguments[1])", pid, tab)
    await asyncio.sleep(1.5)


async def spike_restart_still(phone, tk, label):
    """Whether a screencast that starts on a still, visible page sends a first frame by itself (else only the capture fallback shows it). A new takeover
    of the same pane starts one; its token is what the rest of the run uses."""
    await phone.ask(type="tk_stop", tk=tk)
    await asyncio.sleep(1)
    mark = phone.mark()
    started = await phone.ask(type="tk_start", pane=label, timeout=15)
    first = await phone.frame(mark, timeout=3)
    print(f"SPIKE a screencast started on a still, visible page sends a first frame by itself: {first is not None and not first['meta'].get('captured')} (first frame captured: {first and first['meta'].get('captured')})", flush=True)
    return (started or {}).get("tk") or tk


def refused(d, method, params):
    try:
        d.call(method, params)
        return None
    except SystemExit as e:  # kelpie-call's way of reporting an error
        return str(e)


SITE_PORT = 0
TS_BIN = TS_STATE = TS_LOG = None


def main():
    global SITE_PORT, TS_BIN, TS_STATE, TS_LOG
    site = ThreadingHTTPServer(("127.0.0.1", 0), Site)
    SITE_PORT = site.server_address[1]
    threading.Thread(target=site.serve_forever, daemon=True).start()
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
        if run is not None:
            try:
                run.d.quit()
            except Exception:  # noqa: BLE001
                pass
        stop("driver")
        assert os.environ["KELPIE_ENDPOINT"].startswith("kelpie-takeover-")
        d.stop()
        site.shutdown()
        shutil.rmtree(WORK, ignore_errors=True)
    failed = [r for r in results if not r[1]]
    print(json.dumps({"passed": len(results) - len(failed), "failed": len(failed)}))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
