"""E2E for the mobile companion: Kelpie (tauri-driver) plus a phone-sized Edge (msedgedriver).

    python -X utf8 scripts/e2e/mobile.py <screenshot-dir> <scratch-dir> [--edgedriver PATH]

The server is bound to 127.0.0.1 (no firewall prompt on the test machine). The phone is
headless Edge with mobile emulation, driven through msedgedriver on port 9515.

Covers: off by default; turning it on (status, URL); security headers; pairing with a QR link,
approval on the desktop, the device list; the phone's session list, a session's screen, typing
and quick keys reaching the terminal; a read-only device cannot type; a used or wrong pairing
code is refused, too many attempts are rate limited; a socket from another origin is refused;
unpairing disconnects the phone; turning it off stops the server.
"""
import http.client
import json
import os
import subprocess
import sys
import time
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import Run, daemon  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
edgedriver = sys.argv[sys.argv.index("--edgedriver") + 1] if "--edgedriver" in sys.argv else os.environ.get("KELPIE_EDGEDRIVER", r"D:\Works\edgedriver\msedgedriver.exe")
S = "window.__kelpie.store"
PORT = 48170


class Phone:
    """A minimal W3C WebDriver client for msedgedriver (Edge with mobile emulation)."""

    def __init__(self):
        self.proc = subprocess.Popen([edgedriver, "--port=9515"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        time.sleep(1.5)
        # The performance log carries the page's WebSocket frames (what it sends and what it is sent).
        caps = {"capabilities": {"alwaysMatch": {"browserName": "MicrosoftEdge", "ms:loggingPrefs": {"performance": "ALL"}, "ms:perfLoggingPrefs": {"enableNetwork": True}, "ms:edgeOptions": {
            "args": ["--headless=new", "--window-size=412,900", "--no-first-run", f"--user-data-dir={os.path.join(scratch, 'phone-profile')}"],
            "mobileEmulation": {"deviceMetrics": {"width": 412, "height": 900, "pixelRatio": 2.0}, "userAgent": "Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Mobile Safari/537.36"},
        }}}}
        self.sid = self.req("POST", "/session", caps)["sessionId"]
        self.frames = {}  # the performance log is read once: what was read is kept here

    def req(self, method, path, body=None):
        data = json.dumps(body).encode() if body is not None else None
        r = urllib.request.Request("http://127.0.0.1:9515" + path, data=data, method=method, headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(r, timeout=60) as resp:
                return json.loads(resp.read() or b"{}").get("value", {})
        except urllib.error.HTTPError as e:
            raise RuntimeError(json.loads(e.read() or b"{}").get("value", {}).get("message", "webdriver error")) from None

    def go(self, url):
        self.req("POST", f"/session/{self.sid}/url", {"url": url})

    def socket_frames(self, method):
        """The page's WebSocket frames of `method` (`Network.webSocketFrameSent` or `...Received`) read so far, as JSON."""
        for entry in self.req("POST", f"/session/{self.sid}/log", {"type": "performance"}):
            message = json.loads(entry["message"])["message"]
            kind = message.get("method", "")
            if kind.startswith("Network.webSocketFrame"):  # every frame read is kept, whichever method is asked for
                try:
                    self.frames.setdefault(kind, []).append(json.loads(message["params"]["response"]["payloadData"]))
                except (KeyError, ValueError):
                    pass  # binary or not JSON: not a control message
        return self.frames.get(method, [])

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
        import base64
        open(path, "wb").write(base64.b64decode(self.req("GET", f"/session/{self.sid}/screenshot")))

    def quit(self):
        try:
            self.req("DELETE", f"/session/{self.sid}")
        finally:
            self.proc.kill()


def http_get(url, headers=None):
    u = urllib.parse.urlparse(url)
    c = http.client.HTTPConnection(u.hostname, u.port, timeout=10)
    c.request("GET", u.path or "/", headers=headers or {})
    r = c.getresponse()
    body = r.read()
    return r.status, dict(r.getheaders()), body


def post_pair(base, code):
    r = urllib.request.Request(base + "pair", data=json.dumps({"code": code, "name": "probe"}).encode(), method="POST", headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(r, timeout=10) as resp:
            return resp.status
    except urllib.error.HTTPError as e:
        return e.code


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


def js_await(expr, *args):
    return d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)


try:
    run.boot()
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
    status, headers, body = http_get(base)
    run.check("the web app is served with a strict CSP", status == 200 and b"/app.js" in body and "script-src 'self'" in headers.get("content-security-policy", "") and headers.get("x-frame-options") == "DENY")
    d.js(f"{S}.openSettings('mobile')")
    d.wait("return !!document.querySelector('[data-setting=\"mobile-pair\"] .btn.primary')", 10)
    run.shot("01-mobile-settings")

    # ---------- pairing ----------
    d.js("document.querySelector('[data-setting=\"mobile-pair\"] .btn.primary').click()")
    link = d.wait("return document.querySelector('.mobile-qr-code')?.dataset.link", 10)
    run.check("Pair a phone shows a QR code with a one-time link", bool(link) and "#p=" in link and d.js("return !!document.querySelector('.mobile-qr-code svg')"), str(link))
    run.shot("02-mobile-qr")
    phone = Phone()
    phone.go(link)
    phone.wait("return !!document.querySelector('.card .btn.primary')")
    run.check("the phone opens the pairing page (code taken out of the address bar)", "#p=" not in phone.js("return location.href"))
    phone.js("const i = document.querySelector('.card input'); i.value = 'Test phone';")
    phone.js("document.querySelector('.card .btn.primary').click()")
    run.check("Kelpie asks to approve the phone", bool(d.wait("return [...document.querySelectorAll('.mobile-approve h2')].some(h => h.textContent.includes('Test phone'))", 15)))
    run.shot("03-approve")
    d.js("[...document.querySelectorAll('.mobile-approve .btn.primary')].find(b => b.textContent.includes('typing')).click()")
    run.check("approved phone connects and lists sessions", bool(phone.wait("return document.getElementById('conn').className.includes('on')", 20)))
    # The served page itself: it sends caps [ack, hb] with its auth (token not printed), and the daemon's
    # hello does not put it in the legacy mode.
    sent, got = phone.socket_frames("Network.webSocketFrameSent"), phone.socket_frames("Network.webSocketFrameReceived")
    auth = [m for m in sent if m.get("type") == "auth"]
    run.check("the page's auth says it acknowledges frames and answers heartbeats (caps ack, hb)", any(sorted(m.get("caps") or []) == ["ack", "hb"] for m in auth), f"{len(sent)} frames sent, auth caps {[m.get('caps') for m in auth]}")
    hello = [m for m in got if m.get("type") == "hello"]
    run.check("the daemon's hello is not legacy (hello.hb.legacy false)", any((m.get("hb") or {}).get("legacy") is False for m in hello), f"{len(got)} frames received, hello {len(hello)}")
    devs = daemon(d, "mobile.status")["devices"]
    run.check("the device is listed with typing allowed", len(devs) == 1 and devs[0]["name"] == "Test phone" and devs[0]["can_write"], str(devs))
    run.check("the used pairing code is refused afterwards", post_pair(base, link.split("#p=")[1]) == 403)

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
    time.sleep(1.5)
    phone.js("[...document.querySelectorAll('.keys button')].find(b => b.textContent === 'Ctrl+C').click()")
    time.sleep(1.5)
    daemon(d, "session.input", {"handle": handle, "data": "echo after-ctrl-c\r"})
    run.check("quick keys (Ctrl+C) reach the terminal", bool(wait(lambda: "after-ctrl-c" in daemon(d, "session.read", {"handle": handle, "lines": 40})["text"].split("ping")[-1], 15)))

    # ---------- read-only device, bad requests ----------
    d.js("document.querySelector('[data-setting=\"mobile-pair\"] .btn.primary')?.click()")
    link2 = d.wait("return document.querySelector('.mobile-qr-code')?.dataset.link", 10)
    phone.go(link2)
    phone.wait("return !!document.querySelector('.card .btn.primary')")
    phone.js("document.querySelector('.card input').value = 'Reader'; document.querySelector('.card .btn.primary').click()")
    d.wait("return [...document.querySelectorAll('.mobile-approve h2')].some(h => h.textContent.includes('Reader'))", 15)
    d.js("[...document.querySelectorAll('.mobile-approve .btn')].find(b => b.textContent === 'Allow reading').click()")
    phone.wait("return document.getElementById('conn').className.includes('on')", 20)
    phone.wait("return [...document.querySelectorAll('.row')].some(r => r.textContent.includes('phone-shell'))", 15)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('phone-shell')).click()")
    ro = phone.wait("return document.querySelector('.readonly')?.textContent ?? null")
    run.check("a read-only device sees the screen but no input", bool(ro) and "not type" in ro and not phone.js("return !!document.querySelector('.send textarea')"), str(ro))
    status, _, _ = http_get(base + "ws", {"Upgrade": "websocket", "Connection": "Upgrade", "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==", "Sec-WebSocket-Version": "13", "Origin": "http://evil.example"})
    run.check("a socket from another site is refused", status == 403, str(status))
    run.check("a wrong pairing code is refused", post_pair(base, "0" * 64) == 403)
    for _ in range(10):
        post_pair(base, "1" * 64)
    run.check("repeated bad codes are rate limited", post_pair(base, "2" * 64) == 429)

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
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    if phone:
        phone.quit()
    run.finish()
