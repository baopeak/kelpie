"""Performance gate for Kelpie: measures the release build and compares it with a baseline.

    python scripts/perf/perf_gate.py --label phase-01 --compare scripts/perf/baseline.json
    python scripts/perf/perf_gate.py --label baseline --update-baseline --reason "first baseline"
    python scripts/perf/perf_gate.py --label ci --metrics bundle --compare scripts/perf/baseline.json

Metric groups (--metrics, default all):
  bundle  `vite build` output: entry chunk gzip size, lazy chunks.            (no GUI)
  bench   `kelpied bench --sessions 20 --mb 2` wall time, isolated daemon.     (no GUI)
  gui     the release app driven through WebDriver, timed inside the page:
          cold start, project switch, keystroke echo, long tasks, idle CPU and RSS with 20 shells.
          Reported beside them, never budgeted: the start-up steps, the RSS of each process kind
          (app, daemon, WebView2 browser / GPU / renderer / other) and, on Windows, the RSS while
          the window is minimised and the first frame after it is restored.

The GUI group needs the release app (scripts/build-release.sh --no-bundle), `tauri-driver`, a
`msedgedriver` matching the WebView2 runtime, and `pip install psutil`; see scripts/perf/README.md.
Every run uses its own daemon (KELPIE_ENDPOINT / KELPIE_DATA_DIR) and data folder, never the user's.

Writes plans/reports/perf-<label>.json. Exit status 1 when a budget in the comparison is exceeded.
"""
import argparse
import datetime
import gzip
import json
import os
import platform
import re
import shutil
import socket
import statistics
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
sys.path.insert(0, os.path.join(REPO, "scripts", "e2e"))

EXE = ".exe" if os.name == "nt" else ""

# name -> (unit, kind, limit). ratio: value <= baseline * limit; plus: value <= baseline + limit.
BUDGETS = {
    "cold_start_ms": ("ms", "ratio", 1.05),
    "switch_median_ms": ("ms", "ratio", 1.10),
    "echo_median_ms": ("ms", "ratio", 1.10),
    "idle_cpu_pct": ("% of one core", "plus", 1.0),
    "rss_mb": ("MB", "plus", 30.0),
    "entry_gz_kb": ("KB", "plus", 15.0),
    "long_tasks": ("count", "plus", 0.0),
    "bench_wall_s": ("s", "ratio", 1.05),
}
# Reported next to the budgets; they only warn (tail latency is too noisy to fail on).
WARNINGS = {"switch_p95_ms": ("ms", 52.0, 1.10), "echo_p95_ms": ("ms", None, 1.10)}
GROUPS = {
    "bundle": ["entry_gz_kb"],
    "bench": ["bench_wall_s"],
    "gui": ["cold_start_ms", "switch_median_ms", "echo_median_ms", "long_tasks", "idle_cpu_pct", "rss_mb"],
}


