"""The phone page in real Mobile Safari (WebKit) in the iOS Simulator, against a real daemon on this Mac.

    python3 scripts/e2e/phone_safari_sim.py <kelpied> [--udid SIMULATOR] [--out result-dir] [--tour pictures-dir]

What the Chromium scripts cannot show is how iPhone Safari itself takes the page: its WebKit, its WebCrypto for the encrypted link, its
passkeys behind Face ID, its WebSocket and its layout. This drives Mobile Safari with `safaridriver` (`safari:useSimulator`) and checks, on
the page a person uses:
1. The page loads over HTTPS at `localhost` (the run's certificate is trusted in the simulator's keychain) and raises no script error.
2. Pairing in the browser with a link from the daemon, the code shown, the person's approval on the computer.
3. The app lock is set up with a passkey: Face ID is enrolled in the simulator and a matching face is presented when the sheet asks.
4. The inbox lists a shell session; its view shows the terminal; a command typed in the reply box runs and its output comes back.
5. Settings opens and shows the device's permission.

<kelpied> is a DEBUG build with the phone client embedded (`cd app && pnpm build:phone`, then `KELPIE_REQUIRE_PHONE=1 cargo build -p
kelpied`): only that build takes this script for the Kelpie app and answers the person's verification itself. Needs Xcode with an iPhone
simulator, `safaridriver --enable` once (admin), and Python with `selenium`, `cryptography` and `websockets`. Simulator Safari must allow
Remote Automation (set here through `defaults`). Everything the script starts is its own: a daemon with its own endpoint and data folder,
phone access on 127.0.0.1 with a certificate of this run, opened at `localhost` (a passkey needs a name).
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import phone_support as ps  # noqa: E402

SESSION = "Safari shell"
PROJECT = "proj-safari"
results = []


def check(name, ok, detail=""):
    results.append((name, bool(ok), str(detail)))
    print(f"[{'ok' if ok else 'FAIL'}] {name}" + (f"  ({str(detail)[:300]})" if not ok and detail else ""), flush=True)
    return ok


class Stop(Exception):
    pass


def need(name, ok, detail=""):
    if not check(name, ok, detail):
        raise Stop(name)


def simctl(*args, check_rc=True):
    r = subprocess.run(["xcrun", "simctl", *args], capture_output=True, text=True, timeout=120)
    if check_rc and r.returncode != 0:
        raise RuntimeError(f"simctl {' '.join(args)}: {r.stderr.strip()[:300]}")
    return r.stdout


def booted_iphone(udid):
    if udid:
        state = simctl("list", "devices", udid)
        if "Booted" not in state:
            simctl("boot", udid, check_rc=False)
        return udid
    devices = json.loads(simctl("list", "devices", "available", "-j"))["devices"]
    phones = [d for runtime, ds in devices.items() if "iOS" in runtime for d in ds if d["name"].startswith("iPhone")]
    booted = [d for d in phones if d["state"] == "Booted"]
    pick = (booted or phones)[0]
    if pick["state"] != "Booted":
        simctl("boot", pick["udid"])
    return pick["udid"]


def face_id(udid, what):
    """Face ID in the simulator: `enroll`, or `match` (a matching face is shown to the camera)."""
    if what == "enroll":
        simctl("spawn", udid, "notifyutil", "-s", "com.apple.BiometricKit.enrollmentChanged", "1")
        simctl("spawn", udid, "notifyutil", "-p", "com.apple.BiometricKit.enrollmentChanged")
    else:
        simctl("spawn", udid, "notifyutil", "-p", "com.apple.BiometricKit_Sim.pearl.match")


class Page:
    def __init__(self, udid, out, tour=None):
        from selenium import webdriver
        from selenium.webdriver.safari.options import Options

        o = Options()
        o.set_capability("platformName", "iOS")
        o.set_capability("safari:useSimulator", True)
        o.set_capability("safari:deviceUDID", udid)
        self.d = webdriver.Safari(options=o)
        self.udid, self.out, self.n, self.tour = udid, out, 0, tour

    def shot(self, name):
        """The simulator's picture: numbered into --out, or named phone-safari-<name>.png in --tour."""
        if self.tour:
            path = os.path.join(self.tour, f"phone-safari-{name}.png")
        elif self.out:
            self.n += 1
            path = os.path.join(self.out, f"{self.n:02d}-{name}.png")
        else:
            return
        # The simulator's own picture shows the system sheets (passkeys, Face ID) that the page's picture does not.
        try:
            simctl("io", self.udid, "screenshot", path)
        except Exception as e:  # noqa: BLE001
            print(f"  (no picture {name}: {e})", flush=True)

    def tap_screen(self, fx, fy):
        """A click on the simulator's screen at a fraction of its size, for the system's sheets (needs Accessibility for this shell)."""
        if not getattr(self, "tapper", None):
            self.tapper = os.path.join(tempfile.mkdtemp(prefix="sim-tap-"), "sim_tap")
            subprocess.run(["xcrun", "swiftc", "-O", os.path.join(os.path.dirname(os.path.abspath(__file__)), "sim_tap.swift"), "-o", self.tapper],
                           check=True, capture_output=True, timeout=300)
        r = subprocess.run([self.tapper, str(fx), str(fy)], capture_output=True, text=True, timeout=30)
        print(f"  tap {fx},{fy}: {r.stdout.strip()} {r.stderr.strip()}", flush=True)

    def js(self, script, *args):
        return self.d.execute_script(script, *args)

    def has(self, sel):
        return bool(self.js("return !!document.querySelector(arguments[0])", sel))

    def wait(self, sel, timeout=20, gone=False):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self.has(sel) != gone:
                return True
            time.sleep(0.3)
        raise TimeoutError(f"{'still there' if gone else 'never shown'}: {sel}")

    def click(self, sel):
        # safaridriver's element click on iOS lands as a press that selects the button's text (the loupe shows); the page's own
        # handlers are what is tested, so the element is clicked from the page.
        if not self.js("const e = document.querySelector(arguments[0]); if (!e) return false; e.scrollIntoView({block: 'center'}); e.click(); return true;", sel):
            raise RuntimeError(f"nothing to click: {sel}")

    def tap(self, sel):
        """A finger's tap (W3C touch actions): what a passkey needs, since Safari asks for one only on a person's gesture."""
        from selenium.webdriver.common.actions import interaction
        from selenium.webdriver.common.actions.action_builder import ActionBuilder
        from selenium.webdriver.common.actions.pointer_input import PointerInput
        from selenium.webdriver.common.by import By

        el = self.d.find_element(By.CSS_SELECTOR, sel)
        self.js("arguments[0].scrollIntoView({block: 'center'})", el)
        a = ActionBuilder(self.d, mouse=PointerInput(interaction.POINTER_TOUCH, "finger"))
        a.pointer_action.move_to(el).pointer_down().pause(0.05).pointer_up()
        a.perform()

    def fill(self, sel, text):
        from selenium.webdriver.common.by import By

        el = self.d.find_element(By.CSS_SELECTOR, sel)
        # safaridriver's typing on iOS does not reach the field; the value is set as an input would set it.
        self.js("""const e = arguments[0]; e.focus();
                   const set = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(e), 'value').set; set.call(e, arguments[1]);
                   e.dispatchEvent(new InputEvent('input', {bubbles: true, inputType: 'insertText', data: arguments[1]}));
                   e.dispatchEvent(new Event('change', {bubbles: true}));""", el, text)

    def text(self, sel):
        return (self.js("const e = document.querySelector(arguments[0]); return e ? e.textContent : ''", sel) or "").strip()

    def errors(self):
        return self.js("return (window.__kelpieErrors || []).slice(0, 5)") or []

    def watch_errors(self):
        self.js("window.__kelpieErrors = []; window.addEventListener('error', e => window.__kelpieErrors.push(String(e.message)));"
                "window.addEventListener('unhandledrejection', e => window.__kelpieErrors.push(String(e.reason)));")


