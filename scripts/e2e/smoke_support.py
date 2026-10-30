"""Shared helpers for the WebDriver e2e scripts."""
import json
import os
import shutil
import subprocess
import sys

from wd import Driver

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
LINUX = sys.platform.startswith("linux")


def native_driver():
    """The browser driver tauri-driver hands the app to: WebKitWebDriver (Debian/Ubuntu package
    `webkit2gtk-driver`; WEBKITWEBDRIVER or PATH) on Linux, else msedgedriver matching the installed
    WebView2 (MSEDGEDRIVER or PATH)."""
    if LINUX:
        return os.environ.get("WEBKITWEBDRIVER") or shutil.which("WebKitWebDriver")
    return os.environ.get("MSEDGEDRIVER") or shutil.which("msedgedriver")


def native_driver_hint():
    if LINUX:
        return "WebKitWebDriver (apt install webkit2gtk-driver; PATH or WEBKITWEBDRIVER)"
    return "msedgedriver (PATH or MSEDGEDRIVER)"


def tauri_driver():
    return shutil.which("tauri-driver") or shutil.which(os.path.join(os.path.expanduser("~"), ".cargo", "bin", "tauri-driver"))


def real_data_dir():
    """The user's own Kelpie data folder (kelpie_proto::data_dir without KELPIE_DATA_DIR)."""
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA", "")
    elif sys.platform == "darwin":
        base = os.path.join(os.path.expanduser("~"), "Library", "Application Support")
    else:
        xdg = os.environ.get("XDG_DATA_HOME", "")
        base = xdg if os.path.isabs(xdg) else os.path.join(os.path.expanduser("~"), ".local", "share")
    return os.path.join(base, "Kelpie")


def default_app():
    # KELPIE_E2E_APP: test another build, e.g. the release executable that goes into the installer
    # (start tauri-driver with KELPIE_E2E=1 so it exposes its internals to the tests).
    if os.environ.get("KELPIE_E2E_APP"):
        return os.environ["KELPIE_E2E_APP"]
    meta = json.loads(subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=REPO))
    return os.path.join(meta["target_directory"], "debug", "kelpie.exe" if os.name == "nt" else "kelpie")


def daemon(d, method, params=None):
    r = d.invoke("daemon_call", {"method": method, "params": params or {}})
    if "err" in r:
        raise RuntimeError(r["err"])
    return r["ok"]


def restart_daemon(d, kelpied, kill=False, env=None):
    """Stops the daemon (its sessions keep running unless `kill`) and waits for a new one: the app
    starts it itself when KELPIE_DAEMON_PATH is set, otherwise this starts `kelpied serve` with `env`
    (this process's environment when none is given: callers that must not reach a daemon of the user's
    pass an explicit, checked one)."""
    import time
    old = daemon(d, "daemon.info")["pid"]
    try:
        daemon(d, "daemon.shutdown", {"kill_sessions": kill})
    except Exception:
        pass  # the reply may race the daemon exiting

    def new_pid():
        try:
            return d.js("return window.__kelpie.store.connected()") and daemon(d, "daemon.info")["pid"] != old
        except Exception:
            return False

    def wait(seconds):
        end = time.time() + seconds
        while time.time() < end and not new_pid():
            time.sleep(0.5)
        return new_pid()

    if not wait(8):
        flags = 0x08000000 | 0x00000008 if os.name == "nt" else 0  # CREATE_NO_WINDOW | DETACHED_PROCESS
        subprocess.Popen([kelpied, "serve"], env=env if env is not None else os.environ.copy(), creationflags=flags, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        if not wait(40):
            raise TimeoutError("no new session daemon")


class Run:
    def __init__(self, shots, app=None):
        self.shots = shots
        os.makedirs(shots, exist_ok=True)
        self.results = []
        self.d = Driver(app or default_app())

    def boot(self, tips=False):
        self.d.wait("return !!(window.__kelpie && window.__kelpie.store.loaded())", 60)
        self.d.wait("return window.__kelpie.store.connected()", 30)
        # Scenarios close sessions and remove projects: never against the user's real data.
        data_dir = self.d.js_async("const done = arguments[arguments.length-1]; window.__TAURI_INTERNALS__.invoke('platform_info').then(i => done(i.data_dir));")
        # No KELPIE_DATA_DIR bypass: this process's environment says nothing about the app's.
        real = real_data_dir()
        if os.path.normcase(os.path.normpath(data_dir)) == os.path.normcase(os.path.normpath(real)):
            raise SystemExit(f"refusing to run e2e against the real data folder {data_dir}; start tauri-driver with KELPIE_DATA_DIR set")
        # First-run tips would float over whatever a scenario clicks.
        if not tips:
            self.d.js("window.__kelpie.store.setSettings({ tips: false })")
        # Scenarios read English labels; the OS may well prefer Vietnamese.
        self.d.js("window.__kelpie.store.setSettings({ language: 'en' })")

    def check(self, name, ok, detail=""):
        self.results.append((name, bool(ok), detail))
        print(("PASS " if ok else "FAIL ") + name + (f"  [{detail}]" if detail else ""), flush=True)

    def shot(self, name):
        self.d.screenshot(os.path.join(self.shots, name + ".png"))

    def finish(self):
        self.d.quit()
        failed = [r for r in self.results if not r[1]]
        print(json.dumps({"passed": len(self.results) - len(failed), "failed": len(failed)}))
        sys.exit(1 if failed else 0)
