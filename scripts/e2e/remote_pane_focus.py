"""E2E: two panes on the same SSH host side by side, in the real app: only the focused one looks focused.

    python -u -X utf8 scripts/e2e/remote_pane_focus.py <scratch-dir> <debug kelpie.exe> [--before]

The app is plain `cargo build -p kelpie` (dev mode: it loads the Vite dev server on port 1427, which this script starts from the
repository; the port must be free). Its own daemon (`kelpied` next to the app) runs on an endpoint and data folder of this process
only, and is stopped by its PID. Needs tauri-driver (PATH or ~/.cargo/bin) and msedgedriver matching the WebView2.

The two sessions are shells "on" a host that does not exist (`e2e-host.invalid`): ssh fails at once, which is enough, because the
pane draws its host marks from the session's `remote`, not from a live connection. No real host, key or SSH config is touched.

Checks, from the rendered panes' computed styles: the focused pane carries the focus mark; the other pane has no stripe and no
header chip in the accent colour, so nothing on it reads as focus.

`--before`: the same steps and screenshots (prefixed `before-`) against whatever the dev server serves; failures are printed and
the exit code is 0.
"""
import json
import os
import shutil
import socket
import subprocess
import sys
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import phone_support as ps  # noqa: E402

args = [a for a in sys.argv[1:] if not a.startswith("--")]
BEFORE = "--before" in sys.argv
if len(args) < 2:
    raise SystemExit(__doc__)
scratch = os.path.abspath(args[0])
app = os.path.abspath(args[1])
kelpied = os.path.join(os.path.dirname(app), "kelpied.exe" if os.name == "nt" else "kelpied")
REPO = os.path.abspath(os.path.join(HERE, "..", ".."))
os.makedirs(scratch, exist_ok=True)
DATA = ps.isolate("remote-focus")
ENDPOINT = os.environ["KELPIE_ENDPOINT"]
assert ENDPOINT.startswith("kelpie-remote-focus-")
os.environ["KELPIE_DAEMON_PATH"] = kelpied
os.environ["KELPIE_E2E"] = "1"
os.environ.pop("KELPIE_ALLOW_REAL_DAEMON", None)

from smoke_support import Run, native_driver, native_driver_hint, tauri_driver  # noqa: E402

NATIVE, TAURI_DRIVER = native_driver(), tauri_driver()
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit(f"needs tauri-driver (PATH or ~/.cargo/bin) and {native_driver_hint()}")
for what, path in (("app", app), ("daemon next to the app", kelpied)):
    if not os.path.exists(path):
        raise SystemExit(f"no {what}: {path}")

WORK = os.path.join(scratch, f"remote-focus-{os.getpid()}")
os.makedirs(WORK, exist_ok=True)
HOST = "e2e-host.invalid"
S = "window.__kelpie.store"
PREFIX = "before" if BEFORE else "after"
results = []
procs = {}


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  [{detail[:400]}]" if detail else ""), flush=True)


def until(fn, timeout=30, step=0.25):
    end = time.time() + timeout
    while time.time() < end:
        try:
            v = fn()
            if v:
                return v
        except Exception:  # noqa: BLE001
            pass
        time.sleep(step)
    return None


def free_port():
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def port_open(p):
    for host in ("127.0.0.1", "::1"):
        try:
            socket.create_connection((host, p), 0.5).close()
            return True
        except OSError:
            pass
    return False


# Each pane of the active tab with the colours a person sees on it: the left stripe, the header chip, the focus ring (::after).
# Colours are resolved by the browser to rgb(), and the accent is read the same way, so they compare as equals.
PANES_JS = """
const probe = document.createElement('div');
probe.style.color = 'var(--accent)';
document.body.appendChild(probe);
const accent = getComputedStyle(probe).color;
probe.remove();
const panes = [...document.querySelectorAll('.tab-content .pane')];
return {accent, panes: panes.map(p => {
  const stripe = p.querySelector('.pane-host-stripe');
  const chip = p.querySelector('.pane-header .host-chip');
  const dot = chip && chip.querySelector('.host-dot');
  const term = p.querySelector('.term-wrap');
  const r = p.getBoundingClientRect();
  return {
    session: term ? term.dataset.session : null,
    left: r.left,
    focused: p.classList.contains('focused'),
    ring: getComputedStyle(p, '::after').boxShadow,
    stripe: stripe ? getComputedStyle(stripe).backgroundColor : null,
    chipDot: dot ? getComputedStyle(dot).backgroundColor : null,
    chipBorder: chip ? getComputedStyle(chip).borderTopColor : null,
  };
}).sort((a, b) => a.left - b.left)};
"""


