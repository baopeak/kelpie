"""Closing Kelpie stops its session daemon (Settings → Session Daemon → When Kelpie closes), asking first while
something runs; "Keep it running in the background" leaves it; sessions stopped that way come back interrupted.

    python -X utf8 scripts/e2e/close_stops_daemon.py <scratch-dir> <app>

<app> is a built app binary with its frontend embedded (`pnpm tauri build --debug --no-bundle`) and `kelpied`
next to it. Windows only: the window is closed the way its close button does, with WM_CLOSE. Needs tauri-driver
(PATH or ~/.cargo/bin), msedgedriver matching WebView2 (MSEDGEDRIVER or PATH) and psutil.

Cases, in this order on one isolated endpoint and data folder (the app starts its own daemon each time, from
KELPIE_DAEMON_PATH):
  b. nothing runs: the close asks nothing, the app exits and the daemon stops;
  a. a shell runs: the close asks; Cancel keeps the app and the daemon; Close and stop stops both, and no new daemon
     starts;
  d. the next start shows that shell interrupted, not lost, and it resumes;
  c. "Keep it running in the background": the close leaves the daemon running (this script then stops it).

Everything runs isolated, set in this process before anything starts: its own endpoint (KELPIE_ENDPOINT) and data
folder (KELPIE_DATA_DIR) under <scratch-dir>. The user's daemon and data are never reached. The script stops only
the processes it started or that its own app started, and removes its folders and its endpoint's token file.
"""
import argparse
import ctypes
import importlib.util
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import traceback
import urllib.request
from ctypes import wintypes

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("scratch")
ap.add_argument("app")
args = ap.parse_args()

if os.name != "nt":
    raise SystemExit("Windows only: the window is closed with WM_CLOSE")

scratch = os.path.abspath(args.scratch)
app = os.path.abspath(args.app)
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe")
os.makedirs(scratch, exist_ok=True)
WORK = tempfile.mkdtemp(prefix="kelpie-close-", dir=scratch)
DATA = os.path.join(WORK, "data")
PROJ = os.path.join(WORK, "proj")
ENDPOINT = f"kelpie-close-e2e-{os.getpid()}"

os.environ.update({"KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_DAEMON_PATH": kelpied, "KELPIE_E2E": "1"})
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT", "KELPIE_ALLOW_REAL_DAEMON"):
    os.environ.pop(k, None)

from smoke_support import Run, daemon, native_driver, native_driver_hint, tauri_driver  # noqa: E402

try:
    import psutil
except ImportError:
    raise SystemExit("needs psutil (pip install psutil)")

_spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(HERE, "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(kc)

NATIVE, TAURI_DRIVER = native_driver(), tauri_driver()
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit(f"needs tauri-driver (PATH or ~/.cargo/bin) and {native_driver_hint()}")
for what, path in (("app", app), ("daemon next to the app", kelpied)):
    if not os.path.exists(path):
        raise SystemExit(f"no {what}: {path}")

user32 = ctypes.WinDLL("user32", use_last_error=True)
user32.EnumWindows.argtypes = [ctypes.c_void_p, wintypes.LPARAM]
user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
user32.IsWindowVisible.argtypes = [wintypes.HWND]
user32.GetWindow.argtypes = [wintypes.HWND, wintypes.UINT]
user32.GetWindow.restype = wintypes.HWND
user32.GetWindowTextLengthW.argtypes = [wintypes.HWND]
user32.GetWindowTextW.argtypes = [wintypes.HWND, wintypes.LPWSTR, ctypes.c_int]
user32.PostMessageW.argtypes = [wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM]
WM_CLOSE, GW_OWNER = 0x0010, 4
ENUM_PROC = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)


def app_windows(pid):
    """Visible top-level windows of process `pid` that have a title: (hwnd, title)."""
    found = []

    def each(hwnd, _):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd) and not user32.GetWindow(hwnd, GW_OWNER):
            n = user32.GetWindowTextLengthW(hwnd)
            buf = ctypes.create_unicode_buffer(n + 1)
            user32.GetWindowTextW(hwnd, buf, n + 1)
            if buf.value:
                found.append((hwnd, buf.value))
        return True

    user32.EnumWindows(ENUM_PROC(each), 0)
    return found


def close_window(pid):
    """What the window's close button does: WM_CLOSE to the app's main window."""
    wins = app_windows(pid)
    if not wins:
        raise RuntimeError(f"no window of the app (pid {pid})")
    hwnd = next((h for h, title in wins if "Kelpie" in title), wins[0][0])
    user32.PostMessageW(hwnd, WM_CLOSE, 0, 0)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


