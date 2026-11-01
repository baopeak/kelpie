"""E2E for the mobile companion: Kelpie (tauri-driver) plus a phone-sized Edge (msedgedriver).

    python -X utf8 scripts/e2e/mobile.py <screenshot-dir> <scratch-dir> [--edgedriver PATH]

Start tauri-driver with KELPIE_LOAD_FORCE=L0:calm (and the isolated KELPIE_ENDPOINT / KELPIE_DATA_DIR): the
functional checks here must not depend on how busy the host is. How the page behaves under load is
mobile_resilience.py's and stress.py's job. The script checks that the pin is in effect. Use debug builds of the app and
the daemon, side by side, with KELPIE_TEST_USER_PRESENCE=approve in tauri-driver's environment: the daemon the app starts
inherits it, the one this script starts gets it from here, and it answers the operating system's verification instead of
a prompt (a release build has no such path). The script refuses to go on against a daemon that does not say it is in effect, so
nothing here can raise a real prompt. The daemon the app starts also inherits `KELPIE_TAILSCALE_BIN`, so set that in tauri-driver's environment to
`<scratch-dir>\\fake-tailscale.cmd` (`fake-tailscale` elsewhere), which this script writes: with `KELPIE_E2E` and no such variable the daemon has no Tailscale,
and the Tailscale checks fail saying so. It never reaches the real one either way.

The server is bound to 127.0.0.1 (no firewall prompt on the test machine). The phone is
headless Edge with mobile emulation, driven through msedgedriver on port 9515.

Covers: off by default; turning it on (status, URL); security headers and the embedded phone client
(SolidJS, app/phone); pairing with a QR link, typing the phone's code on the desktop, the device list, nothing readable on the wire, no CSP
violation; the phone's
inbox (a session that needs you comes first), a session's screen, typing and quick keys reaching the
terminal, a prompt answered with one tap, a chat permission answered from the phone (the agent gets
exactly one answer), a message sent to a chat from the phone (the desktop shows the phone's chip, then the
transcript item with its origin; Stop works; either side cancels; a read-only device has no message box;
a desktop message only the computer can send holding the queue is explained on the phone's own chip, with Skip, and
noticed on the desktop outside the chat pane), a `#s=` deep link, the page coming back by itself after the
server went away; a read-only device cannot type; a socket from another origin is refused; unpairing
disconnects the phone; turning it off stops the server; after the computer restarted, what waited is shown
as lost with its text in the stopped chat, also after a reload, until discarded; a used or wrong pairing
code gets no answer, too many unknown phones are rate limited and the desktop says so, the kill switch and the audit list
(last, so the limiter cannot refuse a pairing above). On an HTTPS origin (a certificate of this run's own, at `localhost` on 127.0.0.1, which the phone's browser
is told to accept; needs Python's `cryptography`): the install step, no camera falling back to pasting the link, a pairing that is a secure origin. The app lock on it, with real WebAuthn ceremonies through CDP's virtual authenticator (`WebAuthn.addVirtualAuthenticator`, Blink's and not WebKit's, and no person's face): set up, a copied key refused at the daemon, hide locks, a network flap asks nothing, the idle time locks, the dangerous re-check, the reset, a restart. Then it may
type. The link the daemon really gives there, at 127.0.0.1, unrewritten: a phone paired from it reads, with no lock in its way, since a passkey needs a name. Alerts on a real service worker (`KELPIE_PUSH_TEST_BASE` in tauri-driver's environment, a port this script serves as a stand-in for a push service, which the daemon really POSTs to): the sheet's button, a test alert
and a needs-you event taken from the stand-in, opened with the browser's keys and handed to the worker through CDP's `ServiceWorker.deliverPushMessage`, which shows the notification; a tap on it, as a synthetic `notificationclick` in the worker. The browser's own `pushManager.subscribe` is replaced in the page, since it would reach a real push service, which tests never do. Tailscale mode against a fake CLI (`KELPIE_TAILSCALE_BIN`, never the real one): each state's instruction, the two-step dialog, off.
"""
import asyncio
import base64
import http.client
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(__file__))
import phone_support as ps  # noqa: E402
from smoke_support import Run, daemon, default_app, real_data_dir, restart_daemon  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]


def isolated_env():
    """This script stops and restarts a daemon, so it runs only against one of its own: KELPIE_ENDPOINT (a name that is
    not the default `kelpie-<user>-v<n>`) and KELPIE_DATA_DIR (not the real Kelpie data folder, e.g. %LOCALAPPDATA%\\Kelpie) must be set in THIS
    process, and the app's daemon must turn out to be that one (`check_daemon`). Nothing has been contacted when this
    refuses. Returns the environment a daemon started by this script gets: only what was checked."""
    endpoint, data = os.environ.get("KELPIE_ENDPOINT", ""), os.environ.get("KELPIE_DATA_DIR", "")
    default = re.fullmatch(r"kelpie-[A-Za-z0-9_-]*-v\d+", endpoint) is not None
    real = os.path.normcase(os.path.normpath(real_data_dir()))
    if not endpoint or default:
        raise SystemExit(f"mobile.py stops a daemon: set KELPIE_ENDPOINT to an isolated name in its own environment (got {endpoint or 'nothing'}{', the default' if default else ''}). Refusing.")
    if not data or os.path.normcase(os.path.normpath(data)) == real:
        raise SystemExit("mobile.py stops a daemon: set KELPIE_DATA_DIR to an isolated folder in its own environment (not the real Kelpie folder). Refusing.")
    keep = {k: v for k, v in os.environ.items() if k in ("PATH", "SystemRoot", "SYSTEMROOT", "TEMP", "TMP", "USERPROFILE", "LOCALAPPDATA", "APPDATA", "HOME", "KELPIE_LOAD_FORCE", "KELPIE_E2E", "KELPIE_PUSH_TEST_BASE")}
    # The daemon this script starts has only a fake for Tailscale: `KELPIE_E2E` with `KELPIE_TAILSCALE_BIN`.
    return {**keep, "KELPIE_ENDPOINT": endpoint, "KELPIE_DATA_DIR": data, "KELPIE_TEST_USER_PRESENCE": "approve", "KELPIE_E2E": "1", "KELPIE_TAILSCALE_BIN": FAKE_TS[0]}


def check_daemon(d, env):
    """The daemon the app talks to is the isolated one this script was given, by the daemon's own account."""
    info = daemon(d, "daemon.info")
    same = lambda a, b: os.path.normcase(os.path.normpath(str(a))) == os.path.normcase(os.path.normpath(str(b)))  # noqa: E731
    endpoint_ok = str(info.get("endpoint", "")).endswith(env["KELPIE_ENDPOINT"])
    data_ok = same(info.get("data_dir", ""), env["KELPIE_DATA_DIR"])
    print(f"daemon check: endpoint {'OK' if endpoint_ok else 'WRONG'} | data dir {'OK' if data_ok else 'WRONG'}", flush=True)
    if not (endpoint_ok and data_ok):
        REFUSED.append(True)
        raise SystemExit(f"the app's daemon is not this run's isolated daemon (endpoint {info.get('endpoint')}, data {info.get('data_dir')}): not stopping it. Start tauri-driver with the same KELPIE_ENDPOINT and KELPIE_DATA_DIR.")


def check_no_real_prompt(d):
    """The daemon says verification is answered by the test seam: approving below cannot raise a real prompt."""
    seam = daemon(d, "daemon.info").get("test_user_presence")
    print(f"verification seam: {seam}", flush=True)
    if seam != "approve":
        REFUSED.append(True)
        raise SystemExit(f"the daemon would raise a real verification prompt (test seam: {seam!r}): use a debug build, with KELPIE_TEST_USER_PRESENCE=approve in tauri-driver's environment.")


