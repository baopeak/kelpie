"""E2E for Kip, the mascot: it peeks over the sidebar footer, follows the agents' state (a shell
printing steadily is "working", an agent asking for permission is "needs you"), takes you to that session on click, and
both Appearance switches work (Kip off; animations off). Also measures the CPU the app's
processes use with Kip animating, still and off, so its cost stays visible.

    python -X utf8 scripts/e2e/mascot.py <screenshot-dir> <scratch-dir> [--cpu-seconds 20]
"""
import json
import os
import subprocess
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import Run, daemon, default_app  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
cpu_seconds = int(sys.argv[sys.argv.index("--cpu-seconds") + 1]) if "--cpu-seconds" in sys.argv else 20
S = "window.__kelpie.store"
run = Run(shots)
d = run.d


def js_await(expr, *args):
    return d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve({expr}).then(done, e => done({{err: String(e)}}));", *args)


def mood():
    return d.js("const k = document.querySelector('.kip-companion .kip'); return k ? [...k.classList].find(c => c.startsWith('kip-') && c !== 'kip-body') : null")


def moving():
    return d.js("return !!document.querySelector('.kip-companion .kip.moving')")


def app_cpu_seconds():
    """CPU time of the app and every process under it (the WebView2 browser and renderers)."""
    ps = r"""
$all = Get-CimInstance Win32_Process | Select-Object ProcessId, ParentProcessId, Name, ExecutablePath
$root = $all | Where-Object { $_.Name -eq 'kelpie.exe' -and $_.ExecutablePath -eq $env:KELPIE_E2E_APP_PATH }
$ids = @($root.ProcessId); $grew = $true
while ($grew) { $more = $all | Where-Object { $ids -contains $_.ParentProcessId -and $ids -notcontains $_.ProcessId }; $grew = [bool]$more; $ids += @($more.ProcessId) }
($ids | ForEach-Object { (Get-Process -Id $_ -ErrorAction SilentlyContinue).TotalProcessorTime.TotalSeconds } | Measure-Object -Sum).Sum
"""
    env = dict(os.environ, KELPIE_E2E_APP_PATH=os.path.abspath(default_app()))
    out = subprocess.run(["powershell", "-NoProfile", "-Command", ps], capture_output=True, text=True, env=env)
    return float(out.stdout.strip() or 0)


def cpu_percent(label):
    """Average CPU (% of one core) over the measuring window."""
    time.sleep(2)
    a, t0 = app_cpu_seconds(), time.time()
    time.sleep(cpu_seconds)
    b, t1 = app_cpu_seconds(), time.time()
    pct = round((b - a) / (t1 - t0) * 100, 2)
    print(f"CPU {label}: {pct}% of one core", flush=True)
    return pct


try:
    run.boot()
    d.set_window_size(1400, 900)
    # The machine may have the OS animation effects off: "Always" animates regardless.
    d.js(f"{S}.setSettings({{ mascot: true, mascotMotion: 'on' }})")
    here, busy = os.path.join(scratch, "kip-here"), os.path.join(scratch, "kip-busy")
    for p in (here, busy):
        os.makedirs(p, exist_ok=True)
    d.js(f"{S}.addProject(arguments[0]); {S}.addProject(arguments[1]);", here, busy)
    ids = d.js(f"return {S}.state.projects.filter(p => [arguments[0], arguments[1]].includes(p.path)).map(p => [p.path, p.id])", here, busy)
    pid = dict(ids)
    # The shell runs in a project that is not shown, so its output is never drawn: what the
    # CPU numbers measure is Kip, not the terminal.
    sh = js_await(f"{S}.newSession(arguments[0], 'shell')", pid[busy])
    d.js(f"{S}.switchProject(arguments[0])", pid[here])
    time.sleep(3)

    d.wait("return !!document.querySelector('.sidebar-footer .kip-companion .kip')", 10)
    m = mood()
    run.check("Kip peeks over the sidebar footer", m in ("kip-peek", "kip-done"), m)
    run.check("Kip greets an empty project", bool(d.js("return !!document.querySelector('.welcome .kip, .empty-pane .kip')")))
    run.check("Kip moves while motion is on", moving())
    run.shot("kip-01-peek")

    daemon(d, "session.input", {"handle": sh["handle"], "data": ("ping -t 127.0.0.1" if os.name == "nt" else "ping 127.0.0.1") + "\r"})
    d.wait("return !!document.querySelector('.kip-companion .kip-working')", 20)
    run.check("a working agent makes Kip work", mood() == "kip-working", mood())
    run.check("its tooltip says how many work", "working" in (d.js("return document.querySelector('.kip-companion').title") or ""))
    run.shot("kip-02-working")
    working_on = cpu_percent("working, Kip animated")

    d.js(f"{S}.setSettings({{ mascotMotion: 'system' }})")
    time.sleep(0.5)
    os_reduces = d.js("return matchMedia('(prefers-reduced-motion: reduce)').matches")
    run.check("Follow system obeys the OS animation setting", moving() == (not os_reduces), f"reduce={os_reduces}")
    d.js(f"{S}.setSettings({{ mascotMotion: 'off' }})")
    time.sleep(0.5)
    run.check("animations off: Kip stays still", not moving() and mood() == "kip-working")
    working_still = cpu_percent("working, Kip still")

    d.js(f"{S}.setSettings({{ mascot: false }})")
    time.sleep(0.5)
    run.check("Kip off: gone from the footer and empty screens", not d.js("return !!document.querySelector('.kip')"))
    working_off = cpu_percent("working, Kip off")

    d.js(f"{S}.setSettings({{ mascot: true, mascotMotion: 'on' }})")
    daemon(d, "session.input", {"handle": sh["handle"], "data": "\x03"})
    time.sleep(1)
    # What Claude Code's hook reports when it asks for a permission.
    daemon(d, "hook.report", {"session_id": sh["id"], "agent": "claude", "payload": {"hook_event_name": "PermissionRequest"}})
    try:
        needs = d.wait("return !!document.querySelector('.kip-companion .kip-needs')", 20)
    except TimeoutError:
        needs = False
    run.check("an agent asking for permission makes Kip ask for you", mood() == "kip-needs", mood())
    run.shot("kip-03-needs")
    if needs:
        d.js("document.querySelector('.kip-companion').click()")
        time.sleep(1)
        run.check("clicking Kip opens the session that needs you", d.js(f"return {S}.state.activeProject") == pid[busy])

    d.js(f"{S}.switchProject(arguments[0])", pid[here])
    daemon(d, "session.close", {"handle": sh["handle"]})
    d.wait("return !!document.querySelector('.kip-companion .kip-peek')", 20)
    idle_on = cpu_percent("idle, Kip animated")
    d.js(f"{S}.setSettings({{ mascot: false }})")
    idle_off = cpu_percent("idle, Kip off")
    d.js(f"{S}.setSettings({{ mascot: true }})")
    print(json.dumps({"cpu_percent_of_one_core": {"working": {"animated": working_on, "still": working_still, "off": working_off}, "idle": {"animated": idle_on, "off": idle_off}}}), flush=True)
except Exception as e:
    run.check("scenario ran to the end", False, repr(e))
finally:
    run.finish()