def until(fn, timeout=30, step=0.2):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(step)
    return None


def alive(pid):
    try:
        return psutil.Process(pid).is_running() and psutil.Process(pid).status() != psutil.STATUS_ZOMBIE
    except psutil.Error:
        return False


def pipe_answers():
    """Whether a daemon listens on this script's endpoint (never another one)."""
    assert os.environ["KELPIE_ENDPOINT"] == ENDPOINT
    try:
        kc.Conn().call("daemon.info")
        return True
    except (OSError, SystemExit):
        return False


DRIVER_PORT = free_port()
os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{DRIVER_PORT}"
driver_proc = None
daemon_pids = set()
app_pids = set()
results = []


def check(name, ok, detail=""):
    results.append((name, bool(ok), detail))
    print(("PASS " if ok else "FAIL ") + name + (f"  [{detail}]" if detail else ""), flush=True)


def launch(shots):
    """Starts the app through tauri-driver and waits until it has connected to a daemon of its own; returns
    (run, app pid, daemon pid)."""
    t0 = time.time()
    run = Run(shots, app=app)
    run.results = results
    target = os.path.normcase(os.path.realpath(app))

    def started():
        # The app runs under the driver (tauri-driver → msedgedriver → app): only this script's own processes.
        for p in psutil.Process(driver_proc.pid).children(recursive=True):
            try:
                if p.pid not in app_pids and os.path.normcase(os.path.realpath(p.exe())) == target and p.create_time() >= t0 - 2:
                    return p
            except psutil.Error:
                pass
        return None

    proc = until(started, 20)
    if proc is None:
        names = [f"{p.pid}:{p.name()}" for p in psutil.Process(driver_proc.pid).children(recursive=True)]
        raise RuntimeError(f"the app process was not found under the driver: {names}")
    app_pids.add(proc.pid)
    run.boot()
    dpid = daemon(run.d, "daemon.info")["pid"]
    daemon_pids.add(dpid)
    return run, proc.pid, dpid


def end_session(run):
    """Ends the WebDriver session of an app that may have exited already, without the client's long timeout."""
    try:
        req = urllib.request.Request(f"{run.d.url}/session/{run.d.sid}", method="DELETE")
        urllib.request.urlopen(req, timeout=15).read()
    except Exception:  # noqa: BLE001 - the app is gone: the session is too
        pass


def dialog_shown(d):
    try:
        return d.js("return !!document.querySelector('[data-close-dialog]')")
    except Exception:  # noqa: BLE001
        return False


def case_nothing_running(shots):
    run, app_pid, dpid = launch(shots)
    s = run.d.js("return window.__kelpie.store.state.settings")
    check("b: a new install stops the daemon on close and asks first", s.get("daemonOnClose") == "stop" and s.get("askBeforeStop") is True, json.dumps({k: s.get(k) for k in ("daemonOnClose", "askBeforeStop")}))
    live = [x for x in daemon(run.d, "session.list") if x["alive"]]
    check("b: nothing runs", not live, str(len(live)))
    close_window(app_pid)
    exited = until(lambda: not alive(app_pid), 20)
    check("b: the close asks nothing and the app exits", bool(exited))
    check("b: the daemon stops with it", bool(until(lambda: not alive(dpid), 20)), f"daemon pid {dpid}")
    time.sleep(3)
    check("b: no new daemon starts", not pipe_answers())
    end_session(run)