def pair(d, ph, port):
    ph.d.get(f"https://localhost:{port}/")
    ph.wait("[data-stay-in-browser], [data-scan]", 40)
    ph.watch_errors()
    check("the page loads in Mobile Safari over HTTPS at localhost", True, ph.d.title)
    ph.shot("first")
    if ph.has("[data-stay-in-browser]"):
        ph.click("[data-stay-in-browser]")
    ph.wait("[data-scan]")
    ph.click("[data-scan-start]")
    try:
        ph.wait("[data-scan-problem]", 8)
    except TimeoutError:
        pass
    link = d.call("mobile.pair_start")["url"].replace(f"https://127.0.0.1:{port}/", f"https://localhost:{port}/")
    ph.fill("[data-paste]", link)
    ph.click("[data-paste-go]")
    ph.wait("[data-pair-name], .card input.field")
    name = "Safari phone"
    ph.fill("[data-pair-name], .card input.field", name)
    ph.click("[data-pair-go], .card .btn.primary")
    ph.wait("[data-pair-code]")
    code = ph.text("[data-pair-code]")
    ph.shot("pair-code")
    request = ps.pending_request(d, name)
    d.call("mobile.approve", {"id": request["id"], "allow": True, "code": code, "permission": "write"})
    ph.wait("[data-lock-screen]", 30)
    ph.shot("lock-setup")
    check("pairing in Mobile Safari reaches the app lock's setup", True)

    # The passkey: Safari shows its own sheet; Face ID is enrolled, and a matching face is shown while the sheet waits.
    stop = threading.Event()

    def faces():
        while not stop.wait(1.5):
            try:
                face_id(ph.udid, "match")
            except Exception:  # noqa: BLE001
                pass

    t = threading.Thread(target=faces, daemon=True)
    t.start()
    try:
        # A finger tap through safaridriver lands as a press that selects the button's text on iOS (the loupe), so the button is
        # pressed from the page; WebKit counts what the automation runs as the person's gesture, which a passkey needs.
        ph.click("[data-lock-go]")
        time.sleep(3)
        ph.shot("passkey-sheet")
        # Safari's own "Save a passkey?" sheet: its Add Passkey button sits at the bottom of the screen, outside the page.
        ph.tap_screen(0.5, 0.93)
        time.sleep(2)
        ph.shot("after-add")
        try:
            ph.wait("[data-lock-screen]", 60, gone=True)
        except TimeoutError:
            raise RuntimeError(f"the app lock was not set up: {ph.text('[data-lock-error]') or ph.text('[data-lock-screen]')[:300]}; errors {ph.errors()}")
    finally:
        stop.set()
    return next(x for x in d.call("mobile.status")["devices"] if x["name"] == name)


