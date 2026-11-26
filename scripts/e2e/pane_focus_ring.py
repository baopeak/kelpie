"""E2E: in a split, the focused pane wears a ring in the app accent, at least 2px, whatever its kind; the others wear none.

    python -u -X utf8 scripts/e2e/pane_focus_ring.py <scratch-dir> <debug kelpie.exe> [--before]

The app is plain `cargo build -p kelpie` (dev mode: it loads the Vite dev server on port 1427, which this script starts from the
repository; the port must be free). Its own daemon (`kelpied` next to the app) runs on an endpoint and data folder of this process
only, and is stopped by its PID. Needs tauri-driver (PATH or ~/.cargo/bin) and msedgedriver matching the WebView2.

The agents are stand-ins on PATH (`claude`, `codex`: scripts/e2e/fake_tui_agent.py), so no account or network is used.

Checks, from the rendered panes' computed styles (the ring is the pane's ::after):
1. Two terminals side by side, opened as a person does (a tab, then "right"): after a click on the left one its ring is the accent,
   at least 2px wide, and the right one has no accent ring; a click on the right one moves the ring.
2. The same with two agents (Claude and Codex).
3. The ring stays the accent in the light theme.
4. A broadcasting focused pane keeps its broadcast mark (::before) visible and distinct from the ring; the focused header's
   kind-coloured top rule shows below the ring; a host stripe sits inside it; the reveal pulse is wider than it.
5. A maximized pane and a lone pane in a tab of its own have no ring.

`--before`: the same steps and screenshots (prefixed `before-`) against whatever the dev server serves; failures are printed and
the exit code is 0.
"""
import json
import os
import re
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
WORK = os.path.join(scratch, f"focus-ring-{os.getpid()}")
BIN = os.path.join(WORK, "bin")
os.makedirs(BIN, exist_ok=True)
FAKE_TUI = os.path.join(HERE, "fake_tui_agent.py")
for name, extra in (("claude", ""), ("codex", " --codex")):
    with open(os.path.join(BIN, name + ".cmd"), "w", encoding="utf-8") as f:
        f.write(f'@echo off\r\n"{sys.executable}" "{FAKE_TUI}" "{os.path.join(WORK, name + ".log")}"{extra}\r\n')

DATA = ps.isolate("focus-ring")
ENDPOINT = os.environ["KELPIE_ENDPOINT"]
assert ENDPOINT.startswith("kelpie-focus-ring-")
os.environ["KELPIE_DAEMON_PATH"] = kelpied
os.environ["KELPIE_E2E"] = "1"
os.environ["CLAUDE_CONFIG_DIR"] = os.path.join(WORK, "claude-home")
os.environ["CODEX_HOME"] = os.path.join(WORK, "codex-home")
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


# The panes of the active tab, left to right, with their ring (::after) and broadcast mark (::before) as the browser resolved them.
PANES_JS = """
const probe = document.createElement('div');
probe.style.color = 'var(--accent)';
document.body.appendChild(probe);
const accent = getComputedStyle(probe).color;
probe.remove();
const panes = [...document.querySelectorAll('.tab-content .pane')].filter(p => p.offsetParent !== null);
return {accent, panes: panes.map(p => {
  const r = p.getBoundingClientRect();
  const after = getComputedStyle(p, '::after'), before = getComputedStyle(p, '::before');
  return {
    kind: (window.__kelpie.store.sessions[p.querySelector('[data-session]')?.dataset.session] || {}).kind || null,
    left: r.left, top: r.top,
    focused: p.classList.contains('focused'),
    ring: after.content === 'none' ? 'none' : after.boxShadow,
    mark: before.content === 'none' ? 'none' : before.boxShadow,
    markInset: before.top,
    rule: getComputedStyle(p.querySelector('.pane-header') || p).boxShadow,
    pulse: getComputedStyle(p.querySelector('.pane-pulse') || p).boxShadow,
    kindColour: getComputedStyle(p).getPropertyValue('--pane-color').trim(),
  };
}).sort((a, b) => a.left - b.left || a.top - b.top)};
"""


