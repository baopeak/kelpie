"""The computer's screen as a person uses it from a phone: installed Google Chrome as an iPhone (390×844 at 3×, touch, an iPhone's user agent), the
hosted phone app at the REAL production relay (wss://kelpie-link-relay.baopeak.workers.dev), a daemon and app of this script's own, and one window
of this script's own as the only thing shared (Windows).

    python -X utf8 scripts/e2e/remote_screen_real.py <scratch-dir> <app> [--only a,b] [--headed] [--video] [--json out.json]

<app> is a debug build of the app with its frontend embedded (`cd app && pnpm tauri build --debug --no-bundle`), with the debug kelpied and
kelpie-screen next to it (`cargo build -p kelpied -p kelpie-screen`; `CARGO_PROFILE_DEV_OPT_LEVEL=1` keeps the debug seams while the picture's
encryption runs near release speed). Needs tauri-driver (PATH or ~/.cargo/bin), msedgedriver matching WebView2 (PATH or MSEDGEDRIVER), installed
Google Chrome, Playwright for Python (`pip install playwright`; no browser download: `channel="chrome"`), Python's `cryptography`, `websockets`,
`psutil`, `Pillow` and tkinter. It needs the internet: the phone app, the room and TURN are Cloudflare's.

What is this run's own, and how it is cleaned up:
- the daemon: its own endpoint `kelpie-remote-real-<pid>` and a temporary data folder (phone_support.isolate), started from the debug build given
  (never an installed kelpied), stopped by this script. It is started WITHOUT `KELPIE_E2E`, the one way a debug daemon reaches a relay that is
  not on this machine; it still trusts this script as the app (`KELPIE_TRUSTED_APP`) and answers the OS verification from
  `KELPIE_TEST_USER_PRESENCE=approve`. Tailscale stays off (nothing calls the real CLI). The app and tauri-driver keep `KELPIE_E2E` (agent configs
  read only).
- the room on the production relay: made when the relay is turned on, and dropped at the end by `mobile.reset` (the daemon tells the room
  `drop`), which also forgets the phone. A run killed before that leaves a room that expires after 90 days unused.
- Chrome: a fresh profile folder in the scratch folder, removed at the end; headless unless `--headed`. Recordings (`--video`) and screenshots
  are of the phone page only, under `<scratch-dir>/remote-real-<pid>/`, kept for the report (delete the folder when done).
- the shared window: remote_screen_target.py, topmost at (60, 60); only it is picked on the phone. The phone's input moves this computer's real
  pointer and keys while a check runs: do not use the computer meanwhile.

The page: by default the phone app the production relay serves (what a phone gets today). `--bundle app/dist-phone-hosted` serves this
checkout's hosted build (`cd app && pnpm build:phone:hosted`) at the production origin instead, through Playwright's routing (service workers
off), while the relay's WebSocket stays the real one: a change to the phone app is checked over the real relay before it is deployed.
`--via auto|p2p|relay_turn|relay` picks the way in the picker.

After pairing and starting the screen, the steps run in order (`--only` keeps some): nudge, ramp, path, still, click, move, targets, gestures,
typing, zoom, mode, rotate, background, freeze, network, restart. Each prints PASS or FAIL; numbers go to `--json`. The shared window notes
mouse and key input that was not injected (someone at this computer) and its being minimized: a step during which that happened is reported
as TAINTED, and its result is not the phone's alone.
"""
import argparse
import ctypes
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import phone_support as ps  # noqa: E402

ORIGIN = "https://kelpie-link-relay.baopeak.workers.dev"
IPHONE_UA = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_5 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.5 Mobile/15E148 Safari/604.1"
PHONE_NAME = "Remote audit iPhone"
results, metrics = [], {}


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail[:400]})" if detail and not ok else ""), flush=True)
    return bool(ok)


def metric(name, value):
    metrics[name] = value
    print(f"  · {name}: {json.dumps(value, ensure_ascii=False)}", flush=True)


def until(fn, timeout=30, step=0.1):
    end = time.time() + timeout
    while time.time() < end:
        try:
            v = fn()
        except Exception:  # noqa: BLE001 - a page between two states
            v = None
        if v:
            return v
        time.sleep(step)
    return None


def pct(values, q):
    v = sorted(values)
    return round(v[min(len(v) - 1, int(round((len(v) - 1) * q)))], 1) if v else None


# ---------- this computer: the daemon, the app, the shared window ----------