def certificate_for_loopback(folder):
    """A certificate for localhost and 127.0.0.1 and its key, made for this run: (certificate file, key file). A passkey needs a name, so the page is opened at `localhost`."""
    import datetime
    import ipaddress

    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.x509.oid import NameOID

    key = ec.generate_private_key(ec.SECP256R1())
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")])
    now = datetime.datetime.now(datetime.timezone.utc)
    cert = (x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key()).serial_number(x509.random_serial_number())
            .not_valid_before(now - datetime.timedelta(minutes=5)).not_valid_after(now + datetime.timedelta(days=1))
            .add_extension(x509.SubjectAlternativeName([x509.DNSName("localhost"), x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]), critical=False).sign(key, hashes.SHA256()))
    cert_file, key_file = os.path.join(folder, "loopback.crt"), os.path.join(folder, "loopback.key")
    with open(cert_file, "wb") as f:
        f.write(cert.public_bytes(serialization.Encoding.PEM))
    with open(key_file, "wb") as f:
        f.write(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
    return cert_file, key_file


REFUSED = []  # set when the app's daemon is not the isolated one: the run ends with an error, not with "0 failed"
os.makedirs(scratch, exist_ok=True)
FAKE_TS = ps.write_fake_tailscale(scratch)
ISOLATED = isolated_env()
edgedriver = sys.argv[sys.argv.index("--edgedriver") + 1] if "--edgedriver" in sys.argv else os.environ.get("KELPIE_EDGEDRIVER", r"D:\Works\edgedriver\msedgedriver.exe")
S = "window.__kelpie.store"
PORT = int(os.environ.get("KELPIE_E2E_MOBILE_PORT", "48170"))
# Where this phone's msedgedriver listens: another run's may hold the usual port.
PHONE_DRIVER = int(os.environ.get("KELPIE_E2E_PHONE_DRIVER_PORT", "9515"))


class Phone:
    """A minimal W3C WebDriver client for msedgedriver (Edge with mobile emulation)."""

    def __init__(self):
        self.proc = subprocess.Popen([edgedriver, f"--port={PHONE_DRIVER}"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1.5)
        # The performance log carries the page's WebSocket frames (what it sends and what it is sent).
        # The browser log has what the page's console has, from the moment it loads: content security policy violations too.
        caps = {"capabilities": {"alwaysMatch": {"browserName": "MicrosoftEdge", "acceptInsecureCerts": True, "ms:loggingPrefs": {"performance": "ALL", "browser": "ALL"}, "ms:perfLoggingPrefs": {"enableNetwork": True}, "ms:edgeOptions": {
            "args": ["--headless=new", "--window-size=412,900", "--no-first-run", f"--user-data-dir={tempfile.mkdtemp(prefix='phone-profile-', dir=scratch)}"],  # a phone of its own each run: what an earlier run paired is not this one's
            "mobileEmulation": {"deviceMetrics": {"width": 412, "height": 900, "pixelRatio": 2.0}, "userAgent": "Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Mobile Safari/537.36"},
        }}}}
        made = self.req("POST", "/session", caps)
        self.sid = made["sessionId"]
        self.debugger = made.get("capabilities", {}).get("ms:edgeOptions", {}).get("debuggerAddress")
        self.frames = {}  # the performance log is read once: what was read is kept here

    def req(self, method, path, body=None):
        data = json.dumps(body).encode() if body is not None else None
        r = urllib.request.Request(f"http://127.0.0.1:{PHONE_DRIVER}" + path, data=data, method=method, headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(r, timeout=60) as resp:
                return json.loads(resp.read() or b"{}").get("value", {})
        except urllib.error.HTTPError as e:
            raise RuntimeError(json.loads(e.read() or b"{}").get("value", {}).get("message", "webdriver error")) from None

    def go(self, url):
        self.req("POST", f"/session/{self.sid}/url", {"url": url})

    def socket_frames(self, method):
        """The page's WebSocket frames of `method` (`Network.webSocketFrameSent` or `...Received`) read so far, as
        (opcode, bytes): what anyone on the network could see."""
        for entry in self.req("POST", f"/session/{self.sid}/log", {"type": "performance"}):
            message = json.loads(entry["message"])["message"]
            kind = message.get("method", "")
            if kind.startswith("Network.webSocketFrame"):  # every frame read is kept, whichever method is asked for
                response = message["params"].get("response") or {}
                opcode, payload = response.get("opcode"), response.get("payloadData", "")
                self.frames.setdefault(kind, []).append((opcode, base64.b64decode(payload) if opcode == 2 else payload.encode()))
        return self.frames.get(method, [])

    def csp_violations(self):
        """Every content security policy violation in the browser's log since this was last asked, but for those another page
        is named for: the log also has the browser's own new tab page, which it opens before the page is navigated to. A
        violation need not name the page (`security 2 Loading the script ...`), so only another page's name leaves one out."""
        lines = [e.get("message", "") for e in self.req("POST", f"/session/{self.sid}/log", {"type": "browser"})]
        return [line for line in lines if "Content Security Policy" in line and (line.startswith(f"http://127.0.0.1:{PORT}/") or not line.startswith(("http://", "https://")))]

    def js(self, script, *args):
        return self.req("POST", f"/session/{self.sid}/execute/sync", {"script": script, "args": list(args)})

    def wait(self, script, timeout=20):
        end = time.time() + timeout
        while time.time() < end:
            v = self.js(script)
            if v:
                return v
            time.sleep(0.3)
        return None

    def shot(self, path):
        open(path, "wb").write(base64.b64decode(self.req("GET", f"/session/{self.sid}/screenshot")))

    def quit(self):
        try:
            self.req("DELETE", f"/session/{self.sid}")
        finally:
            self.proc.kill()


class Cdp:
    """One DevTools target of the phone's browser (`page` or `service_worker`), over its own socket."""

    def __init__(self, debugger, kind, url_start):
        from websockets.sync.client import connect

        with urllib.request.urlopen(f"http://{debugger}/json/list", timeout=10) as resp:
            listed = json.loads(resp.read())
        target = next((t for t in listed if t["type"] == kind and t["url"].startswith(url_start)), None)
        if not target:
            raise RuntimeError(f"no {kind} at {url_start}: {[(t['type'], t['url'][:60]) for t in listed]}")
        self.ws = connect(target["webSocketDebuggerUrl"], max_size=None, open_timeout=10)
        self.n = 0
        self.events = []

    def call(self, method, params=None, timeout=15):
        self.n += 1
        self.ws.send(json.dumps({"id": self.n, "method": method, "params": params or {}}))
        end = time.time() + timeout
        while time.time() < end:
            m = json.loads(self.ws.recv(timeout=max(0.1, end - time.time())))
            if m.get("id") == self.n:
                if "error" in m:
                    raise RuntimeError(f"{method}: {m['error']}")
                return m.get("result", {})
            self.events.append(m)
        raise TimeoutError(method)

    def close(self):
        self.ws.close()


def http_get(url, headers=None):
    u = urllib.parse.urlparse(url)
    c = http.client.HTTPConnection(u.hostname, u.port, timeout=10)
    c.request("GET", u.path or "/", headers=headers or {})
    r = c.getresponse()
    body = r.read()
    return r.status, dict(r.getheaders()), body


def pairing_answered(base, link):
    """Whether the daemon answers a pairing handshake made from `link`."""
    return asyncio.run(ps.PairAttempt(base, link, "probe").start()).session is not None


def stranger_refusal(base, dpk):
    """The close code a phone gets that the daemon does not know."""
    async def attempt():
        stranger = ps.Phone(base, ps.Device(*ps.ln.keypair(), dpk, "stranger"))
        await stranger.open()
        await stranger.close()
        return stranger.closed
    return asyncio.run(attempt())


import urllib.parse  # noqa: E402

run = Run(shots)
d = run.d
phone = None


def wait(cond, timeout=20, interval=0.3):
    end = time.time() + timeout
    while time.time() < end:
        v = cond()
        if v:
            return v
        time.sleep(interval)
    return None


LIVE = "return document.querySelector('.conn')?.dataset.state === 'live'"
STATE = "return document.querySelector('.conn')?.dataset.state"
# Connected, with what a loaded machine may show for it (`canAct`, `conn-state.ts`).
CONNECTED = "return ['live', 'busy', 'degraded'].includes(document.querySelector('.conn')?.dataset.state)"


def type_code(digits):
    """Types the phone's six digits into the approval dialog on the desktop."""
    d.js("const i = document.querySelector('.mobile-approve-code'); i.value = arguments[0]; i.dispatchEvent(new Event('input', {bubbles: true}));", digits)


def js_await(expr, *args):
    return d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)


try:
    run.boot()
    # A daemon outlives the app (and the previous run of this script, when the harness reuses its endpoint). Its
    # limiter for failed pairing codes (10 a minute per address, which this script trips on purpose near the end) is
    # kept in memory, so a second run against the same daemon within the minute could not pair. Every run starts
    # from a daemon of its own: the limiter is left as strict as it is.
    kelpied = os.environ.get("KELPIE_DAEMON_PATH") or os.path.join(os.path.dirname(default_app()), "kelpied.exe" if os.name == "nt" else "kelpied")
    check_daemon(d, ISOLATED)
    before = daemon(d, "daemon.info")["pid"]
    restart_daemon(d, kelpied, kill=True, env=ISOLATED)
    check_daemon(d, ISOLATED)
    check_no_real_prompt(d)
    run.check("every run starts with a daemon of its own", daemon(d, "daemon.info")["pid"] != before)
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    for s in daemon(d, "session.list"):
        daemon(d, "session.close", {"handle": s["handle"]})
    daemon(d, "mobile.disable")
    st = daemon(d, "mobile.status")
    for dev in st["devices"]:
        daemon(d, "mobile.revoke", {"id": dev["id"]})
    run.check("mobile access is off by default (or after disabling)", not daemon(d, "mobile.status")["running"])
    proj = os.path.join(scratch, "mobile-proj")
    os.makedirs(proj, exist_ok=True)
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ shortcutHints: false, tips: false }});")
    pid = d.js(f"return {S}.addProject(arguments[0])", proj)

    # ---------- on ----------
    st = js_await("window.__TAURI_INTERNALS__.invoke('daemon_call', {method: 'mobile.enable', params: {ip: '127.0.0.1', port: %d}})" % PORT)
    run.check("turning it on serves on the chosen address", isinstance(st, dict) and st.get("running") and st.get("url") == f"http://127.0.0.1:{PORT}/", str(st)[:200])
    base = f"http://127.0.0.1:{PORT}/"
    load = daemon(d, "mobile.load")
    run.check("the daemon is pinned calm (KELPIE_LOAD_FORCE=L0:calm), so these checks do not depend on the host's load", load.get("level") == 0 and load.get("reason") == ["calm"], str(load)[:200])
    status, headers, body = http_get(base)
    run.check("the web app is served with a strict CSP", status == 200 and b"/assets/" in body and "script-src 'self'" in headers.get("content-security-policy", "") and headers.get("x-frame-options") == "DENY")
    sw_status, sw_headers, _ = http_get(base + "sw.js")
    run.check("the service worker and the manifest are served; the old page's script is gone", sw_status == 200 and "javascript" in sw_headers.get("content-type", "") and http_get(base + "manifest.webmanifest")[0] == 200 and http_get(base + "app.js")[0] == 404)
    d.js(f"{S}.openSettings('mobile')")
    d.wait("return !!document.querySelector('[data-setting=\"mobile-pair\"] .btn.primary')", 10)
    run.shot("01-mobile-settings")

    # ---------- pairing ----------
    run.check("the settings know this window is the verified app", daemon(d, "mobile.status").get("app") is True)
    refused = d.invoke("daemon_call", {"method": "mobile.reset", "params": {}})
    run.check("the webview cannot reach an app-only decision through the plain call", "err" in refused, str(refused)[:120])
    d.js("window.confirm = () => true")
    d.js("document.querySelector('[data-setting=\"mobile-pair\"] .btn.primary').click()")
    link = d.wait("return document.querySelector('.mobile-qr-code')?.dataset.link", 10)
    run.check("Pair a phone shows a QR code with a one-time link", bool(link) and "#p=" in link and d.js("return !!document.querySelector('.mobile-qr-code svg')"))  # never the link in the log: it holds the pairing secret
    run.shot("02-mobile-qr")
    phone = Phone()
    phone.go(link)
    phone.wait("return !!document.querySelector('.card .btn.primary')")
    run.check("the phone opens the pairing page (code taken out of the address bar)", "#p=" not in phone.js("return location.href"))
    phone.js("const i = document.querySelector('.card input'); i.value = 'Test phone';")
    phone.js("document.querySelector('.card .btn.primary').click()")
    run.check("Kelpie asks to approve the phone", bool(d.wait("return [...document.querySelectorAll('.mobile-approve h2')].some(h => h.textContent.includes('Test phone'))", 15)))
    run.shot("03-approve")
    digits = phone.wait("return document.querySelector('[data-pair-code]')?.textContent", 15)
    run.check("the phone shows six digits to type on the computer", bool(re.fullmatch(r"\d{6}", digits or "")))
    phone_key = phone.js("return document.querySelector('[data-pair-key]')?.textContent ?? ''")
    details = d.js("return document.querySelector('.mobile-approve')?.textContent ?? ''")
    run.check("the dialog shows the phone's key, the same one the phone shows", bool(phone_key) and phone_key in details, f"{phone_key!r} in {details[:200]!r}")
    run.check("…asks whether a QR code was just scanned, and where the request came from", "scan a QR" in details and "127.0.0.1" in details, details[:300])
    run.check("…and offers what the phone asked for first (reading)", d.js("return document.querySelector('.mobile-approve [data-allow=read]').classList.contains('primary')"))
    type_code("000000" if digits != "000000" else "111111")
    d.js("document.querySelector('.mobile-approve [data-allow=write]').click()")
    run.check("a wrong code is refused, and the request stays", bool(d.wait("return document.querySelector('.mobile-approve .form-note.error')?.textContent", 10)) and not daemon(d, "mobile.status")["devices"])
    type_code(digits)
    d.js("document.querySelector('.mobile-approve [data-allow=write]').click()")
    run.check("approved phone connects and lists sessions", bool(phone.wait(LIVE, 20)))
    # The served page itself: every frame is binary, and nothing readable is in any of them.
    sent, got = phone.socket_frames("Network.webSocketFrameSent"), phone.socket_frames("Network.webSocketFrameReceived")
    frames = sent + got
    run.check("the page's traffic is binary frames only", len(sent) >= 2 and len(got) >= 2 and all(op == 2 for op, _ in frames), f"{len(sent)} sent, {len(got)} received, opcodes {sorted({op for op, _ in frames}, key=str)}")
    readable = [word for word in (b'"type"', b'"caps"', b"Test phone", b"daemon_epoch", b"hello") if any(word in data for _, data in frames)]
    run.check("the captured bytes hold no plaintext", not readable, str(readable))
    violations = phone.csp_violations()
    run.check("no content security policy violation from the page's load on, through pairing", not violations, str(violations)[:300])
    # The check can see one: a script from elsewhere is refused and shows in the same log.
    phone.js("const s = document.createElement('script'); s.src = 'http://127.0.0.2:9/nothing.js'; document.head.appendChild(s);")
    time.sleep(0.5)
    run.check("…and that log would show one", bool(phone.csp_violations()))
    run.check("the page says it acknowledges frames and answers heartbeats: the daemon does not serve it as a legacy page", phone.js("return document.documentElement.dataset.servedLegacy") == "false", str(phone.js("return document.documentElement.dataset.servedLegacy ?? null")))
    devs = daemon(d, "mobile.status")["devices"]
    run.check("the device is listed with typing allowed", len(devs) == 1 and devs[0]["name"] == "Test phone" and devs[0]["can_write"], str(devs))
    run.check("the used pairing code gets no answer afterwards", not pairing_answered(base, link))

    # ---------- sessions on the phone ----------
    info = js_await(f"{S}.newSession(arguments[0], 'shell', {{ name: 'phone-shell' }})", pid)
    handle = info["handle"]
    ok = phone.wait("return [...document.querySelectorAll('.row')].some(r => r.textContent.includes('phone-shell'))", 15)
    run.check("a new session appears on the phone", bool(ok))
    run.check("the phone shows the project name", bool(phone.js("return [...document.querySelectorAll('.row .sub')].some(s => s.textContent.includes('mobile-proj'))")))
    phone.shot(os.path.join(shots, "04-phone-list.png"))
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-shell')).click()")
    phone.wait("return !!document.querySelector('.screen')")
    time.sleep(2.5)
    phone.js("const t = document.querySelector('.send textarea'); t.value = 'echo phone-$((6*7))';")
    phone.js("document.querySelector('.send .btn.primary').click()")
    out = wait(lambda: "phone-42" in daemon(d, "session.read", {"handle": handle, "lines": 40})["text"], 15)
    run.check("typing on the phone reaches the terminal", bool(out))
    run.check("the phone shows the terminal's screen", bool(phone.wait("return document.querySelector('.screen').textContent.includes('phone-42')", 15)))
    phone.shot(os.path.join(shots, "05-phone-session.png"))
    daemon(d, "session.input", {"handle": handle, "data": "ping -n 30 127.0.0.1\r" if os.name == "nt" else "sleep 30\r"})
    # Ctrl+C only means something once the command is running (on a loaded host PowerShell can take seconds to start it).
    wait(lambda: "Pinging" in daemon(d, "session.read", {"handle": handle, "lines": 40})["text"] if os.name == "nt" else True, 30)
    time.sleep(0.5)
    phone.js("[...document.querySelectorAll('.keys button')].find(b => b.textContent === 'Ctrl+C').click()")
    # The next command is typed only once the Ctrl+C has stopped the running one: the phone's key and this script's input
    # take different ways to the PTY, and a console drops what was typed ahead when it handles Ctrl+C. A fixed pause raced
    # the phone's round trip on a busy host (the echo went first and was dropped, although the Ctrl+C had worked).
    stopped_mark = "Control-C" if os.name == "nt" else "^C"
    stopped = wait(lambda: stopped_mark in daemon(d, "session.read", {"handle": handle, "lines": 40})["text"].split("ping")[-1], 15)
    run.check("quick keys (Ctrl+C) reach the terminal: the running command stops", bool(stopped), str(phone.js("return document.querySelector('.notice')?.textContent ?? null")))
    # The mark comes before the shell is back, and what is typed between the two is dropped too: wait for its prompt.
    if os.name == "nt":
        wait(lambda: daemon(d, "session.read", {"handle": handle, "lines": 40})["text"].split(stopped_mark)[-1].strip().endswith(">"), 15)
    daemon(d, "session.input", {"handle": handle, "data": "echo after-ctrl-c\r"})
    took_next = wait(lambda: "after-ctrl-c" in daemon(d, "session.read", {"handle": handle, "lines": 40})["text"].split("ping")[-1], 15)
    run.check("…and the shell takes the next command", bool(took_next), "" if took_next else repr(daemon(d, "session.read", {"handle": handle, "lines": 12})["text"][-400:]))

    # ---------- inbox, prompt tap ----------
    menu = os.path.join(scratch, "phone_menu.py")
    # Three menus, one right after the other, each reading one key like an agent's TUI menu.
    open(menu, "w", encoding="utf-8").write(r"""import sys
def key():
    try:
        import msvcrt
        return msvcrt.getwch()
    except ImportError:
        return sys.stdin.read(1)
for n in (1, 2, 3):
    print("Bash(npm test %d)" % n)
    print(" Do you want to proceed, round %d?" % n)
    print(" \u276f 1. Yes")
    print("   2. Yes, and don't ask again")
    print("   3. No (esc)")
    print("ANSWERED%d=%s" % (n, key()))
""")
    info2 = js_await(f"{S}.newSession(arguments[0], 'shell', {{ name: 'phone-menu' }})", pid)
    handle2 = info2["handle"]
    time.sleep(2.5)
    daemon(d, "session.input", {"handle": handle2, "data": f'python "{menu}"\r'})
    # Back to the inbox: the session that needs you is first, under its own heading.
    phone.js("history.back()")
    needs = phone.wait("return document.querySelector('section[data-group] .row')?.textContent.includes('phone-menu') && document.querySelector('section[data-group]').dataset.group === 'needs'", 20)
    run.check("the inbox puts the session that needs you first, under its heading", bool(needs), phone.js("return [...document.querySelectorAll('.group')].map(g => g.textContent).join('|')"))
    run.check("the page title counts the sessions that need you", bool(phone.wait("return document.title.startsWith('(1)')", 5)), phone.js("return document.title"))
    phone.shot(os.path.join(shots, "06-phone-inbox.png"))
    phone.wait("const r = [...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-menu')); if (!r) return null; r.click(); return true", 10)

    def card(n):
        """The prompt card for menu `n`, once its buttons are live: the keys on it."""
        return phone.wait("const q = document.querySelector('.prompt .q'); const b = [...document.querySelectorAll('.prompt .key-answer')]; return q && q.textContent.includes('round %d') && b.length && b.every(x => !x.disabled) ? b.map(x => x.textContent.trim()) : null" % n, 20)

    def tap(key):
        return phone.wait("const b = [...document.querySelectorAll('.prompt .key-answer')].find(b => b.textContent.trim() === '%s'); if (!b || b.disabled) return null; b.click(); return true" % key, 10)

    def screen_has(text):
        return bool(wait(lambda: text in daemon(d, "session.read", {"handle": handle2, "lines": 60})["text"], 15))

    keys = card(1)
    # Not "2" ("Yes, and don't ask again"): a choice that lasts is the computer's to make, and the phone hides it.
    run.check("the prompt card offers the prompt's own keys", keys == ["1", "3"], str(keys))
    phone.shot(os.path.join(shots, "07-phone-prompt.png"))
    tap("1")
    run.check("one tap on the card answers the prompt", screen_has("ANSWERED1=1"))
    # The next menu comes at once: its card must start afresh, not stay on the last one's "Sent".
    keys2 = card(2)
    run.check("a prompt right after another gets a fresh card with live buttons", keys2 == ["1", "3"], str(keys2))
    tap("3")
    run.check("one tap answers the second prompt too", screen_has("ANSWERED2=3"))
    # The third: the desktop answers first, while the phone's page is busy and has not heard of it: the phone's tap
    # is late, and the page then hears of the answer (the card goes) before the reply to its tap. A page that is kept
    # busy with a loop for a second queues what arrives, in the order it arrived.
    if card(3):
        import threading

        def desktop_answers_first():
            time.sleep(0.4)
            since = next(x["waiting_since"] for x in daemon(d, "session.list") if x["handle"] == handle2)
            daemon(d, "session.answer", {"handle": handle2, "waiting_since": since, "data": "3", "enter": False})

        helper = threading.Thread(target=desktop_answers_first)
        helper.start()
        phone.js("const end = Date.now() + 1500; while (Date.now() < end) {} const b = [...document.querySelectorAll('.prompt .key-answer')].find(b => b.textContent.trim() === '1'); b.click();")
        helper.join()
        said = phone.wait("return document.querySelector('.notice-bar')?.textContent ?? null", 10)
        run.check("a late answer whose card is gone says who answered first, at session level", bool(said) and "Already answered on the desktop" in said, str(said))
        phone.shot(os.path.join(shots, "07b-phone-stale.png"))
        run.check("the card is gone, the notice stays for at least 5 s", bool(phone.wait("return !document.querySelector('.prompt')", 10)) and (time.sleep(4.5) or bool(phone.js("return !!document.querySelector('.notice-bar')"))))
        run.check("the notice can be dismissed", bool(phone.js("document.querySelector('.notice-bar .icon').click(); return !document.querySelector('.notice-bar')")))
    else:
        run.check("the third prompt came up", False)
    run.check("the card goes away once the prompts are answered", bool(phone.wait("return !document.querySelector('.prompt')", 15)))

    # ---------- chat: a permission answered from the phone ----------
    chat_log = os.path.join(scratch, "phone_chat_agent.log")
    chat_agent = os.path.join(scratch, "phone_chat_agent.py")
    if os.path.exists(chat_log):
        os.remove(chat_log)
    open(chat_agent, "w", encoding="utf-8").write(
        "import json, sys\n"
        "log = sys.argv[1]\n"
        "def out(m):\n    sys.stdout.write(json.dumps(m) + '\\n'); sys.stdout.flush()\n"
        "out({'type': 'control_request', 'request_id': 'req-1', 'request': {'subtype': 'can_use_tool', 'tool_name': 'Bash', 'input': {'command': 'ls', 'description': 'List files'}}})\n"
        "for line in sys.stdin:\n"
        "    open(log, 'a', encoding='utf-8').write(line)\n"
        "    m = json.loads(line)\n"
        "    if m.get('type') == 'control_response':\n"
        "        out({'type': 'assistant', 'message': {'id': 'm1', 'role': 'assistant', 'content': [{'type': 'text', 'text': 'thanks'}]}})\n")
    chat = daemon(d, "session.create", {"kind": "claude", "cwd": proj, "chat_argv": [sys.executable, "-u", chat_agent, chat_log], "name": "phone-chat", "project_id": pid})
    phone.js("history.back()")
    ok = phone.wait("return [...document.querySelectorAll('.row')].some(r => r.textContent.includes('phone-chat'))", 20)
    run.check("a chat session appears in the inbox, marked as a chat", bool(ok) and bool(phone.js("return [...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-chat')).textContent.includes('chat')")))
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-chat')).click()")
    perm = phone.wait("return document.querySelector('.perm .q')?.textContent ?? null", 20)
    run.check("the chat's permission request shows as a card", bool(perm), str(perm))
    phone.shot(os.path.join(shots, "08-phone-chat.png"))
    run.check("opening a chat from the phone writes nothing to the agent", not os.path.exists(chat_log) or open(chat_log, encoding="utf-8").read().strip() == "")
    phone.js("[...document.querySelectorAll('.perm .btn')].find(b => b.textContent.trim() === 'Allow').click()")
    def agent_got():
        """What the chat agent was written, as JSON lines."""
        if not os.path.exists(chat_log):
            return []
        return [json.loads(line) for line in open(chat_log, encoding="utf-8").read().splitlines() if line.strip()]

    got = wait(lambda: agent_got(), 15)
    time.sleep(1.0)
    answers = [m for m in agent_got() if m.get("type") == "control_response" and m["response"].get("request_id") == "req-1"]
    run.check("tapping Allow gives the agent exactly one answer to its request", len(answers) == 1 and len(agent_got()) == 1, str(got))
    resolved = phone.wait("return document.querySelector('.perm .resolved')?.textContent ?? null", 15)
    run.check("the card settles on the answer that was given", bool(resolved) and "Allow" in resolved, str(resolved))
    run.check("the phone shows the agent's reply", bool(phone.wait("return document.querySelector('.chat-log')?.textContent.includes('thanks')", 15)))

    # ---------- chat: a message sent from the phone ----------
    turn_agent, turn_trace, turn_hold = ps.write_turn_agent(scratch)
    for f in (turn_trace, turn_hold):
        if os.path.exists(f):
            os.remove(f)

    def trace_lines():
        return open(turn_trace, encoding="utf-8").read().splitlines() if os.path.exists(turn_trace) else []

    def arrivals():
        return [t[len("ARRIVE "):] for t in trace_lines() if t.startswith("ARRIVE ")]

    def hold(on):
        if on:
            open(turn_hold, "w").close()
        elif os.path.exists(turn_hold):
            os.remove(turn_hold)

    def phone_type(text):
        phone.js("const ta = document.querySelector('.chat-composer textarea'); ta.value = arguments[0]; ta.dispatchEvent(new Event('input', {bubbles: true}));", text)

    last_send = [0.0]

    def phone_send():
        """Taps Send once it can be tapped (an earlier send may still be on its way, or the link reconnecting), and says
        what the page showed when the box did not empty (a notice, a disabled button): a tap that did nothing is not silent.
        Sends are paced a little over a second apart: the daemon lets one phone send three at once and then one a second
        (`rate_limited`), and on an idle host this script would otherwise send faster than that."""
        time.sleep(max(0.0, last_send[0] + 1.1 - time.time()))
        last_send[0] = time.time()
        ready = phone.wait("const b = document.querySelector('.chat-composer .send-row .btn.primary'); return !!b && !b.disabled", 20)
        text = phone.js("return document.querySelector('.chat-composer textarea')?.value ?? ''")
        phone.js("document.querySelector('.chat-composer .send-row .btn.primary').click()")
        if not phone.wait("return document.querySelector('.chat-composer textarea')?.value === ''", 15):
            state = phone.js("const b = document.querySelector('.chat-composer .send-row .btn.primary'); return {notice: document.querySelector('.chat-composer .notice')?.textContent ?? null, disabled: b?.disabled, conn: document.querySelector('.conn')?.dataset.state, box: document.querySelector('.chat-composer textarea')?.value}")
            print(f"NOTE the send of {text!r} did not empty the box (Send was {'ready' if ready else 'not ready'}): {state}", flush=True)

    sendchat = daemon(d, "session.create", {"kind": "claude", "cwd": proj, "chat_argv": [sys.executable, "-u", turn_agent, turn_trace, turn_hold], "name": "phone-send", "project_id": pid})
    d.js(f"const s = {S}; s.setSessions(arguments[0].id, arguments[0]); s.placePane(arguments[1], {{ id: 'send-pane', kind: 'chat', sessionId: arguments[0].id }}, 'tab');", sendchat, pid)
    run.check("the chat is open in the desktop window", bool(d.wait("return !!document.querySelector('.chat-composer textarea')", 20)))
    phone.js("document.querySelector('header .icon').click()")
    phone.wait("return [...document.querySelectorAll('.row')].some(r => r.textContent.includes('phone-send'))", 20)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-send')).click()")
    run.check("a chat opened on a device that may type has a message box and a Send button", bool(phone.wait("return !!document.querySelector('.chat-composer textarea') && !!document.querySelector('.chat-composer .send-row .btn.primary')", 20)))
    run.check("the box is for prose: sentences are capitalised and spelling is checked", phone.js("const t = document.querySelector('.chat-composer textarea'); return t.getAttribute('autocapitalize') === 'sentences' && t.spellcheck === true"))
    phone_type("first line")
    enter_prevented = phone.js("const t = document.querySelector('.chat-composer textarea'); const e = new KeyboardEvent('keydown', {key: 'Enter', bubbles: true, cancelable: true}); t.dispatchEvent(e); return e.defaultPrevented")
    time.sleep(0.5)
    run.check("Enter is a new line: it sends nothing", enter_prevented is False and arrivals() == [])
    phone_type("hello from the phone")
    phone_send()
    run.check("Send writes the message to the agent once", bool(wait(lambda: arrivals() == ["hello from the phone"], 20)), str(arrivals()))
    run.check("the box is empty again", phone.js("return document.querySelector('.chat-composer textarea').value") == "")
    own = phone.wait("return [...document.querySelectorAll('.msg.user')].some(m => m.textContent.includes('hello from the phone') && !m.querySelector('.from'))", 20)
    run.check("it shows in the phone's transcript, with no 'from' label on its own message", bool(own), str(phone.js("return [...document.querySelectorAll('.msg.user')].map(m => m.outerHTML).join(' | ')")))
    shown = d.wait("return [...document.querySelectorAll('.msg-user-wrap')].some(w => w.textContent.includes('hello from the phone') && /from /.test(w.querySelector('.msg-meta')?.textContent ?? ''))", 20)
    run.check("the desktop shows the message in its transcript, with who sent it", bool(shown))

    hold(True)
    phone_type("a long job")
    phone_send()
    wait(lambda: "a long job" in arrivals(), 20)
    phone_type("queued from the phone")
    phone_send()
    run.check("the phone shows what is waiting, with its state", bool(phone.wait("return [...document.querySelectorAll('.chat-composer .chip')].some(c => c.textContent.includes('queued from the phone') && /Queued/.test(c.textContent))", 20)))
    run.check("the desktop shows the phone's message as a chip, with where it came from", bool(d.wait("return [...document.querySelectorAll('.queue-chip')].some(c => c.textContent.includes('queued from the phone') && /from /.test(c.textContent))", 20)))
    run.check("the agent has not been given it", "queued from the phone" not in arrivals())
    run.check("while the agent works the phone has a Stop button of its own", bool(phone.wait("return !!document.querySelector('.chat-composer .stop') && !document.querySelector('.chat-composer .stop').disabled", 20)))
    phone.shot(os.path.join(shots, "10-phone-chat-send.png"))
    phone.js("document.querySelector('.chat-composer .stop').click()")
    run.check("Stop interrupts the agent's turn", bool(wait(lambda: "STOP" in trace_lines(), 15)), str(trace_lines()[-3:]))
    run.check("…and the queue waits: stopping means wait", bool(phone.wait("return /paused/i.test(document.querySelector('.chat-composer .confirm')?.textContent ?? '')", 15)) and "queued from the phone" not in arrivals())
    run.check("the Stop button is gone once the agent is idle", bool(phone.wait("return !document.querySelector('.chat-composer .stop')", 15)))
    run.check("the desktop says the queue is stopped too", bool(d.wait("return !!document.querySelector('.queue-note')", 15)))
    d.js("document.querySelector('.queue-chip .icon-btn.mini').click()")
    run.check("the desktop cancels the phone's waiting message: the phone says so and keeps the text", bool(phone.wait("return [...document.querySelectorAll('.chat-composer .chip')].some(c => c.textContent.includes('queued from the phone') && c.textContent.includes('Cancelled on desktop'))", 15)))
    phone.js("[...document.querySelectorAll('.chat-composer .chip')].find(c => c.textContent.includes('Cancelled on desktop')).querySelectorAll('.link-btn')[1].click()")
    run.check("…and Discard lets it go", bool(phone.wait("return ![...document.querySelectorAll('.chat-composer .chip')].some(c => c.textContent.includes('Cancelled on desktop'))", 10)))
    hold(False)
    phone_type("to cancel on the phone")
    hold(True)
    phone_send()
    wait(lambda: "to cancel on the phone" in arrivals(), 15)
    phone_type("to be cancelled here")
    phone_send()
    phone.wait("return [...document.querySelectorAll('.chat-composer .chip')].some(c => c.textContent.includes('to be cancelled here'))", 15)
    phone.js("[...document.querySelectorAll('.chat-composer .chip')].find(c => c.textContent.includes('to be cancelled here')).querySelector('.icon').click()")
    run.check("the phone cancels its own waiting message", bool(phone.wait("return ![...document.querySelectorAll('.chat-composer .chip')].some(c => c.textContent.includes('to be cancelled here'))", 15)))
    run.check("…and the desktop's chip goes too", bool(d.wait("return ![...document.querySelectorAll('.queue-chip')].some(c => c.textContent.includes('to be cancelled here'))", 15)))
    hold(False)
    time.sleep(1.5)
    run.check("the cancelled messages never reached the agent", "queued from the phone" not in arrivals() and "to be cancelled here" not in arrivals(), str(arrivals()))

    # ---------- a desktop message only the window can send holds the queue: the phone says why, Skip lets the rest go ----------
    # The chat is not open on the desktop, so the picture (queued by the app's own connection) waits at the head in strict order.
    held_chat = daemon(d, "session.create", {"kind": "claude", "cwd": proj, "chat_argv": [sys.executable, "-u", turn_agent, turn_trace, turn_hold], "name": "phone-held", "project_id": pid})
    phone.js("document.querySelector('header .icon').click()")
    phone.wait("return [...document.querySelectorAll('.row')].some(r => r.textContent.includes('phone-held'))", 20)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-held')).click()")
    phone.wait("return !!document.querySelector('.chat-composer textarea')", 20)
    hold(True)
    phone_type("busy before the picture")
    phone_send()
    wait(lambda: "busy before the picture" in arrivals(), 20)
    daemon(d, "chat.send", {"handle": held_chat["handle"], "cid": "e2e-desktop-picture", "text": "desktop picture", "rich": True, "images": ["a.png"]})
    phone.wait("return !document.querySelector('.chat-composer .send-row .btn.primary').disabled", 10)
    phone_type("behind the picture")
    phone_send()
    phone.wait("return [...document.querySelectorAll('.chat-composer .chip')].some(c => c.textContent.includes('behind the picture'))", 15)
    hold(False)
    held_note = phone.wait("return [...document.querySelectorAll('.chat-composer .chip.held')].find(c => c.textContent.includes('behind the picture'))?.querySelector('.chip-note')?.textContent ?? null", 30)
    run.check("a phone message behind a desktop message only the computer can send says why it waits, on its own chip", bool(held_note) and "only the computer can send" in held_note and "Skip" in held_note, str(held_note))
    time.sleep(1.0)
    run.check("…and it does wait: the order is kept", "behind the picture" not in arrivals() and "desktop picture" not in arrivals(), str(arrivals()))
    run.check("the desktop says so outside the chat pane, which is not open there", bool(d.wait("return !!document.querySelector('.chat-wait-notice[data-session=\"%s\"]')" % held_chat["id"], 15)))
    phone.js("[...document.querySelectorAll('.chat-composer .chip.held')].find(c => c.textContent.includes('behind the picture')).querySelector('[data-skip]').click()")
    run.check("Skip on the phone's own chip lets the rest go", bool(wait(lambda: "behind the picture" in arrivals(), 20)), str(arrivals()))
    run.check("…the skipped message is not sent", "desktop picture" not in arrivals())
    run.check("…and the desktop's notice goes", bool(d.wait("return !document.querySelector('.chat-wait-notice')", 15)))
    d.js(f"const s = {S}; if (!s.sessions[arguments[0].id]) s.setSessions(arguments[0].id, arguments[0]); s.placePane(arguments[1], {{ id: 'held-pane', kind: 'chat', sessionId: arguments[0].id }}, 'tab');", held_chat, pid)
    skipped = d.wait("return [...document.querySelectorAll('.chat-scroll')].some(s => s.textContent.includes('desktop picture') && /cancelled from/.test(s.textContent))", 20)
    run.check("the chat opened on the desktop afterwards notes the skipped message as not sent", bool(skipped))

    # ---------- the desktop cancels a phone's message while the phone is offline: back online, it says so and keeps the text ----------
    hold(True)
    phone_type("busy while the phone is away")
    phone_send()
    wait(lambda: "busy while the phone is away" in arrivals(), 20)
    phone_type("cancelled while away")
    phone_send()
    phone.wait("return [...document.querySelectorAll('.chat-composer .chip')].some(c => c.textContent.includes('cancelled while away') && /Queued/.test(c.textContent))", 15)
    phone.js("window.dispatchEvent(new Event('offline'))")
    phone.wait("return document.querySelector('.conn')?.dataset.state !== 'live'", 10)
    d.wait("return [...document.querySelectorAll('.queue-chip')].some(c => c.textContent.includes('cancelled while away'))", 15)
    d.js("[...document.querySelectorAll('.queue-chip')].find(c => c.textContent.includes('cancelled while away')).querySelector('.icon-btn.mini').click()")
    d.wait("return ![...document.querySelectorAll('.queue-chip')].some(c => c.textContent.includes('cancelled while away'))", 15)
    phone.js("window.dispatchEvent(new Event('online'))")
    phone.wait(LIVE, 30)
    away_js = "return [...document.querySelectorAll('.chat-composer .chip')].find(c => c.textContent.includes('cancelled while away'))?.textContent ?? null"
    away = phone.wait("return [...document.querySelectorAll('.chat-composer .chip')].find(c => c.textContent.includes('cancelled while away') && c.textContent.includes('Cancelled on desktop'))?.textContent ?? null", 20)
    run.check("a message the desktop cancelled while the phone was offline shows as cancelled on desktop once it is back, with its text", bool(away), str(phone.js(away_js)))
    time.sleep(6)  # longer than a written message stays as "Sent"
    still = phone.js(away_js)
    run.check("…and it stays (it is not taken for sent and let go)", bool(still) and "Cancelled on desktop" in still, str(still))
    run.check("…and it never reached the agent", "cancelled while away" not in arrivals())
    phone.js("[...[...document.querySelectorAll('.chat-composer .chip')].find(c => c.textContent.includes('cancelled while away')).querySelectorAll('.link-btn')].find(b => b.textContent === 'Discard').click()")
    hold(False)

    # ---------- deep link ----------
    sid = next(x["id"] for x in daemon(d, "session.list") if x["handle"] == handle)
    phone.go("about:blank")
    phone.go(base + "#s=" + sid)
    run.check("a #s= link opens that session", bool(phone.wait("return document.querySelector('.session')?.dataset.id === " + json.dumps(sid), 20)))
    run.check("the link does not stay in the address bar", "#s=" not in phone.js("return location.href"))

    # ---------- the server goes away and comes back ----------
    shown_before = phone.js("return document.querySelector('.screen')?.textContent.length ?? 0")
    daemon(d, "mobile.disable")
    st_down = phone.wait("const s = document.querySelector('.conn')?.dataset.state; return s && s !== 'live' ? s : null", 10)
    run.check("when the connection drops the page says it is reconnecting or the computer is offline", st_down in ("reconnecting", "computer_offline", "connecting"), str(st_down))
    run.check("what was on screen stays readable while disconnected", phone.js("return document.querySelector('.screen')?.textContent.length ?? 0") >= shown_before > 0)
    dead = phone.js("return {send: document.querySelector('.send .btn.primary')?.disabled, key: document.querySelector('.keys .key')?.disabled}")
    run.check("typing and keys are disabled while disconnected", dead == {"send": True, "key": True}, str(dead))
    # A chat that was not open before cannot be read without the computer: it says so, it does not load for ever.
    phone.js("document.querySelector('header .icon').click()")
    phone.wait("return !!document.querySelector('.row')", 5)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-chat')).click()")
    offline_chat = phone.wait("return document.querySelector('.chat-log .center')?.textContent ?? null", 10)
    run.check("a chat opened while disconnected says it is not available offline", bool(offline_chat) and "not available offline" in offline_chat, str(offline_chat))
    phone.js("document.querySelector('header .icon').click()")
    phone.wait("return !!document.querySelector('.row')", 5)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-shell')).click()")
    phone.wait("return !!document.querySelector('.screen')", 5)
    phone.shot(os.path.join(shots, "09-phone-disconnected.png"))
    st = js_await("window.__TAURI_INTERNALS__.invoke('daemon_call', {method: 'mobile.enable', params: {ip: '127.0.0.1', port: %d}})" % PORT)
    run.check("the page reconnects by itself and is live again, without a reload", bool(phone.wait(LIVE, 60)) and phone.js("return document.querySelector('.session') !== null"), str(st)[:100])
    daemon(d, "session.input", {"handle": handle, "data": "echo after-reconnect\r"})
    run.check("the screen keeps following the session after the reconnect", bool(phone.wait("return document.querySelector('.screen').textContent.includes('after-reconnect')", 20)))

    # ---------- read-only device, bad requests ----------
    d.js("document.querySelector('[data-setting=\"mobile-pair\"] .btn.primary')?.click()")
    link2 = d.wait("return document.querySelector('.mobile-qr-code')?.dataset.link", 10)
    phone.go(link2)
    phone.wait("return !!document.querySelector('.card .btn.primary')")
    phone.js("document.querySelector('.card input').value = 'Reader'; document.querySelector('.card .btn.primary').click()")
    d.wait("return [...document.querySelectorAll('.mobile-approve h2')].some(h => h.textContent.includes('Reader'))", 15)
    type_code(phone.wait("return document.querySelector('[data-pair-code]')?.textContent", 15))
    d.js("document.querySelector('.mobile-approve [data-allow=read]').click()")
    phone.wait(LIVE, 20)
    phone.wait("return [...document.querySelectorAll('.row')].some(r => r.textContent.includes('phone-shell'))", 15)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-shell')).click()")
    ro = phone.wait("return document.querySelector('.readonly')?.textContent ?? null")
    run.check("a read-only device sees the screen but no input", bool(ro) and "only read" in ro and not phone.js("return !!document.querySelector('.send textarea')"), str(ro))
    phone.js("document.querySelector('header .icon').click()")
    phone.wait("return [...document.querySelectorAll('.row')].some(r => r.textContent.includes('phone-send'))", 15)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-send')).click()")
    ro_chat = phone.wait("return document.querySelector('.session.chat .readonly')?.textContent ?? null", 15)
    run.check("a read-only device sees a chat but has no message box", bool(ro_chat) and not phone.js("return !!document.querySelector('.chat-composer')"), str(ro_chat))
    phone.js("document.querySelector('header .icon').click()")
    phone.wait("return [...document.querySelectorAll('.row')].some(r => r.textContent.includes('phone-shell'))", 15)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-shell')).click()")
    status, _, _ = http_get(base + "link", {"Upgrade": "websocket", "Connection": "Upgrade", "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==", "Sec-WebSocket-Version": "13", "Origin": "http://evil.example"})
    run.check("a socket from another site is refused", status == 403, str(status))
    # The bad pairing codes are tried last (after the restart scenario, which pairs a phone): the limiter would refuse that pairing.

    # ---------- unpair, off ----------
    reader = next(x for x in daemon(d, "mobile.status")["devices"] if x["name"] == "Reader")
    daemon(d, "mobile.revoke", {"id": reader["id"]})
    run.check("unpairing disconnects the phone at once", bool(phone.wait("return document.querySelector('.card')?.textContent.includes('no longer paired')", 15)))
    daemon(d, "mobile.disable")
    time.sleep(0.5)
    try:
        http_get(base)
        refused = False
    except OSError:
        refused = True
    run.check("turning it off stops the server", refused)
    for dev in daemon(d, "mobile.status")["devices"]:
        daemon(d, "mobile.revoke", {"id": dev["id"]})
    daemon(d, "session.close", {"handle": handle})

    # ---------- after the computer restarted: what waited is shown as lost, with its text ----------
    daemon(d, "mobile.enable", {"ip": "127.0.0.1", "port": PORT})
    d.wait("const b = document.querySelector('[data-setting=\"mobile-pair\"] .btn.primary'); return !!b && !b.disabled", 15)
    d.js("document.querySelector('[data-setting=\"mobile-pair\"] .btn.primary').click()")
    phone.go(d.wait("return document.querySelector('.mobile-qr-code')?.dataset.link", 10))
    phone.wait("return !!document.querySelector('.card .btn.primary')")
    phone.js("document.querySelector('.card input').value = 'Restart phone'; document.querySelector('.card .btn.primary').click()")
    d.wait("return [...document.querySelectorAll('.mobile-approve h2')].some(h => h.textContent.includes('Restart phone'))", 15)
    type_code(phone.wait("return document.querySelector('[data-pair-code]')?.textContent", 15))
    d.js("document.querySelector('.mobile-approve [data-allow=write]').click()")
    phone.wait(LIVE, 20)
    lost_chat = daemon(d, "session.create", {"kind": "claude", "cwd": proj, "chat_argv": [sys.executable, "-u", turn_agent, turn_trace, turn_hold], "name": "phone-lost", "project_id": pid})
    phone.go("about:blank")
    phone.go(base)
    phone.wait(LIVE, 30)
    phone.wait("return [...document.querySelectorAll('.row')].some(r => r.textContent.includes('phone-lost'))", 20)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-lost')).click()")
    phone.wait("return !!document.querySelector('.chat-composer textarea')", 20)
    hold(True)
    phone_type("running when it restarts")
    phone_send()
    wait(lambda: "running when it restarts" in arrivals(), 20)
    phone_type("queued when it restarts")
    phone_send()
    phone.wait("return [...document.querySelectorAll('.chat-composer .chip')].some(c => c.textContent.includes('queued when it restarts'))", 20)
    restart_daemon(d, kelpied, kill=True, env=ISOLATED)
    check_daemon(d, ISOLATED)
    # Every chat comes back not running after a restart: no box, but what waited stays on screen with its text.
    lost_js = "return [...document.querySelectorAll('.kept-chips .chip.warn')].find(c => c.textContent.includes('queued when it restarts') && /restarted/.test(c.textContent))?.textContent ?? null"
    lost = phone.wait(lost_js, 90)
    run.check("after the computer restarted the phone shows what waited as lost, with its text kept, in a chat that is not running", bool(lost), str(lost))
    hint = phone.js("return document.querySelector('.session.chat .readonly')?.textContent ?? ''")
    run.check("…under the hint to open the chat on the desktop", "Open it on the desktop" in hint, hint)
    run.check("…with its text to copy", bool(phone.js("return [...document.querySelectorAll('.kept-chips .chip.warn .link-btn')].some(b => b.textContent === 'Copy')")))
    phone.go("about:blank")
    phone.go(base)
    phone.wait(LIVE, 60)
    phone.wait("return [...document.querySelectorAll('.row')].some(r => r.textContent.includes('phone-lost'))", 20)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-lost')).click()")
    kept = phone.wait(lost_js, 30)
    run.check("a reload of the page keeps it, text and all", bool(kept), str(kept))
    # The chat goes from the computer's list (closed here; resuming it on the desktop does the same: a new session takes its
    # place). The message is not lost with it: the inbox lists it under "Not sent", with the chat's name.
    lost_handle = next(x["handle"] for x in daemon(d, "session.list") if x["id"] == lost_chat["id"])
    daemon(d, "session.close", {"handle": lost_handle})
    not_sent_js = "return [...document.querySelectorAll('.not-sent .chip')].find(c => c.textContent.includes('queued when it restarts'))?.closest('.kept-chips')?.textContent ?? null"
    listed = phone.wait(not_sent_js, 30)
    run.check("once the chat is gone, the inbox lists the message under Not sent, with the chat's name", bool(listed) and "phone-lost" in listed, str(listed))
    phone.go("about:blank")
    phone.go(base)
    phone.wait(LIVE, 60)
    run.check("…also after a reload", bool(phone.wait(not_sent_js, 30)))
    phone.js("[...[...document.querySelectorAll('.not-sent .chip')].find(c => c.textContent.includes('queued when it restarts')).querySelectorAll('.link-btn')].find(b => b.textContent === 'Discard').click()")
    run.check("Discard lets it go", bool(phone.wait("return !document.querySelector('.not-sent')", 15)))
    run.check("…for good: nothing of it is kept on the phone", not phone.js("return Object.keys(localStorage).some(k => k.startsWith('kelpie-draft:chat-pending:'))"))
    hold(False)

    # ---------- a secure origin: the install step, no camera, the pasted link, typing ----------
    cert_file, key_file = certificate_for_loopback(scratch)
    daemon(d, "mobile.enable", {"ip": "127.0.0.1", "port": PORT, "cert": cert_file, "key": key_file})
    secure = f"https://localhost:{PORT}/"
    # The daemon names the address it listens on. A passkey needs a name, so this run's page is at `localhost`, on the same listener.
    on_name = lambda url: url.replace(f"https://127.0.0.1:{PORT}/", secure)  # noqa: E731
    phone.go("about:blank")
    phone.go(secure)
    run.check("in a browser on a secure origin, installing comes first", phone.wait("return document.querySelector('[data-install]')?.dataset.install", 20) == "android")
    run.check("…on a page browsers trust as secure", phone.js("return window.isSecureContext") is True)
    run.check("…with no way yet to send a code", not phone.js("return !!document.querySelector('[data-scan]')"))
    phone.shot(os.path.join(shots, "21-phone-install.png"))
    phone.js("document.querySelector('[data-stay-in-browser]').click()")
    phone.wait("return !!document.querySelector('[data-scan]')", 10)
    phone.js("document.querySelector('[data-scan-start]').click()")
    run.check("no camera falls back to pasting the link, and says so", bool(phone.wait("return document.querySelector('[data-scan-problem]')?.textContent", 15)))
    phone_paste = "const i = document.querySelector('[data-paste]'); i.value = arguments[0]; i.dispatchEvent(new Event('input', {bubbles: true})); document.querySelector('[data-paste-go]').click();"
    phone.js("document.querySelector('[data-scan-problem]').dataset.was = '1'")
    phone.js(phone_paste, "not a pairing link")
    run.check("something else pasted is refused", "not a Kelpie pairing code" in (phone.wait("return document.querySelector('[data-scan-problem]')?.textContent.includes('not a Kelpie') && document.querySelector('[data-scan-problem]').textContent", 10) or ""))
    # A pairing that fails in the installed app must not leave it with a spent code and no way to bring a new one.
    made = daemon(d, "mobile.pair_start")
    phone.js(phone_paste, on_name(made["url"]))
    phone.wait("return !!document.querySelector('.card input.field') && !document.querySelector('[data-scan]')", 10)
    phone.js("document.querySelector('.card input').value = 'Turned down'; document.querySelector('.card .btn.primary').click()")
    d.wait("return [...document.querySelectorAll('.mobile-approve h2')].some(h => h.textContent.includes('Turned down'))", 15)
    d.js("document.querySelector('.mobile-approve .footer .btn').click()")
    run.check("a pairing turned down leads back to the scanner, saying so", bool(phone.wait("return !!document.querySelector('[data-scan]') && document.querySelector('[data-scan]').textContent.includes('Declined')", 15)))
    made = daemon(d, "mobile.pair_start")
    run.check("the link is for the secure origin", made["url"].startswith(f"https://127.0.0.1:{PORT}/#p=") and made["tls"] is True)
    phone.js(phone_paste, on_name(made["url"]))
    phone.wait("return !!document.querySelector('.card input.field') && !document.querySelector('[data-scan]')", 10)
    phone.js("document.querySelector('.card input').value = 'Secure phone'; document.querySelector('.card .btn.primary').click()")
    d.wait("return [...document.querySelectorAll('.mobile-approve h2')].some(h => h.textContent.includes('Secure phone'))", 15)
    type_code(phone.wait("return document.querySelector('[data-pair-code]')?.textContent", 15))
    d.js("[...document.querySelectorAll('.mobile-approve')].find(a => a.textContent.includes('Secure phone')).querySelector('[data-allow=write]').click()")
    run.check("the pasted link pairs, and the phone connects", bool(phone.wait(LIVE, 30)))
    listed = next((x for x in daemon(d, "mobile.status")["devices"] if x["name"] == "Secure phone"), {})
    run.check("…as a secure origin", listed.get("secure_origin") is True and listed.get("can_write") is True, str(listed))
    run.check("the phone paired over plain HTTP is not one", all(x["secure_origin"] is False for x in daemon(d, "mobile.status")["devices"] if x["name"] != "Secure phone"))
    run.check("…and nothing says this page may only read", not phone.js("return !!document.querySelector('[data-banner=plain-http]')"))
    typed_here = daemon(d, "session.create", {"kind": "shell", "cwd": proj, "name": "secure-shell", "project_id": pid})

    # ---------- the app lock: a real ceremony in this browser, a virtual authenticator in place of a person's face, and the computer checking every signature ----------
    run.check("a secure origin stops at the lock, with a passkey to set up", phone.wait("return document.querySelector('[data-lock-screen]')?.dataset.lockScreen", 15) == "enrol", str(phone.js("return document.querySelector('main')?.textContent")))
    run.check("…and nothing of any session on the page", not phone.js("return !!document.querySelector('.row') || document.body.textContent.includes('secure-shell')"))
    phone.shot(os.path.join(shots, "25-phone-lock-enrol.png"))
    webauthn = Cdp(phone.debugger, "page", f"https://localhost:{PORT}")
    webauthn.call("WebAuthn.enable", {"enableUI": False})
    authenticator = webauthn.call("WebAuthn.addVirtualAuthenticator", {"options": {"protocol": "ctap2", "transport": "internal", "hasResidentKey": True, "hasUserVerification": True, "isUserVerified": True, "automaticPresenceSimulation": True}})["authenticatorId"]
    passkeys = lambda: webauthn.call("WebAuthn.getCredentials", {"authenticatorId": authenticator})["credentials"]  # noqa: E731
    lock_device = lambda: next((x for x in daemon(d, "mobile.status")["devices"] if x["name"] == "Secure phone"), {})  # noqa: E731
    lock_error = "return document.querySelector('[data-lock-error]')?.textContent ?? null"
    open_js = "return !document.querySelector('[data-lock-screen]') && [...document.querySelectorAll('.row')].some(r => r.textContent.includes('secure-shell'))"
    phone.js("document.querySelector('[data-lock-go]').click()")
    opened = phone.wait(open_js, 20)
    run.check("a real WebAuthn ceremony sets it up, the computer checks it, and the sessions come", bool(opened), str(phone.js(lock_error)))
    run.check("…one passkey in the authenticator, on record at the computer, and it is unlocked", len(passkeys()) == 1 and lock_device().get("passkey") is True and lock_device().get("unlocked") is True, str(lock_device()))
    run.check("…and the desktop heard of it", bool(d.wait("return [...document.querySelectorAll('.toast')].some(t => t.textContent.includes('set up its app lock'))", 10)))

    # A program with a phone's Noise key and none of its passkey, saying it is the same page: at the daemon, not the page.
    import ssl as ssl_module

    insecure_context = ssl_module.create_default_context()
    insecure_context.check_hostname = False
    insecure_context.verify_mode = ssl_module.CERT_NONE

    async def paired_by_a_program(name):
        link_for = on_name(daemon(d, "mobile.pair_start")["url"])
        attempt = ps.PairAttempt(secure, link_for, name, "write", origin=secure.rstrip("/"), ssl=insecure_context)
        await attempt.start()
        loop = asyncio.get_running_loop()

        def approve():
            d.wait(f"return [...document.querySelectorAll('.mobile-approve h2')].some(h => h.textContent.includes('{name}'))", 15)
            type_code(attempt.code())
            d.js(f"[...document.querySelectorAll('.mobile-approve')].find(a => a.textContent.includes('{name}')).querySelector('[data-allow=write]').click()")

        await loop.run_in_executor(None, approve)
        await attempt.outcome(20)
        device = next(x for x in daemon(d, "mobile.status")["devices"] if x["name"] == name)
        return ps.Device(attempt.priv, attempt.pub, attempt.dpk, device["id"])

    async def copied_key_is_refused():
        keys = await paired_by_a_program("Copied key")
        device = next(x for x in daemon(d, "mobile.status")["devices"] if x["name"] == "Copied key")
        as_page = dict(origin=secure.rstrip("/"), ssl=insecure_context)
        thief = ps.Phone(secure, keys, **as_page)
        hello = await thief.open()
        run.check("a copied key with no passkey gets a locked hello from the daemon, with no session in it", hello.get("locked") is True and hello["sessions"] == [] and hello["projects"] == {}, str(hello.get("lock")))
        replies = [await thief.ask(type=name, id=typed_here["id"], text="x") for name in ("list", "sub", "input", "chat_sub", "push_info", "push_test")]
        run.check("…and reading, typing and alerts are each refused by the daemon", all((r or {}).get("code") == "locked" for r in replies), str(replies))
        # Approving it opened the way for its passkey. The owner sets one up; then a program with the same key, while that window is open.
        owner = ps.SoftPasskey(secure.rstrip("/"))
        opened = await thief.ask(type="lock_enrol", **owner.create((await thief.ask(type="lock_challenge"))["lock"]["challenge"]))
        run.check("just after approval a passkey can be set up, and its window comes with a secret", (opened or {}).get("result") == "ok" and bool(opened.get("resume")), str((opened or {}).get("code")))
        await thief.close()
        copy = ps.Phone(secure, keys, **as_page)
        hello = await copy.open()
        window_open = next(x for x in daemon(d, "mobile.status")["devices"] if x["name"] == "Copied key").get("unlocked")
        run.check("the same key without the secret is locked while the owner's window is open, with no session", window_open is True and hello.get("locked") is True and hello["lock"]["state"] == "unlock" and hello["sessions"] == [], str(hello.get("lock")))
        run.check("…and is refused by the daemon", (await copy.ask(type="list"))["code"] == "locked")
        await copy.close()
        flap = ps.Phone(secure, keys, resume=opened["resume"], **as_page)
        hello = await flap.open()
        run.check("the page holding the secret comes back from a flap with nothing asked", hello.get("locked") is False and hello["lock"]["state"] == "open", str(hello.get("lock")))
        await flap.close()
        daemon(d, "mobile.revoke", {"id": device["id"]})

    asyncio.run(copied_key_is_refused())

    # Leaving: `document.hidden` is set by hand, since a headless browser has no app switcher. All that follows it is the page's and the computer's.
    set_hidden = "Object.defineProperty(document, 'hidden', {value: arguments[0], configurable: true}); Object.defineProperty(document, 'visibilityState', {value: arguments[0] ? 'hidden' : 'visible', configurable: true}); document.dispatchEvent(new Event('visibilitychange'));"
    phone.js(set_hidden, True)
    run.check("hiding the page puts an opaque cover up at once", bool(phone.js("return !!document.querySelector('[data-lock-cover]')")))
    run.check("…and the computer locks it", bool(wait(lambda: lock_device().get("unlocked") is False, 10)), str(lock_device()))
    phone.js(set_hidden, False)
    run.check("coming back shows the lock, and no session", phone.wait("return document.querySelector('[data-lock-screen]')?.dataset.lockScreen", 15) == "unlock" and not phone.js("return !!document.querySelector('.row') || document.title.startsWith('(')"))
    phone.shot(os.path.join(shots, "26-phone-lock.png"))
    signs = passkeys()[0]["signCount"]
    phone.js("document.querySelector('[data-lock-go]').click()")
    run.check("the passkey opens it again, with a signature the computer counted", bool(phone.wait(open_js, 20)) and passkeys()[0]["signCount"] > signs, str(phone.js(lock_error)))

    # A network flap in the foreground: the connection goes and comes, and nothing asks.
    signs = passkeys()[0]["signCount"]
    phone.js("window.dispatchEvent(new Event('offline'))")
    phone.wait("return document.querySelector('.conn')?.dataset.state !== 'live'", 10)
    phone.js("window.dispatchEvent(new Event('online'))")
    run.check("a network flap in the foreground comes back open, asking nothing", bool(phone.wait(LIVE, 30)) and bool(phone.wait(open_js, 15)) and passkeys()[0]["signCount"] == signs)

    # A reload has lost the window's secret, which was only in the page's memory, so it asks again, though the window is still open at the computer.
    phone.go(secure)
    run.check("a reload asks again, while the window is still open at the computer", bool(phone.wait("return document.querySelector('[data-lock-screen]')?.dataset.lockScreen === 'unlock'", 20)) and lock_device().get("unlocked") is True and not phone.js("return !!document.querySelector('.row')"), str(lock_device()))
    phone.js("document.querySelector('[data-lock-go]').click()")
    run.check("…and the passkey opens it", bool(phone.wait(open_js, 20)) and passkeys()[0]["signCount"] > signs, str(phone.js(lock_error)))

    # The idle time, at its shortest. Setting it shorter needs nobody; nothing touches the phone meanwhile.
    d.js(f"{S}.openSettings('mobile')")
    d.wait("return !!document.querySelector('[data-lock-idle]')", 15)
    run.shot("27-mobile-lock-settings")
    set_idle = "const s = document.querySelector('[data-lock-idle]'); s.value = arguments[0]; s.dispatchEvent(new Event('change', {bubbles: true}));"
    d.js(set_idle, "60")
    run.check("the idle time can be shortened from Settings", bool(wait(lambda: daemon(d, "mobile.status")["lock"]["idle_s"] == 60, 10)), str(daemon(d, "mobile.status")["lock"]))
    run.check("the idle time locks a phone nobody touches", phone.wait("return document.querySelector('[data-lock-screen]')?.dataset.lockScreen", 80) == "unlock" and lock_device().get("unlocked") is False, str(lock_device()))
    phone.js("document.querySelector('[data-lock-go]').click()")
    run.check("…and it opens again", bool(phone.wait(open_js, 20)), str(phone.js(lock_error)))
    d.js(set_idle, "300")
    run.check("lengthening it passes the person's verification (the test's stand-in here)", bool(wait(lambda: daemon(d, "mobile.status")["lock"]["idle_s"] == 300, 15)))

    # A dangerous approval: the label, the full command with what hides written out, and with the switch on a fresh assertion.
    danger_log = os.path.join(scratch, "phone_danger_agent.log")
    danger_agent = os.path.join(scratch, "phone_danger_agent.py")
    if os.path.exists(danger_log):
        os.remove(danger_log)
    open(danger_agent, "w", encoding="utf-8").write(
        "import json, sys\n"
        "log = sys.argv[1]\n"
        "def out(m):\n    sys.stdout.write(json.dumps(m) + '\\n'); sys.stdout.flush()\n"
        "out({'type': 'control_request', 'request_id': 'danger-1', 'request': {'subtype': 'can_use_tool', 'tool_name': 'Bash', 'input': {'command': 'rm -rf build \\u202etxt.evil', 'description': 'A harmless cleanup'}}})\n"
        "for line in sys.stdin:\n"
        "    open(log, 'a', encoding='utf-8').write(line)\n"
        "    if json.loads(line).get('type') == 'control_response':\n"
        "        out({'type': 'control_request', 'request_id': 'plain-1', 'request': {'subtype': 'can_use_tool', 'tool_name': 'Bash', 'input': {'command': 'ls'}}})\n")
    d.js("document.querySelector('[data-setting=\"mobile-lock-recheck\"] .switch').click()")
    run.check("the re-check can be turned on from Settings", bool(wait(lambda: daemon(d, "mobile.status")["lock"]["recheck"] is True, 10)))
    danger = daemon(d, "session.create", {"kind": "claude", "cwd": proj, "chat_argv": [sys.executable, "-u", danger_agent, danger_log], "name": "phone-danger", "project_id": pid})
    phone.wait("return [...document.querySelectorAll('.row')].some(r => r.textContent.includes('phone-danger'))", 20)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-danger')).click()")
    risk = phone.wait("return document.querySelector('.perm [data-risk]')?.dataset.risk === 'dangerous' && [...document.querySelectorAll('.perm [data-risk-tag]')].map(t => t.dataset.riskTag)", 20)
    run.check("the card leads with the daemon's label, whatever the agent's description says", bool(risk) and "delete" in risk, str(phone.js("return document.querySelector('.perm')?.textContent")))
    shown = phone.js("return document.querySelector('.perm [data-command]')?.textContent ?? ''")
    run.check("…and the full command, what hides written out", shown == "rm -rf build \\u{202E}txt.evil", repr(shown))
    run.check("…before the agent's own words", phone.js("const c = document.querySelector('.perm'); return c.firstElementChild.matches('[data-command-block]')") is True)
    phone.shot(os.path.join(shots, "28-phone-risk-card.png"))
    signs = passkeys()[0]["signCount"]
    phone.js("[...document.querySelectorAll('.perm .btn')].find(b => b.textContent.trim() === 'Allow').click()")
    run.check("approving it asks again, because the computer refused it without", bool(phone.wait("return !!document.querySelector('[data-recheck]')", 15)))
    run.check("…and the agent has been given nothing", not os.path.exists(danger_log))
    phone.js("document.querySelector('[data-recheck-go]').click()")
    danger_answers = lambda: [json.loads(x) for x in open(danger_log, encoding="utf-8").read().splitlines() if x.strip()] if os.path.exists(danger_log) else []  # noqa: E731
    wait(lambda: danger_answers(), 15)
    run.check("a fresh assertion over that command lets the one answer through", len(danger_answers()) == 1 and danger_answers()[0]["response"]["response"]["behavior"] == "allow" and passkeys()[0]["signCount"] > signs, str(danger_answers()))
    plain_card = phone.wait("return [...document.querySelectorAll('.perm')].find(p => p.dataset.perm === 'plain-1' && p.querySelector('[data-risk=low]'))?.textContent ?? null", 20)
    run.check("what is not dangerous is labelled too, and asks nothing more", bool(plain_card), str(plain_card))
    d.js("document.querySelector('[data-setting=\"mobile-lock-recheck\"] .switch').click()")
    run.check("turning it off passes the person's verification", bool(wait(lambda: daemon(d, "mobile.status")["lock"]["recheck"] is False, 15)))
    daemon(d, "session.close", {"handle": danger["handle"]})
    phone.wait("return !document.querySelector('.perm')", 15)

    # A passkey gone: the desktop opens the way, and only for this phone.
    run.check("Settings offers to reset this phone's lock", bool(d.wait("return !!document.querySelector('[data-reset-lock]')", 15)))
    d.js("document.querySelector('[data-reset-lock]').click()")
    run.check("the reset locks the phone and lets it set up a new passkey", phone.wait("return document.querySelector('[data-lock-screen]')?.dataset.lockScreen", 15) == "enrol", str(lock_device()))
    phone.js("document.querySelector('[data-lock-go]').click()")
    run.check("…which opens it", bool(phone.wait(open_js, 20)) and lock_device().get("lock_reset") is False, str(phone.js(lock_error)))
    d.js(f"{S}.closeSettings?.()")
    phone.wait("return [...document.querySelectorAll('.row')].some(r => r.textContent.includes('secure-shell'))", 20)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('secure-shell')).click()")
    phone.wait("return !!document.querySelector('.send textarea')", 15)
    time.sleep(2.5)
    phone.js("const t = document.querySelector('.send textarea'); t.value = 'echo secure-$((6*7))';")
    phone.js("document.querySelector('.send .btn.primary').click()")
    run.check("a secure origin may type", bool(wait(lambda: "secure-42" in daemon(d, "session.read", {"handle": typed_here["handle"], "lines": 40})["text"], 20)))
    daemon(d, "session.close", {"handle": typed_here["handle"]})
    # A phone paired, with no passkey yet, when the computer restarts: the way approval opened is gone with it.
    no_window = asyncio.run(paired_by_a_program("No window"))
    # A restart: the window is memory, so every phone is locked, and the phone shows the lock and not an error.
    restart_daemon(d, kelpied, kill=True, env=ISOLATED)
    check_daemon(d, ISOLATED)
    run.check("after the computer restarted the phone is locked, and shows the lock rather than an error", phone.wait("return document.querySelector('[data-lock-screen]')?.dataset.lockScreen", 90) == "unlock", str(phone.js("return document.querySelector('.conn')?.dataset.state")))
    webauthn.close()

    async def passkey_takes_the_computers_window():
        as_page = dict(origin=secure.rstrip("/"), ssl=insecure_context)
        waiting = ps.Phone(secure, no_window, **as_page)
        hello = await waiting.open()
        run.check("with no window from the computer a phone is told to press the button there, and has no challenge to answer", hello.get("locked") is True and hello["lock"]["state"] == "desktop" and "challenge" not in hello["lock"], str(hello.get("lock")))
        passkey = ps.SoftPasskey(secure.rstrip("/"))
        refused = await waiting.ask(type="lock_enrol", **passkey.create("AAAA"))
        run.check("…and the daemon refuses a passkey from it", (refused or {}).get("code") == "needs_desktop", str(refused))
        mark = waiting.mark()
        loop = asyncio.get_running_loop()

        def press():
            d.js(f"{S}.openSettings('mobile')")
            d.wait(f"return !!document.querySelector('[data-device=\"{no_window.id}\"] [data-reset-lock]')", 15)
            d.js(f"document.querySelector('[data-device=\"{no_window.id}\"] [data-reset-lock]').click()")

        await loop.run_in_executor(None, press)
        told = await waiting.wait(lambda m: m["type"] == "locked" and m["lock"]["state"] == "enrol", 15, mark)
        run.check("the computer's button lets that phone set one up, and only then", told is not None and bool(told["lock"].get("challenge")), str(told))
        reply = await waiting.ask(type="lock_enrol", **passkey.create(told["lock"]["challenge"]))
        run.check("…which opens it", (reply or {}).get("result") == "ok", str((reply or {}).get("code")))
        await waiting.close()
        daemon(d, "mobile.revoke", {"id": no_window.id})

    asyncio.run(passkey_takes_the_computers_window())
    d.js(f"{S}.closeSettings?.()")

    # The link as the daemon makes it, not rewritten: on a certificate listener it is at the address it listens on, a number, where no passkey can be. The phone must read there, and not wait at a lock nothing can open.
    numbered = daemon(d, "mobile.pair_start")
    run.check("the daemon's own link on a certificate listener is at the number it listens on, and says no passkey can be had there", numbered["url"].startswith(f"https://127.0.0.1:{PORT}/#p=") and numbered["tls"] is True and numbered["passkey"] is False, str(numbered["passkey"]))
    number_shell = daemon(d, "session.create", {"kind": "shell", "cwd": proj, "name": "number-shell", "project_id": pid})
    phone.go("about:blank")
    phone.go(numbered["url"])
    phone.wait("return !!document.querySelector('.card input.field')", 20)
    phone.js("document.querySelector('.card input').value = 'Number phone'; document.querySelector('.card .btn.primary').click()")
    d.wait("return [...document.querySelectorAll('.mobile-approve h2')].some(h => h.textContent.includes('Number phone'))", 15)
    type_code(phone.wait("return document.querySelector('[data-pair-code]')?.textContent", 15))
    d.js("[...document.querySelectorAll('.mobile-approve')].find(a => a.textContent.includes('Number phone')).querySelector('[data-allow=write]').click()")
    run.check("it pairs and connects", bool(phone.wait(LIVE, 30)))
    number_device = next((x for x in daemon(d, "mobile.status")["devices"] if x["name"] == "Number phone"), {})
    run.check("…and is not recorded as a page that could have a passkey", number_device.get("secure_origin") is False, str(number_device))
    run.check("…so it reads, with no lock in its way", bool(phone.wait("return !document.querySelector('[data-lock-screen]') && [...document.querySelectorAll('.row')].some(r => r.textContent.includes('number-shell'))", 20)), str(phone.js("return document.querySelector('main')?.textContent")))
    run.check("…says why it may only read", bool(phone.wait("return !!document.querySelector('[data-banner=no-passkey]')", 10)))
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('number-shell')).click()")
    run.check("…and has no way to type", bool(phone.wait("return !!document.querySelector('.readonly') && !document.querySelector('.send textarea')", 15)))
    number_footer = phone.js("return document.querySelector('.readonly')?.textContent ?? ''")
    run.check("…and is not sent to Settings for what Settings cannot change", "Settings" not in number_footer and "pair it" in number_footer, number_footer)
    phone.shot(os.path.join(shots, "29-phone-at-a-number.png"))
    daemon(d, "mobile.revoke", {"id": number_device["id"]})
    daemon(d, "session.close", {"handle": number_shell["handle"]})
    daemon(d, "mobile.enable", {"ip": "127.0.0.1", "port": PORT, "cert": "", "key": ""})
    d.js(f"{S}.closeSettings?.()")
    phone.go("about:blank")
    phone.go(base)
    phone.wait(LIVE, 30)

    # ---------- alerts: a real service worker, a stand-in for the push service, nothing sent to a real one ----------
    push_base = os.environ.get("KELPIE_PUSH_TEST_BASE", "")
    run.check("tauri-driver has this script's stand-in for a push service in its environment", push_base.startswith("http://127.0.0.1:"), "set KELPIE_PUSH_TEST_BASE to http://127.0.0.1:<free port>")
    if push_base.startswith("http://127.0.0.1:"):
        origin = base.rstrip("/")
        push = ps.FakePushService(int(push_base.rsplit(":", 1)[1]))
        sub = ps.browser_subscription(push.base, "/push/page")
        # Everything up to the browser's own push service is real; that one call is replaced, as it would reach Apple, Google, Mozilla or Microsoft.
        phone.js("""
            const [endpoint, p256dh, auth] = arguments;
            const bytes = (t) => Uint8Array.from(atob(t.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0)).buffer;
            let held = null;
            const make = (options) => (held = { endpoint, options, getKey: (name) => bytes(name === "p256dh" ? p256dh : auth), unsubscribe: async () => { held = null; window.__unsubscribed = (window.__unsubscribed || 0) + 1; return true; } });
            PushManager.prototype.subscribe = async function (options) { window.__subscribeKey = options.applicationServerKey; return make({ applicationServerKey: options.applicationServerKey.buffer ?? options.applicationServerKey }); };
            PushManager.prototype.getSubscription = async function () { return held; };
        """, sub["endpoint"], sub["p256dh"], sub["auth"])
        page_cdp = Cdp(phone.debugger, "page", origin)
        phone.req("POST", f"/session/{phone.sid}/permissions", {"descriptor": {"name": "notifications"}, "state": "granted"})
        run.check("the app's service worker runs here", bool(phone.wait("return navigator.serviceWorker.ready.then(() => true) && !!navigator.serviceWorker.controller || navigator.serviceWorker.getRegistration().then((r) => !!r)", 20)))
        phone.js("document.querySelector('.conn').click()")
        run.check("the sheet offers alerts on a page that can have them", phone.wait("return document.querySelector('[data-alerts]')?.dataset.alerts", 15) == "off" and bool(phone.js("return !!document.querySelector('[data-alerts-on]')")), str(phone.js("return document.querySelector('[data-alerts]')?.dataset.alerts")))
        phone.js("document.querySelector('[data-alerts-on]').click()")
        run.check("turning alerts on subscribes", phone.wait("return document.querySelector('[data-alerts]')?.dataset.alerts === 'on'", 20) is True, str(phone.js("return document.querySelector('.alerts-row')?.textContent")))
        alert_device = next((x for x in daemon(d, "mobile.status")["devices"] if x.get("push")), None)
        run.check("…and the computer has it, for this phone", bool(alert_device), str(daemon(d, "mobile.status")["devices"]))
        run.check("…and Settings counts it", daemon(d, "notify.get")["webpush"]["devices"] == 1)
        phone.js("document.querySelector('[data-alerts-test]').click()")
        run.check("a test alert reaches the stand-in", wait(lambda: len(push.at("/push/page")) >= 1, 15) is True, str(len(push.at("/push/page"))))
        run.check("…and the sheet says it went", "Sent" in (phone.wait("return document.querySelector('[data-alerts-tested]')?.textContent?.includes('Sent') && document.querySelector('[data-alerts-tested]').textContent", 15) or ""))
        phone.js("[...document.querySelectorAll('.sheet .btn')].find((b) => b.textContent === 'Close').click()")

        # What the service would hand the browser goes to the real worker, which shows it.
        page_cdp.call("ServiceWorker.enable")
        time.sleep(1)
        page_cdp.call("Runtime.evaluate", {"expression": "1"})  # what came meanwhile is read with its answer
        # The https page of the step above has a worker of its own: this origin's is the one.
        registration = next((r["registrationId"] for e in page_cdp.events if e.get("method") == "ServiceWorker.workerRegistrationUpdated" for r in e["params"]["registrations"] if r["scopeURL"].startswith(origin) and not r["isDeleted"]), None)
        run.check("the worker's registration is known to DevTools", registration is not None, str([e.get("method") for e in page_cdp.events][:8]))
        shell_alert = daemon(d, "session.create", {"kind": "shell", "cwd": proj, "name": "alert-secret-name", "project_id": pid})
        wait(lambda: next((x for x in daemon(d, "session.list") if x["id"] == shell_alert["id"]), {}).get("status") == "idle", 60)
        hook_alert = lambda event: daemon(d, "hook.report", {"session_id": shell_alert["id"], "agent": "claude", "payload": {"hook_event_name": event}})  # noqa: E731
        hook_alert("UserPromptSubmit")
        wait(lambda: next((x for x in daemon(d, "session.list") if x["id"] == shell_alert["id"]), {}).get("status") == "working", 15)
        before = len(push.at("/push/page"))
        hook_alert("PermissionRequest")
        run.check("a needs-you event reaches the stand-in", wait(lambda: len(push.at("/push/page")) > before, 15) is True)
        taken = push.at("/push/page")[before] if len(push.at("/push/page")) > before else None
        plain = ps.open_alert(taken["body"], sub) if taken else b"{}"
        note = json.loads(plain).get("notification", {})
        run.check("…opened with the browser's keys it holds no text of the session and links to it", b"alert-secret-name" not in plain and note.get("title") == "Kelpie" and note.get("navigate", "").endswith(f"#s={shell_alert['id']}&a=1"), plain.decode()[:200])
        run.check("…and on the wire it is ciphertext", taken is not None and b"alert-secret-name" not in taken["body"] and b"navigate" not in taken["body"])
        shown_js = "const done = arguments[0]; navigator.serviceWorker.getRegistration().then((r) => r.getNotifications()).then((n) => done(n.map((x) => ({title: x.title, body: x.body, tag: x.tag, icon: x.icon, navigate: x.data && x.data.navigate}))), (e) => done({err: String(e)}));"
        def shown():
            got = phone.req("POST", f"/session/{phone.sid}/execute/async", {"script": shown_js, "args": []})
            return got if isinstance(got, list) and got else None
        # A cold worker first, as on a real phone: stopped, with nothing attached to it, and only then a push. What follows attaches to it, and could no longer show this.
        if registration is not None:
            page_cdp.call("ServiceWorker.stopAllWorkers")
            def worker_stopped():
                page_cdp.call("Runtime.evaluate", {"expression": "1"})  # what the browser said of its workers meanwhile is read with this
                states = [v.get("runningStatus") for e in page_cdp.events if e.get("method") == "ServiceWorker.workerVersionUpdated" for v in e["params"]["versions"] if v.get("registrationId") == registration]
                return bool(states) and states[-1] == "stopped"
            run.check("the worker is stopped, as a phone's is between alerts", wait(worker_stopped, 10) is True)
            cold = json.loads(plain)
            cold["notification"]["tag"] = "cold-start"
            page_cdp.call("ServiceWorker.deliverPushMessage", {"origin": origin, "registrationId": registration, "data": json.dumps(cold)})
            # The browser reconciles `getNotifications` with what it has on display: a read at the very moment of showing may find nothing.
            time.sleep(1)
            cold_shown = wait(lambda: (n := shown()) and [x for x in n if x["tag"] == "cold-start"], 20)
            run.check("a push starts a stopped worker, which shows it", bool(cold_shown) and cold_shown[0]["body"] == note.get("body"), str(cold_shown))
            phone.req("POST", f"/session/{phone.sid}/execute/async", {"script": "const done = arguments[0]; navigator.serviceWorker.getRegistration().then((r) => r.getNotifications()).then((n) => { n.forEach((x) => x.close()); done(true); }, () => done(false));", "args": []})
        delivered = False
        # What the worker does with it, from inside: whether the push reached its handler, and what became of `showNotification`.
        sw_cdp = Cdp(phone.debugger, "service_worker", origin) if registration is not None else None
        if sw_cdp:
            sw_cdp.call("Runtime.evaluate", {"expression": """(() => {
                const seen = (self.__seen = {pushes: 0, shown: [], failed: []});
                self.addEventListener("push", () => void seen.pushes++);
                const real = self.registration.showNotification.bind(self.registration);
                self.registration.showNotification = (title, options) => real(title, options).then(() => void seen.shown.push({title, tag: options.tag}), (e) => { seen.failed.push(String(e)); throw e; });
            })()"""})
        worker_saw = lambda: json.loads(sw_cdp.call("Runtime.evaluate", {"expression": "JSON.stringify(self.__seen ?? null)", "returnByValue": True})["result"]["value"] or "null")  # noqa: E731
        if registration is not None:
            try:
                page_cdp.call("ServiceWorker.deliverPushMessage", {"origin": origin, "registrationId": registration, "data": plain.decode()})
                delivered = True
            except RuntimeError as e:
                print(f"note: ServiceWorker.deliverPushMessage did not work here: {e}", flush=True)
        # The browser reconciles `getNotifications` with what it has on display, so it is looked at once the worker says it has shown one.
        settled = wait(lambda: (s := worker_saw()) and (s["shown"] or s["failed"]) and s, 10) if delivered and sw_cdp else None
        run.check("the push reaches the worker's handler, and `showNotification` succeeds", bool(settled) and settled["pushes"] == 1 and len(settled["shown"]) == 1 and not settled["failed"], str(settled or (worker_saw() if sw_cdp else None)))
        notes = wait(shown, 10) if delivered else None
        if delivered and not notes:
            page_cdp.call("Runtime.evaluate", {"expression": "1"})  # what the browser said of its workers meanwhile is read with this
            workers = [(r["registrationId"], r["scopeURL"], r["isDeleted"]) for e in page_cdp.events if e.get("method") == "ServiceWorker.workerRegistrationUpdated" for r in e["params"]["registrations"]]
            versions = [(v.get("registrationId"), v.get("runningStatus"), v.get("status")) for e in page_cdp.events if e.get("method") == "ServiceWorker.workerVersionUpdated" for v in e["params"]["versions"]]
            raw = phone.req("POST", f"/session/{phone.sid}/execute/async", {"script": shown_js, "args": []})
            print(f"note: no notification from registration {registration} of {workers}; versions {versions[-6:]}; permission {phone.js('return Notification.permission')}; getNotifications says {raw}", flush=True)
            # Which of the two was lost: the push, or the notification? The check below still fails.
            page_cdp.call("ServiceWorker.deliverPushMessage", {"origin": origin, "registrationId": registration, "data": plain.decode()})
            print(f"note: a second push of the same shows: {wait(shown, 10)}; the worker saw {worker_saw()}", flush=True)
        run.check("the worker shows it: the sentence, its tag, its link", bool(notes) and notes[0]["title"] == "Kelpie" and notes[0]["tag"] == note.get("tag") and notes[0]["body"] == note.get("body") and notes[0]["navigate"] == note.get("navigate"), str(notes))
        run.check("…and an icon that needs no request to the computer", bool(notes) and notes[0]["icon"].startswith("data:image/png;base64,"), str(notes)[:200])
        if notes:
            # A person's tap cannot be made through DevTools: the event is dispatched in the worker, on the notification it shows, and the rest is the worker's own code.
            hook_alert("Stop")
            wait(lambda: next((x for x in daemon(d, "session.list") if x["id"] == shell_alert["id"]), {}).get("status") != "waiting", 15)
            phone.wait("return ![...document.querySelectorAll('.row')].some((r) => r.textContent.includes('alert-secret-name') && r.classList.contains('waiting'))", 10)
            sw_cdp.call("Runtime.evaluate", {"expression": "self.registration.getNotifications().then((ns) => { const e = new ExtendableEvent('notificationclick'); e.notification = ns[0]; self.dispatchEvent(e); })", "awaitPromise": True})
            run.check("a tap on it opens the session in the running app", bool(phone.wait("return !!document.querySelector('.session')", 15)))
            handled = phone.wait("return document.body.textContent.includes('Already handled') && document.body.textContent.match(/Already handled[^.]*\\./)[0]", 10)
            run.check("…and says when what it was about has been dealt with", bool(handled), str(handled))
            phone.js("document.querySelector('#top .icon')?.click()")
        if sw_cdp:
            sw_cdp.close()
        daemon(d, "session.close", {"handle": shell_alert["handle"]})
        phone.js("document.querySelector('.conn').click()")
        phone.wait("return !!document.querySelector('[data-alerts-off]')", 10)
        phone.js("document.querySelector('[data-alerts-off]').click()")
        run.check("turning alerts off ends it on both sides", wait(lambda: daemon(d, "notify.get")["webpush"]["devices"] == 0, 10) is True and phone.wait("return document.querySelector('[data-alerts]')?.dataset.alerts === 'off'", 10) is True)
        phone.js("[...document.querySelectorAll('.sheet .btn')].find((b) => b.textContent === 'Close').click()")
        page_cdp.close()
        push.stop()

    # ---------- Tailscale mode, against a fake CLI: what Settings says, the two steps, off ----------
    run.check("tauri-driver has this script's fake Tailscale in its environment, so the daemon the app starts does too", os.environ.get("KELPIE_TAILSCALE_BIN") == FAKE_TS[0], f"KELPIE_TAILSCALE_BIN must be {FAKE_TS[0]}")
    d.js(f"{S}.openSettings('mobile')")
    d.wait("return !!document.querySelector('[data-tailscale-check]') && !document.querySelector('[data-tailscale-check]').disabled", 15)
    ts_state = "return document.querySelector('[data-tailscale-state]')?.dataset.tailscaleState"
    ts_serve = "return document.querySelector('[data-tailscale-serve]')?.dataset.tailscaleServe"
    ts_text = "return document.querySelector('[data-tailscale-state]')?.textContent ?? ''"
    for status, state, word in (('{"BackendState":"Stopped"}', "stopped", "Start it"), ('{"BackendState":"NeedsLogin"}', "needs_login", "Sign in"), ('{"BackendState":"Running","CertDomains":null}', "https_off", "MagicDNS"), ("not json", "unreadable", "Update Tailscale")):
        ps.fake_tailscale_says(FAKE_TS[1], status=status)
        d.js("document.querySelector('[data-tailscale-check]').click()")
        shown = d.wait(f"return document.querySelector('[data-tailscale-state]')?.dataset.tailscaleState === {state!r} && document.querySelector('[data-tailscale-state]').textContent", 15)
        run.check(f"{state}: one plain instruction", bool(shown) and word in shown, str(shown))
    ps.fake_tailscale_says(FAKE_TS[1], status=ps.FAKE_TAILSCALE_READY)
    d.js("document.querySelector('[data-tailscale-check]').click()")
    run.check("ready: it says so", d.wait(f"return {ts_state[7:]} === 'ready' && {ts_serve[7:]} === 'off'", 15) is True)
    run.check("the price is stated: the name in public certificate logs", "certificate logs" in d.js("return document.querySelector('[data-tailscale-privacy]')?.textContent ?? ''"))
    run.shot("22-tailscale-ready")
    d.js("document.querySelector('[data-setting=\"mobile-tailscale\"] .switch').click()")
    try:
        published = d.wait(f"return {ts_serve[7:]} === 'ours'", 20) is True
    except TimeoutError:
        published = False
    # What it is about, if it did not: the steps go on only when it did.
    said = "" if published else f"; Settings says {d.js('return [...document.querySelectorAll(`.form-note.error, [data-setting=mobile-tailscale] .muted-inline`)].map((e) => e.textContent)')}; the daemon says {daemon(d, 'mobile.status').get('tailscale')}"
    run.check("turning it on publishes it", published, str(ps.fake_tailscale_calls(FAKE_TS[2])[-6:]) + said)
    if not published:
        raise TimeoutError("Tailscale mode did not turn on")
    run.check("…and says where", ps.FAKE_TAILNET_NAME in d.js(ts_text))
    d.wait("return !!document.querySelector('[data-tailscale-pair]')", 10)
    d.js("document.querySelector('[data-tailscale-pair]').click()")
    install_link = d.wait("return document.querySelector('[data-tailscale-guide=\"1\"] [data-install-link]')?.dataset.installLink", 10)
    run.check("Step 1 shows the page's address, with no secret", install_link == f"https://{ps.FAKE_TAILNET_NAME}:8443/" and d.js("return !!document.querySelector('[data-tailscale-guide=\"1\"] svg')"), str(install_link))
    run.shot("23-tailscale-step-1")
    d.js("document.querySelector('[data-guide-next]').click()")
    link2 = d.wait("return document.querySelector('[data-tailscale-guide=\"2\"] .mobile-qr-code')?.dataset.link", 10)
    run.check("Step 2 shows the pairing code, at that origin", bool(link2) and link2.startswith(f"https://{ps.FAKE_TAILNET_NAME}:8443/#p="))  # never the link in the log
    run.shot("24-tailscale-step-2")
    d.js("document.querySelector('[data-guide-close]').click()")
    ps.fake_tailscale_says(FAKE_TS[1], serve="{}")
    d.js("document.querySelector('[data-tailscale-check]').click()")
    run.check("an entry removed by hand is told, with a way back", d.wait(f"return {ts_serve[7:]} === 'missing' && !!document.querySelector('[data-tailscale-restore]')", 15) is True)
    d.js("document.querySelector('[data-tailscale-restore]').click()")
    run.check("…which restores it", d.wait(f"return {ts_serve[7:]} === 'ours'", 20) is True)
    d.js("document.querySelector('[data-setting=\"mobile-tailscale\"] .switch').click()")
    run.check("turning it off removes the entry", d.wait(f"return {ts_serve[7:]} === 'off'", 20) is True and json.load(open(FAKE_TS[1], encoding="utf-8"))["serve"] == "{}")
    calls = ps.fake_tailscale_calls(FAKE_TS[2])
    run.check("…and only ever by the command that removes ours", not [c for c in calls if "reset" in c or "funnel" in c] and calls.count("serve --https=8443 off") == 1, str([c for c in calls if not c.endswith("--json")]))
    # Phone access off with Tailscale down: the entry stays, Settings says so with phone access off, and Check again clears it once Tailscale answers.
    d.js("document.querySelector('[data-setting=\"mobile-tailscale\"] .switch').click()")
    d.wait(f"return {ts_serve[7:]} === 'ours'", 20)
    ps.fake_tailscale_says(FAKE_TS[1], serve_down=True)
    daemon(d, "mobile.disable")
    run.check("phone access off with Tailscale down: Settings says the entry is still there", bool(d.wait("return !!document.querySelector('[data-tailscale-to-remove]')", 20)))
    run.check("…and the daemon has kept it", daemon(d, "mobile.status")["tailscale"]["to_remove"] is True and json.load(open(FAKE_TS[1], encoding="utf-8"))["serve"] != "{}")
    ps.fake_tailscale_says(FAKE_TS[1], serve_down=False)
    d.js("document.querySelector('[data-tailscale-check]').click()")
    run.check("…and once Tailscale answers, Check again removes it and the note goes", d.wait("return !document.querySelector('[data-tailscale-to-remove]')", 20) is True and json.load(open(FAKE_TS[1], encoding="utf-8"))["serve"] == "{}")
    daemon(d, "mobile.enable", {"ip": "127.0.0.1", "port": PORT})
    # Turning access off cut the phone. It must be back before the limiter below, which would keep it out, and the kill switch, which only a connected phone can hear.
    run.check("the phone comes back once phone access is on again", bool(phone.wait(CONNECTED, 30)), str(phone.js(STATE)))

    # ---------- a page that is closed at the kill switch cannot hear it: access ends all the same, and what it kept stays until it is forgotten there ----------
    missed_row = "return [...document.querySelectorAll('.row')].some(r => r.textContent.includes('missed-shell'))"
    missed = daemon(d, "session.create", {"kind": "shell", "cwd": proj, "name": "missed-shell", "project_id": pid})
    phone.go("about:blank")
    phone.go(base)  # the list the page keeps is saved as it connects
    phone.wait(CONNECTED, 30)
    phone.wait(missed_row, 20)
    # A write waits 1.5 s (`cache.ts`), and leaving the page before that drops it.
    list_saved = """if (!window.__askedList) { window.__askedList = true; const open = indexedDB.open('kelpie-phone');
      open.onsuccess = () => { const got = open.result.transaction('kv').objectStore('kv').get('list');
        got.onsuccess = () => { window.__listSaved = (got.result?.sessions ?? []).some(s => s.name === 'missed-shell'); window.__askedList = false; open.result.close(); }; }; }
      return window.__listSaved === true"""
    run.check("the page has saved the list that names the new session", bool(phone.wait(list_saved, 15)))
    phone.go("about:blank")
    daemon(d, "mobile.revoke_all")
    daemon(d, "mobile.enable", {"ip": "127.0.0.1", "port": PORT})
    phone.go(base)
    run.check("a page closed at the kill switch says Not paired here once access is back", bool(phone.wait("return document.querySelector('[data-banner=not_paired_here]')?.textContent.includes('Not paired here')", 30)), str(phone.js(STATE)))
    run.check("…with no session, and the computer lists no phone", phone.js(STATE) == "not_paired_here" and not daemon(d, "mobile.status")["devices"], str(phone.js(STATE)))
    run.check("…and keeps what it showed", bool(phone.wait(missed_row, 15)))
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('missed-shell')).click()")
    phone.wait("return !!document.querySelector('.session')", 15)
    phone.js("const t = document.querySelector('.send textarea'); if (t) t.value = 'echo missed-$((6*7))';")
    phone.js("document.querySelector('.send .btn.primary')?.click()")
    time.sleep(2)
    run.check("…and it cannot act", phone.js("const b = document.querySelector('.send .btn.primary'); return !b || b.disabled") is True and "missed-42" not in daemon(d, "session.read", {"handle": missed["handle"], "lines": 40})["text"])
    phone.js("window.confirm = () => true")
    phone.js("document.querySelector('[data-forget]').click()")
    run.check("Forget this device lets go of what it kept", bool(phone.wait("return !!document.querySelector('.card') && !document.querySelector('.row') && !document.querySelector('.conn')", 15)))
    daemon(d, "session.close", {"handle": missed["handle"]})
    made = daemon(d, "mobile.pair_start")
    phone.go("about:blank")
    phone.go(made["url"])
    phone.wait("return !!document.querySelector('.card .btn.primary')")
    phone.js("document.querySelector('.card input').value = 'Paired again'; document.querySelector('.card .btn.primary').click()")
    d.wait("return [...document.querySelectorAll('.mobile-approve h2')].some(h => h.textContent.includes('Paired again'))", 15)
    type_code(phone.wait("return document.querySelector('[data-pair-code]')?.textContent", 15))
    d.js("[...document.querySelectorAll('.mobile-approve')].find(a => a.textContent.includes('Paired again')).querySelector('[data-allow=write]').click()")
    run.check("…and pairing it again brings it back", bool(phone.wait(CONNECTED, 30)), str(phone.js(STATE)))
    d.js(f"{S}.closeSettings?.()")

    # Last, so the limiter does not refuse a pairing this script still needs.
    dpk, _ = ps.pair_secrets(link)
    made_up = base + "#p=" + ps.ln.b64(json.dumps({"v": 1, "dpk": ps.ln.b64(dpk), "psk": ps.ln.b64(os.urandom(32))}).encode())
    run.check("a wrong pairing code gets no answer", not pairing_answered(base, made_up))
    run.check("a phone the daemon does not know is told so", stranger_refusal(base, dpk) == 4401)
    refusals = [stranger_refusal(base, dpk) for _ in range(12)]
    run.check("repeated unknown phones are rate limited", refusals[-1] == 4429, str(refusals))
    run.check("…and the desktop says so", bool(d.wait("return [...document.querySelectorAll('.toast')].some(t => t.textContent.includes('Too many failed'))", 15)))

    # ---------- settings: how long a silent phone stays, what phones did, the kill switch ----------
    d.js(f"{S}.openSettings('mobile')")
    d.wait("return !!document.querySelector('[data-dormant]')", 10)
    run.check("phones not seen for 90 days are removed by default", d.js("return document.querySelector('[data-dormant]').value") == "90")
    d.js("const s = document.querySelector('[data-dormant]'); s.value = '30'; s.dispatchEvent(new Event('change', {bubbles: true}));")
    run.check("…and that can be changed", bool(wait(lambda: daemon(d, "mobile.status").get("dormant_days") == 30, 10)))
    d.js("const a = document.querySelector('[data-audit]'); a.open = true;")
    run.check("the list of what phones did shows entries, with an intact log", bool(d.wait("return document.querySelectorAll('[data-audit-entry]').length > 0 && !document.querySelector('[data-audit-broken]')", 10)))
    run.shot("20-mobile-audit")
    d.js("document.querySelector('[data-revoke-all]').click()")
    run.check("the kill switch turns phone access off and removes every phone", bool(wait(lambda: (lambda s: not s["running"] and not s["devices"])(daemon(d, "mobile.status")), 10)))
    run.check("…and the phone hears it at once", bool(phone.wait("return document.querySelector('.card')?.textContent.includes('no longer paired')", 15)))
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    if phone:
        phone.quit()
    ps.forget_secrets(ISOLATED["KELPIE_ENDPOINT"])
    if REFUSED:
        run.d.quit()
        sys.exit(2)
    run.finish()
