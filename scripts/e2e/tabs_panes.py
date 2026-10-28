"""E2E for the tabs ↔ panes model: a lone pane has no header (its tab names it), Tile all gathers
one-pane tabs into an even grid, Spread into tabs undoes it.

    python -X utf8 scripts/e2e/tabs_panes.py <screenshot-dir> <scratch-dir>

Scenario from the field: open 4 CLIs (4 tabs), tile them into 2 × 2, open a 5th (new tab), tile
again (3 + 2), split a cell, tile again, then spread everything back into tabs. Tabs that hold a
split are left alone. Uses shells only; Claude Code is checked when `claude` is on PATH.

Needs the dev environment (KELPIE_ENDPOINT / KELPIE_DATA_DIR set for both tauri-driver and this
script).
"""
import os
import shutil
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import Run, daemon  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
S = "window.__kelpie.store"
L = "window.__kelpie.layout"
proj = os.path.join(scratch, "tabs-panes-proj")
os.makedirs(proj, exist_ok=True)
with open(os.path.join(proj, "README.md"), "w", encoding="utf-8") as f:
    f.write("tabs and panes\n")

run = Run(shots)
d = run.d


def js_await(expr, *args):
    return d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)


def wait(cond, timeout=20, interval=0.25):
    end = time.time() + timeout
    while time.time() < end:
        v = cond()
        if v:
            return v
        time.sleep(interval)
    return None


def tabs():
    """[(tab id, [pane kind or session id…], grid line lengths)] of the active project."""
    return d.js(f"""
      const ws = {S}.workspace({S}.state.activeProject);
      const lines = (n) => n.type === 'leaf' ? [1] : n.children.map(c => c.type === 'leaf' ? 1 : {L}.allPanes(c).length);
      return ws.tabs.map(t => [t.id, {L}.allPanes(t.root).map(p => p.sessionId || p.kind), lines(t.root)]);
    """)


def active_tab():
    return d.js(f"return {S}.activeTab({S}.state.activeProject).id")


def alive(sid):
    return any(s["id"] == sid and s["alive"] for s in daemon(d, "session.list"))


def open_menu():
    d.click(".layout-btn")
    d.wait("return !!document.querySelector('.layout-menu')", 5)


def close_tab(tab_id):
    """Closes every pane of a tab (the tab goes with its last pane)."""
    js_await(f"(async () => {{ const p = {S}.state.activeProject; const t = {S}.workspace(p).tabs.find(x => x.id === arguments[0]); for (const pane of {L}.allPanes(t.root)) await {S}.closePane(p, t.id, pane.id); }})()", tab_id)


def new_shell(pid):
    info = js_await(f"{S}.newSession(arguments[0], 'shell')", pid)
    return info["id"]