def open_session(ph):
    ph.wait("[data-inbox-view], .view-switch", 30)
    deadline = time.time() + 30
    while time.time() < deadline:
        found = ph.js("""const rows = [...document.querySelectorAll('.row, [data-id]')].filter(r => r.textContent.includes(arguments[0]));
                         if (rows.length) { rows[0].click(); return true; } return false;""", SESSION)
        if found:
            return
        time.sleep(1.0)
    raise RuntimeError("the shell session never showed in the inbox")


def scenario(d, ph, port, folder):
    need("the daemon answers the person's verification itself (a debug build)", d.call("daemon.info").get("test_user_presence") == "approve")
    cert, key = ps.certificate_for_loopback(folder)
    simctl("keychain", ph.udid, "add-root-cert", cert)
    st = d.call("mobile.enable", {"ip": "127.0.0.1", "port": port, "cert": cert, "key": key})
    need("phone access serves HTTPS on the chosen address", bool(st.get("running")), st)
    d.call("mobile.set_projects", {"projects": {PROJECT: "Safari"}})
    d.call("session.create", {"kind": "shell", "cwd": folder, "name": SESSION, "project_id": PROJECT})
    if ph.tour:
        demo_sessions(d, folder)

    dev = pair(d, ph, port)
    need("the phone paired in Safari and its app lock was set up with a passkey (Face ID)", dev.get("permission") == "write", dev)
    ph.shot("inbox")

    open_session(ph)
    ph.wait(".screen", 30)
    ph.shot("session")
    check("the shell session's view shows the terminal", ph.has(".screen"))
    marker = f"safari-{os.getpid()}"
    ph.fill(".send-row textarea", f"echo {marker}-$((6*7))")
    ph.click(".send-row .btn.primary")
    deadline, seen = time.time() + 30, ""
    while time.time() < deadline:
        seen = ph.text(".screen")
        if f"{marker}-42" in seen:
            break
        time.sleep(0.5)
    ph.shot("typed")
    check("a command typed in Safari's reply box runs, and its output comes back", f"{marker}-42" in seen, seen[-300:])

    while ph.has("[data-back]"):
        ph.click("[data-back]")
        time.sleep(0.5)
    ph.click("[data-open-settings]")
    ph.wait("[data-settings]")
    ph.shot("settings")
    check("settings open in Safari", ph.has("[data-settings]"))
    check("the page raised no script error", not ph.errors(), ph.errors())

    # ---------- the page opened again: unlocked with the passkey (a WebAuthn get behind Face ID) ----------
    ph.d.refresh()
    ph.wait("[data-lock-screen], [data-inbox-view], .view-switch", 40)
    ph.watch_errors()
    # "unknown" while the link reconnects: the computer decides what the page shows.
    deadline, face = time.time() + 40, "unknown"
    while time.time() < deadline and face == "unknown":
        face = ph.js("const e = document.querySelector('[data-lock-screen]'); return e ? e.getAttribute('data-lock-screen') : 'open'")
        time.sleep(0.5)
    ph.shot("reopened")
    need("opened again, the page asks for the passkey to unlock", face == "unlock", face)
    unlock(ph)
    check("the passkey unlocks the page in Safari", not ph.has("[data-lock-screen]"), ph.text("[data-lock-error]") or ph.text("[data-lock-screen]")[:200])
    check("…and the inbox is back", ph.has("[data-inbox-view], .view-switch"))
    check("the page raised no script error after reopening", not ph.errors(), ph.errors())

    if ph.tour:
        missed = []
        tour(d, ph, missed)
        check("the page raised no script error during the tour", not ph.errors(), ph.errors())
        ph.d.quit()
        tour_vietnamese(lambda: Page(ph.udid, ph.out, ph.tour), ph.udid, f"https://localhost:{port}/", missed)
        check("the tour captured every screen", not missed, missed)


