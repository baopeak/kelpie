"""E2E for the split picker and arranging tabs and panes.

    python -X utf8 scripts/e2e/split_picker.py <screenshot-dir> <scratch-dir>

A split opens an empty pane that asks what to run: a click or key 1-9 starts that kind in place
(same pane id), Esc closes it, "More…" opens the dialog with "This pane". Tabs reorder (store,
palette, pointer drag); a pane moves to another tab or a new tab (store, header drag onto a tab or
the empty tab bar); a tab merges beside a pane (store, tab drag onto a pane edge). After each
operation the tree is checked: shares sum to 1, and no split sits directly in a split of the same
direction.

Starts no agent: the picker is only driven to "Terminal" (the dialog's default kind is Claude).
Needs the dev environment as the other scripts do (KELPIE_ENDPOINT / KELPIE_DATA_DIR set for both
tauri-driver and this script).
"""
import os
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import Run, daemon  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
S = "window.__kelpie.store"
proj = os.path.join(scratch, "split-picker-proj")
os.makedirs(proj, exist_ok=True)
with open(os.path.join(proj, "README.md"), "w", encoding="utf-8") as f:
    f.write("split picker\n")

run = Run(shots)
d = run.d
_n = iter(range(10**6))


def new_id():
    return f"sp{next(_n)}"


def js_await(expr, *args):
    return d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)


def sessions():
    return daemon(d, "session.list")


def wait(cond, timeout=15, interval=0.25):
    end = time.time() + timeout
    while time.time() < end:
        v = cond()
        if v:
            return v
        time.sleep(interval)
    return None


def snap():
    """Tabs of the active project: id, focus, layout tree."""
    return d.js(f"const p = {S}.state.activeProject; return {S}.workspace(p).tabs.map(t => ({{id: t.id, focus: t.focus, root: t.root}}));")


def tab_of(tab_id):
    return next((t for t in snap() if t["id"] == tab_id), None)


def tab_order():
    return [t["id"] for t in snap()]


def leaves(node):
    return [node["pane"]] if node["type"] == "leaf" else [p for c in node["children"] for p in leaves(c)]


def pane_ids(tab_id):
    t = tab_of(tab_id)
    return [p["id"] for p in leaves(t["root"])] if t else []


def pane_obj(tab_id, pane_id):
    t = tab_of(tab_id)
    return next((p for p in leaves(t["root"]) if p["id"] == pane_id), None) if t else None


def problems(node, out=None):
    """Layout invariants: shares sum to 1; no split directly inside a split of the same direction."""
    out = [] if out is None else out
    if node["type"] == "leaf":
        return out
    total = sum(node["sizes"])
    if abs(total - 1) > 0.01:
        out.append(f"sizes sum {total:.3f}")
    for c in node["children"]:
        if c["type"] == "split" and c["dir"] == node["dir"]:
            out.append(f"nested {node['dir']} split")
        problems(c, out)
    return out


def new_tab(ids):
    """A tab (selected) whose first pane is ids[0]; the rest split to its right. Empty panes only."""
    return d.js(f"""const S = {S}; const p = S.state.activeProject; const ids = arguments[0];
      const t = S.addTab(p, {{id: ids[0], kind: 'empty'}});
      for (const id of ids.slice(1)) S.placePane(p, {{id, kind: 'empty'}}, 'right');
      return t.id;""", ids)


def empty_panes():
    return d.js("return document.querySelectorAll('.empty-pane').length")


def picker_kinds():
    return d.js("return [...document.querySelectorAll('.empty-pane .empty-pane-kind')].map(b => b.title)")


def pane_dom_id():
    return d.js("const e = document.querySelector('.empty-pane'); return e ? e.closest('.pane').dataset.pane : null;")


def has_term(pane_id):
    return d.js("return !!document.querySelector('.pane[data-pane=\"' + arguments[0] + '\"] .term-host .xterm')", pane_id)


def rect(css, i=0):
    return d.js("const el = document.querySelectorAll(arguments[0])[arguments[1]]; if (!el) return null; const r = el.getBoundingClientRect(); return {x: r.left, y: r.top, w: r.width, h: r.height};", css, i)