def ring_of(shadow):
    """(colour, spread px) of the first inset box-shadow, or (None, 0)."""
    if not shadow or shadow == "none":
        return None, 0
    m = re.match(r"((?:rgba?|color)\([^)]*\))\s+0px\s+0px\s+0px\s+([\d.]+)px", shadow)
    return (m.group(1), float(m.group(2))) if m else (None, 0)


def lit(p, accent):
    """True when the pane wears the focus ring: the accent colour, at least 2px."""
    colour, width = ring_of(p["ring"])
    return colour == accent and width >= 2


def click(w, i):
    """Focuses pane i (left to right) the way a person does: a press in it."""
    w.js("const ps = [...document.querySelectorAll('.tab-content .pane')].filter(p => p.offsetParent !== null).sort((a, b) => a.getBoundingClientRect().left - b.getBoundingClientRect().left);"
         " ps[arguments[0]].dispatchEvent(new PointerEvent('pointerdown', {bubbles: true}));", i)
    time.sleep(0.6)
    return w.js(PANES_JS)


def new_session(w, pid, kind, where):
    w.js_async(f"const done = arguments[arguments.length - 1]; {S}.newSession(arguments[0], arguments[1], {{ where: arguments[2] }}).then(() => done(true), () => done(false));", pid, kind, where)


def split_checks(run, w, pid, label, kind, step):
    new_session(w, pid, kind[0], "tab")
    until(lambda: len(w.js(PANES_JS)["panes"]) == 1, 20)
    new_session(w, pid, kind[1], "right")
    g = until(lambda: (lambda g: g if len(g["panes"]) == 2 and all(p["kind"] for p in g["panes"]) else None)(w.js(PANES_JS)), 20)
    check(f"{label}: two panes side by side", bool(g), w.js(PANES_JS))
    g = click(w, 0)
    run.shot(f"{PREFIX}-{step}a-{label}-left-focused")
    accent = g["accent"]
    lp, rp = g["panes"]
    check(f"{label}: the panes are {kind}", [lp["kind"], rp["kind"]] == list(kind), (lp["kind"], rp["kind"]))
    check(f"{label}: the focused left pane's ring is the accent, at least 2px", lp["focused"] and lit(lp, accent), (lp["ring"], accent))
    check(f"{label}: the right pane has no accent ring", not rp["focused"] and ring_of(rp["ring"])[0] != accent, rp["ring"])
    rule = re.match(r"((?:rgba?|color)\([^)]*\))\s+0px\s+([\d.]+)px", lp["rule"] or "")
    check(f"{label}: the focused header's kind-coloured top rule shows below the ring", bool(rule) and rule.group(1) != accent and float(rule.group(2)) > ring_of(lp["ring"])[1],
          (lp["rule"], lp["ring"]))
    g = click(w, 1)
    run.shot(f"{PREFIX}-{step}b-{label}-right-focused")
    lp, rp = g["panes"]
    check(f"{label}: after a click on the right pane the ring moves there", rp["focused"] and lit(rp, accent), (rp["ring"], accent))
    check(f"{label}: and the left pane has none", not lp["focused"] and ring_of(lp["ring"])[0] != accent, lp["ring"])
    return g