def median(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def pct(xs, p):
    xs = sorted(xs)
    return xs[round((len(xs) - 1) * p)] if xs else None


def r2(x):
    return None if x is None else round(x, 2)


def log(msg):
    print(msg, flush=True)


# ---------- environment ----------

def target_dir():
    meta = json.loads(subprocess.check_output(["cargo", "metadata", "--format-version", "1", "--no-deps"], cwd=REPO))
    return meta["target_directory"]


def machine_description():
    cpu = platform.processor()
    if os.name == "nt":
        try:
            import winreg
            with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0") as k:
                cpu = winreg.QueryValueEx(k, "ProcessorNameString")[0].strip()
        except OSError:
            pass
    info = {"os": platform.platform(), "cpu": cpu, "python": platform.python_version()}
    try:
        import psutil
        info["cores"] = f"{psutil.cpu_count(logical=False)} physical / {psutil.cpu_count()} logical"
        info["ram_gb"] = round(psutil.virtual_memory().total / 2**30, 1)
    except ImportError:
        pass
    try:
        info["commit"] = subprocess.check_output(["git", "rev-parse", "--short", "HEAD"], cwd=REPO, text=True).strip()
    except (OSError, subprocess.CalledProcessError):
        pass
    return info


# ---------- bundle ----------

ANSI = re.compile(r"\x1b\[[0-9;]*m")
CHUNK = re.compile(r"(?P<file>[^\s/\\]+\.js)\s+(?P<size>[\d,.]+)\s*kB\s*\S{1,3}\s*gzip:\s*(?P<gz>[\d,.]+)\s*kB")


def measure_bundle():
    """Builds the frontend into a scratch folder and reads Vite's own size report."""
    pnpm = shutil.which("pnpm")
    if not pnpm:
        raise SystemExit("pnpm not found")
    out = tempfile.mkdtemp(prefix="kelpie-perf-dist-")
    try:
        env = {**os.environ, "NO_COLOR": "1", "FORCE_COLOR": "0"}
        log("bundle: vite build")
        res = subprocess.run([pnpm, "exec", "vite", "build", "--outDir", out, "--emptyOutDir"], cwd=os.path.join(REPO, "app"), env=env, capture_output=True, text=True, encoding="utf-8", errors="replace")
        text = ANSI.sub("", res.stdout + res.stderr)
        if res.returncode != 0:
            raise SystemExit("vite build failed:\n" + text[-2000:])
        chunks = {m["file"]: float(m["gz"].replace(",", "")) for m in CHUNK.finditer(text)}
        html = open(os.path.join(out, "index.html"), encoding="utf-8").read()
        entry = re.search(r'<script[^>]+type="module"[^>]+src="[^"]*?([^/"]+\.js)"', html)
        if not entry:
            raise SystemExit("no entry script in the built index.html")
        entry = entry.group(1)
        if entry not in chunks:  # report format changed: measure the file the way Vite does
            data = open(os.path.join(out, "assets", entry), "rb").read()
            chunks[entry] = len(gzip.compress(data, 9)) / 1000
            log(f"bundle: {entry} not in vite's report, gzipped it directly")
        lazy = {k: v for k, v in chunks.items() if k != entry}
        return {
            "entry_gz_kb": r2(chunks[entry]),
            "entry_file": entry,
            "lazy_chunks": len(lazy),
            "lazy_gz_kb": r2(sum(lazy.values())),
            "largest_lazy": {k: v for k, v in sorted(lazy.items(), key=lambda kv: -kv[1])[:5]},
        }
    finally:
        shutil.rmtree(out, ignore_errors=True)


# ---------- daemon bench ----------

def kelpied_path(args):
    return args.kelpied or os.environ.get("KELPIE_DAEMON_PATH") or os.path.join(target_dir(), "release", "kelpied" + EXE)


def stop_tree(proc):
    """Terminates a process this script started, with its children, politely first."""
    if proc is None or proc.poll() is not None:
        return
    try:
        import psutil
        kids = psutil.Process(proc.pid).children(recursive=True)
    except Exception:  # noqa: BLE001 - psutil missing or the process already gone
        kids = []
    proc.terminate()
    try:
        proc.wait(8)
    except subprocess.TimeoutExpired:
        proc.kill()
    for k in kids:
        try:
            k.kill()
        except Exception:  # noqa: BLE001
            pass


def measure_bench(args, sessions=20, mb=2):
    kelpied = kelpied_path(args)
    if not os.path.exists(kelpied):
        raise SystemExit(f"{kelpied} not found: cargo build --release -p kelpied")
    data = tempfile.mkdtemp(prefix="kelpie-perf-bench-")
    env = {**os.environ, "KELPIE_ENDPOINT": f"kelpie-perf-bench-{os.getpid()}", "KELPIE_DATA_DIR": data}
    daemon = subprocess.Popen([kelpied, "serve"], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        # A one-session run both waits for the daemon and warms the ConPTY host.
        end = time.time() + 30
        while True:
            warm = subprocess.run([kelpied, "bench", "--sessions", "1", "--mb", "1", "--attach", "1"], env=env, capture_output=True, text=True)
            if warm.returncode == 0:
                break
            if time.time() > end:
                raise SystemExit("daemon did not come up for the bench:\n" + warm.stdout + warm.stderr)
            time.sleep(0.5)
        walls, outputs = [], []
        for i in range(args.runs):
            log(f"bench {i + 1}/{args.runs}: kelpied bench --sessions {sessions} --mb {mb}")
            t0 = time.perf_counter()
            res = subprocess.run([kelpied, "bench", "--sessions", str(sessions), "--mb", str(mb)], env=env, capture_output=True, text=True)
            walls.append(time.perf_counter() - t0)
            if res.returncode != 0:
                raise SystemExit("kelpied bench failed (stream integrity?):\n" + res.stdout + res.stderr)
            outputs.append(res.stdout)
        mibs = [float(m.group(1)) for o in outputs if (m := re.search(r"\(([\d.]+) MiB/s\)", o))]
        return {"bench_wall_s": r2(median(walls)), "bench_runs_s": [r2(w) for w in walls], "bench_mib_s": r2(median(mibs))}
    finally:
        stop_tree(daemon)
        shutil.rmtree(data, ignore_errors=True)


# ---------- GUI scenario ----------

PROMPTS = ("PS ", "$ ", "% ", "> ")


def echo_text(n):
    """Pseudo-random letters whose every 6-character window is unique, so a stale echo never matches."""
    import random
    rnd = random.Random(7)
    s = ""
    while len(s) < n:
        c = rnd.choice("abcdefghijklmnopqrstuvwxyz")
        if (s + c)[-6:] not in s:
            s += c
    return s


def wait_port(port, timeout=20):
    end = time.time() + timeout
    while time.time() < end:
        with socket.socket() as s:
            s.settimeout(0.5)
            if s.connect_ex(("127.0.0.1", port)) == 0:
                return
        time.sleep(0.2)
    raise SystemExit(f"tauri-driver did not listen on port {port}")


def port_free(port):
    with socket.socket() as s:
        s.settimeout(0.5)
        return s.connect_ex(("127.0.0.1", port)) != 0


def free_port():
    """A port nothing listens on right now, chosen by the system."""
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def sum_cpu(procs):
    out = {}
    for p in procs:
        try:
            t = p.cpu_times()
            out[(p.pid, p.create_time())] = t.user + t.system
        except Exception:  # noqa: BLE001 - exited while sampling
            pass
    return out


def sum_rss(procs):
    total = 0
    for p in procs:
        try:
            total += p.memory_info().rss
        except Exception:  # noqa: BLE001
            pass
    return total / 2**20


def sum_private(procs):
    """Private bytes in MB (Windows; None elsewhere): memory no other process shares."""
    total = 0
    for p in procs:
        try:
            private = getattr(p.memory_info(), "private", None)
        except Exception:  # noqa: BLE001
            continue
        if private is None:
            return None
        total += private
    return total / 2**20


_webview_kinds = {}


def webview_kind(p):
    """A WebView2 process's role from its command line: browser (no --type), gpu, renderer or other."""
    key = (p.pid, p.create_time())
    if key not in _webview_kinds:
        try:
            m = re.search(r"--type=([\w-]+)", " ".join(p.cmdline()))
        except Exception:  # noqa: BLE001 - exited while sampling
            return "other"
        _webview_kinds[key] = "browser" if not m else {"gpu-process": "gpu", "renderer": "renderer"}.get(m.group(1), "other")
    return _webview_kinds[key]


def is_webview(p):
    try:
        return p.name().lower().startswith("msedgewebview2")
    except Exception:  # noqa: BLE001 - exited while sampling
        return False


def memory_sample(app_proc, daemon_proc, webview):
    """Working set in MB of each part of the budget, their total and the total private bytes.
    `webview` is every other child of the app: WebView2's processes by role, anything else (git,
    the speech helper, a console host) with the app."""
    parts = {"app": [app_proc], "daemon": [daemon_proc], "webview_browser": [], "webview_gpu": [], "webview_renderer": [], "webview_other": []}
    for p in webview:
        parts["webview_" + webview_kind(p) if is_webview(p) else "app"].append(p)
    out = {f"rss_{name}_mb": sum_rss(procs) for name, procs in parts.items()}
    out["rss_mb"] = sum(out.values())
    out["private_mb"] = sum_private([app_proc, daemon_proc] + webview)
    return out


def median_sample(samples, suffix=""):
    """Per key, the median over memory samples (keys renamed `<name><suffix>_mb`); None values dropped."""
    out = {}
    for key in samples[0] if samples else []:
        vals = [s[key] for s in samples if s[key] is not None]
        if vals:
            out[key[:-3] + suffix + "_mb"] = r2(median(vals))
    return out


def main_window(pid):
    """The visible top-level window of process `pid` that no other window owns (the app's main window)."""
    import ctypes
    from ctypes import wintypes
    user32 = ctypes.windll.user32
    user32.GetWindow.restype = wintypes.HWND
    user32.GetWindow.argtypes = [wintypes.HWND, wintypes.UINT]
    user32.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    user32.IsWindowVisible.argtypes = [wintypes.HWND]
    user32.ShowWindow.argtypes = [wintypes.HWND, ctypes.c_int]
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def visit(hwnd, _):
        owner = wintypes.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(owner))
        if owner.value == pid and user32.IsWindowVisible(hwnd) and not user32.GetWindow(hwnd, 4):  # GW_OWNER
            found.append(hwnd)
        return True

    user32.EnumWindows(visit, 0)
    return found[0] if found else None


def gui_run(args, index):
    """One launch of the release app: returns this run's metrics."""
    import psutil
    from smoke_support import Run, daemon

    app = os.path.abspath(args.app or os.environ.get("KELPIE_E2E_APP") or os.path.join(target_dir(), "release", "kelpie" + EXE))
    kelpied = kelpied_path(args)
    native = args.msedgedriver or shutil.which("msedgedriver")
    tauri_driver = shutil.which("tauri-driver")
    for what, path in (("release app (scripts/build-release.sh --no-bundle)", app), ("kelpied", kelpied), ("msedgedriver (--msedgedriver)", native), ("tauri-driver (cargo install tauri-driver)", tauri_driver)):
        if not path or not os.path.exists(path):
            raise SystemExit(f"missing {what}: {path}")
    # The previous run's tauri-driver may still be letting its port go.
    end = time.time() + 15
    while time.time() < end and not port_free(args.driver_port):
        time.sleep(0.25)
    if not port_free(args.driver_port):
        raise SystemExit(f"port {args.driver_port} is in use: stop the other tauri-driver (netstat -ano) or pass --driver-port")
    # tauri-driver's own msedgedriver gets a fresh free port each run: the default 4445 is shared with
    # e2e runs of other worktrees, and the previous run's port can stay unbindable for a while after
    # its msedgedriver is stopped.
    native_port = free_port()

    data = tempfile.mkdtemp(prefix="kelpie-perf-data-")
    projects_dir = tempfile.mkdtemp(prefix="kelpie-perf-proj-")
    env = {
        **os.environ, "KELPIE_E2E": "1", "KELPIE_DAEMON_PATH": kelpied, "KELPIE_DATA_DIR": data,
        "KELPIE_ENDPOINT": f"kelpie-perf-{os.getpid()}-{index}",
    }
    os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{args.driver_port}"
    driver_proc = subprocess.Popen([tauri_driver, "--port", str(args.driver_port), "--native-port", str(native_port), "--native-driver", native], env=env)
    run = None
    daemon_pid = None
    try:
        wait_port(args.driver_port)
        t_before = time.time()
        run = Run(os.path.join(data, "shots"), app)
        d = run.d
        app_proc = next((p for p in psutil.process_iter(["exe", "create_time"]) if p.info["exe"] and os.path.normcase(p.info["exe"]) == os.path.normcase(app) and p.info["create_time"] >= t_before - 2), None)
        if not app_proc:
            raise SystemExit("could not find the app process")
        run.boot()
        d._req("POST", d.s("/timeouts"), {"script": 900_000})
        d.wait("return window.__kelpie.perf && window.__kelpie.perf.interactiveAt() !== null", 60, 0.1)
        cold = d.js("return window.__kelpie.perf.interactiveAt()") / 1000 - app_proc.create_time()
        # Where the start-up time goes, in ms from the process's creation: WebView2 starts the page,
        # the HTML arrives, the entry module runs, the fonts are in, the saved state is applied.
        startup = {}
        steps = d.js("return window.__kelpie.perf.startup ? window.__kelpie.perf.startup() : null")
        if steps:
            page_start = steps["timeOrigin"] - app_proc.create_time() * 1000
            startup["start_webview_ms"] = r2(page_start)
            for step in ("html", "script", "fonts", "state"):
                if steps[step] is not None:
                    startup[f"start_{step}_ms"] = r2(page_start + steps[step])
        log(f"run {index + 1}: cold start {cold * 1000:.0f} ms ({', '.join(f'{k[6:-3]} {v:.0f}' for k, v in startup.items())})")
        daemon_pid = daemon(d, "daemon.info")["pid"]

        # Three projects, the sessions spread over them.
        projects = [REPO] + [os.path.join(projects_dir, f"p{i}") for i in (1, 2)]
        for p in projects[1:]:
            os.makedirs(p)
        d.js("const s = window.__kelpie.store; for (const p of [...s.state.projects]) s.removeProject(p.id);")
        for p in projects:
            d.js("window.__kelpie.store.addProject(arguments[0]);", p)
        ids = d.js("return window.__kelpie.store.state.projects.map(p => p.id);")
        if len(ids) != 3:
            raise SystemExit(f"expected 3 projects, got {len(ids)}")
        handles = []
        for i in range(args.sessions):
            pid = ids[i % 3]
            d.js("window.__kelpie.store.switchProject(arguments[0]);", pid)
            info = d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.store.newSession(arguments[0], 'shell').then(done);", pid)
            handles.append(info["handle"])
        end = time.time() + 120
        while not all(any(m in daemon(d, "session.read", {"handle": h})["text"] for m in PROMPTS) for h in handles):
            if time.time() > end:
                raise SystemExit("shells did not print their prompt")
            time.sleep(0.5)
        d.js("window.__kelpie.store.switchProject(arguments[0]);", ids[0])
        log(f"run {index + 1}: {len(handles)} shells ready, settling")
        time.sleep(args.settle)

        # Idle: nothing happens for a minute; app, WebView2 and daemon budgeted, shells reported.
        daemon_proc = psutil.Process(daemon_pid)

        def groups():
            # The daemon is a child of the app: its subtree (the shells) is the PTY group, not the budget.
            pty = daemon_proc.children(recursive=True)
            owned = {p.pid for p in pty} | {daemon_proc.pid}
            webview = [p for p in app_proc.children(recursive=True) if p.pid not in owned]
            return [app_proc, daemon_proc] + webview, pty, webview

        budget, pty, _ = groups()
        cpu0, pty_cpu0, t0 = sum_cpu(budget), sum_cpu(pty), time.time()
        memory, pty_rss = [], []
        log(f"run {index + 1}: idle for {args.idle} s")
        while time.time() - t0 < args.idle:
            time.sleep(max(0.0, min(2.0, args.idle - (time.time() - t0))))
            _, p, webview = groups()
            memory.append(memory_sample(app_proc, daemon_proc, webview))
            pty_rss.append(sum_rss(p))
        idle_memory = median_sample(memory)
        log(f"run {index + 1}: idle memory (MB) " + ", ".join(f"{k[:-3]} {v:.0f}" for k, v in idle_memory.items()))
        wall = time.time() - t0
        cpu1, pty_cpu1 = sum_cpu(budget), sum_cpu(pty)
        idle_cpu = sum(cpu1[k] - cpu0[k] for k in cpu0 if k in cpu1) / wall * 100
        pty_cpu = sum(pty_cpu1[k] - pty_cpu0[k] for k in pty_cpu0 if k in pty_cpu1) / wall * 100

        # The standard scenario: project switches, then typing into a visible shell.
        page = "window.__kelpie.perf"
        d.js_async(f"const done = arguments[arguments.length-1]; {page}.switchProjects(arguments[0], 9).then(done);", ids)  # warm-up
        d.js(f"{page}.clearLongTasks()")
        if args.inject_busy_loop:
            # Self-test of the long-task metric: one task of this length, started from the page at runtime.
            d.js("setTimeout(() => { const t = performance.now(); while (performance.now() - t < arguments[0]); }, 800)", args.inject_busy_loop)
        switches = d.js_async(f"const done = arguments[arguments.length-1]; {page}.switchProjects(arguments[0], arguments[1]).then(done);", ids, args.switches)
        d.js("window.__kelpie.store.switchProject(arguments[0]);", ids[0])
        sid = d.wait("const v = window.__kelpie.views().find(v => v.visible && v.attached); return v ? v.sessionId : null", 20, 0.2)
        time.sleep(1)
        text = echo_text(args.echo_chars)
        echo = d.js_async(f"const done = arguments[arguments.length-1]; {page}.echo(arguments[0], arguments[1]).then(done);", sid, text)
        long_tasks = d.js(f"return {page}.longTasks()")
        # Escape clears the typed line in PowerShell and cmd.
        daemon(d, "session.input", {"handle": d.js("return window.__kelpie.peekView(arguments[0]).handle", sid), "data": "\x1b"})
        echo_ok = [x for x in echo[5:] if x is not None]  # the first keys' windows are shorter than the check
        missed = sum(1 for x in echo[5:] if x is None)
        if len(switches) < args.switches * 0.95 or len(echo_ok) < len(echo[5:]) * 0.95:
            raise SystemExit(f"too few samples: {len(switches)} switches, {len(echo_ok)} echoes ({missed} missed)")
        long_over = [t for t in long_tasks if t["ms"] > 50]
        log(f"run {index + 1}: switch p50 {median(switches):.1f} p95 {pct(switches, .95):.1f} ms; echo p50 {median(echo_ok):.1f} p95 {pct(echo_ok, .95):.1f} ms; long tasks {len(long_over)}")

        # Minimised: the memory the window keeps while nobody looks at it, then what showing it again costs.
        minimised = {}
        hwnd = main_window(app_proc.pid) if args.minimized > 0 and os.name == "nt" else None
        if hwnd:
            import ctypes
            user32 = ctypes.windll.user32
            user32.ShowWindow(hwnd, 6)  # SW_MINIMIZE
            log(f"run {index + 1}: minimised for {args.minimized} s")
            samples, t_min = [], time.time()
            while time.time() - t_min < args.minimized:
                time.sleep(2)
                samples.append(memory_sample(app_proc, daemon_proc, groups()[2]))
            # Trimming takes a moment: the second half of the samples count.
            minimised = median_sample(samples[len(samples) // 2:], "_minimized")
            t_restore = time.time() * 1000
            user32.ShowWindow(hwnd, 9)  # SW_RESTORE
            painted = d.js_async("const done = arguments[arguments.length-1]; setTimeout(() => done(null), 10000); requestAnimationFrame(() => requestAnimationFrame(() => done(performance.timeOrigin + performance.now())));")
            if painted is None:
                raise SystemExit("the restored window did not paint within 10 s")
            minimised["restore_frame_ms"] = r2(painted - t_restore)
            after = d.js_async(f"const done = arguments[arguments.length-1]; {page}.switchProjects(arguments[0], 30).then(done);", ids)
            minimised["switch_after_restore_median_ms"] = r2(median(after))
            minimised["switch_after_restore_max_ms"] = r2(max(after))
            log(f"run {index + 1}: minimised rss {minimised.get('rss_minimized_mb')} MB (private {minimised.get('private_minimized_mb')}); restore frame {minimised['restore_frame_ms']:.0f} ms; switches after p50 {minimised['switch_after_restore_median_ms']:.1f} max {minimised['switch_after_restore_max_ms']:.1f} ms")
        elif args.minimized > 0 and os.name == "nt":
            log(f"run {index + 1}: main window not found, minimised phase skipped")
        return {
            "cold_start_ms": r2(cold * 1000),
            "switch_median_ms": r2(median(switches)), "switch_p95_ms": r2(pct(switches, .95)), "switch_samples": len(switches),
            "echo_median_ms": r2(median(echo_ok)), "echo_p95_ms": r2(pct(echo_ok, .95)), "echo_samples": len(echo_ok), "echo_missed": missed,
            "long_tasks": len(long_over), "long_task_ms": [t["ms"] for t in long_over],
            "idle_cpu_pct": r2(idle_cpu), **idle_memory,
            "pty_idle_cpu_pct": r2(pty_cpu), "pty_rss_mb": r2(median(pty_rss)),
            "sessions": len(handles), "idle_seconds": args.idle,
            **startup, **minimised,
        }
    finally:
        if run is not None:
            try:
                daemon(run.d, "daemon.shutdown", {"kill_sessions": True})
            except Exception:  # noqa: BLE001 - the reply may race the daemon exiting
                pass
            run.d.quit()
        stop_tree(driver_proc)
        if daemon_pid:
            try:
                psutil.Process(daemon_pid).kill()
            except Exception:  # noqa: BLE001
                pass
        shutil.rmtree(data, ignore_errors=True)
        shutil.rmtree(projects_dir, ignore_errors=True)


# ---------- comparison ----------

def aggregate(per_run):
    """Median across runs of every numeric metric, its relative spread (max-min)/median and its min..max."""
    out, spread, ranges = {}, {}, {}
    for key in sorted({k for r in per_run for k in r}):
        vals = [r[key] for r in per_run if isinstance(r.get(key), (int, float))]
        if not vals:
            continue
        m = median(vals)
        out[key] = r2(m)
        if len(vals) > 1 and m and (key in BUDGETS or key in WARNINGS):
            spread[key] = round((max(vals) - min(vals)) / m * 100, 1)
        if len(vals) > 1 and (key in BUDGETS or key in WARNINGS):
            ranges[key] = [r2(min(vals)), r2(max(vals))]
    return out, spread, ranges


def compare(current, base):
    """Returns (rows, failed): rows of (metric, current, baseline, limit text, verdict)."""
    rows, failed = [], False
    for name, (unit, kind, lim) in BUDGETS.items():
        cur, ref = current.get(name), base.get(name)
        if cur is None:
            continue
        if ref is None:
            rows.append((name, cur, None, "no baseline", "skip"))
            continue
        limit = ref * lim if kind == "ratio" else ref + lim
        ok = cur <= limit + 1e-9
        failed |= not ok
        rows.append((name, cur, ref, f"<= {limit:.2f} {unit}", "ok" if ok else "FAIL"))
    for name, (unit, absolute, ratio) in WARNINGS.items():
        cur, ref = current.get(name), base.get(name)
        if cur is None:
            continue
        over = (absolute is not None and cur > absolute) or (ref is not None and cur > ref * ratio)
        rows.append((name, cur, ref, "warn only", "warn" if over else "ok"))
    return rows, failed


def print_table(rows):
    log(f"{'metric':<20}{'current':>12}{'baseline':>12}  {'limit':<24}verdict")
    for name, cur, ref, limit, verdict in rows:
        log(f"{name:<20}{cur:>12}{'-' if ref is None else ref:>12}  {limit:<24}{verdict}")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--label", required=True, help="names the report: plans/reports/perf-<label>.json")
    ap.add_argument("--compare", help="baseline file to compare with (scripts/perf/baseline.json)")
    ap.add_argument("--update-baseline", action="store_true", help="store this run's medians in the --compare file (default scripts/perf/baseline.json)")
    ap.add_argument("--reason", help="why the baseline moves (required with --update-baseline after the first one)")
    ap.add_argument("--runs", type=int, default=3, help="app launches / bench repeats; medians are compared")
    ap.add_argument("--metrics", default="bundle,bench,gui", help="comma list of: bundle, bench, gui")
    ap.add_argument("--out", help="report path (default plans/reports/perf-<label>.json)")
    ap.add_argument("--app", help="release app executable (default: <target>/release/kelpie)")
    ap.add_argument("--kelpied", help="kelpied executable (default: <target>/release/kelpied)")
    ap.add_argument("--msedgedriver", help="path of msedgedriver (default: PATH)")
    ap.add_argument("--driver-port", type=int, default=4444, help="tauri-driver's port (its msedgedriver gets a free port of its own)")
    ap.add_argument("--sessions", type=int, default=20, help="shells open during the idle and scenario phases")
    ap.add_argument("--switches", type=int, default=210, help="project switches timed per run (at least 200 samples)")
    ap.add_argument("--echo-chars", type=int, default=210)
    ap.add_argument("--idle", type=float, default=60, help="seconds of idle CPU / RSS sampling")
    ap.add_argument("--settle", type=float, default=20, help="seconds to let the new shells finish starting before the idle phase")
    ap.add_argument("--minimized", type=float, default=30, help="seconds the window stays minimised for the minimised RSS (Windows; 0 skips)")
    ap.add_argument("--inject-busy-loop", type=int, default=0, metavar="MS", help="self-test: block the page for MS ms during the switches; long_tasks must then fail the gate")
    args = ap.parse_args()

    wanted = {g.strip() for g in args.metrics.split(",") if g.strip()}
    if not wanted <= set(GROUPS):
        ap.error(f"--metrics takes {', '.join(GROUPS)}")
    if args.update_baseline and not args.compare:
        args.compare = os.path.join(HERE, "baseline.json")

    metrics, extra, spread, ranges, per_run = {}, {}, {}, {}, []
    if "bundle" in wanted:
        b = measure_bundle()
        metrics["entry_gz_kb"] = b.pop("entry_gz_kb")
        extra["bundle"] = b
        log(f"bundle: entry {metrics['entry_gz_kb']} KB gzip, {b['lazy_chunks']} lazy chunks ({b['lazy_gz_kb']} KB)")
    if "bench" in wanted:
        b = measure_bench(args)
        metrics["bench_wall_s"] = b.pop("bench_wall_s")
        extra["bench"] = b
        log(f"bench: {metrics['bench_wall_s']} s wall (runs {b['bench_runs_s']})")
    if "gui" in wanted:
        try:
            import psutil  # noqa: F401
        except ImportError:
            raise SystemExit("the gui group needs psutil: pip install psutil (or --metrics bundle,bench)")
        for i in range(args.runs):
            per_run.append(gui_run(args, i))
        agg, spread, ranges = aggregate(per_run)
        metrics.update(agg)
        extra["gui_runs"] = per_run

    report = {
        "label": args.label, "created": datetime.datetime.now().astimezone().isoformat(timespec="seconds"),
        "machine": machine_description(), "runs": args.runs, "metrics": metrics, "spread_pct": spread, "range": ranges, **extra,
    }
    if args.inject_busy_loop:
        report["injected_busy_loop_ms"] = args.inject_busy_loop

    failed = False
    if args.compare and os.path.exists(args.compare):
        base = json.load(open(args.compare, encoding="utf-8"))
        rows, failed = compare(metrics, base["metrics"])
        print_table(rows)
        report["comparison"] = [dict(zip(("metric", "current", "baseline", "limit", "verdict"), r)) for r in rows]
        for name, s in spread.items():
            if s > 10 and name in BUDGETS:
                log(f"note: {name} spread over {args.runs} runs is {s}% (target <= 10%)")
    elif args.compare and not args.update_baseline:
        log(f"no baseline at {args.compare}: nothing to compare with")

    out = args.out or os.path.join(REPO, "plans", "reports", f"perf-{args.label}.json")
    os.makedirs(os.path.dirname(out), exist_ok=True)
    json.dump(report, open(out, "w", encoding="utf-8"), indent=2)
    log(f"report: {out}")

    if args.update_baseline:
        base = json.load(open(args.compare, encoding="utf-8")) if os.path.exists(args.compare) else {"metrics": {}, "history": []}
        if base["history"] and not args.reason:
            raise SystemExit("--update-baseline needs --reason: a baseline never moves without a written reason")
        # A group that was not measured keeps its old numbers; every budgeted metric stays listed.
        merged = {k: base["metrics"].get(k) for k in BUDGETS}
        merged.update({k: v for k, v in metrics.items() if k in BUDGETS or k.endswith("_p95_ms")})
        base.update({"label": args.label, "created": report["created"], "machine": report["machine"], "runs": args.runs, "metrics": merged})
        base["spread_pct"] = {**base.get("spread_pct", {}), **spread}
        base["range"] = {**base.get("range", {}), **ranges}
        for k in ("bundle", "bench"):
            if k in extra:
                base.setdefault("details", {})[k] = extra[k]
        base["history"].append({"date": report["created"], "label": args.label, "reason": args.reason or "initial baseline", "metrics": sorted(metrics)})
        json.dump(base, open(args.compare, "w", encoding="utf-8"), indent=2)
        log(f"baseline updated: {args.compare}")
        return 0
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