def unlock(ph):
    """The lock screen's button, then Safari's "Sign In" sheet with Face ID showing a matching face."""
    stop = threading.Event()
    threading.Thread(target=lambda: [face_id(ph.udid, "match") for _ in iter(lambda: stop.wait(1.5), True)], daemon=True).start()
    try:
        ph.click("[data-lock-go]")
        time.sleep(3)
        ph.shot("unlock-sheet")
        # Safari's "Sign In" sheet: Use Passkey sits at 86 % of the screen's height, More Options under it at 92 %.
        ph.tap_screen(0.5, 0.86)
        time.sleep(3)
        try:
            ph.wait("[data-lock-screen]", 40, gone=True)
        except TimeoutError:
            ph.shot("unlock-stuck")
    finally:
        stop.set()


# ---------- the README tour: every screen of the phone page in Mobile Safari, with simulated agents ----------


def demo_sessions(d, folder):
    """Sessions in every state the inbox shows, about the demo projects. The chats are demo_agents.py's simulated agents."""
    from demo_agents import write_programs

    # A Git repository whose `origin` names a demo repository on GitHub, never reached: `gh` is the stand-in main() puts on PATH, which
    # finds no pull request, so a session's pull-request line says so instead of an error about the folder.
    repo = os.path.join(folder, "demo-repo")
    os.makedirs(repo, exist_ok=True)
    for args in (["init", "-q", "-b", "main"], ["-c", "user.name=Kelpie Demo", "-c", "user.email=demo@kelpie.invalid", "commit", "-q", "--allow-empty", "-m", "demo"],
                 # The SSH form: a git `insteadOf` rule for https://github.com/ (a proxy on a test machine) would hide that it is GitHub.
                 ["remote", "add", "origin", "git@github.com:kelpie-demo/atlas-commerce.git"]):
        subprocess.run(["git", "-C", repo, *args], check=True, capture_output=True)
    programs = write_programs(os.path.join(folder, "agents"))
    projects = {"p-atlas": "atlas-commerce", "p-beacon": "beacon-api", "p-relay": "relay-workers", PROJECT: "field-notes"}
    d.call("mobile.set_projects", {"projects": projects})
    d.call("mobile.set_launch", {"projects": [{"id": pid, "name": name, "path": repo} for pid, name in projects.items()],
                                 "agents": [{"kind": "claude", "label": "Claude Code", "args": [], "env": {}}, {"kind": "codex", "label": "Codex", "args": [], "env": {}}]})
    py = sys.executable

    def program(name, pid, which=None):
        """A terminal session running the program itself (no shell prompt in the picture), or a plain shell."""
        d.call("session.create", {"kind": "shell", "cwd": repo, "name": name, "project_id": pid, **({"args": [py, "-u", programs[which]]} if which else {})})

    def chat(name, pid, which):
        d.call("session.create", {"kind": "claude", "cwd": repo, "chat_argv": [py, "-u", programs[which]], "name": name, "project_id": pid})

    program("Checks (watch)", "p-atlas", "ticker")
    program("Migration", "p-relay", "menu")
    chat("Release 0.4.0", "p-beacon", "chat_approval")
    chat("Fix pasted coupon codes", "p-atlas", "chat_work")
    chat("How retries work", "p-relay", "chat_done")
    program("Catalog server", "p-beacon")
    time.sleep(4)


