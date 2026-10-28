"""E2E for grid layouts and resuming sessions after the session daemon restarts.

    python -X utf8 scripts/e2e/layouts.py <screenshot-dir> <scratch-dir> [--ai]

Layouts: layout tree operations, Layout menu presets and rows × columns picker, per-row counts,
empty-cell launcher, gutter drag, drag a pane onto another (swap / move), spatial focus,
broadcast input, grid launch from the new-session dialog, saved layouts (save, open, rename,
delete), tab rename, state v3 round trip.
Resume: the daemon is stopped with sessions running and started again; sessions come back
`interrupted` and resume when shown / all at once / only on request. Shells restart below their
old screen. `--ai` also resumes a real Claude Code conversation (terminal and chat view) and
checks the agent still knows what was said before the restart.

Needs the dev environment (KELPIE_ENDPOINT / KELPIE_DATA_DIR set for both tauri-driver and this
script): the script stops the dev daemon and starts the release `kelpied` itself.
"""
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import REPO, Run, daemon, restart_daemon as support_restart  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
with_ai = "--ai" in sys.argv
S = "window.__kelpie.store"
L = "window.__kelpie.layout"
DOWN, ENTER = chr(27) + "[B", chr(13)
kelpied = os.path.join(REPO, "target", "release", "kelpied.exe" if os.name == "nt" else "kelpied")
proj = os.path.join(scratch, "layouts-proj")
os.makedirs(proj, exist_ok=True)
with open(os.path.join(proj, "README.md"), "w", encoding="utf-8") as f:
    f.write("layouts\n")

run = Run(shots)
d = run.d


def js_await(expr, *args):
    return d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)


def screen(handle, lines=80):
    return daemon(d, "session.read", {"handle": handle, "lines": lines})["text"]


def wait(cond, timeout=30, interval=0.3):
    end = time.time() + timeout
    while time.time() < end:
        v = cond()
        if v:
            return v
        time.sleep(interval)
    return None


def sess(sid):
    return next((s for s in daemon(d, "session.list") if s["id"] == sid), None)


def tab_panes():
    return d.js(f"const p = {S}.state.activeProject; const t = {S}.activeTab(p); return t ? {L}.allPanes(t.root) : [];")


def rect(css, i=0):
    return d.js("const el = document.querySelectorAll(arguments[0])[arguments[1]]; if (!el) return null; const r = el.getBoundingClientRect(); return {x: r.left, y: r.top, w: r.width, h: r.height};", css, i)


def restart_daemon(kill=False):
    """Stops the daemon (sessions still running unless `kill`); a new one takes over."""
    support_restart(d, kelpied, kill)

