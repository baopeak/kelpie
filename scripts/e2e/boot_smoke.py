"""Minimal GUI smoke of an app build: boots, opens a shell, types `echo hello`, sees the output, closes.
Records cold start and idle CPU / memory as numbers to compare with this platform's own baseline.

    python scripts/e2e/boot_smoke.py <scratch-dir> <app> [--out result.json] [--baseline baseline.json] [--idle 30]

<app> is a built app binary (`target/release/kelpie` from scripts/build-release.sh, or a debug build with its
frontend embedded) with `kelpied` next to it. Needs `tauri-driver` (PATH or ~/.cargo/bin) and the platform's
native driver: WebKitWebDriver on Linux (`apt install webkit2gtk-driver`; run under Xvfb on a headless machine,
e.g. `xvfb-run -a`), msedgedriver matching WebView2 on Windows. `pip install psutil` adds the cold start and idle
numbers (the check still runs without it).

Everything runs isolated, set in this process before anything starts: its own daemon on its own endpoint
(KELPIE_ENDPOINT) and data folder (KELPIE_DATA_DIR) under <scratch-dir>; on Linux the XDG data, config and cache
folders too (the WebKit profile and the daemon's auth token land there). The user's daemon, data and keyring are
never reached. The script stops only what it started and removes its folders.

The baseline is informational: the numbers are printed next to it, never gated (budgets come later, relative
to this platform's own history).
"""
import argparse
import importlib.util
import json
import os
import platform
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
ap.add_argument("scratch")
ap.add_argument("app")
ap.add_argument("--out", help="write the result (checks and numbers) as JSON here")
ap.add_argument("--baseline", help="print the numbers next to this baseline (never fails on it)")
ap.add_argument("--idle", type=float, default=30, help="seconds of idle CPU / memory sampling")
args = ap.parse_args()

scratch = os.path.abspath(args.scratch)
app = os.path.abspath(args.app)
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe" if os.name == "nt" else "kelpied")
os.makedirs(scratch, exist_ok=True)
WORK = tempfile.mkdtemp(prefix="kelpie-boot-smoke-", dir=scratch)
DATA = os.path.join(WORK, "data")
PROJ = os.path.join(WORK, "proj")
# A Unix socket path must stay short (108 bytes on Linux): the temp folder, not the scratch folder.
ENDPOINT = f"kelpie-boot-smoke-{os.getpid()}" if os.name == "nt" else os.path.join(tempfile.gettempdir(), f"kelpie-boot-smoke-{os.getpid()}.sock")

os.environ.update({
    "KELPIE_ENDPOINT": ENDPOINT, "KELPIE_DATA_DIR": DATA, "KELPIE_DAEMON_PATH": kelpied, "KELPIE_E2E": "1",
})
if os.name != "nt":
    for var, sub in (("XDG_DATA_HOME", "xdg-data"), ("XDG_CONFIG_HOME", "xdg-config"), ("XDG_CACHE_HOME", "xdg-cache")):
        os.environ[var] = os.path.join(WORK, sub)
for k in ("KELPIE_SESSION_ID", "KELPIE_SESSION_TOKEN", "KELPIE_SESSION_ENDPOINT", "KELPIE_ALLOW_REAL_DAEMON"):
    os.environ.pop(k, None)

from smoke_support import Run, daemon, native_driver, native_driver_hint, tauri_driver  # noqa: E402

_spec = importlib.util.spec_from_file_location("kelpie_call", os.path.join(HERE, "..", "kelpie-call.py"))
kc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(kc)

NATIVE, TAURI_DRIVER = native_driver(), tauri_driver()
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit(f"needs tauri-driver (PATH or ~/.cargo/bin) and {native_driver_hint()}")
for what, path in (("app", app), ("daemon next to the app", kelpied)):
    if not os.path.exists(path):
        raise SystemExit(f"no {what}: {path}")
try:
    import psutil
except ImportError:
    psutil = None
    print("psutil is not installed: no cold start / idle numbers", flush=True)


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def until(fn, timeout=30, step=0.2):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(step)
    return None


DRIVER_PORT = free_port()
os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{DRIVER_PORT}"
procs = {}
numbers = {}


def spawn(name, argv):
    # Own process group on Unix, so the driver's WebKitWebDriver goes down with it.
    extra = {"start_new_session": True} if os.name != "nt" else {}
    procs[name] = subprocess.Popen(argv, env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, **extra)


def killpg(pgid, sig):
    try:
        os.killpg(pgid, sig)
        return True
    except OSError:  # the group is gone
        return False