class Rig:
    def __init__(self, scratch, app):
        self.scratch, self.app = os.path.abspath(scratch), os.path.abspath(app)
        self.bin = os.path.dirname(self.app)
        self.kelpied = os.path.join(self.bin, "kelpied.exe")
        self.work = os.path.join(self.scratch, f"remote-real-{os.getpid()}")
        os.makedirs(self.work, exist_ok=True)
        self.d = self.run = self.target = self.driver = None
        self.events_path = os.path.join(self.work, "target-events.jsonl")
        self.title = f"Kelpie remote target {os.getpid()}"

    def start(self):
        for what, path in (("app", self.app), ("daemon next to the app", self.kelpied), ("screen helper next to the app", os.path.join(self.bin, "kelpie-screen.exe"))):
            if not os.path.exists(path):
                raise SystemExit(f"no {what}: {path}")
        self.data = ps.isolate("remote-real")
        os.environ["KELPIE_DAEMON_PATH"] = self.kelpied
        os.environ["KELPIE_SCREEN_PATH"] = os.path.join(self.bin, "kelpie-screen.exe")
        os.environ.pop("KELPIE_E2E", None)
        os.environ.pop("KELPIE_ALLOW_REAL_DAEMON", None)
        # The daemon first, with no KELPIE_E2E (the production relay); its load is measured, as on the owner's computer.
        self.d = ps.Daemon(self.kelpied, env={"KELPIE_LOAD_FORCE": LOAD[0]} if LOAD[0] else None).start()
        if self.d.call("daemon.info").get("test_user_presence") != "approve":
            raise SystemExit("the daemon would raise a real verification prompt: use a debug build")
        # The app (through tauri-driver) with KELPIE_E2E: agent configs read only, nothing opened in Explorer.
        os.environ["KELPIE_E2E"] = "1"
        import smoke_support  # noqa: E402 - only once this process is isolated

        native, driver = smoke_support.native_driver(), smoke_support.tauri_driver()
        if not native or not driver:
            raise SystemExit(f"needs tauri-driver and {smoke_support.native_driver_hint()}")
        port = ps_free_port()
        os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{port}"
        self.driver = subprocess.Popen([driver, "--port", str(port), "--native-port", str(ps_free_port()), "--native-driver", native], env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if not until(lambda: port_open(port), 30):
            raise SystemExit("tauri-driver did not open its port")
        self.run = smoke_support.Run(os.path.join(self.work, "app-shots"), app=self.app)
        info = smoke_support.daemon(self.run.d, "daemon.info") if until(lambda: self.run.d.js("return !!(window.__kelpie && window.__kelpie.store.connected())"), 60) else {}
        check("the app is connected to this script's daemon", info.get("pid") == self.d.pid, info.get("pid"))
        self.target = subprocess.Popen([sys.executable, os.path.join(HERE, "remote_screen_target.py"), self.title, self.events_path])
        check("the shared window is up", bool(until(lambda: self.hwnd() and self.events("ready"), 20)))

    def hwnd(self):
        return ctypes.windll.user32.FindWindowW(None, self.title)

    def events(self, kind=None, since=0.0):
        try:
            with open(self.events_path, encoding="utf-8") as f:
                rows = [json.loads(line) for line in f if line.strip()]
        except OSError:
            return []
        return [r for r in rows if (kind is None or r["k"] == kind) and r["t"] >= since]

    def bounds(self):
        """The window as DWM draws it (what the helper captures): its picture's pixel (0, 0) is (left, top)."""
        import screen_support as ss

        return ss.frame_bounds(self.hwnd())

    def client_origin(self):
        r = self.events("ready")[-1]
        return r["x"], r["y"]

    def log(self):
        try:
            with open(os.path.join(self.data, "logs", "kelpied.log"), encoding="utf-8", errors="replace") as f:
                return f.read()
        except OSError:
            return ""

    def stop(self):
        if self.d is not None:
            try:
                # Forgets the phone and drops the room on the production relay (the daemon tells it `drop`).
                self.d.call("mobile.reset")
                time.sleep(2)
            except Exception as e:  # noqa: BLE001
                print(f"mobile.reset failed: {e}", flush=True)
        if self.run is not None:
            try:
                self.run.d.quit()
            except Exception:  # noqa: BLE001
                pass
        for p in (self.target, self.driver):
            if p is not None and p.poll() is None:
                subprocess.run(["taskkill", "/PID", str(p.pid), "/T", "/F"], capture_output=True)
        assert os.environ["KELPIE_ENDPOINT"].startswith("kelpie-remote-real-")
        if self.d is not None:
            self.d.stop()


def ps_free_port():
    import socket

    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def port_open(port):
    import socket

    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", port)) == 0


# ---------- the phone: installed Chrome as an iPhone ----------

PROBE_JS = r"""
(pts) => {
  const canvas = document.querySelector('.screen-canvas canvas');
  const ctx = canvas.getContext('2d');
  const dpr = window.devicePixelRatio || 1;
  window.__probe?.stop?.();
  const p = { log: [], last: {}, running: true };
  p.stop = () => { p.running = false; };
  const read = () => {
    const r = canvas.getBoundingClientRect();
    for (const pt of pts) {
      const d = ctx.getImageData(Math.round((pt.x - r.left) * dpr), Math.round((pt.y - r.top) * dpr), 1, 1).data;
      const c = d[0] > 160 && d[1] < 90 && d[2] > 160 ? 'magenta' : d[1] > 160 && d[0] < 90 && d[2] < 90 ? 'green' : d[0] > 160 && d[1] < 90 && d[2] < 90 ? 'red' : d[2] > 160 && d[0] < 90 && d[1] < 90 ? 'blue' : d[0] < 40 && d[1] < 40 && d[2] < 40 ? 'black' : 'other';
      if (p.last[pt.name] !== c) { p.last[pt.name] = c; p.log.push({ name: pt.name, c, at: performance.now() }); }
    }
    if (p.running) requestAnimationFrame(read);
  };
  requestAnimationFrame(read);
  window.__probe = p;
  window.__downs = [];
  if (!window.__downsHooked) {
    window.__downsHooked = true;
    document.addEventListener('pointerdown', (e) => window.__downs.push(e.timeStamp), { capture: true, passive: true });
    document.addEventListener('pointermove', (e) => { (window.__moves ||= []).push([e.timeStamp, e.clientX]); if (window.__moves.length > 4000) window.__moves.splice(0, 2000); }, { capture: true, passive: true });
  }
}
"""


TYPES = {".html": "text/html", ".js": "text/javascript", ".css": "text/css", ".json": "application/json", ".svg": "image/svg+xml", ".png": "image/png",
         ".webmanifest": "application/manifest+json", ".woff2": "font/woff2", ".ico": "image/x-icon", ".txt": "text/plain"}


def serve_bundle(route, folder):
    """A GET of the hosted app's files at the production origin, answered from this checkout's hosted build (`pnpm build:phone:hosted`)."""
    from urllib.parse import urlparse

    path = urlparse(route.request.url).path
    if path.startswith("/v1/") or route.request.method != "GET":
        return route.continue_()
    rel = path.lstrip("/") or "index.html"
    file = os.path.normpath(os.path.join(folder, rel))
    if not file.startswith(os.path.normpath(folder)) or not os.path.isfile(file):
        file = os.path.join(folder, "index.html")
    with open(file, "rb") as f:
        body = f.read()
    route.fulfill(status=200, body=body, headers={"content-type": TYPES.get(os.path.splitext(file)[1], "application/octet-stream"), "cache-control": "no-store"})


class Phone:
    def __init__(self, rig, headed=False, video=False, bundle=None):
        from playwright.sync_api import sync_playwright

        self.rig = rig
        self.pw = sync_playwright().start()
        self.profile = tempfile.mkdtemp(prefix="kelpie-remote-real-chrome-", dir=rig.work)
        opts = dict(channel="chrome", headless=not headed, viewport={"width": 390, "height": 844}, device_scale_factor=3, is_mobile=True, has_touch=True,
                    user_agent=IPHONE_UA, locale="en-US", args=["--disable-features=CalculateNativeWinOcclusion", "--disable-backgrounding-occluded-windows"])
        if bundle:
            # The page is this checkout's hosted build, served at the production origin (the relay's WebSocket is not routed: it is the real one).
            opts["service_workers"] = "block"
        if video:
            opts["record_video_dir"] = os.path.join(rig.work, "video")
            opts["record_video_size"] = {"width": 390, "height": 844}
        self.ctx = self.pw.chromium.launch_persistent_context(self.profile, **opts)
        if bundle:
            self.ctx.route(ORIGIN + "/**", lambda route: serve_bundle(route, bundle))
        self.page = self.ctx.pages[0] if self.ctx.pages else self.ctx.new_page()
        self.cdp = self.ctx.new_cdp_session(self.page)
        self.authenticator = None
        self.console = []
        self.page.on("console", lambda m: self.console.append(f"{time.strftime('%H:%M:%S')} {m.type}: {m.text}"[:400]))

    def js(self, expr, arg=None):
        return self.page.evaluate(expr, arg) if arg is not None else self.page.evaluate(expr)

    def wait(self, expr, timeout=30, arg=None):
        return until(lambda: self.js(expr, arg), timeout)

    def shot(self, name):
        path = os.path.join(self.rig.work, f"{len(os.listdir(self.rig.work)):03d}-{name}.png")
        self.page.screenshot(path=path)
        return path

    def passkeys(self):
        self.cdp.send("WebAuthn.enable", {"enableUI": False})
        self.authenticator = self.cdp.send("WebAuthn.addVirtualAuthenticator", {"options": {"protocol": "ctap2", "transport": "internal", "hasResidentKey": True,
                                                                                               "hasUserVerification": True, "isUserVerified": True, "automaticPresenceSimulation": True}})["authenticatorId"]

    # Touches: real touch events (CDP), in CSS pixels of the page.
    def _touch(self, kind, points):
        self.cdp.send("Input.dispatchTouchEvent", {"type": kind, "touchPoints": [{"x": x, "y": y, "id": i, "radiusX": 4, "radiusY": 4, "force": 1} for i, (x, y) in enumerate(points)]})

    def tap(self, x, y, hold=0.05):
        self._touch("touchStart", [(x, y)])
        time.sleep(hold)
        self._touch("touchEnd", [])

    def slide(self, x0, y0, x1, y1, ms=250, hold=0.0, steps=None):
        steps = steps or max(4, int(ms / 16))
        self._touch("touchStart", [(x0, y0)])
        if hold:
            time.sleep(hold)
        # Paced by the clock (each touch is a DevTools round trip): the slide takes `ms`, not `ms` plus the round trips.
        start = time.monotonic()
        for i in range(1, steps + 1):
            f = i / steps
            self._touch("touchMove", [(x0 + (x1 - x0) * f, y0 + (y1 - y0) * f)])
            time.sleep(max(0.0, start + ms / 1000 * i / steps - time.monotonic()))
        self._touch("touchEnd", [])

    def two(self, a0, b0, a1, b1, ms=300, steps=None, lift=True):
        steps = steps or max(4, int(ms / 16))
        self._touch("touchStart", [a0])
        self._touch("touchStart", [a0, b0])
        for i in range(1, steps + 1):
            f = i / steps
            self._touch("touchMove", [(a0[0] + (a1[0] - a0[0]) * f, a0[1] + (a1[1] - a0[1]) * f), (b0[0] + (b1[0] - b0[0]) * f, b0[1] + (b1[1] - b0[1]) * f)])
            time.sleep(ms / 1000 / steps)
        if lift:
            self._touch("touchEnd", [])

    def pinch(self, cx, cy, d0, d1, ms=350):
        self.two((cx - d0 / 2, cy), (cx + d0 / 2, cy), (cx - d1 / 2, cy), (cx + d1 / 2, cy), ms)

    def box(self):
        return self.js("(() => { const r = document.querySelector('.screen-canvas')?.getBoundingClientRect(); return r && {x: r.left, y: r.top, w: r.width, h: r.height}; })()")

    def state(self):
        return self.js("""(() => ({phase: document.querySelector('.screen-live')?.dataset.phase ?? (document.querySelector('[data-screen-ended]') ? 'ended' : document.querySelector('.takeover-pick') ? 'pick' : null),
          details: document.querySelector('[data-screen-details]')?.textContent ?? null, quality: document.querySelector('.screen-quality')?.textContent ?? null,
          lost: [...document.querySelectorAll('.takeover-lost')].map(e => e.textContent), lock: document.querySelector('[data-lock-screen]')?.dataset.lockScreen ?? null,
          conn: document.querySelector('.conn')?.dataset.state ?? null, ended: document.querySelector('[data-screen-ended]')?.textContent ?? null}))()""")

    def times(self):
        return self.js("window.__kelpieScreenTimes ? window.__kelpieScreenTimes() : null")

    def cursor_css(self):
        """The phone's cursor layer, in page CSS pixels (its point), or None when hidden."""
        return self.js("""(() => { const c = document.querySelector('[data-screen-cursor]'); if (!c || c.style.display === 'none') return null;
          const m = /translate3d\\(([-\\d.]+)px, ([-\\d.]+)px/.exec(c.style.transform); const b = document.querySelector('.screen-canvas').getBoundingClientRect();
          return m && {x: b.left + Number(m[1]), y: b.top + Number(m[2])}; })()""")

    def close(self):
        try:
            self.ctx.close()
        finally:
            self.pw.stop()


# ---------- mapping between the window and the picture ----------


def screen_to_css(phone, rig, sx, sy):
    """A point on this computer's screen (physical pixels) → where it is on the phone's page, with the picture fitted (no zoom)."""
    left, top, w, h = rig.bounds()
    b = phone.box()
    scale = min(b["w"] / w, b["h"] / h)
    ox, oy = b["x"] + (b["w"] - w * scale) / 2, b["y"] + (b["h"] - h * scale) / 2
    return ox + (sx - left) * scale, oy + (sy - top) * scale


def in_window(rig, cx, cy):
    """A point of the window's client area (physical pixels from its corner) → the screen."""
    x0, y0 = rig.client_origin()
    return x0 + cx, y0 + cy


def physical_cursor():
    import screen_support as ss

    return ss.cursor()


# ---------- the steps ----------


def step_pair(rig, phone):
    d = rig.d
    t0 = time.time()
    try:
        d.call("mobile.enable", {"lan": False})
    except SystemExit:
        pass  # refused until a way in exists: the relay below
    d.call("mobile.set_relay", {"on": True})
    relay = until(lambda: (d.call("mobile.status").get("relay") or {}).get("connected") and d.call("mobile.status")["relay"], 60)
    check("the daemon made its room on the production relay and is in it", bool(relay), d.call("mobile.status").get("relay"))
    metric("relay", {k: relay.get(k) for k in ("url", "version", "rtt_ms")} if relay else None)
    d.call("mobile.enable", {"lan": False})
    phone.page.goto(ORIGIN + "/")
    phone.wait("!!document.querySelector('[data-stay-in-browser]') || !!document.querySelector('[data-scan]')", 30)
    phone.js("document.querySelector('[data-stay-in-browser]')?.click()")
    check("the hosted page offers to pair", bool(phone.wait("!!document.querySelector('[data-scan]')", 15)), phone.js("document.body.innerText.slice(0, 200)"))
    made = until(lambda: d.call("mobile.pair_start", {"hosted": True}), 60, 2)
    phone.js("(link) => { const i = document.querySelector('[data-paste]'); i.value = link; i.dispatchEvent(new Event('input', {bubbles: true})); document.querySelector('[data-paste-go]').click(); }", made["url"])
    phone.wait("!!document.querySelector('.card input.field') && !document.querySelector('[data-scan]')", 20)
    phone.js("(n) => { const i = document.querySelector('.card input.field'); i.value = n; i.dispatchEvent(new Event('input', {bubbles: true})); document.querySelector('.card .btn.primary').click(); }", PHONE_NAME)
    digits = phone.wait("document.querySelector('[data-pair-code]')?.textContent", 40)
    req = ps.pending_request(d, PHONE_NAME, 30)
    check("the request came through the relay from the hosted page", req.get("address") == "the relay" and req.get("origin_kind") == "hosted", req)
    d.call("mobile.approve", {"id": req["id"], "allow": True, "code": digits, "permission": "write"})
    check("approved: the page says what it trusts", bool(phone.wait("!!document.querySelector('[data-trust]')", 40)))
    phone.js("document.querySelector('[data-trust] .btn')?.click()")
    phone.passkeys()
    check("the hosted app stops at its lock, a passkey to set up", phone.wait("document.querySelector('[data-lock-screen]')?.dataset.lockScreen === 'enrol'", 30) is True, phone.state())
    phone.js("document.querySelector('[data-lock-go]').click()")
    # The person allows the screen through the relay (Settings → Mobile, verified) and control for this phone.
    until(lambda: any(x["name"] == PHONE_NAME and x.get("passkey") for x in d.call("mobile.status")["devices"]), 40)
    d.call("mobile.set_screen", {"remote": True})
    dev = [x for x in d.call("mobile.status")["devices"] if x["name"] == PHONE_NAME][0]
    d.call("mobile.set_permission", {"id": dev["id"], "permission": "control"})
    rig.device = dev["id"]
    t_lock = time.time()
    opened = phone.wait("!document.querySelector('[data-lock-screen]') && !!document.querySelector('[data-see-screen]:not([disabled])')", 90)
    metric("lock_to_app_s", round(time.time() - t_lock, 1))
    if not check("the passkey is set up and the app opens", bool(opened), phone.state()):
        phone.shot("after-passkey")
        print(phone.js("document.querySelector('main')?.innerText?.slice(0, 600)"))
    metric("pair_s", round(time.time() - t0, 1))


def step_live(rig, phone, hwnd=None):
    """Opens the picker, picks the shared window (by its handle in the row's id: never another window or a monitor), passes Face ID (the
    virtual authenticator), waits for the picture."""
    phone.wait("!!document.querySelector('[data-see-screen]:not([disabled])')", 30)
    phone.js("document.querySelector('[data-see-screen]').click()")
    rows = phone.wait("(() => { const r = [...document.querySelectorAll('[data-target]')]; return r.length ? r.map(e => ({id: e.dataset.target, text: e.innerText})) : null; })()", 30)
    metric("picker_rows", [r["text"].replace("\n", " | ") for r in rows or []])
    metric("connect_through", phone.js("[...document.querySelectorAll('[data-via]')].map(r => ({via: r.dataset.via, ok: r.dataset.ok, text: r.innerText.split(String.fromCharCode(10)).join(' | '), checked: r.querySelector('input')?.checked}))"))
    metric("webrtc_in_page", phone.js("typeof RTCPeerConnection"))
    u = ctypes.windll.user32
    metric("target_minimized_before_pick", bool(u.IsIconic(rig.hwnd())))
    if u.IsIconic(rig.hwnd()):
        u.ShowWindow(rig.hwnd(), 9)  # SW_RESTORE: the shared window shown, as the person would before sharing it
        time.sleep(0.5)
    if VIA[0] != "auto":
        phone.js("(v) => document.querySelector(`[data-via=\"${v}\"] input`)?.click()", VIA[0])
        time.sleep(0.3)
    want = f"w:{hwnd or rig.hwnd()}:"
    row = next((r for r in rows or [] if r["id"].startswith(want)), None)
    if not check("the picker lists the shared window", row is not None, rows):
        return False
    t0 = time.time()
    live = None
    for _ in range(3):
        phone.js("(id) => document.querySelector(`[data-target=\"${id}\"] button`)?.click()", row["id"])
        live = phone.wait("document.querySelector('.screen-live')?.dataset.phase === 'live' || (!!document.querySelector('.takeover-pick .banner') && 'pick')", 60)
        if live is True:
            break
        # Refused while the computer was busy, then "less busy now: tap See it": tapped again, as a person does (and said in the numbers).
        metric("start_refused", phone.js("document.querySelector('.takeover-pick .banner')?.textContent"))
        phone.wait("!!document.querySelector(`[data-target] button:not([disabled])`)", 120)
    live = live is True
    first = until(lambda: (phone.times() or {}).get("presentInterval") is not None or (phone.times() or {}).get("decodeMs"), 30, 0.05)
    t1 = time.time()
    if not check("Face ID passes and the picture comes", bool(live and first), phone.state()):
        phone.shot("no-picture")
        print(phone.js("document.querySelector('main')?.innerText?.slice(0, 600)"))
        print(chr(10).join(phone.console[-20:]))
        return False
    metric("first_picture_s", round(t1 - t0, 2))
    # The help sheet shows once: closed as a person does.
    phone.js("document.querySelector('[data-screen-help-close]')?.click()")
    rig.target_row = row["id"]
    return True


def step_path(rig, phone, wait_s=30):
    seen, t0 = [], time.time()
    while time.time() - t0 < wait_s:
        d = phone.js("document.querySelector('[data-screen-details]')?.textContent")
        if d and (not seen or seen[-1][1].split(' · ')[0] != d.split(' · ')[0]):
            seen.append((round(time.time() - t0, 1), d))
        time.sleep(0.5)
    metric("path_changes", seen)
    metric("load", {k: v for k, v in rig.d.call("mobile.load").items() if k in ("level", "reasons", "reading", "since")})
    st = phone.state()
    metric("details_after_%ds" % wait_s, st["details"])
    metric("quality", st["quality"])
    log = rig.log()
    lines = [ln for ln in log.splitlines() if re.search(r"p2p|turn|rtc|ice|relay", ln, re.I)][-25:]
    metric("daemon_log_path_lines", lines)
    path = phone.js("document.querySelector('[data-screen-details]')?.dataset.screenP2p || document.querySelector('[data-screen-details]')?.dataset.screenPath")
    metric("path", path)
    return path


def step_stats(phone, label, secs=10):
    a = phone.times()
    time.sleep(secs)
    b = phone.times()
    st = phone.state()
    metric(f"{label}_times", b)
    metric(f"{label}_details", st["details"])
    return b


def set_mode(phone, want):
    """Trackpad or Direct touch, by the button at the top, as a person does."""
    for _ in range(3):
        now = phone.js("document.querySelector('[data-screen-mode]')?.textContent")
        if (want == "direct") == ("Direct" in (now or "")):
            return True
        phone.js("document.querySelector('[data-screen-mode]').click()")
        time.sleep(0.2)
    return False


def zoom_of(phone):
    return bool(phone.js("!!document.querySelector('[data-screen-fit]')"))


def fit(phone):
    phone.js("document.querySelector('[data-screen-fit]')?.click()")
    time.sleep(0.3)


def put_cursor(rig, phone, sx, sy, tol=4, tries=8):
    """Trackpad: slides one finger, as a person does, until this computer's cursor is within `tol` physical pixels of (sx, sy). Returns the
    slides it took (None if it never got there) and the distance left."""
    b = phone.box()
    left, top, w, h = rig.bounds()
    scale = min(b["w"] / w, b["h"] / h)
    gain = 1.2
    for n in range(1, tries + 1):
        cx, cy = physical_cursor()
        dx, dy = sx - cx, sy - cy
        if abs(dx) <= tol and abs(dy) <= tol:
            return n - 1, 0
        # Finger distance (CSS px) for this computer distance, at the slide's speed; corrected by what the last slide did.
        fx, fy = dx * scale / gain, dy * scale / gain
        dist = max(1.0, (fx * fx + fy * fy) ** 0.5)
        ms = max(120, min(600, dist * 8))
        x0, y0 = b["x"] + b["w"] / 2 - fx / 2, b["y"] + b["h"] / 2 - fy / 2
        phone.slide(x0, y0, x0 + fx, y0 + fy, ms=ms)
        time.sleep(0.35)
        nx, ny = physical_cursor()
        moved = ((nx - cx) ** 2 + (ny - cy) ** 2) ** 0.5
        wanted = (dx * dx + dy * dy) ** 0.5
        if moved > 1 and wanted > 1:
            gain *= moved / wanted
    cx, cy = physical_cursor()
    left_px = round(((sx - cx) ** 2 + (sy - cy) ** 2) ** 0.5)
    return (None, left_px) if left_px > tol else (tries, left_px)


def tap_center(phone):
    b = phone.box()
    phone.tap(b["x"] + b["w"] / 2, b["y"] + b["h"] / 2)


def latency_from_probe(phone, name, downs_from):
    """Pairs each touch (pointerdown) with the first colour change of the probe point after it."""
    log = [e for e in phone.js("window.__probe.log") if e["name"] == name]
    downs = phone.js("window.__downs")[downs_from:]
    out = []
    for i, t in enumerate(downs):
        nxt = downs[i + 1] if i + 1 < len(downs) else float("inf")
        hit = next((e["at"] for e in log if t < e["at"] < nxt and e["c"] in ("green", "magenta")), None)
        out.append(round(hit - t, 1) if hit else None)
    return out


def step_click_latency(rig, phone, n=10):
    """Direct touch: a tap on the patch flips its colour on the computer; the time from the touch to the new colour in the phone's picture."""
    set_mode(phone, "direct")
    fit(phone)
    sx, sy = in_window(rig, 470, 40)
    x, y = screen_to_css(phone, rig, sx, sy)
    phone.js(PROBE_JS, [{"name": "patch", "x": x, "y": y}])
    since = time.time() * 1000
    metric("click_debug", {"want_screen": [sx, sy], "css": [round(x, 1), round(y, 1)], "box": phone.box(), "bounds": rig.bounds(), "mode": phone.js("document.querySelector('[data-screen-mode]')?.textContent")})
    for i in range(n):
        phone.tap(x, y)
        time.sleep(0.9)
        if i == 0:
            metric("click_debug_cursor_after_tap", physical_cursor())
    lat = latency_from_probe(phone, "patch", 0)
    got = len(rig.events("patch", since))
    phone.js("window.__probe.stop()")
    check(f"each of {n} taps on the patch reached the window", got == n, got)
    ok = [v for v in lat if v is not None]
    metric("click_to_photon_ms", {"p50": pct(ok, .5), "p95": pct(ok, .95), "max": max(ok) if ok else None, "seen": f"{len(ok)}/{n}", "all": lat})
    return lat


def step_move_latency(rig, phone, n=8):
    """Trackpad: the cursor slid across the middle of the move patch flips it red/blue on the computer; the time from the phone's own cursor
    crossing (its echo) to the new colour in the picture."""
    set_mode(phone, "trackpad")
    fit(phone)
    lx, ly = in_window(rig, 580 + 120, 30)
    put_cursor(rig, phone, lx, ly, tol=10)
    mid_sx, _ = in_window(rig, 580 + 195, 30)
    mx, my = screen_to_css(phone, rig, *in_window(rig, 580 + 100, 45))
    mid_css, _ = screen_to_css(phone, rig, mid_sx, ly)
    phone.js(PROBE_JS, [{"name": "mover", "x": mx, "y": my}])
    phone.js("""(mid) => { const c = document.querySelector('[data-screen-cursor]'); const b = document.querySelector('.screen-canvas');
      window.__cross = []; let side = null;
      const tick = () => { const m = /translate3d\\(([-\\d.]+)px/.exec(c.style.transform); if (m) { const x = b.getBoundingClientRect().left + Number(m[1]);
        const s = x < mid ? 'L' : 'R'; if (s !== side) { side = s; window.__cross.push([s, performance.now()]); } } if (window.__probe?.running) requestAnimationFrame(tick); };
      requestAnimationFrame(tick); }""", mid_css)
    b = phone.box()
    for i in range(n):
        dx = 40 if i % 2 == 0 else -40
        x0 = b["x"] + b["w"] / 2
        phone.slide(x0, b["y"] + b["h"] / 2, x0 + dx, b["y"] + b["h"] / 2, ms=120)
        time.sleep(0.8)
    cross = phone.js("window.__cross")
    log = [e for e in phone.js("window.__probe.log") if e["name"] == "mover"]
    phone.js("window.__probe.stop()")
    lat = []
    for s, at in cross[1:]:
        want = "blue" if s == "R" else "red"
        hit = next((e["at"] for e in log if e["at"] > at and e["c"] == want), None)
        lat.append(round(hit - at, 1) if hit else None)
    ok = [v for v in lat if v is not None]
    metric("move_to_photon_ms", {"p50": pct(ok, .5), "p95": pct(ok, .95), "seen": f"{len(ok)}/{len(lat)}", "all": lat})
    check("the computer's reaction to the cursor shows in the picture for each crossing", len(ok) == len(lat) and lat, lat)


def step_targets(rig, phone):
    """Small targets with the trackpad: the five 18-pixel buttons, clicked one by one, at full view and then zoomed in."""
    set_mode(phone, "trackpad")
    fit(phone)
    report = {}
    for zoomed in (False, True):
        if zoomed:
            b = phone.box()
            put_cursor(rig, phone, *in_window(rig, 64, 37), tol=12)
            phone.pinch(b["x"] + b["w"] / 2, b["y"] + b["h"] / 2, 60, 260)
            time.sleep(0.5)
            metric("zoomed_for_targets", zoom_of(phone))
        hits = []
        for i, name in enumerate("ABCDE"):
            since = time.time() * 1000
            t0 = time.time()
            slides, left_px = put_cursor(rig, phone, *in_window(rig, 10 + i * 22 + 9 + 10, 10 + 18 + 9), tol=4)
            tap_center(phone)
            time.sleep(0.6)
            got = [e["name"] for e in rig.events("small", since)]
            hits.append({"want": name, "got": got, "slides": slides, "left_px": left_px, "s": round(time.time() - t0, 1)})
        report["zoomed" if zoomed else "fit"] = hits
        check(f"each small button is clicked, {'zoomed in' if zoomed else 'at full view'}", all(h["got"] == [h["want"]] for h in hits), hits)
    fit(phone)
    metric("small_targets", report)


def step_gestures(rig, phone):
    """Trackpad: double click a word, right click (long press), drag-select text, drag the list's scrollbar, drag and drop, two-finger scroll."""
    set_mode(phone, "trackpad")
    fit(phone)
    b = phone.box()
    c = (b["x"] + b["w"] / 2, b["y"] + b["h"] / 2)
    out = {}
    # Double click a word of the text.
    since = time.time() * 1000
    put_cursor(rig, phone, *in_window(rig, 10 + 150, 80 + 12), tol=3)
    phone.tap(*c)
    time.sleep(0.12)
    phone.tap(*c)
    time.sleep(0.6)
    sel = [e["text"] for e in rig.events("selection", since)]
    out["double"] = {"double": len(rig.events("double", since)), "selection": sel[-1:] if sel else None}
    check("a double tap double-clicks: a word is selected", bool(rig.events("double", since)) and sel and " " not in sel[-1].strip(), out["double"])
    # Right click with a long press, then Esc from the key bar closes the menu.
    since = time.time() * 1000
    put_cursor(rig, phone, *in_window(rig, 300, 400 + 200), tol=6)
    phone.tap(*c, hold=0.8)
    time.sleep(0.6)
    out["right"] = len(rig.events("right", since))
    phone.shot("right-click-menu")
    phone.js("document.querySelector('.keybar button[aria-label=\"Escape\"]').click()")
    check("a long press right-clicks", out["right"] == 1, out["right"])
    # Drag-select text: long press on a line's start, then move along it.
    since = time.time() * 1000
    put_cursor(rig, phone, *in_window(rig, 10 + 6, 80 + 12 + 20), tol=3)
    phone.slide(c[0] - 60, c[1], c[0] + 40, c[1] + 6, ms=700, hold=0.6)
    time.sleep(0.6)
    sel = [e["text"] for e in rig.events("selection", since)]
    out["drag_select"] = sel[-1][:60] if sel else None
    check("a long press then a slide selects text", bool(sel) and len(sel[-1]) > 8, out["drag_select"])
    # The list's scrollbar thumb, dragged down.
    since = time.time() * 1000
    put_cursor(rig, phone, *in_window(rig, 590 + 400 - 9, 80 + 30), tol=4)
    phone.slide(c[0], c[1] - 40, c[0], c[1] + 60, ms=700, hold=0.6)
    time.sleep(0.6)
    tops = [e["top"] for e in rig.events("list_scroll", since)]
    out["scrollbar"] = tops[-1] if tops else None
    check("dragging the scrollbar scrolls the list", bool(tops) and tops[-1] > 0.05, tops[-3:])
    # Two-finger scroll over the list (back up).
    since = time.time() * 1000
    put_cursor(rig, phone, *in_window(rig, 700, 200), tol=10)
    phone.two((c[0] - 20, c[1] - 60), (c[0] + 20, c[1] - 60), (c[0] - 20, c[1] + 60), (c[0] + 20, c[1] + 60), ms=500)
    time.sleep(0.8)
    tops2 = [e["top"] for e in rig.events("list_scroll", since)]
    out["two_finger_scroll"] = tops2[-3:]
    check("two fingers scroll the list", bool(tops2), tops2)
    # Drag and drop the orange square onto the target.
    since = time.time() * 1000
    put_cursor(rig, phone, *in_window(rig, 10 + 80, 400 + 140), tol=6)
    t0 = time.time()
    _, _, w, _ = rig.bounds()
    scale = min(b["w"] / w, b["h"] / rig.bounds()[3])
    fx = (850 - 80) * scale / 1.2
    phone.slide(c[0] - fx / 2, c[1], c[0] + fx / 2, c[1], ms=1200, hold=0.6)
    time.sleep(0.8)
    drops = rig.events("drop", since)
    out["drop"] = drops[-1] if drops else None
    out["drop_s"] = round(time.time() - t0, 1)
    check("a long press then a slide drags the square and drops it on the target", bool(drops) and drops[-1]["inside"], out["drop"])
    metric("gestures", out)


def type_on_phone(phone, text):
    phone.page.fill("[data-screen-text]", text)
    phone.js("document.querySelector('[data-screen-type]').click()")
    return phone.wait("document.querySelector('[data-text-result]')?.dataset.textResult === 'sent' || document.querySelector('[data-text-result]')?.dataset.textResult", 10)


def step_typing(rig, phone):
    set_mode(phone, "trackpad")
    fit(phone)
    since = time.time() * 1000
    put_cursor(rig, phone, *in_window(rig, 300, 367), tol=6)
    tap_center(phone)
    time.sleep(0.4)
    words = "Xin chào Việt Nam, đẹp quá ạ! Ừ, được."
    t0 = time.time()
    res = type_on_phone(phone, words)
    got = until(lambda: next((e["value"] for e in reversed(rig.events("entry", since)) if e["value"]), None) if any(e["value"] == words for e in rig.events("entry", since)) else None, 8)
    metric("type_text_s", round(time.time() - t0, 2))
    last = [e["value"] for e in rig.events("entry", since)]
    check("text with Vietnamese diacritics arrives whole", got == words, (res, last[-1:] if last else None))
    # Enter from the key bar; Ctrl+A, Ctrl+C, End, Ctrl+V with the Ctrl key and one letter.
    since = time.time() * 1000
    phone.js("document.querySelector('.keybar button[aria-label=\"Enter\"]').click()")
    time.sleep(0.4)
    check("Enter from the key bar", bool(rig.events("enter", since)))
    for letter in ("a", "c"):
        phone.js("[...document.querySelectorAll('.keybar button')].find(b => b.textContent === 'Ctrl').click()")
        type_on_phone(phone, letter)
        time.sleep(0.3)
    phone.js("document.querySelector('.keybar button[aria-label=\"End\"]').click()")
    time.sleep(0.2)
    phone.js("[...document.querySelectorAll('.keybar button')].find(b => b.textContent === 'Ctrl').click()")
    type_on_phone(phone, "v")
    doubled = until(lambda: any(e["value"] == words + words for e in rig.events("entry", since)), 5)
    check("Ctrl+A, Ctrl+C, End, Ctrl+V from the phone copy and paste", bool(doubled), [e["value"][-50:] for e in rig.events("entry", since)][-2:])
    # The phone's clipboard to the computer, and back: is there a way at all?
    metric("clipboard_ui", phone.js("[...document.querySelectorAll('.screen-live button')].map(b => b.textContent || b.getAttribute('aria-label')).join(' | ')"))


def step_zoom(rig, phone):
    """Pinch in, then work zoomed: a one-finger slide moves the cursor (does the view pan instead?), a click near the view's edge lands where
    the cursor is, Fit goes back."""
    for mode in ("trackpad", "direct"):
        set_mode(phone, mode)
        fit(phone)
        b = phone.box()
        c = (b["x"] + b["w"] / 2, b["y"] + b["h"] / 2)
        if mode == "trackpad":
            put_cursor(rig, phone, *in_window(rig, 500, 550), tol=10)
        phone.pinch(*c, 60, 280)
        time.sleep(0.5)
        zoomed = zoom_of(phone)
        check(f"{mode}: a pinch zooms the view", zoomed)
        cur0 = physical_cursor()
        css0 = phone.cursor_css()
        since = time.time() * 1000
        phone.slide(c[0] - 50, c[1], c[0] + 50, c[1] + 20, ms=400)
        time.sleep(0.5)
        cur1 = physical_cursor()
        css1 = phone.cursor_css()
        moved = ((cur1[0] - cur0[0]) ** 2 + (cur1[1] - cur0[1]) ** 2) ** 0.5
        wheel = rig.events("wheel", since)
        metric(f"zoomed_{mode}_slide", {"computer_cursor_moved_px": round(moved), "phone_cursor_css": [css0, css1], "wheels": len(wheel)})
        check(f"{mode}, zoomed: a one-finger slide moves the computer's cursor (no scroll)", moved > 5 and not wheel, {"moved": moved, "wheels": len(wheel)})
        phone.shot(f"zoomed-{mode}")
        fit(phone)
        check(f"{mode}: Fit shows the whole screen again", not zoom_of(phone))


def step_mode(rig, phone):
    for want in ("direct", "trackpad"):
        check(f"the button switches to {want}", set_mode(phone, want))


def step_rotate(rig, phone):
    """Landscape and back: the picture fills the new shape, the cursor layer stays on the computer's cursor."""
    out = {}
    for size, name in (({"width": 844, "height": 390}, "landscape"), ({"width": 390, "height": 844}, "portrait")):
        phone.page.set_viewport_size(size)
        time.sleep(1.0)
        b = phone.box()
        cx, cy = physical_cursor()
        want = screen_to_css(phone, rig, cx, cy)
        got = phone.cursor_css()
        err = round(((want[0] - got[0]) ** 2 + (want[1] - got[1]) ** 2) ** 0.5, 1) if got else None
        out[name] = {"box": b, "cursor_err_css": err}
        phone.shot(f"rotate-{name}")
        check(f"{name}: the cursor layer is on the computer's cursor (within 3 CSS px)", err is not None and err <= 3, out[name])
    metric("rotate", out)


SET_HIDDEN = "(h) => { Object.defineProperty(document, 'hidden', {value: h, configurable: true}); Object.defineProperty(document, 'visibilityState', {value: h ? 'hidden' : 'visible', configurable: true}); document.dispatchEvent(new Event('visibilitychange')); }"


def back_to_picture(rig, phone, label):
    """After the screen ended: from wherever the page is (lock, picker, ended card) to a live picture again, as a person does. Seconds taken."""
    t0 = time.time()
    if phone.js("document.querySelector('[data-lock-screen]')?.dataset.lockScreen === 'unlock'"):
        until(lambda: phone.js("document.querySelector('[data-lock-go]:not([disabled])')?.click() ?? false") is None or not phone.js("!!document.querySelector('[data-lock-screen]')"), 30, 0.5)
    phone.js("document.querySelector('[data-screen-ended] .btn')?.click()")
    phone.js("document.querySelector('.takeover-pick') || document.querySelector('[data-see-screen]')?.click()")
    ok = phone.wait("!!document.querySelector('[data-target]')", 30)
    if ok:
        phone.js("(id) => document.querySelector(`[data-target^=\"${id}\"] button`)?.click()", f"w:{rig.hwnd()}:")
        ok = phone.wait("document.querySelector('.screen-live')?.dataset.phase === 'live' && !!window.__kelpieScreenTimes && window.__kelpieScreenTimes().presentInterval", 40)
    phone.js("document.querySelector('[data-screen-help-close]')?.click()")
    took = round(time.time() - t0, 1)
    metric(f"{label}_back_to_picture_s", took if ok else None)
    return ok


def picture_moves(phone, secs=3):
    """Whether new pictures are drawn now (the window's clock line changes every frame? not here: the patch probe) – by the page's own count."""
    a = (phone.times() or {}).get("presentInterval") or {}
    time.sleep(secs)
    b = (phone.times() or {}).get("presentInterval") or {}
    return a.get("n") != b.get("n") if a or b else False


def step_background(rig, phone):
    """Leaving the app (hidden) and coming back within the lock's grace; then a long absence (after the grace)."""
    phone.js(SET_HIDDEN, True)
    time.sleep(2)
    st = phone.state()
    phone.js(SET_HIDDEN, False)
    time.sleep(1.5)
    metric("background_short", {"while_hidden": st, "after": phone.state()})
    check("back from the background: a way back to the picture in at most two taps", back_to_picture(rig, phone, "background_short"))


def step_freeze(rig, phone, secs=20):
    """An iPhone lock as Safari does it: hidden, then frozen (no JavaScript runs, sockets may be gone), then active and visible again. And a
    freeze with no hidden first (a page the system suspended)."""
    for hidden in (True, False):
        label = "lock" if hidden else "suspend"
        if hidden:
            phone.js(SET_HIDDEN, True)
        phone.cdp.send("Page.setWebLifecycleState", {"state": "frozen"})
        time.sleep(secs)
        phone.cdp.send("Page.setWebLifecycleState", {"state": "active"})
        if hidden:
            phone.js(SET_HIDDEN, False)
        time.sleep(3)
        st = phone.state()
        moving = picture_moves(phone, 3)
        metric(f"{label}_{secs}s", {"state": st, "picture_moves": moving})
        phone.shot(f"after-{label}")
        if st["phase"] == "live":
            check(f"after a {label} of {secs} s the live picture moves again (no frozen picture)", moving, st)
        else:
            check(f"after a {label} of {secs} s the screen comes back", back_to_picture(rig, phone, label), phone.state())


def step_network(rig, phone):
    """Offline for 5 s and 20 s, then slow links (as Chrome's DevTools throttles a WebSocket; peer to peer UDP is not throttled by it)."""
    phone.cdp.send("Network.enable")
    out = {}
    for secs in (5, 20):
        phone.cdp.send("Network.emulateNetworkConditions", {"offline": True, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1})
        time.sleep(secs)
        during = phone.state()
        phone.cdp.send("Network.emulateNetworkConditions", {"offline": False, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1})
        t0 = time.time()
        back = until(lambda: phone.state()["phase"] == "live" and picture_moves(phone, 1), 40, 0.5)
        out[f"offline_{secs}s"] = {"during": during, "back_s": round(time.time() - t0, 1) if back else None, "after": phone.state()}
        if not back:
            back_to_picture(rig, phone, f"offline_{secs}")
    for name, lat, down, up in (("3g", 300, 750_000 / 8, 250_000 / 8), ("4g", 80, 9_000_000 / 8, 3_000_000 / 8), ("high_rtt", 600, -1, -1)):
        phone.cdp.send("Network.emulateNetworkConditions", {"offline": False, "latency": lat, "downloadThroughput": down, "uploadThroughput": up})
        time.sleep(8)
        out[name] = {"times": phone.times(), "state": phone.state()}
    phone.cdp.send("Network.emulateNetworkConditions", {"offline": False, "latency": 0, "downloadThroughput": -1, "uploadThroughput": -1})
    metric("network", out)


def step_restart(rig, phone):
    since = time.time()
    phone.js("document.querySelector('[data-screen-stop]').click()")
    ended = phone.wait("!!document.querySelector('[data-screen-ended]') || !document.querySelector('.screen-live')", 10)
    check("Stop ends the screen", bool(ended), phone.state())
    check("…and it starts again", back_to_picture(rig, phone, "restart"))
    metric("restart_total_s", round(time.time() - since, 1))


def step_ramp(rig, phone, secs=60):
    """A moving picture (the square crossing the board) for a minute: the frame rate, size and rate as the stream settles, every 5 s."""
    open(rig.events_path + ".anim", "w").close()
    rows = []
    try:
        t0 = time.time()
        last_n = (phone.times() or {}).get("presentInterval", {}).get("n", 0)
        while time.time() - t0 < secs:
            time.sleep(5)
            tm = phone.times() or {}
            n = tm.get("presentInterval", {}).get("n", 0)
            rows.append({"t": round(time.time() - t0), "fps_seen": round((n - last_n) / 5, 1) if n >= last_n else None, "details": phone.state()["details"], "quality": phone.state()["quality"]})
            last_n = n
            print(f"    {rows[-1]}", flush=True)
        phone.shot("ramp-end")
        metric("ramp_times", phone.times())
    finally:
        os.remove(rig.events_path + ".anim")
    metric("ramp", rows)


VIA = ["auto"]
LOAD = [None]


def step_memory(rig, phone, secs=15):
    """With the daemon's load held at L2 for memory alone (`--load L2:memory`), a moving picture keeps its quality: no "computer busy" on the
    quality line, and the window's full height (the reduced rungs scale it down). The frame rate seen is measured, not checked: in headless
    Chrome on this computer the decoder, not the load, sets it."""
    load = rig.d.call("mobile.load")
    open(rig.events_path + ".anim", "w").close()
    try:
        time.sleep(3)
        a = (phone.times() or {}).get("presentInterval", {}).get("n", 0)
        time.sleep(secs)
        b = (phone.times() or {}).get("presentInterval", {}).get("n", 0)
    finally:
        os.remove(rig.events_path + ".anim")
    st = phone.state()
    fps = round((b - a) / secs, 1)
    metric("memory_load", {"level": load.get("level"), "reason": load.get("reason"), "quality": st["quality"], "details": st["details"], "fps_seen": fps})
    check("the daemon is held at L2 for memory alone", load.get("level") == 2 and load.get("reason") == ["memory"], load)
    check("...and the picture does not say the computer is busy", "computer busy" not in (st["quality"] or ""), st["quality"])
    # The picture at the window's own height (the reduced rungs scale it down; memory alone must not).
    lines = int((re.match(r"(\d+)p", st["quality"] or "") or [0, 0])[1])
    check("...and is at the window's full height, not scaled down", lines >= rig.bounds()[3] - 2, (lines, rig.bounds()[3]))
    phone.shot("memory-l2")
def step_nudge(rig, phone, n=6):
    """Trackpad at full view: slow small slides (8 CSS px over 400 ms, under the tap's 10 px) move the cursor and click nothing; a long press
    whose finger drifts 5 px as it lands still right-clicks; a quick tap is still one click at once, and two quick taps a double click."""
    set_mode(phone, "trackpad")
    fit(phone)
    put_cursor(rig, phone, *in_window(rig, 400, 400 + 120), tol=15)
    b = phone.box()
    c = (b["x"] + b["w"] / 2, b["y"] + b["h"] / 2)
    since = time.time() * 1000
    moves = []
    for i in range(n):
        before = physical_cursor()
        d = 8 if i % 2 == 0 else -8
        phone.slide(c[0], c[1], c[0] + d, c[1], ms=360, steps=8)
        time.sleep(0.5)
        after = physical_cursor()
        moves.append(after[0] - before[0])
    clicks = rig.events("click_any", since)
    metric("nudge_cursor_dx_px", moves)
    check(f"{n} slow small slides click nothing", not clicks, [(round(e["t"] - since), e["x"], e["y"]) for e in clicks])
    check("...and each moves the computer's cursor its way", all((m > 0) == (i % 2 == 0) and m != 0 for i, m in enumerate(moves)), moves)
    since = time.time() * 1000
    phone._touch("touchStart", [c])
    for k in (2, 4, 5):
        time.sleep(0.025)
        phone._touch("touchMove", [(c[0] + k, c[1])])
    time.sleep(0.7)
    phone._touch("touchEnd", [])
    time.sleep(0.6)
    rights, lefts = rig.events("right", since), rig.events("click_any", since)
    phone.js("document.querySelector('.keybar button[aria-label=\"Escape\"]').click()")
    time.sleep(0.3)
    check("a long press that drifts 5 px as it lands still right-clicks", len(rights) == 1 and not lefts, (len(rights), len(lefts)))
    since = time.time() * 1000
    phone.tap(*c, hold=0.06)
    time.sleep(0.8)
    check("a quick tap is still one click", len(rig.events("click_any", since)) == 1, len(rig.events("click_any", since)))
    since = time.time() * 1000
    phone.tap(*c, hold=0.05)
    time.sleep(0.12)
    phone.tap(*c, hold=0.05)
    time.sleep(0.8)
    check("two quick taps are a double click", len(rig.events("double_any", since)) == 1, rig.events(None, since))


STEPS = {"memory": step_memory, "nudge": step_nudge, "ramp": step_ramp, "path": step_path, "still": lambda r, p: step_stats(p, "still"), "click": step_click_latency, "move": step_move_latency, "targets": step_targets,
         "gestures": step_gestures, "typing": step_typing, "zoom": step_zoom, "mode": step_mode, "rotate": step_rotate, "background": step_background,
         "freeze": step_freeze, "network": step_network, "restart": step_restart}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("scratch")
    ap.add_argument("app")
    ap.add_argument("--only", default="")
    ap.add_argument("--headed", action="store_true")
    ap.add_argument("--video", action="store_true")
    ap.add_argument("--json", default=None)
    ap.add_argument("--bundle", default=None, help="this checkout's hosted build (app/dist-phone-hosted) served at the production origin")
    ap.add_argument("--load", default=None, help="the daemon's load held at a level and reason (KELPIE_LOAD_FORCE), such as L2:memory")
    ap.add_argument("--via", default="auto", help="the way chosen in the picker: auto, p2p, relay_turn, relay")
    ap.add_argument("--hold", type=float, default=0, help="seconds to keep everything up at the end (exploring)")
    args = ap.parse_args()
    if os.name != "nt":
        raise SystemExit(__doc__)
    VIA[0] = args.via
    LOAD[0] = args.load
    rig = Rig(args.scratch, args.app)
    phone = None
    try:
        rig.start()
        phone = Phone(rig, headed=args.headed, video=args.video, bundle=args.bundle and os.path.abspath(args.bundle))
        step_pair(rig, phone)
        if step_live(rig, phone):
            phone.shot("live")
            for name in [s for s in (args.only.split(",") if args.only else STEPS) if s]:
                print(f"--- {name}", flush=True)
                if ctypes.windll.user32.IsIconic(rig.hwnd()):
                    ctypes.windll.user32.ShowWindow(rig.hwnd(), 9)
                    time.sleep(0.5)
                step_since = time.time() * 1000
                try:
                    STEPS[name](rig, phone)
                except Exception:  # noqa: BLE001 - one step's failure is reported, the next still runs
                    traceback.print_exc()
                    check(f"{name} ran to its end", False)
                    phone.shot(f"error-{name}")
                other = [e["k"] + ":" + e.get("what", "") for e in rig.events(None, step_since) if e["k"] in ("human_input", "unmapped")]
                if other:
                    metric(f"{name}_TAINTED_by_someone_at_the_computer", other[:12])
                if phone.state()["phase"] != "live":
                    back_to_picture(rig, phone, f"after_{name}")
        if args.hold:
            time.sleep(args.hold)
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("the run went to its end", False)
        if phone:
            try:
                phone.shot("error")
                print("\n".join(phone.console[-30:]))
            except Exception:  # noqa: BLE001
                pass
    finally:
        if phone:
            phone.close()
        rig.stop()
        if phone:
            shutil.rmtree(phone.profile, ignore_errors=True)
    failed = [r for r in results if not r[1]]
    summary = {"passed": len(results) - len(failed), "failed": len(failed), "metrics": metrics}
    if args.json:
        with open(args.json, "w", encoding="utf-8") as f:
            json.dump(summary, f, indent=1, ensure_ascii=False)
    print(json.dumps({"passed": summary["passed"], "failed": summary["failed"]}))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