try:
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 240000})
    for s in daemon(d, "session.list"):
        daemon(d, "session.close", {"handle": s["handle"]})
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ shortcutHints: false, layouts: [], resumeSessions: 'shown', hibernate: false, tips: false }});")
    pid = d.js(f"return {S}.addProject(arguments[0])", proj)

    # ---------- pure layout operations ----------
    r = d.js(f"""
      const L = {L};
      const P = (id) => ({{ id, kind: 'empty' }});
      const out = {{}};
      const g = L.buildGrid({{ mode: 'rows', counts: [2, 3] }}, [P('a'), P('b')], () => P('x' + Math.random()));
      out.grid = [g.type, g.dir, g.children.length, g.children.map(c => c.children.length), L.allPanes(g).length, L.allPanes(g).slice(0, 2).map(p => p.id)];
      const cols = L.buildGrid({{ mode: 'cols', counts: [1, 2] }}, [P('m'), P('n'), P('o')], () => P('z'));
      out.cols = [cols.dir, cols.children[0].type, cols.children[1].dir, cols.children[1].children.length];
      // Splitting inside a same-direction split adds a sibling, not a nested split.
      let t = L.leaf(P('a'));
      t = L.splitPane(t, 'a', P('b'), 'row');
      t = L.splitPane(t, 'b', P('c'), 'row');
      out.flat = [t.children.length, t.sizes.map(x => +x.toFixed(3))];
      // Removing collapses and flattens.
      let u = L.splitPane(t, 'b', P('d'), 'col');
      u = L.removePane(u, 'd');
      out.collapse = [u.children.length, u.children.every(c => c.type === 'leaf'), +u.sizes.reduce((a, b) => a + b, 0).toFixed(6)];
      const moved = L.moveGutter(t, t.id, 0, 0.1);
      out.gutter = moved.sizes.map(x => +x.toFixed(3));
      const swapped = L.swapPanes(t, 'a', 'c');
      out.swap = L.allPanes(swapped).map(p => p.id);
      const mv = L.movePane(t, 'a', 'c', 'bottom');
      out.move = [L.allPanes(mv).map(p => p.id), mv.children.length];
      const legacy = {{ type: 'split', id: 's1', dir: 'row', ratio: 0.3, a: {{ type: 'leaf', id: 'l1', pane: P('a') }},
        b: {{ type: 'split', id: 's2', dir: 'row', ratio: 0.5, a: {{ type: 'leaf', id: 'l2', pane: P('b') }}, b: {{ type: 'leaf', id: 'l3', pane: P('c') }} }} }};
      const m = L.migrateLayout(legacy);
      out.migrate = [m.children.length, m.sizes.map(x => +x.toFixed(3))];
      const tpl = L.toTemplate(g, (p) => ({{ kind: p.id === 'a' ? 'shell' : null }}));
      out.tpl = L.templateKinds(tpl);
      const back = L.fromTemplate(tpl, (k) => ({{ id: 'n' + Math.random(), kind: k ? 'terminal' : 'empty' }}));
      out.tplShape = [back.children.length, back.children.map(c => c.children.length)];
      out.eq = L.equalize(moved).sizes.map(x => +x.toFixed(3));
      return out;
    """)
    run.check("grid 2+3 builds rows of 2 and 3 cells, panes in reading order", r["grid"] == ["split", "col", 2, [2, 3], 5, ["a", "b"]], str(r["grid"]))
    run.check("columns 1+2: tall pane left, two stacked right", r["cols"] == ["row", "leaf", "col", 2], str(r["cols"]))
    run.check("splitting within a same-direction split adds a sibling", r["flat"] == [3, [0.5, 0.25, 0.25]], str(r["flat"]))
    run.check("removing a pane collapses and flattens the tree", r["collapse"] == [3, True, 1.0], str(r["collapse"]))
    run.check("gutter drag clamps and keeps the pair's total", r["gutter"] == [0.1, 0.65, 0.25], str(r["gutter"]))
    run.check("swap exchanges two panes", r["swap"] == ["c", "b", "a"], str(r["swap"]))
    run.check("move puts a pane below another", r["move"][0] == ["b", "c", "a"] and r["move"][1] == 2, str(r["move"]))
    run.check("binary layouts from older state migrate to n-ary", r["migrate"] == [3, [0.3, 0.35, 0.35]], str(r["migrate"]))
    run.check("templates keep shape and kinds", r["tpl"][:2] == ["shell", None] and len(r["tpl"]) == 5 and r["tplShape"] == [2, [2, 3]], str(r))
    run.check("equalize gives equal shares", r["eq"] == [0.333, 0.333, 0.333], str(r["eq"]))

    # ---------- arrange a tab with the Layout menu ----------
    info = js_await(f"{S}.newSession(arguments[0], 'shell')", pid)
    first_sid = info["id"]
    d.wait("return !!document.querySelector('.pane .term-host .xterm')", 20)
    d.click(".layout-btn")
    d.wait("return !!document.querySelector('.layout-menu')", 5)
    run.shot("01-layout-menu")
    presets = d.js("return document.querySelectorAll('.layout-menu .layout-preset').length")
    run.check("layout menu shows grid presets", presets == 10, str(presets))
    d.click(".layout-menu .layout-preset:nth-child(7)")  # 2 × 2
    d.wait("return document.querySelectorAll('.pane[data-pane]').length === 4", 5)
    panes = tab_panes()
    run.check("2 × 2 preset: 4 panes, the terminal first, 3 empty cells", len(panes) == 4 and panes[0].get("sessionId") == first_sid and sum(p["kind"] == "empty" for p in panes) == 3)
    run.check("empty cells show a launcher", d.js("return document.querySelectorAll('.empty-pane .empty-pane-kind').length") >= 2)
    run.shot("02-grid-2x2-empty")
    # Start a shell in the second cell from its launcher.
    d.js("[...document.querySelectorAll('.pane[data-pane]')][1].querySelector('.empty-pane-kind[title=\"Terminal\"]').click();")
    second = wait(lambda: (lambda p: p[1].get("sessionId") if p[1]["kind"] == "terminal" else None)(tab_panes()), 15)
    run.check("launcher in an empty cell starts a session in that cell", bool(second))
    second_kind = sess(second)["kind"] if second else None

    # Rows × columns picker: 2 rows × 3 columns.
    d.click(".layout-btn")
    d.wait("return !!document.querySelector('.layout-menu')", 5)
    d.js("const cells = document.querySelectorAll('.grid-picker-cell'); cells[1 * 6 + 2].dispatchEvent(new PointerEvent('pointerenter')); cells[1 * 6 + 2].click();")
    d.wait("return document.querySelectorAll('.pane[data-pane]').length === 6", 5)
    shape = d.js(f"const t = {S}.activeTab({S}.state.activeProject); return [t.root.dir, t.root.children.map(c => c.children.length)];")
    run.check("picker 2 rows × 3 columns", shape == ["col", [3, 3]], str(shape))
    # Per-row counts: 1, 3.
    d.click(".layout-btn")
    d.wait("return !!document.querySelector('.layout-menu')", 5)
    d.js("const i = document.querySelector('.layout-custom input'); i.value = '1, 3'; i.dispatchEvent(new InputEvent('input', {bubbles: true}));")
    d.wait("return !!document.querySelector('.layout-custom-preview')", 5)
    d.js("document.querySelector('.layout-custom .btn').click()")
    d.wait("return document.querySelectorAll('.pane[data-pane]').length === 4", 5)
    shape = d.js(f"const t = {S}.activeTab({S}.state.activeProject); return [t.root.dir, t.root.children.map(c => c.type === 'leaf' ? 1 : c.children.length)];")
    run.check("custom per-row counts 1 + 3", shape == ["col", [1, 3]], str(shape))
    run.check("live sessions kept through re-arranging", all(sess(x) and sess(x)["alive"] for x in (first_sid, second)))
    run.shot("03-grid-1-3")

    # Back to 1 × 2 for drag tests.
    d.js(f"{S}.arrangeTab({S}.state.activeProject, {S}.activeTab({S}.state.activeProject).id, {{ mode: 'rows', counts: [2] }})")
    d.wait("return document.querySelectorAll('.pane[data-pane]').length === 2", 5)
    time.sleep(0.5)
    g = rect(".split.row > .gutter")
    w0 = rect(".pane[data-pane]", 0)["w"]
    d.drag(g["x"] + 0.5, g["y"] + g["h"] / 2, g["x"] - 200, g["y"] + g["h"] / 2)
    w1 = rect(".pane[data-pane]", 0)["w"]
    run.check("dragging the gutter resizes panes", w0 - w1 > 150, f"{w0:.0f} -> {w1:.0f}")
    d.js("document.querySelector('.split.row > .gutter').dispatchEvent(new MouseEvent('dblclick', {bubbles: true}))")
    time.sleep(0.3)
    sizes = d.js(f"return {S}.activeTab({S}.state.activeProject).root.sizes")
    run.check("double-clicking a gutter equalizes", abs(sizes[0] - 0.5) < 0.01, str(sizes))

    # Drag pane 1's header onto pane 2's centre: swap.
    before = [p.get("sessionId") for p in tab_panes()]
    h = rect(".pane[data-pane] .pane-header", 0)
    t2 = rect(".pane[data-pane]", 1)
    d.drag(h["x"] + 60, h["y"] + h["h"] / 2, t2["x"] + t2["w"] / 2, t2["y"] + t2["h"] / 2)
    after = [p.get("sessionId") for p in tab_panes()]
    run.check("dropping a pane on another's centre swaps them", after == list(reversed(before)), f"{before} -> {after}")
    # Drag pane 1 to the bottom half of pane 2: stacked.
    h = rect(".pane[data-pane] .pane-header", 0)
    t2 = rect(".pane[data-pane]", 1)
    d.drag(h["x"] + 60, h["y"] + h["h"] / 2, t2["x"] + t2["w"] / 2, t2["y"] + t2["h"] - 12)
    dir_ = d.js(f"return {S}.activeTab({S}.state.activeProject).root.dir")
    run.check("dropping on a pane's bottom edge stacks them", dir_ == "col", str(dir_))

    # Spatial focus: stacked panes, focus up / down.
    panes = tab_panes()
    d.js(f"{S}.focusPane({S}.state.activeProject, {S}.activeTab({S}.state.activeProject).id, arguments[0])", panes[1]["id"])
    d.js("window.__kelpie.commands.find(c => c.id === 'pane.up').run()")
    run.check("Focus Pane Up moves focus to the pane above", d.js(f"return {S}.activeTab({S}.state.activeProject).focus") == panes[0]["id"])
    d.js("window.__kelpie.commands.find(c => c.id === 'pane.down').run()")
    run.check("Focus Pane Down moves focus back", d.js(f"return {S}.activeTab({S}.state.activeProject).focus") == panes[1]["id"])

    # Broadcast input: typed once, both shells receive it.
    tab_id = d.js(f"return {S}.activeTab({S}.state.activeProject).id")
    d.js(f"{S}.setBroadcast({S}.state.activeProject, arguments[0], true)", tab_id)
    d.wait("return document.querySelectorAll('.pane.broadcast').length === 2", 5)
    marker = f"bcast{int(time.time())}"
    d.js("const ta = document.querySelectorAll('.pane[data-pane] .xterm-helper-textarea')[0]; ta.focus();")
    d.keys(f"echo {marker}", "enter")
    h1, h2 = sess(panes[0]["sessionId"])["handle"], sess(panes[1]["sessionId"])["handle"]
    ok = wait(lambda: screen(h1).count(marker) >= 2 and screen(h2).count(marker) >= 2, 15)
    run.check("broadcast input reaches every terminal of the tab", bool(ok))
    run.shot("04-broadcast")
    d.js(f"{S}.setBroadcast({S}.state.activeProject, arguments[0], false)", tab_id)
    marker2 = f"single{int(time.time())}"
    d.js("const ta = document.querySelectorAll('.pane[data-pane] .xterm-helper-textarea')[0]; ta.focus();")
    d.keys(f"echo {marker2}", "enter")
    time.sleep(2)
    run.check("broadcast off: only the focused terminal receives input", marker2 in screen(h1) and marker2 not in screen(h2))

    # Save the layout, rename the tab, open the saved layout in a new tab.
    d.js(f"{S}.saveLayout({S}.state.activeProject, arguments[0], 'Two shells stacked')", tab_id)
    layouts = d.js(f"return {S}.state.settings.layouts")
    run.check("layout saved with its shape and kinds", len(layouts) == 1 and layouts[0]["template"]["dir"] == "col" and [c["kind"] for c in layouts[0]["template"]["children"]] == ["shell", "shell"], json.dumps(layouts)[:200])
    d.js(f"{S}.renameTab({S}.state.activeProject, arguments[0], 'Builds')", tab_id)
    run.check("tab renamed", d.js("return [...document.querySelectorAll('.tab .title')].some(e => e.textContent.includes('Builds'))"))
    ntabs = d.js(f"return {S}.workspace({S}.state.activeProject).tabs.length")
    js_await(f"{S}.applyLayout({S}.state.activeProject, {S}.state.settings.layouts[0].template)")
    d.wait(f"return {S}.workspace({S}.state.activeProject).tabs.length === {ntabs + 1}", 10)
    newp = tab_panes()
    run.check("saved layout opens in a new tab with fresh sessions", len(newp) == 2 and all(p.get("sessionId") and p["sessionId"] not in (first_sid, second) for p in newp))
    run.check("saved layout's sessions are running", bool(wait(lambda: all(sess(p["sessionId"]) and sess(p["sessionId"])["alive"] for p in newp), 10)))
    # Layout settings page lists it; rename + delete.
    d.js(f"{S}.openSettings('layouts')")
    d.wait("return !!document.querySelector('.saved-layout')", 5)
    run.shot("05-settings-layouts")
    lid = layouts[0]["id"]
    d.js(f"{S}.renameLayout(arguments[0], 'Stacked')", lid)
    run.check("layout renamed", d.js(f"return {S}.state.settings.layouts[0].name") == "Stacked")
    d.js(f"{S}.setOverlay(null)")

    # Grid launch from the new-session dialog: 2 × 2 shells.
    d.js(f"{S}.setOverlay('newsession')")
    d.wait("return !!document.querySelector('.modal')", 5)
    d.js("[...document.querySelectorAll('.modal .launch-card')].find(c => c.textContent.includes('Terminal') || c.textContent.includes('Shell')).click()")
    d.js("[...document.querySelectorAll('.modal .toggle')].find(b => b.textContent.startsWith('Grid')).click()")
    d.wait("return !!document.querySelector('.modal .layout-presets')", 5)
    d.js("[...document.querySelectorAll('.modal .layout-preset')].find(b => b.title === '2 × 2').click()")
    run.shot("06-newsession-grid")
    ntabs = d.js(f"return {S}.workspace({S}.state.activeProject).tabs.length")
    d.js("[...document.querySelectorAll('.modal .btn.primary')].find(b => b.textContent.includes('Start')).click()")
    d.wait(f"return {S}.workspace({S}.state.activeProject).tabs.length === {ntabs + 1}", 15)
    gp = tab_panes()
    run.check("grid launch: new tab with 4 sessions of the chosen kind", len(gp) == 4 and all(p["kind"] == "terminal" for p in gp))
    run.check("grid sessions all running", bool(wait(lambda: all(sess(p["sessionId"]) and sess(p["sessionId"])["alive"] for p in gp), 15)))
    run.shot("07-grid-launch")
    d.js(f"{S}.deleteLayout(arguments[0])", lid)
    run.check("layout deleted", d.js(f"return {S}.state.settings.layouts.length") == 0)

    # State file: v3 with n-ary splits.
    time.sleep(1)
    data_dir = d.js_async("const done = arguments[arguments.length-1]; window.__TAURI_INTERNALS__.invoke('platform_info').then(i => done(i.data_dir));")
    st = json.load(open(os.path.join(data_dir, "app-state.json"), encoding="utf-8"))
    roots = [t["root"] for w in st["workspaces"].values() for t in w["tabs"]]
    run.check("state saved as v3 with children/sizes splits", st["version"] == 3 and any(r.get("type") == "split" and "children" in r and "a" not in r for r in roots))

    # ---------- resume after the daemon restarts ----------
    # Close the grid tab's shells; keep: tab 1 (two shells, visible) and the saved-layout tab.
    for p in gp:
        s = sess(p["sessionId"])
        if s:
            daemon(d, "session.close", {"handle": s["handle"]})
    d.js(f"{S}.selectTab({S}.state.activeProject, arguments[0])", tab_id)
    visible = [p["sessionId"] for p in tab_panes()]
    hidden = [p["sessionId"] for p in newp]
    mark = f"before-restart-{int(time.time())}"
    daemon(d, "session.input", {"handle": sess(visible[0])["handle"], "data": f"echo {mark}\r"})
    wait(lambda: screen(sess(visible[0])["handle"]).count(mark) >= 2, 10)
    time.sleep(1)
    restart_daemon()
    after = {x: sess(x) for x in visible + hidden}
    # Visible ones may already be resuming (the app reacts as soon as it reconnects).
    run.check("after a daemon restart, sessions that ran come back as interrupted", all(a and (a["interrupted"] or (x in visible and a["alive"])) for x, a in after.items()) and all(after[x]["interrupted"] for x in hidden), str({k[:6]: (v or {}).get("interrupted") for k, v in after.items()}))
    ok = wait(lambda: all(sess(x)["alive"] for x in visible), 20)
    run.check("'When shown': visible sessions resume by themselves", bool(ok))
    run.check("'When shown': sessions in other tabs wait", all(not sess(x)["alive"] for x in hidden))
    txt = screen(sess(visible[0])["handle"], 200)
    run.check("a resumed shell keeps its old screen above the new prompt", mark in txt and "--- restarted ---" in txt and txt.index(mark) < txt.index("--- restarted ---"))
    run.check("resumed sessions are no longer flagged interrupted", not sess(visible[0])["interrupted"])
    run.shot("08-resumed-visible")
    d.js(f"const p = {S}.state.activeProject; const t = {S}.workspace(p).tabs.find(t => {L}.allPanes(t.root).some(x => x.sessionId === arguments[0])); {S}.selectTab(p, t.id);", hidden[0])
    ok = wait(lambda: all(sess(x)["alive"] for x in hidden), 20)
    run.check("showing the other tab resumes its sessions", bool(ok))

    # Mode 'all'.
    d.js(f"{S}.setSettings({{ resumeSessions: 'all' }})")
    d.js(f"{S}.selectTab({S}.state.activeProject, arguments[0])", tab_id)
    time.sleep(0.5)
    restart_daemon()
    ok = wait(lambda: all(sess(x)["alive"] for x in visible + hidden), 25)
    run.check("'All at once': every interrupted session resumes, shown or not", bool(ok))

    # Mode 'off': nothing restarts until Resume is clicked.
    d.js(f"{S}.setSettings({{ resumeSessions: 'off' }})")
    restart_daemon()
    time.sleep(3)
    run.check("'Off': sessions stay stopped", all(not sess(x)["alive"] for x in visible + hidden))
    d.wait("return [...document.querySelectorAll('.exit-card .msg')].some(e => e.textContent.includes('session daemon stopped'))", 10)
    run.shot("09-resume-off")
    d.js("[...document.querySelectorAll('.exit-card .btn.primary')][0].click()")
    ok = wait(lambda: sum(1 for x in visible if sess(x)["alive"]) == 1, 15)
    run.check("the Resume button restarts that session", bool(ok))

    # Stopped on purpose: not resumed after a restart. (Interrupted ones never resumed stay
    # interrupted across restarts, so resume them first.)
    d.js(f"{S}.setSettings({{ resumeSessions: 'all' }})")
    d.js(f"for (const id of arguments[0]) void {S}.resumeSession(id)", visible + hidden)
    wait(lambda: all(sess(x)["alive"] for x in visible + hidden), 20)
    for x in visible + hidden:
        s = sess(x)
        if s["alive"]:
            daemon(d, "session.kill", {"handle": s["handle"]})
    wait(lambda: all(not sess(x)["alive"] for x in visible + hidden), 10)
    time.sleep(1.5)
    restart_daemon()
    time.sleep(3)
    run.check("sessions that had already exited are not resumed", all(not sess(x)["alive"] and not sess(x)["interrupted"] for x in visible + hidden))

    if with_ai:
        d.js(f"{S}.setSettings({{ resumeSessions: 'shown' }})")
        word = f"PAPAYA{int(time.time()) % 10000}"
        info = js_await(f"{S}.newSession(arguments[0], 'claude', {{ args: ['--model', 'haiku'], prompt: arguments[1] }})", pid, f"Remember this code word: {word}. Reply with just OK.")
        cid = info["id"]
        # A folder Claude has not seen asks to be trusted first (answer: "Yes, I trust").
        if wait(lambda: "trust this folder" in screen(sess(cid)["handle"]) or sess(cid).get("agent_session_id"), 40) and not sess(cid).get("agent_session_id"):
            daemon(d, "session.input", {"handle": sess(cid)["handle"], "data": DOWN})
            time.sleep(0.4)
            daemon(d, "session.input", {"handle": sess(cid)["handle"], "data": ENTER})
        conv = wait(lambda: (sess(cid) or {}).get("agent_session_id"), 60)
        run.check("claude reported its conversation id", bool(conv))
        # The reply line ("● OK"): the turn is saved, so it can be resumed.
        run.check("first turn answered", bool(wait(lambda: "● OK" in screen(sess(cid)["handle"]), 240, 1)))
        time.sleep(2)
        restart_daemon()
        ok = wait(lambda: sess(cid)["alive"], 30)
        run.check("claude session resumes after the restart", bool(ok))
        run.check("same conversation id after resume", sess(cid)["agent_session_id"] == conv)
        # Resumed: the earlier turn is on screen again; then let its startup hooks finish.
        run.check("resumed claude shows the earlier conversation", bool(wait(lambda: "● OK" in screen(sess(cid)["handle"]) and word in screen(sess(cid)["handle"]), 120, 1)))
        wait(lambda: "SessionStart hooks" not in screen(sess(cid)["handle"]), 120, 1)
        time.sleep(3)
        daemon(d, "session.input", {"handle": sess(cid)["handle"], "data": "What was the code word? Reply with just the word."})
        time.sleep(0.6)
        daemon(d, "session.input", {"handle": sess(cid)["handle"], "data": "\r"})
        # Any wording, as long as the answer (after the question) has the word.
        got = wait(lambda: word in screen(sess(cid)["handle"], 200).split("What was the code word?")[-1], 240, 1)
        run.check("resumed claude still knows what was said before the restart", bool(got))
        run.shot("10-claude-resumed")
        daemon(d, "session.close", {"handle": sess(cid)["handle"]})

        # Chat view: same check through a chat session.
        chat = js_await("window.__kelpie.chat.newChat(arguments[0], 'claude')", pid)
        chid = chat["id"]
        d.wait("return !!document.querySelector('.chat-composer textarea')", 20)
        d.js("const t = document.querySelector('.chat-composer textarea'); t.value = arguments[0]; t.dispatchEvent(new InputEvent('input', {bubbles: true}));", f"Remember the word {word}X. Reply with just OK.")
        d.js("document.querySelector('.chat-composer .btn.primary').click()")
        wait(lambda: (sess(chid) or {}).get("agent_session_id") and d.js("return document.querySelectorAll('.msg-assistant').length") >= 1, 120, 1)
        time.sleep(3)
        restart_daemon()
        new_chat = wait(lambda: d.js(f"const p = {S}.state.activeProject; const t = {S}.activeTab(p); const pane = {L}.allPanes(t.root).find(x => x.kind === 'chat'); return pane && pane.sessionId !== arguments[0] && {S}.sessions[pane.sessionId]?.alive ? pane.sessionId : null;", chid), 40)
        run.check("chat session resumes in the same pane with a new front-end", bool(new_chat))
        if new_chat:
            d.wait("return !!document.querySelector('.chat-composer textarea:not([disabled])')", 30)
            d.js("const t = document.querySelector('.chat-composer textarea'); t.value = 'What word did I ask you to remember? Reply with just the word.'; t.dispatchEvent(new InputEvent('input', {bubbles: true}));")
            d.js("document.querySelector('.chat-composer .btn.primary').click()")
            got = wait(lambda: d.js("return [...document.querySelectorAll('.msg-assistant')].some(e => e.textContent.includes(arguments[0]))", f"{word}X"), 120, 1)
            run.check("resumed chat still knows the earlier conversation", bool(got))
            run.shot("11-chat-resumed")
            daemon(d, "session.close", {"handle": sess(new_chat)["handle"]})

    for s in daemon(d, "session.list"):
        daemon(d, "session.close", {"handle": s["handle"]})
    d.js(f"{S}.setSettings({{ resumeSessions: 'shown' }})")
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    run.finish()