def tab_rect(tab_id):
    return d.js("const el = document.querySelector('[data-tab-id=\"' + arguments[0] + '\"]'); if (!el) return null; const r = el.getBoundingClientRect(); return {x: r.left, y: r.top, w: r.width, h: r.height};", tab_id)


def view_state(sid):
    """The terminal view of a session (window.__kelpie.views lists the live TermViews)."""
    return d.js("const v = window.__kelpie.views().find(x => x.sessionId === arguments[0]); return v ? {visible: v.visible, attached: v.attached, handle: v.handle} : null;", sid)


def view_text(sid):
    return d.js("const v = window.__kelpie.views().find(x => x.sessionId === arguments[0]); if (!v) return null; const b = v.term.buffer.active; const out = []; for (let i = 0; i < b.length; i++) out.push(b.getLine(i)?.translateToString(true) ?? ''); return out.join('\\n');", sid)


def dom_tab_order():
    return d.js("return [...document.querySelectorAll('.tabs [data-tab-id]')].map(e => e.dataset.tabId);")


try:
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 240000})
    for s in sessions():
        daemon(d, "session.close", {"handle": s["handle"]})
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ shortcutHints: false, layouts: [], resumeSessions: 'shown', hibernate: false, tips: false }});")
    pid = d.js(f"return {S}.addProject(arguments[0])", proj)
    js_await(f"{S}.newSession(arguments[0], 'shell')", pid)
    d.wait("return !!document.querySelector('.pane .term-host .xterm')", 20)
    base = d.js(f"return {S}.activeTab({S}.state.activeProject).id")
    n0 = len(sessions())

    # ---------- split entry points open an empty picker pane, no session ----------
    d.js("[...document.querySelectorAll('.tabbar-actions button')].find(b => b.title.startsWith('Split right')).click()")
    wait(lambda: empty_panes() == 1)
    time.sleep(0.5)
    run.check("tab bar Split right opens an empty pane", empty_panes() == 1 and len(pane_ids(base)) == 2)
    run.check("the split starts no session", len(sessions()) == n0, f"{n0} -> {len(sessions())}")
    run.check("the new empty pane takes focus", wait(lambda: d.js("return document.activeElement?.classList.contains('empty-pane') ?? false")))
    kinds = picker_kinds()
    run.check("picker lists Terminal among its launchers", "Terminal" in kinds, ", ".join(kinds))
    run.shot("sp-01-picker")

    d.keys("escape")
    run.check("Esc closes the empty pane", wait(lambda: empty_panes() == 0) is not None and len(pane_ids(base)) == 1)
    run.check("closing the picker starts no session", len(sessions()) == n0)

    d.js("document.querySelector('.pane-tools button[title^=\"Split down\"]').click()")
    wait(lambda: empty_panes() == 1)
    root = tab_of(base)["root"]
    run.check("pane toolbar Split down puts the empty pane below", empty_panes() == 1 and root["type"] == "split" and root["dir"] == "col", str(root.get("dir")))
    d.keys("escape")
    wait(lambda: empty_panes() == 0)

    # ---------- keyboard: Ctrl+Shift+D, then key 1-9 starts that kind in place ----------
    d.click(".pane .xterm")
    time.sleep(0.3)
    d.keys("ctrl+shift+d")
    wait(lambda: empty_panes() == 1)
    run.check("Ctrl+Shift+D splits the focused pane with an empty pane", empty_panes() == 1 and len(sessions()) == n0)
    kinds = picker_kinds()
    key_pane = pane_dom_id()
    if "Terminal" in kinds:
        key = kinds.index("Terminal") + 1
        d.keys(str(key))
        wait(lambda: has_term(key_pane))
        p = pane_obj(base, key_pane)
        run.check(f"key {key} starts Terminal in place (same pane id)", p is not None and p["kind"] == "terminal" and len(sessions()) == n0 + 1, str(p and p.get("kind")))
    else:
        run.check("key 1-9 starts Terminal in place", False, "no Terminal launcher")
    run.shot("sp-02-keyed-terminal")

    # ---------- Ctrl+Shift+S splits down; click starts in place ----------
    d.keys("ctrl+shift+s")
    wait(lambda: empty_panes() == 1)
    below = d.js("const e = document.querySelector('.empty-pane').closest('.pane').getBoundingClientRect(); const t = document.querySelector('.pane[data-pane=\"' + arguments[0] + '\"]').getBoundingClientRect(); return e.top >= t.bottom - 2 && Math.abs(e.left - t.left) < 4;", key_pane)
    run.check("Ctrl+Shift+S splits the focused pane downward", bool(below))
    click_pane = pane_dom_id()
    d.click('.empty-pane .empty-pane-kind[title="Terminal"]')
    wait(lambda: has_term(click_pane))
    p = pane_obj(base, click_pane)
    run.check("clicking Terminal starts it in place", p is not None and p["kind"] == "terminal" and len(sessions()) == n0 + 2, str(p and p.get("kind")))

    # ---------- More… opens the dialog for this pane ----------
    d.js(f"{S}.splitPicker(arguments[0], 'right')", pid)
    wait(lambda: empty_panes() == 1)
    more_pane = pane_dom_id()
    d.js("[...document.querySelectorAll('.empty-pane .empty-pane-kind')].find(b => b.textContent.includes('More')).click()")
    wait(lambda: d.js("return !!document.querySelector('.modal')"), 5)
    on = d.js("return [...document.querySelectorAll('.modal .toggle')].some(b => b.textContent.trim() === 'This pane' && b.classList.contains('on'))")
    run.check('"More…" opens the dialog with "This pane" selected', bool(on))
    run.shot("sp-03-more-dialog")
    d.js("[...document.querySelectorAll('.modal .launch-card')].find(c => c.textContent.includes('Terminal')).click()")
    d.js("document.querySelector('.modal .footer .btn.primary').click()")
    wait(lambda: pane_obj(base, more_pane) and pane_obj(base, more_pane)["kind"] == "terminal")
    run.check("dialog Start fills that pane, keeping its id", pane_obj(base, more_pane) is not None and pane_obj(base, more_pane)["kind"] == "terminal" and not d.js("return !!document.querySelector('.modal')"))

    # ---------- picker guards: one start per pane, no repeats, focus after dialogs and Esc ----------
    def close_session_pane(sid):
        js_await(f"""(() => {{ const p = {S}.state.activeProject;
            for (const t of {S}.workspace(p).tabs) for (const x of window.__kelpie.layout.allPanes(t.root))
              if (x.sessionId === arguments[0]) return {S}.closePane(p, t.id, x.id);
            return null; }})()""", sid)

    # A start into a pane that already runs a session must not replace it; the new session goes
    # to a tab of its own (and is closed again here).
    filled_sid = pane_obj(base, more_pane)["sessionId"]
    n1 = len(sessions())
    extra = js_await(f"{S}.newSession(arguments[0], 'shell', {{ into: {{ tabId: arguments[1], paneId: arguments[2] }} }})", pid, base, more_pane)
    wait(lambda: len(sessions()) == n1 + 1)
    run.check("a start into an already-filled pane keeps the first session", pane_obj(base, more_pane)["sessionId"] == filled_sid)
    close_session_pane(extra["id"])
    wait(lambda: len(sessions()) == n1)

    # Two quick clicks on Terminal: one session.
    d.js(f"{S}.splitPicker(arguments[0], 'right')", pid)
    wait(lambda: empty_panes() == 1)
    dbl = pane_dom_id()
    n2 = len(sessions())
    d.js("const b = [...document.querySelectorAll('.empty-pane .empty-pane-kind')].find(b => b.title === 'Terminal'); b.click(); b.click();")
    wait(lambda: has_term(dbl))
    time.sleep(1)
    run.check("two quick clicks on Terminal start one session", len(sessions()) == n2 + 1 and pane_obj(base, dbl)["kind"] == "terminal", f"{n2} -> {len(sessions())}")

    # A held key (auto-repeat) must not start anything; the first press does.
    d.js(f"{S}.splitPicker(arguments[0], 'right')", pid)
    wait(lambda: empty_panes() == 1)
    rep = pane_dom_id()
    term_key = d.js("return [...document.querySelectorAll('.empty-pane .empty-pane-kind')].findIndex(b => b.title === 'Terminal') + 1")
    n3 = len(sessions())
    d.js("const e = document.querySelector('.empty-pane'); for (let i = 0; i < 3; i++) e.dispatchEvent(new KeyboardEvent('keydown', { key: String(arguments[0]), repeat: true, bubbles: true }));", term_key)
    time.sleep(1)
    run.check("auto-repeated keys start nothing", len(sessions()) == n3 and empty_panes() == 1)
    d.keys(str(term_key))
    wait(lambda: has_term(rep))
    run.check("the first key press starts the pane", pane_obj(base, rep)["kind"] == "terminal" and len(sessions()) == n3 + 1)

    # Esc refocuses the pane that was split, not the first pane in the tab.
    prev = d.js(f"return {S}.activeTab({S}.state.activeProject).focus")
    order_before = pane_ids(base)
    d.js(f"{S}.splitPicker(arguments[0], 'right')", pid)
    wait(lambda: empty_panes() == 1)
    order_split = pane_ids(base)
    d.keys("escape")
    wait(lambda: empty_panes() == 0)
    time.sleep(0.5)
    run.check("Esc refocuses the pane that was split", tab_of(base)["focus"] == prev and prev in pane_ids(base), f"focus={tab_of(base)['focus']} expected={prev} order {order_before} -> {order_split} -> {pane_ids(base)}")

    # After More… then Cancel, the picker takes the keyboard back: keys start it.
    d.js(f"{S}.splitPicker(arguments[0], 'right')", pid)
    wait(lambda: empty_panes() == 1)
    cancel_pane = pane_dom_id()
    cancel_key = d.js("return [...document.querySelectorAll('.empty-pane .empty-pane-kind')].findIndex(b => b.title === 'Terminal') + 1")
    d.js("[...document.querySelectorAll('.empty-pane .empty-pane-kind')].find(b => b.textContent.includes('More')).click()")
    wait(lambda: d.js("return !!document.querySelector('.modal')"), 5)
    d.js("[...document.querySelectorAll('.modal .btn')].find(b => b.textContent.trim() === 'Cancel').click()")
    wait(lambda: not d.js("return !!document.querySelector('.modal')"), 5)
    back = wait(lambda: d.js("return document.activeElement?.classList.contains('empty-pane') ?? false"), 5)
    run.check("the picker takes the keyboard back after the dialog closes", bool(back))
    d.keys(str(cancel_key))
    wait(lambda: has_term(cancel_pane))
    run.check("keys start the picker after More… then Cancel", pane_obj(base, cancel_pane) is not None and pane_obj(base, cancel_pane)["kind"] == "terminal")

    # Stale focus (a pane that no longer exists) must not break moves or merges.
    SA = new_tab([new_id(), new_id()])
    sa1, sa2 = pane_ids(SA)
    SB = new_tab([new_id()])
    d.js(f"{S}.focusPane(arguments[0], arguments[1], 'stale-pane')", pid, SB)
    d.js(f"{S}.movePaneToTab(arguments[0], arguments[1], arguments[2], arguments[3])", pid, SA, sa2, SB)
    run.check("moving into a tab with stale focus lands the pane", sa2 in pane_ids(SB) and not problems(tab_of(SB)["root"]), f"B={pane_ids(SB)}")
    SC = new_tab([new_id()])
    SD = new_tab([new_id(), new_id()])
    sd1, sd2 = pane_ids(SD)
    d.js(f"{S}.focusPane(arguments[0], arguments[1], 'stale-pane')", pid, SD)
    d.js(f"{S}.mergeTab(arguments[0], arguments[1], arguments[2], arguments[3], arguments[4])", pid, SD, SC, pane_ids(SC)[0], "right")
    mt = tab_of(SC)
    run.check("merging a tab with stale focus keeps a valid focus", mt is not None and mt["focus"] in pane_ids(SC) and tab_of(SD) is None, f"focus={mt and mt['focus']} panes={pane_ids(SC) if mt else None}")

    # ---------- tabs: store reorder, clamped; commands Move Tab Left / Right ----------
    tb = new_tab([new_id()])
    tc = new_tab([new_id()])
    run.check("tabs are created in order", tab_order().index(base) < tab_order().index(tb) < tab_order().index(tc), str(tab_order()))
    d.js(f"{S}.moveTab(arguments[0], arguments[1], 999)", pid, base)
    run.check("moveTab past the end clamps to last", tab_order()[-1] == base, str(tab_order()))
    d.js(f"{S}.moveTab(arguments[0], arguments[1], 0)", pid, base)
    run.check("moveTab to index 0", tab_order()[0] == base, str(tab_order()))
    d.js(f"{S}.moveTab(arguments[0], arguments[1], -5)", pid, tc)
    run.check("moveTab below zero clamps to first", tab_order()[0] == tc, str(tab_order()))
    d.js(f"{S}.moveTab(arguments[0], arguments[1], 0)", pid, base)
    d.js(f"{S}.moveTab(arguments[0], arguments[1], 1)", pid, tb)
    run.check("moveTab moves a tab to an index", tab_order().index(tb) == 1, str(tab_order()))
    d.js(f"{S}.selectTab(arguments[0], arguments[1])", pid, tc)
    i0 = tab_order().index(tc)
    d.js("window.__kelpie.commands.find(c => c.id === 'tab.moveRight').run()")
    run.check("palette Move Tab Right", tab_order().index(tc) == i0 + 1, str(tab_order()))
    d.js("window.__kelpie.commands.find(c => c.id === 'tab.moveLeft').run()")
    run.check("palette Move Tab Left", tab_order().index(tc) == i0, str(tab_order()))

    # ---------- movePaneToTab: existing tab, emptied source, new tab, live session ----------
    A = new_tab([new_id(), new_id()])
    a1, a2 = pane_ids(A)
    B = new_tab([new_id()])
    b1 = pane_ids(B)[0]
    d.js(f"{S}.movePaneToTab(arguments[0], arguments[1], arguments[2], arguments[3])", pid, A, a2, B)
    run.check("movePaneToTab moves a pane into another tab, source kept", pane_ids(A) == [a1] and sorted(pane_ids(B)) == sorted([b1, a2]), f"A={pane_ids(A)} B={pane_ids(B)}")
    run.check("the destination tab becomes active", d.js(f"return {S}.activeTab({S}.state.activeProject).id") == B)
    d.js(f"{S}.movePaneToTab(arguments[0], arguments[1], arguments[2], arguments[3])", pid, A, a1, B)
    run.check("a source tab emptied by the move disappears", tab_of(A) is None and sorted(pane_ids(B)) == sorted([b1, a1, a2]), f"B={pane_ids(B)}")
    C = new_tab([new_id(), new_id()])
    c1, c2 = pane_ids(C)
    d.js(f"{S}.movePaneToTab(arguments[0], arguments[1], arguments[2], null)", pid, C, c2)
    order = tab_order()
    new_at = order.index(C) + 1 if C in order else -1
    run.check("pane to a new tab opens right after its tab", new_at > 0 and pane_ids(order[new_at]) == [c2] and pane_ids(C) == [c1], str(order))
    before = len(tab_order())
    d.js(f"{S}.movePaneToTab(arguments[0], arguments[1], arguments[2], null)", pid, order[new_at], c2)
    run.check("a lone pane moved to a new tab is a no-op", len(tab_order()) == before)
    # A terminal survives the move: same session, rendered in the destination tab.
    term_obj = pane_obj(base, click_pane)
    sid = term_obj["sessionId"]
    d.js(f"{S}.movePaneToTab(arguments[0], arguments[1], arguments[2], arguments[3])", pid, base, click_pane, C)
    wait(lambda: has_term(click_pane))
    alive = d.js(f"return !!({S}.sessions[arguments[0]] || {{}}).alive", sid)
    run.check("moving a terminal pane keeps its session alive and visible", alive and has_term(click_pane) and click_pane in pane_ids(C), f"alive={alive}")
    run.shot("sp-04-moved-terminal")

    # ---------- mergeTab: layout invariants when a tab lands beside a pane ----------
    def merge_case(name, dest, src, target, side):
        X = new_tab(dest)
        Y = new_tab(src)
        d.js(f"{S}.mergeTab(arguments[0], arguments[1], arguments[2], arguments[3], arguments[4])", pid, Y, X, target, side)
        t = tab_of(X)
        probs = problems(t["root"]) if t else ["target tab missing"]
        ok = not probs and tab_of(Y) is None and sorted(pane_ids(X)) == sorted(dest + src)
        run.check(name, ok, f"{probs} panes={pane_ids(X) if t else None}")

    # A row merged beside a lone pane, a row merged into the same-direction row holding the
    # target, and a row merged below a lone pane. Each must keep shares at 1 and no split nested
    # directly in a split of its own direction.
    dx, sx = [new_id()], [new_id(), new_id()]
    merge_case("merge a row beside a lone pane leaves no nested same-direction split", dx, sx, dx[0], "right")
    dy, sy = [new_id(), new_id()], [new_id(), new_id()]
    merge_case("merge into a same-direction parent leaves no nested split", dy, sy, dy[1], "left")
    dz, sz = [new_id()], [new_id(), new_id()]
    merge_case("merge below a lone pane gives a column, no nesting", dz, sz, dz[0], "bottom")

    # ---------- pointer tests on a fresh project: the store tests above leave many tabs ----------
    for s in sessions():
        daemon(d, "session.close", {"handle": s["handle"]})
    d.js(f"window.__kelpie.store.removeProject(arguments[0])", pid)
    pid = d.js(f"return {S}.addProject(arguments[0])", proj)

    # ---------- pointer: tab reorder by dragging along the bar ----------
    P = new_tab([new_id()])
    Q = new_tab([new_id()])
    R = new_tab([new_id()])
    d.js(f"{S}.selectTab(arguments[0], arguments[1])", pid, R)
    order0 = dom_tab_order()
    first = order0[0]
    fr = tab_rect(first)
    last = tab_rect(order0[-1])
    d.drag(fr["x"] + fr["w"] / 2, fr["y"] + fr["h"] / 2, last["x"] + last["w"] - 4, last["y"] + last["h"] / 2)
    wait(lambda: dom_tab_order() == order0[1:] + [first], 5)
    run.check("dragging a tab along the bar reorders the tabs", dom_tab_order() == order0[1:] + [first], f"{order0} -> {dom_tab_order()}")
    run.shot("sp-05-tab-reordered")

    # ---------- pointer: pane header dropped on another tab moves the pane there ----------
    X = new_tab([new_id(), new_id()])
    x1, x2 = pane_ids(X)
    target_tab = P
    h = rect(f'.pane[data-pane="{x1}"] .pane-header')
    tr = tab_rect(target_tab)
    d.drag(h["x"] + 60, h["y"] + h["h"] / 2, tr["x"] + tr["w"] / 2, tr["y"] + tr["h"] / 2)
    wait(lambda: x1 in pane_ids(target_tab), 5)
    run.check("dropping a pane header on another tab moves the pane there", x1 in pane_ids(target_tab) and pane_ids(X) == [x2], f"X={pane_ids(X)} target={pane_ids(target_tab)}")

    # ---------- pointer: pane header dropped on empty tab bar space moves it to a new tab ----------
    cur = d.js(f"return {S}.activeTab({S}.state.activeProject).id")
    src_tab = cur
    src_panes = pane_ids(src_tab)
    if len(src_panes) >= 2:
        move_pane = src_panes[0]
        h = rect(f'.pane[data-pane="{move_pane}"] .pane-header')
        # A point on the bar that is neither a tab nor a bar button (hit-tested, from the right).
        bar = d.js("""const r = document.querySelector('.tabbar').getBoundingClientRect(); const y = r.top + r.height / 2;
          for (let x = Math.floor(r.right) - 4; x > r.left; x -= 4) {
            const el = document.elementFromPoint(x, y);
            if (el && el.closest('.tabbar') && !el.closest('[data-tab-id], button')) return {x, y};
          }
          return null;""")
        if bar is None:
            run.check("pane header dropped on empty tab bar space makes a new tab", False, "no empty bar space to aim at")
        else:
            n_tabs = len(tab_order())
            d.drag(h["x"] + 60, h["y"] + h["h"] / 2, bar["x"], bar["y"])
            wait(lambda: len(tab_order()) == n_tabs + 1, 5)
            order = tab_order()
            new_idx = next((i for i, t in enumerate(order) if move_pane in pane_ids(t)), -1)
            run.check("empty bar drop makes a new tab right after the source", len(order) == n_tabs + 1 and new_idx == order.index(src_tab) + 1, f"order={order} pane_at={new_idx}")
    else:
        run.check("pane header dropped on empty tab bar space makes a new tab", False, f"active tab has {len(src_panes)} pane(s)")

    # ---------- pointer: a tab dragged onto a pane's left edge merges beside it ----------
    A2 = new_tab([new_id(), new_id()])
    a2a, a2b = pane_ids(A2)
    Bt = new_tab([new_id()])
    bt1 = pane_ids(Bt)[0]
    d.js(f"{S}.selectTab(arguments[0], arguments[1])", pid, A2)
    tr = tab_rect(Bt)
    pr = rect(f'.pane[data-pane="{a2b}"]')
    d.drag(tr["x"] + tr["w"] / 2, tr["y"] + tr["h"] / 2, pr["x"] + 8, pr["y"] + pr["h"] / 2)
    wait(lambda: tab_of(Bt) is None, 5)
    run.check("a tab dropped on a pane's left edge merges beside it", tab_of(Bt) is None and pane_ids(A2) == [a2a, bt1, a2b] and problems(tab_of(A2)["root"]) == [], f"panes={pane_ids(A2)}")
    run.shot("sp-06-merged")

    # ---------- window hidden: on-screen terminals detach, then replay what they missed on show ----------
    hs = js_await(f"{S}.newSession(arguments[0], 'shell')", pid)
    hsid, hh = hs["id"], hs["handle"]
    wait(lambda: (view_state(hsid) or {}).get("attached"), 20)
    run.check("terminal is attached while the window is visible", bool((view_state(hsid) or {}).get("attached")), str(view_state(hsid)))
    d.js("Object.defineProperty(document, 'hidden', {configurable: true, get: () => true}); document.dispatchEvent(new Event('visibilitychange'));")
    wait(lambda: view_state(hsid) and not view_state(hsid)["attached"], 5)
    st = view_state(hsid)
    run.check("hiding the window detaches the on-screen terminal", st is not None and st["visible"] and not st["attached"], str(st))
    marker = new_id()
    daemon(d, "session.input", {"handle": hh, "data": f"echo hidden-{marker}\r"})
    time.sleep(2)
    run.check("output written while hidden is not rendered", f"hidden-{marker}" not in (view_text(hsid) or ""))
    d.js("Object.defineProperty(document, 'hidden', {configurable: true, get: () => false}); document.dispatchEvent(new Event('visibilitychange'));")
    back = wait(lambda: (view_state(hsid) or {}).get("attached") and f"hidden-{marker}" in (view_text(hsid) or ""), 20)
    run.check("showing the window re-attaches and replays the missed output", bool(back), str(view_state(hsid)))
    run.shot("sp-07-shown-again")

    # ---------- cleanup: sessions this run started ----------
    for s in sessions():
        daemon(d, "session.close", {"handle": s["handle"]})
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    run.finish()
