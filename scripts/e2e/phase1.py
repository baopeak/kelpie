"""E2E for UI phase 1: key hints, intercept toast, overlay key guard, explorer filter and
ignored dimming, pane tools + maximize, editor-host env scrub.

    python -X utf8 scripts/e2e/phase1.py <screenshot-dir> <project-dir>
"""
import os
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import Run, daemon  # noqa: E402

shots, project = sys.argv[1], sys.argv[2]
run = Run(shots)
d = run.d


def wait_text(handle, needle, timeout=15):
    end = time.time() + timeout
    while time.time() < end:
        if needle in daemon(d, "session.read", {"handle": handle})["text"]:
            return True
        time.sleep(0.25)
    return False


try:
    run.boot()
    # Other scenarios may have turned the shortcut notes off in the shared dev state.
    d.js("const s = window.__kelpie.store; s.setSettings({ shortcutHints: true }); for (const p of [...s.state.projects]) s.removeProject(p.id); s.addProject(arguments[0]);", project)
    pid = d.js("return window.__kelpie.store.state.activeProject")
    info = d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.store.newSession(arguments[0], 'shell').then(done);", pid)
    h = info["handle"]
    d.wait("return !!document.querySelector('.pane .term-host .xterm')", 20)
    time.sleep(2)

    # Env scrub: the daemon was started from a VS Code terminal that sets ELECTRON_RUN_AS_NODE.
    daemon(d, "session.input", {"handle": h, "data": "echo \"[EN=$($env:ELECTRON_RUN_AS_NODE)][VS=$($env:VSCODE_PID)]\"\r"})
    run.check("editor-host env vars do not leak into sessions", wait_text(h, "[EN=][VS=]"))

    # Intercept toast: app chord pressed inside the terminal.
    d.click(".pane .xterm")
    time.sleep(0.5)
    d.keys("ctrl+shift+o")
    d.wait("return !!document.querySelector('.palette')", 5)
    toast = d.wait("return document.querySelector('.toast')?.innerText ?? null", 5)
    run.check("intercepted shortcut explained by a toast", "kept from the terminal" in toast, toast.replace("\n", " "))
    hints = d.js("return document.querySelector('.palette .key-hints')?.innerText ?? ''")
    run.check("palette shows key hints", "Esc" in hints and "Enter" in hints, hints.replace("\n", " "))
    run.shot("p1-01-palette-hints")
    d.keys("escape")

    # Overlay guard: typing while the dashboard is open must not reach the terminal.
    d.click(".pane .xterm")
    d.keys("ctrl+shift+m")
    d.wait("return !!document.querySelector('.dashboard')", 5)
    d.keys("zzqq")
    time.sleep(0.8)
    run.check("keys do not leak to the terminal under an overlay", "zzqq" not in daemon(d, "session.read", {"handle": h})["text"])
    d.keys("escape")

    # Explorer: header, ignored dimming, name filter, Names/Contents.
    d.js("const s = window.__kelpie.store; s.showRightPanel('files');")
    d.wait("return document.querySelectorAll('.tree-row').length > 3", 15)
    ignored = d.js("return [...document.querySelectorAll('.tree-row.ignored .fname')].map(e => e.textContent)")
    run.check("gitignored entries are dimmed", "target" in ignored and "crates" not in ignored, ",".join(ignored))
    run.shot("p1-02-explorer")
    d.click(".explorer-tools .text-input")
    d.keys("session.rs")
    d.wait("return [...document.querySelectorAll('.explorer .list-row .dir')].some(e => e.textContent.includes('kelpied'))", 10)
    run.check("explorer name filter finds files", True)
    run.shot("p1-03-explorer-filter")
    d.js("[...document.querySelectorAll('.seg button')].find(b => b.textContent === 'Contents').click()")
    d.wait("return !!document.querySelector('.explorer .search-box input')", 5)
    run.check("Contents mode shows search in files", True)
    d.js("[...document.querySelectorAll('.seg button')].find(b => b.textContent === 'Names').click()")

    # Pane tools: split from the hover toolbar, then maximize and restore.
    d.js("document.querySelector('.pane-tools button[title^=\"Split right\"]').click()")
    # The new pane asks what to run: key 1..9 picks; Terminal is the last launcher entry.
    d.wait("return !!document.querySelector('.empty-pane')", 10)
    d.js("[...document.querySelectorAll('.empty-pane-kind')].find(b => b.title === 'Terminal').click()")
    d.wait("return document.querySelectorAll('.pane .term-host .xterm').length === 2", 20)
    run.check("pane toolbar splits", True)
    d.js("document.querySelector('.pane-header button[title^=\"Maximize pane\"]').click()")
    time.sleep(0.6)
    n = d.js("return document.querySelectorAll('.pane').length")
    run.check("maximize shows a single pane", n == 1, f"panes={n}")
    run.shot("p1-04-maximized")
    d.js("document.querySelector('.pane-header button[title^=\"Restore layout\"]').click()")
    time.sleep(0.6)
    run.check("restore brings the split back", d.js("return document.querySelectorAll('.pane').length") == 2)
    run.shot("p1-05-split-tools")

    for s in daemon(d, "session.list"):
        if s.get("project_id") == pid:
            daemon(d, "session.close", {"handle": s["handle"]})
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    run.finish()
