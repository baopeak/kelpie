"""E2E for UI phase 2: rich sidebar (all projects' sessions, branch subtitle, model, age,
collapse, context menu, filter) and the unified search box (recents, scopes, prefixes).

    python -X utf8 scripts/e2e/phase2.py <screenshot-dir> <scratch-dir>
"""
import os
import shutil
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import REPO, Run, daemon  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
run = Run(shots)
d = run.d
# A second project with no git repository; removed at the end.
second = tempfile.mkdtemp(prefix="kelpie-phase2-")
# The subtitle shows the checked-out branch, or "detached" when HEAD is not on one.
head = subprocess.run(["git", "rev-parse", "--abbrev-ref", "HEAD"], cwd=REPO, capture_output=True, text=True).stdout.strip()
branch = "detached" if head == "HEAD" else head

ROWS_JS = "return [...document.querySelectorAll('.session-row')].map(r => r.innerText.replace(/\\n/g, ' '))"

try:
    run.boot()
    # Leftover sessions of earlier runs would show under "Other sessions".
    for s in daemon(d, "session.list"):
        daemon(d, "session.close", {"handle": s["handle"]})
    d.js("const s = window.__kelpie.store; for (const p of [...s.state.projects]) s.removeProject(p.id);")
    for p in [REPO, second, scratch]:
        d.js("window.__kelpie.store.addProject(arguments[0]);", p)
    ids = d.js("return window.__kelpie.store.state.projects.map(p => p.id)")
    created = []
    for pid, n in ((ids[0], 2), (ids[1], 1)):
        d.js("window.__kelpie.store.switchProject(arguments[0]);", pid)
        for _ in range(n):
            info = d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.store.newSession(arguments[0], 'shell').then(done);", pid)
            created.append(info)
    d.js("window.__kelpie.store.switchProject(arguments[0]);", ids[2])
    time.sleep(1.5)

    rows = d.js(ROWS_JS)
    run.check("sidebar lists sessions of every project", len(rows) == 3, f"{len(rows)} rows")
    sub = d.wait("return [...document.querySelectorAll('.project-sub')].map(e => e.innerText).join('|') || null", 15)
    run.check("project subtitle shows the git branch", branch in sub, f"{sub} (expected {branch})")

    daemon(d, "session.update", {"handle": created[0]["handle"], "model": "claude-opus-5-5"})
    model = d.wait("return document.querySelector('.session-row .model')?.innerText ?? null", 10)
    run.check("model shown under the session name (provider prefix trimmed)", model == "opus-5-5", model)
    ages = d.js("return [...document.querySelectorAll('.session-row .age')].map(e => e.innerText)")
    run.check("relative age shown for each session", all(a in ("now",) or a[-1] in "mhdw" for a in ages), ",".join(ages))
    run.shot("p2-01-sidebar")

    # Collapse the first project's session list.
    d.js("document.querySelector('.project .chev').click()")
    time.sleep(0.4)
    run.check("collapse hides that project's sessions", len(d.js(ROWS_JS)) == 1)
    d.js("document.querySelector('.project .chev').click()")
    time.sleep(0.4)

    # Context menu → mark as unread (from a known read state: shells can turn unread on their own
    # after their startup output).
    for c in created:
        daemon(d, "session.set_unread", {"handle": c["handle"], "unread": False})
    d.wait("return !document.querySelector('.session-row .dot.done')", 5)
    d.js("const r = document.querySelector('.session-row'); r.dispatchEvent(new MouseEvent('contextmenu', {bubbles: true, clientX: 120, clientY: 160}));")
    labels = d.js("return [...document.querySelectorAll('.ctx-item')].map(e => e.innerText)")
    run.check("session context menu has actions", any("unread" in l.lower() or "read" in l.lower() for l in labels), "|".join(labels))
    run.shot("p2-02-session-menu")
    d.js("[...document.querySelectorAll('.ctx-item')].find(e => /unread/i.test(e.innerText))?.click()")
    ok = d.wait("return !!document.querySelector('.session-row .dot.done')", 5)
    run.check("mark as unread reflected in the sidebar", ok)

    # Filter: only projects needing attention (the unread one) + the active project.
    for c in created[1:]:
        daemon(d, "session.set_unread", {"handle": c["handle"], "unread": False})
    time.sleep(0.6)
    d.js("window.__kelpie.store.setSettings({ projectFilter: 'attention' })")
    time.sleep(0.4)
    names = d.js("return [...document.querySelectorAll('.project-name')].map(e => e.innerText)")
    dbg = d.js("const s = window.__kelpie.store; return JSON.stringify({active: s.activeProject()?.name, rows: s.state.projects.map(p => [p.name, Object.values(s.sessions).filter(x => x.project_id === p.id).map(x => [x.alive, x.status, x.unread])])})")
    run.check("attention filter hides quiet projects", len(names) == 2 and os.path.basename(second) not in names, ",".join(names) + " " + dbg)
    d.js("window.__kelpie.store.setSettings({ projectFilter: 'all' })")

    # Unified search.
    d.js("window.__kelpie.store.switchProject(arguments[0]);", ids[0])
    time.sleep(0.5)
    d.keys("ctrl+shift+p")
    d.wait("return !!document.querySelector('.palette .scopes')", 5)
    groups = d.js("return [...document.querySelectorAll('.palette .group')].map(e => e.textContent)")
    run.check("empty search shows Recent and commands", "Recent" in groups and "Commands" in groups, ",".join(groups))
    run.shot("p2-03-search-empty")
    d.keys("session.rs")
    d.wait("return [...document.querySelectorAll('.palette .group')].some(e => e.textContent.startsWith('Files'))", 10)
    groups = d.js("return [...document.querySelectorAll('.palette .group')].map(e => e.textContent)")
    run.check("free text searches files and more", any(g.startswith("Files") for g in groups), ",".join(groups))
    run.shot("p2-04-search-mixed")
    d.keys("tab")
    time.sleep(0.3)
    scope = d.js("return document.querySelector('.scope.on')?.innerText")
    value = d.js("return document.querySelector('.palette input').value")
    run.check("Tab switches scope and rewrites the prefix", scope.startswith("Sessions") and value.startswith("@"), f"{scope!r} {value!r}")
    d.js("const i = document.querySelector('.palette input'); i.value = ''; i.dispatchEvent(new Event('input', {bubbles: true}));")
    d.keys("/font")
    d.wait("return [...document.querySelectorAll('.palette .item .label')].some(e => e.innerText.includes('Terminal font'))", 5)
    run.check("/ prefix searches settings", d.js("return document.querySelector('.scope.on').innerText").startswith("Settings"))
    run.shot("p2-05-search-settings")
    d.keys("enter")
    run.check("choosing a setting opens Settings", bool(d.wait("return !!document.querySelector('.overlay .modal, .settings-page')", 5)))
    d.keys("escape")

    for s in daemon(d, "session.list"):
        if s.get("project_id") in ids:
            daemon(d, "session.close", {"handle": s["handle"]})
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    run.finish()
    shutil.rmtree(second, ignore_errors=True)