def scenario(run, d):
    w = run.d
    w.set_window_size(1280, 800)
    pid = w.js(f"return {S}.addProject(arguments[0])", WORK)
    w.js(f"{S}.switchProject(arguments[0])", pid)
    left = d.call("session.create", {"kind": "shell", "cwd": "/root", "name": "left", "project_id": pid, "remote": {"dest": HOST}})
    right = d.call("session.create", {"kind": "shell", "cwd": "/root", "name": "right", "project_id": pid, "remote": {"dest": HOST}})
    w.js(f"{S}.revealSession(arguments[0])", left["id"])
    until(lambda: w.js(PANES_JS)["panes"], 20)
    w.js(f"{S}.placePane(arguments[0], {{ id: 'pane-right-e2e', kind: 'terminal', sessionId: arguments[1] }}, 'right')", pid, right["id"])
    check("set-up: two panes side by side", until(lambda: len(w.js(PANES_JS)["panes"]) == 2, 20), w.js(PANES_JS))
    # Focus the left pane the way a person does: a click in it.
    w.js("document.querySelectorAll('.tab-content .pane')[0].dispatchEvent(new PointerEvent('pointerdown', {bubbles: true}))")
    time.sleep(0.6)
    g = w.js(PANES_JS)
    run.shot(f"{PREFIX}-01-left-focused")
    accent = g["accent"]
    lp, rp = g["panes"]
    check("set-up: both panes run on the same host", lp["session"] in (left["id"], right["id"]) and rp["session"] in (left["id"], right["id"]), g)
    check("the left pane is the focused one", lp["focused"] and not rp["focused"], (lp["focused"], rp["focused"]))
    check("the unfocused pane has no stripe in the accent colour", rp["stripe"] != accent, (rp["stripe"], accent))
    check("the unfocused pane's host chip is not in the accent colour", rp["chipDot"] != accent and rp["chipBorder"] != accent, (rp["chipDot"], rp["chipBorder"], accent))
    check("the focused pane keeps its focus ring", lp["ring"] not in (None, "", "none"), lp["ring"])
    # Focus moves to the right pane: the marks follow it.
    w.js("document.querySelectorAll('.tab-content .pane')[1].dispatchEvent(new PointerEvent('pointerdown', {bubbles: true}))")
    time.sleep(0.6)
    g = w.js(PANES_JS)
    run.shot(f"{PREFIX}-02-right-focused")
    lp, rp = g["panes"]
    check("after a click on the right pane, it is the focused one", rp["focused"] and not lp["focused"], (lp["focused"], rp["focused"]))
    check("now the left pane has no stripe or chip in the accent colour",
          lp["stripe"] != accent and lp["chipDot"] != accent, (lp["stripe"], lp["chipDot"], accent))


def main():
    if port_open(1427):
        raise SystemExit("port 1427 is taken (another Vite dev server?): this script starts the app's own; stop that one first")
    d = ps.Daemon(kelpied, env={"KELPIE_E2E": "1"}).start()
    procs["daemon"] = d.proc
    procs["vite"] = subprocess.Popen(["cmd", "/c", "pnpm", "exec", "vite", "--port", "1427", "--strictPort"] if os.name == "nt" else ["pnpm", "exec", "vite", "--port", "1427", "--strictPort"], cwd=os.path.join(REPO, "app"), stdout=open(os.path.join(scratch, "vite.log"), "w"), stderr=subprocess.STDOUT)
    until(lambda: port_open(1427), 60)
    driver_port = free_port()
    os.environ["KELPIE_E2E_DRIVER"] = f"http://127.0.0.1:{driver_port}"
    procs["driver"] = subprocess.Popen([TAURI_DRIVER, "--port", str(driver_port), "--native-port", str(free_port()), "--native-driver", NATIVE], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    run = None
    try:
        if not until(lambda: port_open(driver_port), 30):
            raise SystemExit("tauri-driver did not open its port")
        run = Run(os.path.join(scratch, "shots"), app=app)
        run.boot()
        info = run.d.invoke("daemon_call", {"method": "daemon.info", "params": {}}).get("ok") or {}
        check("the app is connected to this script's daemon", info.get("pid") == d.proc.pid, info.get("pid"))
        if info.get("pid") != d.proc.pid:
            raise SystemExit("the app is not on this script's daemon: stopping before any scenario")
        scenario(run, d)
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False)
    finally:
        if run is not None:
            run.d.quit()
        for name in ("driver", "vite"):
            p = procs.get(name)
            if p and p.poll() is None:
                if os.name == "nt":
                    subprocess.run(["taskkill", "/PID", str(p.pid), "/T", "/F"], capture_output=True)
                else:
                    p.terminate()
        assert os.environ["KELPIE_ENDPOINT"] == ENDPOINT
        d.stop()
        shutil.rmtree(WORK, ignore_errors=True)
        shutil.rmtree(DATA, ignore_errors=True)
    failed = [r for r in results if not r[1]]
    print(json.dumps({"mode": PREFIX, "passed": len(results) - len(failed), "failed": len(failed)}))
    sys.exit(0 if BEFORE or not failed else 1)


if __name__ == "__main__":
    main()
