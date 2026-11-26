"""E2E: a name typed in the New session dialog names a grid launch's sessions, numbered in the order the cells sit on screen.

    python -u -X utf8 scripts/e2e/grid_session_name.py <scratch-dir> <debug kelpie.exe> [--before]

The app is plain `cargo build -p kelpie` (dev mode: it loads the Vite dev server on port 1427, which this script starts from the
repository; the port must be free). Its own daemon (`kelpied` next to the app) runs on an endpoint and data folder of this process
only, and is stopped by its PID. Needs tauri-driver (PATH or ~/.cargo/bin) and msedgedriver matching the WebView2.

"claude" is a stand-in on PATH (scripts/e2e/fake_tui_agent.py), so no account or network is used.

As a person would, in the New session dialog:
1. Claude, Grid… (2 × 2), name "api", Start: the four sessions are "api 1" … "api 4", in that order left to right and top to
   bottom on screen, in their pane headers and sidebar rows; the tab shows the focused one's name.
2. The same with the Name field left empty: the sessions keep the default name, the one a single Claude session gets.
3. The "Columns 2 + 1" preset, name "web": numbered by screen position (the tall right cell is "web 2"), not column by column.
4. A one-cell grid (1 typed in "panes per row"), name "solo": the session is "solo".

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
WORK = os.path.join(scratch, f"grid-name-{os.getpid()}")
BIN = os.path.join(WORK, "bin")
os.makedirs(BIN, exist_ok=True)
with open(os.path.join(BIN, "claude.cmd"), "w", encoding="utf-8") as f:
    f.write(f'@echo off\r\n"{sys.executable}" "{os.path.join(HERE, "fake_tui_agent.py")}" "{os.path.join(WORK, "claude.log")}"\r\n')

DATA = ps.isolate("grid-name")
ENDPOINT = os.environ["KELPIE_ENDPOINT"]
assert ENDPOINT.startswith("kelpie-grid-name-")
os.environ["KELPIE_DAEMON_PATH"] = kelpied
os.environ["KELPIE_E2E"] = "1"
os.environ["CLAUDE_CONFIG_DIR"] = os.path.join(WORK, "claude-home")
os.environ["PATH"] = BIN + os.pathsep + os.environ.get("PATH", "")
os.environ.pop("KELPIE_ALLOW_REAL_DAEMON", None)

from smoke_support import Run, native_driver, native_driver_hint, tauri_driver  # noqa: E402

NATIVE, TAURI_DRIVER = native_driver(), tauri_driver()
if not NATIVE or not TAURI_DRIVER:
    raise SystemExit(f"needs tauri-driver (PATH or ~/.cargo/bin) and {native_driver_hint()}")
for what, path in (("app", app), ("daemon next to the app", kelpied)):
    if not os.path.exists(path):
        raise SystemExit(f"no {what}: {path}")

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


# The active tab as a person sees it: its label, and each pane's header title and session in screen order (top to bottom, then
# left to right), with that session's sidebar row name.
SCREEN_JS = """
const s = window.__kelpie.store;
const tab = document.querySelector('.tab.active');
const panes = [...document.querySelectorAll('.tab-content .pane')].filter(p => p.offsetParent !== null).map(p => {
  const r = p.getBoundingClientRect();
  const sid = p.querySelector('[data-session]')?.dataset.session || null;
  const row = sid && document.querySelector(`.session-row[data-row-session='${sid}'] .name`);
  return { top: Math.round(r.top), left: Math.round(r.left), sid, title: p.querySelector('.pane-header .title')?.textContent.trim() ?? null,
           store: sid ? (s.sessions[sid]?.name ?? null) : null, row: row ? row.textContent.trim() : null };
}).sort((a, b) => a.top - b.top || a.left - b.left);
return { tab: tab ? tab.innerText.trim() : null, panes };
"""


def session_ids(w):
    return set(w.js(f"return Object.keys({S}.sessions)") or [])


def open_dialog(w):
    w.js(f"{S}.setOverlay('newsession')")
    w.wait("return !!document.querySelector('.modal .launch-card')", 10)
    ok = w.js("const c = [...document.querySelectorAll('.modal .launch-card')].find(x => x.lastElementChild.textContent.includes('Claude')); if (c) c.click(); return !!c;")
    check("the dialog offers Claude", ok)


def toggle(w, label):
    return w.js("const b = [...document.querySelectorAll('.modal .toggle')].find(b => b.innerText.trim() === arguments[0]); if (b && !b.disabled) b.click(); return !!b && !b.disabled;", label)


def type_name(w, text):
    found = w.js("const r = [...document.querySelectorAll('.modal .form-row')].find(x => x.querySelector('label')?.textContent.trim() === 'Name (optional)'); const i = r?.querySelector('input'); if (i) i.setAttribute('data-e2e-name', ''); return !!i;")
    if not found:
        return False
    w.click(".modal [data-e2e-name]")
    w.keys(text)
    return w.js("return document.querySelector('.modal [data-e2e-name]').value") == text


def start(w, n):
    before = session_ids(w)
    w.js("[...document.querySelectorAll('.modal .footer .btn.primary')].find(b => b.innerText.trim() === 'Start').click();")
    new = until(lambda: (lambda x: x if len(x) >= n else None)(list(session_ids(w) - before)), 60)
    return new or []


def screen(w, n):
    """The active tab once n panes with sessions and names are drawn."""
    return until(lambda: (lambda g: g if len(g["panes"]) == n and all(p["sid"] and (p["title"] or n == 1) and p["row"] for p in g["panes"]) else None)(w.js(SCREEN_JS)), 30) or w.js(SCREEN_JS)


def grid_launch(run, w, name, step, n=4, preset=None, per_row=None):
    """Starts a grid from the dialog: the default 2 x 2, the preset titled `preset`, or `per_row` typed in "panes per row"."""
    open_dialog(w)
    toggle(w, "Terminal (TUI)")
    check(f"{step}: Grid... is offered", toggle(w, "Grid…"))
    if preset:
        check(f"{step}: the {preset} preset is offered", w.js("const b = [...document.querySelectorAll('.modal .layout-preset')].find(b => b.title === arguments[0]); if (b) b.click(); return !!b;", preset))
    if per_row:
        w.js("document.querySelector('.modal .layout-presets + input').setAttribute('data-e2e-per-row', '')")
        w.click(".modal [data-e2e-per-row]")
        w.keys(per_row)
    label = w.js("return [...document.querySelectorAll('.modal .form-row label')].map(l => l.textContent.trim()).find(t => t.startsWith('Grid')) ?? null")
    check(f"{step}: the grid has {n} cell(s)", bool(label) and label.endswith(f"{n} sessions"), label)
    if name:
        check(f"{step}: the name is typed in the Name field", type_name(w, name))
    run.shot(f"{PREFIX}-{step}-dialog")
    new = start(w, n)
    check(f"{step}: {n} session(s) start", len(new) == n, new)
    g = screen(w, n)
    run.shot(f"{PREFIX}-{step}-grid")
    print(f"{step} SCREEN " + json.dumps(g), flush=True)
    return g


def scenario(run, d):
    w = run.d
    w.set_window_size(1300, 860)
    w._req("POST", w.s("/timeouts"), {"script": 60000})
    w.js(f"{S}.setSettings({{ shortcutHints: false, tips: false }})")
    pid = w.js(f"return {S}.addProject(arguments[0])", WORK)
    w.js(f"{S}.switchProject(arguments[0])", pid)

    # The default name: what one Claude session started from the dialog with no name is called.
    open_dialog(w)
    toggle(w, "Terminal (TUI)")
    one = start(w, 1)
    shown = until(lambda: w.js("return document.querySelector(`.session-row[data-row-session='${arguments[0]}'] .name`)?.textContent.trim() || null", one[0]), 20) if one else None
    stored = w.js(f"return {S}.sessions[arguments[0]]?.name ?? null", one[0]) if one else None
    check("set-up: a single Claude session with no name", bool(shown), (stored, shown))

    # 1. Named.
    g = grid_launch(run, w, "api", "01-named")
    want = ["api 1", "api 2", "api 3", "api 4"]
    check("named: the sessions are api 1 ... api 4 in screen order", [p["store"] for p in g["panes"]] == want, [p["store"] for p in g["panes"]])
    check("named: the pane headers show api 1 ... api 4 in screen order", [p["title"] for p in g["panes"]] == want, [p["title"] for p in g["panes"]])
    check("named: the sidebar rows show api 1 ... api 4", [p["row"] for p in g["panes"]] == want, [p["row"] for p in g["panes"]])
    focused = w.js("return document.querySelector('.tab-content .pane.focused [data-session]')?.dataset.session ?? null")
    fname = w.js(f"return {S}.sessions[arguments[0]]?.name ?? null", focused) if focused else None
    check("named: the tab shows the focused session's numbered name", bool(fname) and fname in want and fname in (g["tab"] or ""), (g["tab"], fname))

    # 3. A column grid (left column of two, a tall right one): numbered by where the cells sit, not column by column.
    g = grid_launch(run, w, "web", "03-columns", n=3, preset="Columns 2 + 1")
    want = ["web 1", "web 2", "web 3"]
    check("columns: the sessions are web 1 ... web 3 in screen order", [p["store"] for p in g["panes"]] == want, [(p["store"], p["top"], p["left"]) for p in g["panes"]])
    check("columns: the pane headers show them in screen order", [p["title"] for p in g["panes"]] == want, [p["title"] for p in g["panes"]])

    # 4. A one-cell grid takes the name as typed.
    g = grid_launch(run, w, "solo", "04-one-cell", n=1, per_row="1")
    check("one cell: the session is named as typed, without a number", [p["store"] for p in g["panes"]] == ["solo"], [p["store"] for p in g["panes"]])

    # 2. No name: the defaults, as before.
    g = grid_launch(run, w, "", "02-unnamed")
    names = [p["store"] for p in g["panes"]]
    check("unnamed: every session keeps the default name", names == [stored] * 4, (names, stored))
    check("unnamed: the pane headers and sidebar rows show the default label",
          [p["title"] for p in g["panes"]] == [shown] * 4 and [p["row"] for p in g["panes"]] == [shown] * 4, ([p["title"] for p in g["panes"]], [p["row"] for p in g["panes"]], shown))


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
            try:
                run.d.invoke("daemon_call", {"method": "daemon.shutdown", "params": {"kill_sessions": True}})
            except Exception:  # noqa: BLE001
                pass
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