def stop(name):
    """Stops what this script started as `name`, with the processes it started (its own process group on Unix:
    the group is signalled even when its leader already exited, so a WebKitWebDriver or app left in it goes too)."""
    p = procs.get(name)
    if p is None:
        return
    if os.name == "nt":
        if p.poll() is None:
            subprocess.run(["taskkill", "/PID", str(p.pid), "/T", "/F"], capture_output=True)
    else:
        killpg(p.pid, signal.SIGTERM)
    try:
        p.wait(timeout=10)
    except subprocess.TimeoutExpired:
        if os.name == "nt":
            p.kill()
    if os.name != "nt":
        # Whatever ignored SIGTERM, leader or not.
        end = time.time() + 5
        while killpg(p.pid, 0) and time.time() < end:
            time.sleep(0.2)
        killpg(p.pid, signal.SIGKILL)
    try:
        p.wait(timeout=5)
    except subprocess.TimeoutExpired:
        print(f"{name} (pid {p.pid}) did not stop", flush=True)


def port_open():
    with socket.socket() as s:
        return s.connect_ex(("127.0.0.1", DRIVER_PORT)) == 0


def daemon_up():
    if os.name == "nt":
        return procs["daemon"].poll() is None and os.path.exists(os.path.join(DATA, "kelpied.pid"))
    return os.path.exists(ENDPOINT)


def cpu_seconds(ps):
    total = 0.0
    for p in ps:
        try:
            t = p.cpu_times()
            total += t.user + t.system
        except Exception:  # noqa: BLE001 - exited while sampling
            pass
    return total


def rss_mb(ps):
    total = 0
    for p in ps:
        try:
            total += p.memory_info().rss
        except Exception:  # noqa: BLE001
            pass
    return total / 2**20