def case_running_shell(shots):
    run, app_pid, dpid = launch(shots)
    d = run.d
    # Left interrupted on the next start, so that start can be checked before anything resumes it.
    d.js("window.__kelpie.store.setSettings({ resumeSessions: 'off' })")
    pid = d.js("return window.__kelpie.store.addProject(arguments[0])", PROJ)
    d.js("window.__kelpie.store.switchProject(arguments[0]);", pid)
    info = d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.store.newSession(arguments[0], 'shell').then(done);", pid)
    sid = info["id"]
    up = until(lambda: any(x["id"] == sid and x["alive"] for x in daemon(d, "session.list")), 20)
    check("a: a shell session runs", bool(up), sid)
    d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.store.saveNow().then(() => done(true), () => done(false));")

    close_window(app_pid)
    shown = until(lambda: dialog_shown(d), 15)
    check("a: the close asks first", bool(shown))
    text = d.js("return document.querySelector('[data-close-dialog]')?.innerText || ''") if shown else ""
    check("a: …and names the running session", "1 session is running" in text, text[:300].replace("\n", " | "))
    run.shot("a-close-dialog")
    d.click('[data-action="close-cancel"]')
    gone = until(lambda: not dialog_shown(d), 5)
    time.sleep(2)
    check("a: Cancel closes the question and keeps the app", bool(gone) and alive(app_pid))
    check("a: …and the daemon with its session", alive(dpid) and any(x["id"] == sid and x["alive"] for x in daemon(d, "session.list")))

    close_window(app_pid)
    shown = until(lambda: dialog_shown(d), 15)
    check("a: closing again asks again", bool(shown))
    d.click('[data-action="close-stop"]')
    check("a: Close and stop exits the app", bool(until(lambda: not alive(app_pid), 20)))
    check("a: …and stops the daemon process", bool(until(lambda: not alive(dpid), 20)), f"daemon pid {dpid}")
    time.sleep(3)
    check("a: no new daemon starts", not pipe_answers())
    end_session(run)
    return sid


def case_relaunch(shots, sid):
    run, app_pid, dpid = launch(shots)
    d = run.d
    s = until(lambda: next((x for x in daemon(d, "session.list") if x["id"] == sid), None), 20)
    check("d: the next start has the shell session", s is not None, sid)
    check("d: …interrupted, not running and not lost", bool(s) and s.get("interrupted") is True and not s.get("alive"), json.dumps({k: (s or {}).get(k) for k in ("alive", "interrupted")}))
    d.js("window.__kelpie.store.resumeSession(arguments[0])", sid)
    back = until(lambda: any(x["id"] == sid and x["alive"] for x in daemon(d, "session.list")), 30)
    check("d: …and it resumes", bool(back))
    return run, app_pid, dpid


def case_keep(run, dpid, app_pid):
    d = run.d
    d.js("window.__kelpie.store.setSettings({ daemonOnClose: 'keep' })")
    d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.store.saveNow().then(() => done(true), () => done(false));")
    close_window(app_pid)
    exited = until(lambda: not alive(app_pid), 20)
    check("c: Keep running: the close asks nothing and the app exits", bool(exited) and not dialog_shown(d))
    time.sleep(3)
    check("c: …and the daemon keeps running", alive(dpid) and pipe_answers(), f"daemon pid {dpid}")
    end_session(run)


def stop_driver():
    if driver_proc is not None and driver_proc.poll() is None:
        subprocess.run(["taskkill", "/PID", str(driver_proc.pid), "/T", "/F"], capture_output=True)
        try:
            driver_proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            driver_proc.kill()


def teardown():
    """Apps first (one would start a daemon again), the driver, then the test daemons: gracefully on this script's
    endpoint, then by pid. Removes the test folder and this endpoint's token file."""
    for pid in app_pids:
        if alive(pid):
            subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"], capture_output=True)
    stop_driver()
    if pipe_answers():
        try:
            kc.Conn().call("daemon.shutdown", {"kill_sessions": True})
        except (Exception, SystemExit):  # noqa: BLE001 - the reply may race the daemon exiting
            pass
    for pid in daemon_pids:
        if until(lambda: not alive(pid), 10) is None:
            subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"], capture_output=True)
    try:
        os.remove(os.path.join(os.environ.get("LOCALAPPDATA", ""), "Kelpie", "auth", ENDPOINT + ".token"))
    except OSError:
        pass
    shutil.rmtree(WORK, ignore_errors=True)


def main():
    global driver_proc
    os.makedirs(PROJ)
    shots = os.path.join(scratch, "shots")
    try:
        driver_proc = subprocess.Popen([TAURI_DRIVER, "--port", str(DRIVER_PORT), "--native-port", str(free_port()), "--native-driver", NATIVE],
                                       env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if not until(port_open, 30):
            raise SystemExit("tauri-driver did not open its port")
        case_nothing_running(shots)
        sid = case_running_shell(shots)
        run, app_pid, dpid = case_relaunch(shots, sid)
        case_keep(run, dpid, app_pid)
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False)
    finally:
        teardown()
    failed = [r for r in results if not r[1]]
    print(json.dumps({"passed": len(results) - len(failed), "failed": len(failed)}))
    sys.exit(1 if failed or not results else 0)


if __name__ == "__main__":
    main()
