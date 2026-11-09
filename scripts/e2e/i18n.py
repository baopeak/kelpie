"""E2E for the interface language (Settings → Appearance → Language).

    python -X utf8 scripts/e2e/i18n.py <screenshot-dir> <scratch-dir>

Switches to Vietnamese and walks the main surfaces — start page, sidebar, tab bar, status bar,
right panel tabs, search, layout menu, new-session menu, runs, dashboard, board and every
settings section — and fails on any visible line that is an English dictionary key whose
Vietnamese text differs (a place that shows text without t()). Also: the html lang, dates in
Vietnamese, switching back to English without a reload, the setting persists, and "System".
"""
import glob
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import REPO, Run  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
S = "window.__kelpie.store"
proj = os.path.join(scratch, "i18nproj")
os.makedirs(proj, exist_ok=True)

VI = {}
for f in glob.glob(os.path.join(REPO, "app", "src", "locales", "vi", "*.json")):
    VI.update(json.load(open(f, encoding="utf-8")))
# Lines that may legitimately stay English: the same in both languages, or brand-like.
SUSPECT = {k for k, v in VI.items() if v != k and "{" not in k and len(k) > 2}
# A line that is also some key's Vietnamese text is translated text, not a leftover.
SUSPECT -= set(VI.values())
# The language picker names each language in itself; "Windows" is also the OS name, shown as is.
SUSPECT -= {"English", "Windows"}

run = Run(shots)
d = run.d


def wait(cond, timeout=10, interval=0.25):
    end = time.time() + timeout
    while time.time() < end:
        v = cond()
        if v:
            return v
        time.sleep(interval)
    return None


def leftovers(css="body"):
    text = d.js("const e = document.querySelector(arguments[0]); return e ? e.innerText : ''", css) or ""
    lines = {l.strip() for l in text.splitlines() if l.strip()}
    return sorted(l for l in lines if l in SUSPECT)


def surface(name, css, setup=None, teardown=None):
    if setup:
        setup()
        time.sleep(0.6)
    left = leftovers(css)
    run.check(f"{name}: no untranslated text", not left, "; ".join(left[:12]))
    if teardown:
        teardown()
        time.sleep(0.3)


try:
    run.boot()
    d.set_window_size(1480, 920)
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id);")
    pid = d.js(f"return {S}.addProject(arguments[0])", proj)
    run.check("dictionaries loaded", len(VI) > 300, f"{len(VI)} entries")

    d.js(f"{S}.setSettings({{ language: 'vi' }})")
    run.check("html lang follows the language", wait(lambda: d.js("return document.documentElement.lang") == "vi"))
    run.shot("vi-start")

    surface("start page and sidebar", "body")
    surface("search everything (commands)", ".palette",
            lambda: (d.js(f"{S}.setOverlay('palette')"), wait(lambda: d.js("return !!document.querySelector('.palette input')")),
                     d.js("const i = document.querySelector('.palette input'); i.value = '>'; i.dispatchEvent(new InputEvent('input', {bubbles: true}))")),
            lambda: d.js(f"{S}.setOverlay(null)"))
    surface("dashboard", "body", lambda: d.js(f"{S}.setOverlay('dashboard')"), lambda: d.js(f"{S}.setOverlay(null)"))
    surface("workspace board", "body", lambda: d.js(f"{S}.setOverlay('board')"), lambda: d.js(f"{S}.setOverlay(null)"))
    surface("runs", "body", lambda: d.js(f"{S}.setOverlay('runs')"), lambda: d.js(f"{S}.setOverlay(null)"))

    # A terminal so the tab bar, pane tools and layout menu exist.
    d.js(f"{S}.newSession(arguments[0], 'shell')", pid)
    wait(lambda: d.js("return !!document.querySelector('.pane')"), 15)
    surface("workspace with a terminal", "body")
    surface("layout menu", "body", lambda: d.js("document.querySelector('.layout-btn').click()"), lambda: d.keys("escape"))
    for panel in ["files", "git", "history", "issues"]:
        surface(f"right panel: {panel}", "body", lambda p=panel: d.js(f"{S}.setRightPanel(arguments[0])", p))
    run.shot("vi-workspace")

    d.js(f"{S}.openSettings('agents')")
    wait(lambda: d.js("return !!document.querySelector('.settings-main')"))
    nav = d.js("return [...document.querySelectorAll('.settings-nav button, .settings-nav a, nav button')].map(b => b.innerText.trim()).filter(Boolean)")
    run.check("settings navigation is in Vietnamese", nav and not [n for n in nav if n in SUSPECT], str(nav))
    for sec in ["agents", "accounts", "integrations", "remote", "mobile", "commands", "automations", "voice", "terminal", "layouts",
                "appearance", "shortcuts", "notifications", "plugins", "usage", "daemon", "about"]:
        surface(f"settings: {sec}", ".settings-main", lambda s=sec: d.js(f"{S}.openSettings(arguments[0])", s))
    run.shot("vi-settings")
    d.js(f"{S}.openSettings('appearance')")
    time.sleep(0.5)
    run.shot("vi-appearance")

    # Dates.
    short = d.js("return document.querySelector('.sidebar') ? document.querySelector('.sidebar').innerText : ''")
    run.check("relative times are Vietnamese", "vừa xong" in short or "ph" in short or "giờ" in short, short[:200])

    # Back to English without a reload.
    d.js(f"{S}.setSettings({{ language: 'en' }})")
    ok = wait(lambda: "Appearance" in d.js("return document.body.innerText") and d.js("return document.documentElement.lang") == "en")
    run.check("switching back to English updates the open page", ok)
    run.check("setting is saved", d.js(f"return {S}.state.settings.language") == "en")
    d.js(f"{S}.setSettings({{ language: 'system' }})")
    sys_lang = d.js("return (navigator.languages?.[0] ?? navigator.language).toLowerCase().startsWith('vi') ? 'vi' : 'en'")
    run.check("System follows the computer's language", wait(lambda: d.js("return document.documentElement.lang") == sys_lang), sys_lang)
    d.js(f"{S}.setOverlay(null)")
except Exception as e:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to the end", False, repr(e))
finally:
    try:
        d.js(f"{S}.setSettings({{ language: 'en' }})")
    except Exception:
        pass
    run.finish()