def scenario(run, t_launch):
    d = run.d
    app_proc = None
    if psutil:
        same = lambda p: p.info["exe"] and os.path.realpath(p.info["exe"]) == os.path.realpath(app)  # noqa: E731
        app_proc = until(lambda: next((p for p in psutil.process_iter(["exe", "create_time"]) if same(p) and p.info["create_time"] >= t_launch - 2), None), 10)
    run.boot()
    run.check("the app boots and connects to its own daemon", True)
    info = daemon(d, "daemon.info")
    run.check("…the isolated one", info.get("pid") == procs["daemon"].pid, f"daemon pid {info.get('pid')}, started {procs['daemon'].pid}")
    if app_proc is not None:
        d.wait("return window.__kelpie.perf && window.__kelpie.perf.interactiveAt() !== null", 60, 0.1)
        numbers["cold_start_ms"] = round(d.js("return window.__kelpie.perf.interactiveAt()") - app_proc.create_time() * 1000, 1)

    pid = d.js("return window.__kelpie.store.addProject(arguments[0])", PROJ)
    d.js("window.__kelpie.store.switchProject(arguments[0]);", pid)
    handle = d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.store.newSession(arguments[0], 'shell').then(i => done(i.handle));", pid)
    run.check("a shell session opens", handle is not None, str(handle))
    rendered = until(lambda: d.js("return document.querySelectorAll('.pane .xterm-screen').length > 0"), 20)
    run.check("its terminal renders", bool(rendered))
    screen = lambda: daemon(d, "session.read", {"handle": handle})["text"]  # noqa: E731
    # The screen text comes with trailing spaces trimmed: bash's "user@host:~$ " reads as "...$".
    prompt = until(lambda: any(line.rstrip().endswith(("$", "%", "#", ">")) for line in screen().splitlines()), 60, 0.3)
    run.check("the shell prints its prompt", bool(prompt), repr(screen()[-200:]))

    # Real keyboard events through the terminal view.
    d.click(".pane .xterm")
    time.sleep(1)
    d.keys("echo hello", "enter")
    # The output line itself, not the echoed command.
    out = until(lambda: "hello" in [line.strip() for line in screen().splitlines()], 15, 0.25)
    run.check("typed `echo hello` and the shell printed hello", bool(out), repr(screen()[-300:]))
    view_text = lambda: d.js(  # noqa: E731 - the mounted xterm buffers (WebGL draws to a canvas)
        "const out = []; for (const v of window.__kelpie.views()) { if (!v.visible) continue; const b = v.term.buffer.active;"
        " for (let i = 0; i < b.length; i++) out.push(b.getLine(i)?.translateToString(true) ?? ''); } return out.join('\\n');"
    ) or ""
    shown = until(lambda: "hello" in [line.strip() for line in view_text().splitlines()], 10, 0.2)
    run.check("…and the terminal view shows it", bool(shown))
    time.sleep(1)  # software rendering under Xvfb draws the canvas a frame or two after the buffer
    run.shot("01-echo-hello")

    if app_proc is not None:
        daemon_proc = psutil.Process(procs["daemon"].pid)

        def group():
            # The app and its web processes, and the daemon; the shell under the daemon is not the app's cost.
            shells = {p.pid for p in daemon_proc.children(recursive=True)}
            return [app_proc, daemon_proc] + [p for p in app_proc.children(recursive=True) if p.pid not in shells and p.pid != daemon_proc.pid]

        time.sleep(3)  # let the screen settle after the echo
        ps = group()
        cpu0, t0, rss = cpu_seconds(ps), time.time(), []
        while time.time() - t0 < args.idle:
            time.sleep(min(2.0, max(0.0, args.idle - (time.time() - t0))))
            rss.append(rss_mb(group()))
        numbers["idle_cpu_pct"] = round((cpu_seconds(ps) - cpu0) / (time.time() - t0) * 100, 2)
        numbers["rss_mb"] = round(sorted(rss)[len(rss) // 2], 1)
        numbers["idle_seconds"] = args.idle

    daemon(d, "session.close", {"handle": handle})
    gone = until(lambda: not any(s["handle"] == handle for s in daemon(d, "session.list")), 10)
    run.check("the session closes", bool(gone))


def compare(baseline_path):
    try:
        with open(baseline_path, encoding="utf-8") as f:
            base = json.load(f).get("metrics", {})
    except (OSError, ValueError) as e:
        print(f"baseline {baseline_path}: {e}", flush=True)
        return
    for k, v in numbers.items():
        b = base.get(k)
        if isinstance(b, (int, float)) and b and k != "idle_seconds":
            print(f"  {k}: {v} (baseline {b}, x{v / b:.2f}; not gated)", flush=True)


def teardown(run):
    """The app first (it would start a daemon of its own if the test daemon went away), then the driver, then
    the test daemon; removes the test folders."""
    if run is not None:
        try:
            run.d.quit()
        except Exception:  # noqa: BLE001
            pass
    stop("driver")
    if "daemon" in procs:
        assert os.environ["KELPIE_ENDPOINT"] == ENDPOINT
        try:
            kc.Conn().call("daemon.shutdown", {"kill_sessions": True})
        except (Exception, SystemExit):  # noqa: BLE001 - the reply may race the daemon exiting
            pass
        try:
            procs["daemon"].wait(timeout=10)
        except subprocess.TimeoutExpired:
            stop("daemon")
    if os.name == "nt":
        try:
            os.remove(os.path.join(os.environ.get("LOCALAPPDATA", ""), "Kelpie", "auth", ENDPOINT + ".token"))
        except OSError:
            pass
    else:
        try:
            os.remove(ENDPOINT)
        except OSError:
            pass
    shutil.rmtree(WORK, ignore_errors=True)


def main():
    os.makedirs(PROJ)
    shots = os.path.join(scratch, "shots")
    spawn("daemon", [kelpied, "serve"])
    run = None
    try:
        if not until(daemon_up, 20, 0.1):
            raise SystemExit("the test daemon did not start")
        spawn("driver", [TAURI_DRIVER, "--port", str(DRIVER_PORT), "--native-port", str(free_port()), "--native-driver", NATIVE])
        if not until(port_open, 30):
            raise SystemExit("tauri-driver did not open its port")
        t_launch = time.time()
        run = Run(shots, app=app)
        scenario(run, t_launch)
    except SystemExit:
        raise
    except Exception:
        traceback.print_exc()
        if run is None:
            raise SystemExit("could not start the app")
        run.check("scenario ran to completion", False)
    finally:
        results = list(run.results) if run is not None else []
        teardown(run)
    failed = [r for r in results if not r[1]]
    if numbers:
        print("numbers: " + json.dumps(numbers), flush=True)
        if args.baseline:
            compare(args.baseline)
    if args.out:
        with open(args.out, "w", encoding="utf-8") as f:
            json.dump({
                "platform": platform.platform(), "machine": platform.machine(), "cores": os.cpu_count(),
                "checks": [{"name": n, "ok": ok, "detail": det} for n, ok, det in results],
                "metrics": numbers,
            }, f, indent=2)
    print(json.dumps({"passed": len(results) - len(failed), "failed": len(failed)}))
    sys.exit(1 if failed or not results else 0)


if __name__ == "__main__":
    main()
