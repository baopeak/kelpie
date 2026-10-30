"""E2E for the mobile companion: Kelpie (tauri-driver) plus a phone-sized Edge (msedgedriver).

    python -X utf8 scripts/e2e/mobile.py <screenshot-dir> <scratch-dir> [--edgedriver PATH]

Start tauri-driver with KELPIE_LOAD_FORCE=L0:calm (and the isolated KELPIE_ENDPOINT / KELPIE_DATA_DIR): the
functional checks here must not depend on how busy the host is. How the page behaves under load is
mobile_resilience.py's and stress.py's job. The script checks that the pin is in effect.

The server is bound to 127.0.0.1 (no firewall prompt on the test machine). The phone is
headless Edge with mobile emulation, driven through msedgedriver on port 9515.

Covers: off by default; turning it on (status, URL); security headers and the embedded phone client
(SolidJS, app/phone); pairing with a QR link, approval on the desktop, the device list; the phone's
inbox (a session that needs you comes first), a session's screen, typing and quick keys reaching the
terminal, a prompt answered with one tap, a chat permission answered from the phone (the agent gets
exactly one answer), a message sent to a chat from the phone (the desktop shows the phone's chip, then the
transcript item with its origin; Stop works; either side cancels; a read-only device has no message box;
a desktop message only the computer can send holding the queue is explained on the phone's own chip, with Skip, and
noticed on the desktop outside the chat pane), a `#s=` deep link, the page coming back by itself after the
server went away; a read-only device cannot type; a socket from another origin is refused; unpairing
disconnects the phone; turning it off stops the server; after the computer restarted, what waited is shown
as lost with its text in the stopped chat, also after a reload, until discarded; a used or wrong pairing
code is refused, too many attempts are rate limited (last, so the limiter cannot refuse a pairing above).
"""
import http.client
import json
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(__file__))
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
    keep = {k: v for k, v in os.environ.items() if k in ("PATH", "SystemRoot", "SYSTEMROOT", "TEMP", "TMP", "USERPROFILE", "LOCALAPPDATA", "APPDATA", "HOME", "KELPIE_LOAD_FORCE", "KELPIE_E2E")}
    return {**keep, "KELPIE_ENDPOINT": endpoint, "KELPIE_DATA_DIR": data}


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


REFUSED = []  # set when the app's daemon is not the isolated one: the run ends with an error, not with "0 failed"
ISOLATED = isolated_env()
edgedriver = sys.argv[sys.argv.index("--edgedriver") + 1] if "--edgedriver" in sys.argv else os.environ.get("KELPIE_EDGEDRIVER", r"D:\Works\edgedriver\msedgedriver.exe")
S = "window.__kelpie.store"
PORT = int(os.environ.get("KELPIE_E2E_MOBILE_PORT", "48170"))


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


LIVE = "return document.querySelector('.conn')?.dataset.state === 'live'"
STATE = "return document.querySelector('.conn')?.dataset.state"


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
    run.check("approved phone connects and lists sessions", bool(phone.wait(LIVE, 20)))
    # The served page itself: it sends caps [ack, hb] with its auth (token not printed), and the daemon's
    # hello does not put it in the legacy mode.
    sent, got = phone.socket_frames("Network.webSocketFrameSent"), phone.socket_frames("Network.webSocketFrameReceived")
    auth = [m for m in sent if "token" in m]
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
    daemon(d, "session.input", {"handle": handle, "data": "echo after-ctrl-c\r"})
    run.check("…and the shell takes the next command", bool(wait(lambda: "after-ctrl-c" in daemon(d, "session.read", {"handle": handle, "lines": 40})["text"].split("ping")[-1], 15)))

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
    run.check("the prompt card offers the prompt's own keys", keys == ["1", "2", "3"], str(keys))
    phone.shot(os.path.join(shots, "07-phone-prompt.png"))
    tap("1")
    run.check("one tap on the card answers the prompt", screen_has("ANSWERED1=1"))
    # The next menu comes at once: its card must start afresh, not stay on the last one's "Sent".
    keys2 = card(2)
    run.check("a prompt right after another gets a fresh card with live buttons", keys2 == ["1", "2", "3"], str(keys2))
    tap("2")
    run.check("one tap answers the second prompt too", screen_has("ANSWERED2=2"))
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
    import phone_support as ps
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
            sent = [(f.get("cid"), f.get("text"), f.get("req")) for f in phone.socket_frames("Network.webSocketFrameSent") if f.get("type") == "chat_send"]
            got = [(f.get("type"), f.get("code"), f.get("result"), f.get("req")) for f in phone.socket_frames("Network.webSocketFrameReceived") if f.get("re") or f.get("req") or f.get("type") in ("ack", "error")][-12:]
            print(f"NOTE the send of {text!r} did not empty the box (Send was {'ready' if ready else 'not ready'}): {state}; chat_send frames sent: {sent}; last replies: {got}", flush=True)

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
    d.js("[...document.querySelectorAll('.mobile-approve .btn')].find(b => b.textContent === 'Allow reading').click()")
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
    status, _, _ = http_get(base + "ws", {"Upgrade": "websocket", "Connection": "Upgrade", "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==", "Sec-WebSocket-Version": "13", "Origin": "http://evil.example"})
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
    class _Calls:
        """What phone_support.pair needs of the daemon: `call`."""

        def call(self, method, params=None):
            return daemon(d, method, params or {})

    daemon(d, "mobile.enable", {"ip": "127.0.0.1", "port": PORT})
    lost_token, _lost_id = ps.pair(_Calls(), base, "Restart phone", True)
    lost_chat = daemon(d, "session.create", {"kind": "claude", "cwd": proj, "chat_argv": [sys.executable, "-u", turn_agent, turn_trace, turn_hold], "name": "phone-lost", "project_id": pid})
    phone.go(base)
    phone.js("localStorage.setItem('kelpie-token', arguments[0])", lost_token)
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
    phone.js("[...[...document.querySelectorAll('.kept-chips .chip.warn')].find(c => c.textContent.includes('queued when it restarts')).querySelectorAll('.link-btn')].find(b => b.textContent === 'Discard').click()")
    run.check("Discard lets it go", bool(phone.wait("return !document.querySelector('.kept-chips')", 15)))
    run.check("…for good: nothing of it is kept on the phone", not phone.js("return Object.keys(localStorage).some(k => k.startsWith('kelpie-draft:chat-pending:'))"))
    hold(False)
    # Last, so the limiter does not refuse a pairing this script still needs.
    run.check("a wrong pairing code is refused", post_pair(base, "0" * 64) == 403)
    for _ in range(10):
        post_pair(base, "1" * 64)
    run.check("repeated bad codes are rate limited", post_pair(base, "2" * 64) == 429)
    for dev in daemon(d, "mobile.status")["devices"]:
        daemon(d, "mobile.revoke", {"id": dev["id"]})
    daemon(d, "mobile.disable")
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    if phone:
        phone.quit()
    if REFUSED:
        run.d.quit()
        sys.exit(2)
    run.finish()