def scenario(run, d):
    w = run.d
    w.set_window_size(1280, 800)
    w._req("POST", w.s("/timeouts"), {"script": 60000})
    w.js(f"{S}.setSettings({{ shortcutHints: false, tips: false }})")
    pid = w.js(f"return {S}.addProject(arguments[0])", WORK)
    w.js(f"{S}.switchProject(arguments[0])", pid)

    # 1. Two terminals: the kind colour is grey, so only the accent ring tells them apart.
    split_checks(run, w, pid, "terminals", ("shell", "shell"), "01")
    # 4. Broadcast on the focused pane: its mark stays visible, distinct from the ring.
    w.js(f"const s = {S}; s.setBroadcast(arguments[0], s.activeTab(arguments[0]).id, true)", pid)
    time.sleep(0.6)
    g = w.js(PANES_JS)
    run.shot(f"{PREFIX}-02-broadcast")
    rp = g["panes"][1]
    mark, ring = ring_of(rp["mark"]), ring_of(rp["ring"])
    # The mark shows when it is not hidden under the ring: drawn inside it (inset by the ring's width), or wider than it.
    inset = float(rp["markInset"][:-2]) if rp["markInset"].endswith("px") else 0
    visible = mark[0] is not None and mark[0] != ring[0] and (inset >= ring[1] or mark[1] > ring[1])
    check("broadcast: the focused pane's broadcast mark is not hidden under its ring", visible, (rp["mark"], rp["markInset"], rp["ring"]))
    w.js(f"const s = {S}; s.setBroadcast(arguments[0], s.activeTab(arguments[0]).id, false)", pid)

    # A host stripe (drawn only for a host colour the person chose: one is put on the focused pane here, styled by the app's own
    # CSS) sits inside the ring, so the ring stays whole on all four sides.
    stripe = w.js("const p = document.querySelector('.tab-content .pane.focused'); const el = document.createElement('div'); el.className = 'pane-host-stripe';"
                  " el.style.setProperty('--host-color', 'rgb(0, 160, 80)'); p.appendChild(el); const cs = getComputedStyle(el); const r = {left: cs.left, top: cs.top}; el.remove(); return r;")
    check("host stripe: on the focused pane it sits inside the ring", stripe["left"] == "2px" and stripe["top"] == "2px", stripe)

    # The reveal pulse (a sidebar click on the session) is wider than the ring, so it shows on the focused pane and never looks
    # like the ring on another one.
    pulse = ring_of(rp["pulse"])
    check("reveal pulse: wider than the focus ring", pulse[0] == ring[0] and pulse[1] > ring[1], (rp["pulse"], rp["ring"]))

    # 3. Light theme: the ring follows the theme's accent.
    w.js(f"{S}.setSettings({{ theme: 'light' }})")
    time.sleep(0.6)
    g = click(w, 0)
    run.shot(f"{PREFIX}-03-light")
    check("light theme: the focused pane's ring is the light accent, at least 2px", g["panes"][0]["focused"] and lit(g["panes"][0], g["accent"]), (g["panes"][0]["ring"], g["accent"]))
    check("light theme: the other pane has no accent ring", ring_of(g["panes"][1]["ring"])[0] != g["accent"], g["panes"][1]["ring"])
    w.js(f"{S}.setSettings({{ theme: 'dark' }})")

    # 5a. Maximized: the pane is alone on screen, with no ring.
    w.js(f"const s = {S}; const t = s.activeTab(arguments[0]); s.toggleZoom(t.id, t.focus)", pid)
    time.sleep(0.6)
    g = w.js(PANES_JS)
    run.shot(f"{PREFIX}-04-maximized")
    check("maximized: one pane on screen", len(g["panes"]) == 1, len(g["panes"]))
    check("maximized: it has no accent ring", g["panes"] and ring_of(g["panes"][0]["ring"])[0] != g["accent"], g["panes"] and g["panes"][0]["ring"])
    w.js(f"const s = {S}; const t = s.activeTab(arguments[0]); s.toggleZoom(t.id, t.focus)", pid)

    # 2. Two agents.
    split_checks(run, w, pid, "agents", ("claude", "codex"), "05")

    # 5b. A lone pane in a tab of its own.
    new_session(w, pid, "shell", "tab")
    g = until(lambda: (lambda g: g if len(g["panes"]) == 1 else None)(w.js(PANES_JS)), 20)
    run.shot(f"{PREFIX}-06-lone")
    check("lone pane: it is not marked as focused and has no ring", bool(g) and not g["panes"][0]["focused"] and g["panes"][0]["ring"] == "none", g and (g["panes"][0]["focused"], g["panes"][0]["ring"]))


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