try:
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    for s in daemon(d, "session.list"):
        daemon(d, "session.close", {"handle": s["handle"]})
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ shortcutHints: false, layouts: [], tips: false }});")
    pid = d.js(f"return {S}.addProject(arguments[0])", proj)

    # ---------- the even grid ----------
    grids = d.js(f"return [1,2,3,4,5,6,7,8,9,10,12,40].map(n => {L}.autoGrid(n).counts)")
    run.check("even grid: 1..10, 12, and a cap at 6 × 6",
              grids == [[1], [2], [2, 1], [2, 2], [3, 2], [3, 3], [3, 2, 2], [3, 3, 2], [3, 3, 3], [4, 3, 3], [4, 4, 4], [6, 6, 6, 6, 6, 6]], str(grids))

    # ---------- a lone pane: no header, tools in the hover toolbar ----------
    s1 = new_shell(pid)
    d.wait("return !!document.querySelector('.pane .term-host .xterm')", 20)
    run.check("lone pane: no pane header", d.js("return document.querySelectorAll('.pane[data-pane] .pane-header').length") == 0)
    run.check("lone pane: hover toolbar has split / close",
              d.js("return !!document.querySelector('.pane-tools button[title=\"Split right\"]') && !!document.querySelector('.pane-tools button[title=\"Close pane\"]')"))
    run.check("the tab names the pane", bool(d.js("return (document.querySelector('.tabs [data-tab-id] .title')?.textContent || '').trim()")))
    run.shot("01-lone-pane")

    # ---------- 4 CLIs in 4 tabs → Tile all → 2 × 2 ----------
    sids = [s1] + [new_shell(pid) for _ in range(3)]
    run.check("4 CLIs open as 4 tabs", len(tabs()) == 4, str(len(tabs())))
    first_tab = tabs()[0][0]
    d.js(f"{S}.selectTab({S}.state.activeProject, arguments[0])", first_tab)
    open_menu()
    run.shot("02-layout-menu-tile")
    hint = d.js("return document.querySelector('.layout-menu [data-action=tile] .muted-inline')?.textContent || ''")
    run.check("Tile all says how many tabs it will gather", "+3" in hint, hint)
    run.check("Spread is disabled for a lone pane", d.js("return document.querySelector('.layout-menu [data-action=spread]').disabled"))
    d.click(".layout-menu [data-action=tile]")
    wait(lambda: len(tabs()) == 1)
    tb = tabs()
    run.check("Tile all: one tab, 2 × 2, the 4 CLIs in tab order", len(tb) == 1 and tb[0][1] == sids and tb[0][2] == [2, 2], str(tb))
    run.check("Tile all keeps the tab that was tiled", tb and tb[0][0] == first_tab)
    run.check("every session still runs", all(alive(x) for x in sids))
    wait(lambda: d.js("return document.querySelectorAll('.pane[data-pane] .pane-header').length") == 4, 5)
    run.check("in a split every pane has a header", d.js("return document.querySelectorAll('.pane[data-pane] .pane-header').length") == 4)
    run.check("the tab shows one dot per pane", d.js("return document.querySelectorAll('.tabs [data-tab-id] .tab-panes i').length") == 4)
    run.shot("03-tiled-2x2")

    # ---------- a 5th CLI: new tab; Tile all again → 3 + 2 ----------
    s5 = new_shell(pid)
    tb = tabs()
    run.check("a 5th CLI opens in a new tab, the grid stays", len(tb) == 2 and tb[0][2] == [2, 2] and tb[1][1] == [s5] and active_tab() == tb[1][0], str(tb))
    run.shot("04-fifth-in-new-tab")
    d.js(f"{S}.selectTab({S}.state.activeProject, arguments[0])", first_tab)
    d.js("window.__kelpie.commands.find(c => c.id === 'layout.tile').run()")
    wait(lambda: len(tabs()) == 1)
    tb = tabs()
    run.check("Tile all (palette) again: 5 CLIs in 3 + 2", len(tb) == 1 and tb[0][1] == sids + [s5] and tb[0][2] == [3, 2], str(tb))
    run.shot("05-tiled-3-2")

    # ---------- split a cell: an empty launcher appears; Tile all evens it out ----------
    panes = d.js(f"return {L}.allPanes({S}.activeTab({S}.state.activeProject).root).map(p => p.id)")
    d.js(f"{S}.focusPane({S}.state.activeProject, arguments[0], arguments[1])", first_tab, panes[4])
    d.js(f"{S}.splitPicker({S}.state.activeProject, 'right')")
    d.wait("return !!document.querySelector('.empty-pane')", 5)
    d.js("[...document.querySelectorAll('.empty-pane .empty-pane-kind')].find(b => b.title === 'Terminal').click()")
    s6 = wait(lambda: next((p for p in tabs()[0][1] if p not in sids + [s5, "empty"]), None), 15)
    tb = tabs()
    run.check("split halves the focused cell (uneven: 3 + 3 with a half cell)", s6 is not None and tb[0][2] == [3, 3], str(tb))
    d.js("window.__kelpie.commands.find(c => c.id === 'layout.tile').run()")
    time.sleep(0.3)
    tb = tabs()
    sizes = d.js(f"const r = {S}.activeTab({S}.state.activeProject).root; return r.children.map(c => c.sizes.map(x => +x.toFixed(3)))")
    run.check("Tile all evens a lopsided grid: 3 + 3, equal cells", tb[0][2] == [3, 3] and all(all(abs(x - 1 / 3) < 0.01 for x in row) for row in sizes), f"{tb[0][2]} {sizes}")
    all_sids = sids + [s5, s6]

    # ---------- Spread into tabs ----------
    focus_sid = d.js(f"const t = {S}.activeTab({S}.state.activeProject); return {L}.findPane(t.root, t.focus).sessionId")
    open_menu()
    d.click(".layout-menu [data-action=spread]")
    wait(lambda: len(tabs()) == 6)
    tb = tabs()
    run.check("Spread: one tab per pane, every session kept", len(tb) == 6 and sorted(x[1][0] for x in tb) == sorted(all_sids), str(tb))
    run.check("Spread keeps the focused pane in the original tab", active_tab() == first_tab and tb[0][1] == [focus_sid], str(tb[0]))
    wait(lambda: d.js("return document.querySelectorAll('.pane[data-pane] .pane-header').length") == 0, 5)
    run.check("after Spread the lone pane has no header again", d.js("return document.querySelectorAll('.pane[data-pane] .pane-header').length") == 0)
    run.shot("06-spread")

    # ---------- a tab with a split is a deliberate layout: Tile all leaves it ----------
    d.js(f"{S}.selectTab({S}.state.activeProject, arguments[0])", tb[1][0])
    d.js(f"{S}.arrangeTab({S}.state.activeProject, arguments[0], {{ mode: 'rows', counts: [2] }})", tb[1][0])
    d.js("[...document.querySelectorAll('.empty-pane .empty-pane-kind')].find(b => b.title === 'Terminal').click()")
    wait(lambda: any(len(x[1]) == 2 and "empty" not in x[1] for x in tabs()), 15)
    split_tab = next(x for x in tabs() if len(x[1]) == 2)
    d.js(f"{S}.selectTab({S}.state.activeProject, arguments[0])", first_tab)
    d.js("window.__kelpie.commands.find(c => c.id === 'layout.tile').run()")
    time.sleep(0.3)
    tb = tabs()
    kept = next((x for x in tb if x[0] == split_tab[0]), None)
    run.check("Tile all gathers one-pane tabs only; the split tab stays", len(tb) == 2 and kept is not None and kept[1] == split_tab[1], str(tb))
    run.check("palette: Spread is offered for a tab with several panes",
              d.js("const c = window.__kelpie.commands.find(c => c.id === 'layout.spread'); return !!c && c.when()"))
    d.js(f"{S}.selectTab({S}.state.activeProject, arguments[0])", split_tab[0])
    d.js("window.__kelpie.commands.find(c => c.id === 'layout.spread').run()")
    run.check("palette: Spread splits the 2-pane tab into 2 tabs", len(tabs()) == 3, str(len(tabs())))
    # A lone pane with nothing else to gather: Tile all would change nothing, so it is not offered.
    d.js(f"{S}.spreadTab({S}.state.activeProject, arguments[0])", first_tab)
    for x in tabs():
        if x[0] != first_tab:
            close_tab(x[0])
    wait(lambda: len(tabs()) == 1, 10)
    d.js(f"{S}.selectTab({S}.state.activeProject, arguments[0])", first_tab)
    run.check("palette: Tile all is not offered for a lone pane with nothing to gather",
              len(tabs()) == 1 and not d.js("return window.__kelpie.commands.find(c => c.id === 'layout.tile').when()"), str(tabs()))

    # ---------- only CLI tabs join; broadcast goes off when tabs join ----------
    js_await(f"{S}.openFile(arguments[0], arguments[1])", pid, os.path.join(proj, "README.md"))
    d.js(f"{S}.splitPicker(arguments[0], 'tab')", pid)
    s7 = new_shell(pid)
    file_and_empty = [x[0] for x in tabs() if x[1][0] in ("file", "empty")]
    run.check("set-up: a file tab and an empty launcher tab next to the CLIs", len(file_and_empty) == 2, str(tabs()))
    d.js(f"{S}.selectTab({S}.state.activeProject, arguments[0]); {S}.setBroadcast({S}.state.activeProject, arguments[0], true)", first_tab)
    d.js("window.__kelpie.commands.find(c => c.id === 'layout.tile').run()")
    tb = tabs()
    grid = next((x for x in tb if x[0] == first_tab), None)
    run.check("Tile all gathers the CLI tab only; file and empty tabs stay",
              grid is not None and s7 in grid[1] and all(any(x[0] == f for x in tb) for f in file_and_empty) and len(tb) == 3, str(tb))
    run.check("broadcast is turned off when other tabs join",
              d.js(f"return !{S}.workspace({S}.state.activeProject).tabs.find(t => t.id === arguments[0]).broadcast", first_tab))
    for x in tabs():
        if x[0] != first_tab:
            close_tab(x[0])

    # ---------- Claude Code: chat switch in the hover toolbar, not a header ----------
    if shutil.which("claude"):
        info = js_await(f"{S}.newSession(arguments[0], 'claude')", pid)
        d.wait("return !!document.querySelector('.pane .term-host .xterm')", 20)
        lone = d.js(f"return {L}.allPanes({S}.activeTab({S}.state.activeProject).root).length") == 1
        run.check("Claude alone in its tab: no pane header", lone and d.js("return document.querySelectorAll('.pane[data-pane] .pane-header').length") == 0)
        run.check("Claude alone in its tab: 'Open in chat view' in the hover toolbar", bool(wait(lambda: d.js("return !!document.querySelector('.pane-tools button[title=\"Open in chat view\"]')"), 10)))
        run.shot("07-claude-lone")
        daemon(d, "session.close", {"handle": next(s["handle"] for s in daemon(d, "session.list") if s["id"] == info["id"])})
    else:
        print("note: claude not on PATH, chat-switch check skipped")

    for s in daemon(d, "session.list"):
        daemon(d, "session.close", {"handle": s["handle"]})
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    run.finish()
