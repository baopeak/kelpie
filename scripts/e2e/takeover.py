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
4. The OAuth window (`window.open` with a size) opens as a window of its own: the frames come from it, a tap on Allow reaches it with
   `window.opener` set, the opener hears it, and once it closes the frames come from the pane again. A link with `target=_blank` still opens in the pane.
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
import phone_support as ps  # noqa: E402

if len(sys.argv) < 3 or os.name != "nt":
    raise SystemExit(__doc__)
scratch = os.path.abspath(sys.argv[1])
app = os.path.abspath(sys.argv[2])
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
<script>
const report = (q) => fetch('/report?' + new URLSearchParams(q));
document.getElementById('signin').addEventListener('click', (e) => report({login: document.getElementById('user').value + ':' + document.getElementById('pass').value, trusted: e.isTrusted}));
document.getElementById('oauth').addEventListener('click', () => window.open('/oauth', 'oauth', 'width=480,height=520'));
window.addEventListener('message', (e) => report({granted: String(e.data)}));
</script></body></html>"""
OAUTH = """<!doctype html><html><head><title>Example consent</title><style>body{margin:0} #allow{position:absolute;left:40px;top:40px;width:200px;height:48px}</style></head>
<body><button id="allow">Allow</button><script>
document.getElementById('allow').addEventListener('click', (e) => {
  fetch('/report?' + new URLSearchParams({opener: String(!!window.opener), trusted: e.isTrusted})).finally(() => { if (window.opener) window.opener.postMessage('yes', '*'); window.close(); });
});
</script></body></html>"""
PAGE2 = "<!doctype html><html><head><title>Second Page</title></head><body><h1>Page two</h1></body></html>"
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
            body = (OAUTH if path == "/oauth" else PAGE2 if path == "/page2" else LOGIN).encode()
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
                        await self.send(type="tk_ack", tk=m["tk"], seq=m["seq"])
                if time.time() - touched > 20:
                    touched = time.time()
                    await self.send(type="lock_touch")
                await asyncio.sleep(0.03)
        except Exception:  # noqa: BLE001 - the socket closed under it
            pass

    async def tap(self, tk, x, y, st=None):
        await self.send(type="tk_input", tk=tk, ops=[{"k": "tap", "x": x, "y": y}], st=st if st is not None else now_ms())

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
    check("the plain user token cannot provide panes", "needs the Kelpie app" in (plain or ""), plain)

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
    await phone.tap(tk, 150, 58)
    typed = await phone.ask(type="tk_text", tk=tk, text=USER, st=now_ms())
    await phone.tap(tk, 150, 118)
    secret = await phone.ask(type="tk_text", tk=tk, text=PASSWORD, st=now_ms())
    check("text goes with its own answer", (typed or {}).get("result") == "sent" and (secret or {}).get("result") == "sent", (typed, secret and secret.get("type")))
    await phone.tap(tk, 150, 178)
    login = await asyncio.to_thread(until, lambda: reported("login", since), 5)
    check("the page signed in with what the phone typed", bool(login) and login[-1]["login"] == f"{USER}:{PASSWORD}")
    check("…with a trusted click", bool(login) and login[-1].get("trusted") == "true", login and login[-1].get("trusted"))

    # ---------- 4. the OAuth window ----------
    since = report_count()
    mark = phone.mark()
    await phone.tap(tk, 150, 238)
    popup = await phone.frame(mark, lambda m: m["meta"].get("window") == "popup", 15)
    check("the OAuth window opens as its own, and the phone sees it", popup is not None, phone.log[-1:] if phone.log else None)
    mark = phone.mark()
    await phone.tap(tk, 140, 64)
    opener = await asyncio.to_thread(until, lambda: reported("opener", since), 8)
    check("a tap on Allow reaches it, with window.opener", bool(opener) and opener[-1]["opener"] == "true" and opener[-1].get("trusted") == "true", opener)
    granted = await asyncio.to_thread(until, lambda: reported("granted", since), 8)
    check("…and the page that opened it hears it", bool(granted) and granted[-1]["granted"] == "yes", granted)
    back = await phone.frame(mark, lambda m: m["meta"].get("window") == "pane", 10)
    check("once it closes, the frames come from the pane again", back is not None)
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
