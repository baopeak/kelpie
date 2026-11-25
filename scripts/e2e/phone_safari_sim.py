"""The phone page in real Mobile Safari (WebKit) in the iOS Simulator, against a real daemon on this Mac.

    python3 scripts/e2e/phone_safari_sim.py <kelpied> [--udid SIMULATOR] [--out result-dir]

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
    def __init__(self, udid, out):
        from selenium import webdriver
        from selenium.webdriver.safari.options import Options

        o = Options()
        o.set_capability("platformName", "iOS")
        o.set_capability("safari:useSimulator", True)
        o.set_capability("safari:deviceUDID", udid)
        self.d = webdriver.Safari(options=o)
        self.udid, self.out, self.n = udid, out, 0

    def shot(self, name):
        if not self.out:
            return
        self.n += 1
        path = os.path.join(self.out, f"{self.n:02d}-{name}.png")
        # The simulator's own picture shows the system sheets (passkeys, Face ID) that the page's picture does not.
        try:
            simctl("io", self.udid, "screenshot", path)
        except Exception as e:  # noqa: BLE001
            print(f"  (no picture {name}: {e})", flush=True)

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
        from selenium.webdriver.common.by import By

        self.d.find_element(By.CSS_SELECTOR, sel).click()

    def fill(self, sel, text):
        from selenium.webdriver.common.by import By

        el = self.d.find_element(By.CSS_SELECTOR, sel)
        el.clear()
        el.send_keys(text)

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
        ph.click("[data-lock-go]")
        time.sleep(3)
        ph.shot("passkey-sheet")
        ph.wait("[data-lock-screen]", 60, gone=True)
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


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("kelpied")
    ap.add_argument("--udid", help="the iPhone simulator to use (default: a booted one, else the first iPhone)")
    ap.add_argument("--out", help="a folder for result.json, the pictures and the daemon's log")
    args = ap.parse_args()
    args.kelpied = os.path.abspath(args.kelpied)
    out = os.path.abspath(args.out) if args.out else None
    if out:
        os.makedirs(out, exist_ok=True)
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
        d = ps.Daemon(args.kelpied, env={"KELPIE_LOAD_FORCE": "L0:calm", "KELPIE_E2E": "1"}).start()
        ph = Page(udid, out)
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