def fake_gh(folder):
    """A `gh` that finds no pull request, first on PATH for the daemon (README tour only): GitHub is never asked."""
    bin_dir = os.path.join(folder, "fake-gh")
    os.makedirs(bin_dir, exist_ok=True)
    path = os.path.join(bin_dir, "gh")
    with open(path, "w", encoding="utf-8") as f:
        f.write("#!/bin/sh\necho 'no pull requests found for branch \"main\"' >&2\nexit 1\n")
    os.chmod(path, 0o755)
    os.environ["PATH"] = bin_dir + os.pathsep + os.environ["PATH"]


def tour(d, ph, missed):
    """Each screen once; a failed one is noted and the tour goes on."""

    def step(name, fn):
        try:
            fn()
        except Exception as e:  # noqa: BLE001
            missed.append(f"{name}: {str(e).splitlines()[0][:200]}")
            print(f"  MISSED {name}: {str(e).splitlines()[0][:200]}", flush=True)

    def inbox():
        for _ in range(5):
            if not ph.has("[data-back]"):
                break
            ph.click("[data-back]")
            time.sleep(0.6)
        ph.wait("[data-inbox-view], .view-switch", 20)

    def open_row(text):
        ph.wait("[data-inbox-view], .view-switch", 20)
        deadline = time.time() + 20
        while time.time() < deadline:
            if ph.js("""const rows = [...document.querySelectorAll('.row, [data-id]')].filter(r => r.textContent.includes(arguments[0]));
                        if (rows.length) { rows[0].click(); return true; } return false;""", text):
                time.sleep(1.2)
                return
            time.sleep(0.5)
        raise RuntimeError(f"no row {text}")

    def view(which):
        inbox()
        if ph.has(f"[data-view={which}]"):
            ph.click(f"[data-view={which}]")
        time.sleep(1.0)
        ph.shot(f"inbox-{which}" if which != "status" else "inbox")

    def screen(row, sel, name):
        inbox()
        open_row(row)
        ph.wait(sel, 25)
        time.sleep(1.2)
        ph.shot(name)

    def new_session():
        inbox()
        ph.click("[data-open-new-session]")
        ph.wait("[data-new-start]", 15)
        time.sleep(1.0)
        ph.shot("new-session")
        ph.js("const b = [...document.querySelectorAll('[data-new-session] button')].pop(); if (b) b.click();")
        time.sleep(0.6)

    def settings():
        inbox()
        ph.click("[data-open-settings]")
        ph.wait("[data-settings]")
        time.sleep(1.0)
        ph.shot("settings")

    def locked():
        if not ph.has("[data-settings]"):
            settings()
        ph.click("[data-setting=lock-now]")
        ph.wait("[data-lock-screen]", 15)
        time.sleep(1.0)
        ph.shot("locked")
        unlock(ph)

    step("inbox", lambda: view("status"))
    step("inbox-workspace", lambda: view("workspace"))
    step("inbox-status-again", lambda: (inbox(), ph.has("[data-view=status]") and ph.click("[data-view=status]")))
    step("terminal", lambda: screen("Checks (watch)", ".screen", "terminal"))
    step("approval", lambda: screen("Migration", "[data-prompt]", "approval"))
    step("chat-approval", lambda: screen("Release 0.4.0", "[data-perm]", "chat-approval"))
    step("chat", lambda: screen("Fix pasted coupon codes", ".chat-log, [data-chat-log]", "chat"))
    step("new-session", new_session)
    step("settings", settings)
    step("locked", locked)
    # Dark: the simulator's own appearance, which the page follows.
    try:
        simctl("ui", ph.udid, "appearance", "dark")
        time.sleep(1.5)
        step("inbox-dark", lambda: (inbox(), time.sleep(0.8), ph.shot("inbox-dark")))
        step("chat-approval-dark", lambda: screen("Release 0.4.0", "[data-perm]", "chat-approval-dark"))
        step("terminal-dark", lambda: screen("Checks (watch)", ".screen", "terminal-dark"))
    finally:
        simctl("ui", ph.udid, "appearance", "light")
        time.sleep(1.0)
    inbox()


def tour_vietnamese(ph_factory, udid, url, missed):
    """The simulator in Vietnamese (its language, which Safari reports to the page), the page opened again and unlocked: the inbox and
    settings in Vietnamese. The simulator's own language is restored after."""
    before = simctl("spawn", udid, "defaults", "read", "-g", "AppleLanguages", check_rc=False)
    langs = [x.strip().strip('",') for x in before.strip().strip("()").split("\n") if x.strip().strip('",')] or ["en"]
    locale = simctl("spawn", udid, "defaults", "read", "-g", "AppleLocale", check_rc=False).strip() or "en_US"

    def restart(languages, loc):
        simctl("spawn", udid, "defaults", "write", "-g", "AppleLanguages", "-array", *languages)
        simctl("spawn", udid, "defaults", "write", "-g", "AppleLocale", loc)
        # Safari reads the languages when it starts: it is quit, and the next WebDriver session opens it again.
        simctl("terminate", udid, "com.apple.mobilesafari", check_rc=False)
        time.sleep(2)

    ph = None
    try:
        restart(["vi", "en"], "vi_VN")
        ph = ph_factory()
        ph.d.get(url)
        ph.wait("[data-lock-screen], [data-inbox-view], .view-switch", 60)
        time.sleep(3)
        if ph.has("[data-lock-screen]"):
            unlock(ph)
        ph.wait("[data-inbox-view], .view-switch", 30)
        time.sleep(1.5)
        ph.shot("inbox-vi")
        ph.click("[data-open-settings]")
        ph.wait("[data-settings]")
        time.sleep(1.0)
        ph.shot("settings-vi")
    except Exception as e:  # noqa: BLE001
        missed.append(f"vietnamese: {str(e).splitlines()[0][:200]}")
        print(f"  MISSED vietnamese: {e}", flush=True)
        ph and ph.shot("vi-stuck")
    finally:
        if ph is not None:
            try:
                ph.d.quit()
            except Exception:  # noqa: BLE001
                pass
        restart(langs, locale)


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("kelpied")
    ap.add_argument("--udid", help="the iPhone simulator to use (default: a booted one, else the first iPhone)")
    ap.add_argument("--out", help="a folder for result.json, the pictures and the daemon's log")
    ap.add_argument("--tour", help="a folder for the README's pictures, phone-safari-<screen>.png: pairing, passkey sheets, inbox, terminal, "
                    "approvals, chat, new session, settings, lock, dark and Vietnamese, with demo_agents.py's simulated agents")
    args = ap.parse_args()
    args.kelpied = os.path.abspath(args.kelpied)
    out = os.path.abspath(args.out) if args.out else None
    tour_dir = os.path.abspath(args.tour) if args.tour else None
    for folder in (out, tour_dir):
        if folder:
            os.makedirs(folder, exist_ok=True)
    if not os.path.isfile(args.kelpied):
        raise SystemExit(f"no such daemon: {args.kelpied}")

    data_dir = ps.isolate("phone-safari")
    port = 48300 + os.getpid() % 500
    d, ph = None, None
    try:
        udid = booted_iphone(args.udid)
        for domain, key in (("com.apple.mobilesafari", "WebDriverEnabled"), ("com.apple.WebInspector", "RemoteAutomationEnabled"),
                            ("com.apple.mobilesafari", "RemoteAutomationEnabled")):
            simctl("spawn", udid, "defaults", "write", domain, key, "-bool", "true")
        face_id(udid, "enroll")
        if tour_dir:
            fake_gh(data_dir)
        d = ps.Daemon(args.kelpied, env={"KELPIE_LOAD_FORCE": "L0:calm", "KELPIE_E2E": "1"}).start()
        ph = Page(udid, out, tour_dir)
        print(f"Mobile Safari {ph.d.capabilities.get('browserVersion')} on simulator {udid}", flush=True)
        try:
            scenario(d, ph, port, out or data_dir)
        except Exception:
            ph.shot("stopped")
            raise
    except Stop as e:
        print(f"stopped: {e}", flush=True)
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False, "see the traceback above")
    finally:
        if ph is not None:
            try:
                ph.d.quit()
            except Exception:  # noqa: BLE001
                pass
        if d is not None:
            d.stop()
        failed = [r for r in results if not r[1]]
        print(f"{len(results) - len(failed)} ok, {len(failed)} failed", flush=True)
        if out:
            with open(os.path.join(out, "result.json"), "w", encoding="utf-8") as f:
                json.dump({"ok": not failed and bool(results), "checks": [{"name": n, "ok": ok, "detail": x[:500]} for n, ok, x in results]}, f, indent=2)
            log_file = os.path.join(data_dir, "logs", "kelpied.log")
            if os.path.isfile(log_file):
                shutil.copyfile(log_file, os.path.join(out, "daemon.log"))
        shutil.rmtree(data_dir, ignore_errors=True)
    sys.exit(1 if failed or not results else 0)


if __name__ == "__main__":
    main()
