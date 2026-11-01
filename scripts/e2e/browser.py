"""E2E for browser panes and design mode (trusted mouse / keyboard input inside the page).

    python -X utf8 scripts/e2e/browser.py <screenshot-dir> <scratch-dir> [--port 47812]

A local test site is served on 127.0.0.1:<port>. The page lives in a native child webview the
WebDriver session cannot see into, so the checks use Kelpie's own events and commands, and
clicks / keys go into the page as trusted input through the WebView2 DevTools protocol
(`browser_test_input`, dev and e2e builds only) — the same events a real mouse produces, without
needing the window on screen or touching the user's mouse.

Covers: open a browser pane (command, address bar), navigation events (address, title, pane
title), back / forward / reload, the webview following the pane (gutter drag), hidden while a
dialog or menu is open, design mode (hover + click picks an element: selector, text, HTML,
attributes with secrets redacted, box), the picked element's screenshot, Esc ends design mode,
a link opening a new window opens in the same pane, the agent prompt for marked elements,
whole-page screenshot, the page kept in the layout, localhost link routing, closing the pane
closes its webview.
"""
import base64
import os
import struct
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import Run, daemon  # noqa: E402

shots, scratch = sys.argv[1], sys.argv[2]
port = int(sys.argv[sys.argv.index("--port") + 1]) if "--port" in sys.argv else 47812
S = "window.__kelpie.store"
B = "window.__kelpie.browser"
BASE = f"http://127.0.0.1:{port}"

PAGE1 = """<!doctype html><html><head><title>Kelpie Test Shop</title>
<style>body{margin:0;font:16px sans-serif} #save{position:absolute;left:60px;top:80px;width:160px;height:48px;background:#2d7ff9;color:#fff;border:0;border-radius:8px}
#popup{position:absolute;left:60px;top:200px}</style></head>
<body><main class="shell"><button id="save" class="btn primary" data-testid="save-btn" aria-label="Save cart" title="Save">Save cart</button>
<a id="next" href="/page2">Next page</a>
<a id="popup" href="/page2?from=popup&access_token=SECRET123" target="_blank">Open in new window</a>
<input id="pw" type="password" value="hunter2" style="position:absolute;left:60px;top:260px"></main></body></html>"""
PAGE2 = "<!doctype html><html><head><title>Second Page</title></head><body><h1 id=h>Page two</h1></body></html>"
# Tells the site whether the page is really on screen: the page itself is out of the test's reach,
# and Kelpie's own state says what it asked for, not what happened.
PAGE_WATCHED = """<!doctype html><html><head><title>Watched</title><script>
const report = () => fetch('/report' + location.pathname + '?' + document.visibilityState);
document.addEventListener('visibilitychange', report); report();
</script></head><body>Watched page</body></html>"""
on_screen = {}  # page path -> its last reported visibilityState


class Site(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        if self.path.startswith("/report/"):
            path, _, seen = self.path[len("/report"):].partition("?")
            on_screen[path] = seen
        body = (PAGE2 if self.path.startswith("/page2") else PAGE_WATCHED if self.path.startswith("/watched") else PAGE1).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


server = ThreadingHTTPServer(("127.0.0.1", port), Site)
threading.Thread(target=server.serve_forever, daemon=True).start()

run = Run(shots)
d = run.d
label = None


def page_input(*events):
    """Trusted input into the page (coordinates in the page's CSS pixels)."""
    r = d.invoke("browser_test_input", {"label": label, "events": list(events)})
    if "err" in r:
        raise RuntimeError(r["err"])


def mouse(kind, x, y, **extra):
    return {"method": "Input.dispatchMouseEvent", "params": {"type": kind, "x": x, "y": y, **extra}}


def page_hover(x, y):
    page_input(mouse("mouseMoved", x - 30, y - 10), mouse("mouseMoved", x, y))


def page_click(x, y):
    page_input(mouse("mouseMoved", x, y), mouse("mousePressed", x, y, button="left", clickCount=1), mouse("mouseReleased", x, y, button="left", clickCount=1))


def page_escape():
    key = {"key": "Escape", "code": "Escape", "windowsVirtualKeyCode": 27, "nativeVirtualKeyCode": 27}
    page_input({"method": "Input.dispatchKeyEvent", "params": {"type": "rawKeyDown", **key}}, {"method": "Input.dispatchKeyEvent", "params": {"type": "keyUp", **key}})


def js_await(expr, *args):
    return d.js_async(f"const done = arguments[arguments.length-1]; Promise.resolve().then(() => {expr}).then(v => done(v === undefined ? null : v), e => done({{err: String(e)}}));", *args)


def wait(cond, timeout=20, interval=0.3):
    end = time.time() + timeout
    while time.time() < end:
        v = cond()
        if v:
            return v
        time.sleep(interval)
    return None


def pane():
    return d.js(f"const t = {S}.activeTab({S}.state.activeProject); return t ? window.__kelpie.layout.allPanes(t.root).find(p => p.kind === 'browser') ?? null : null")


def info(label):
    r = d.invoke("browser_info", {"label": label})
    return r.get("ok")


def png_size(data_b64):
    raw = base64.b64decode(data_b64)
    assert raw[:8] == b"\x89PNG\r\n\x1a\n"
    return struct.unpack(">II", raw[16:24])


try:
    run.boot()
    d.set_window_size(1480, 920)
    d._req("POST", d.s("/timeouts"), {"script": 120000})
    for s in daemon(d, "session.list"):
        daemon(d, "session.close", {"handle": s["handle"]})
    proj = os.path.join(scratch, "browser-proj")
    os.makedirs(proj, exist_ok=True)
    d.js(f"const s = {S}; for (const p of [...s.state.projects]) s.removeProject(p.id); s.setSettings({{ shortcutHints: false, tips: false, floatingTerminal: true }}); s.toggleFloating(false);")
    pid = d.js(f"return {S}.addProject(arguments[0])", proj)

    # ---------- open ----------
    d.js("window.__kelpie.commands.find(c => c.id === 'browser.open').run()")
    d.wait("return !!document.querySelector('.browser-address input')", 10)
    p = pane()
    label = f"browser-{p['id']}"
    run.check("Open Browser adds a browser pane with an address bar", p is not None)
    d.js("const i = document.querySelector('.browser-address input'); i.value = arguments[0]; i.dispatchEvent(new InputEvent('input', {bubbles: true})); i.form.dispatchEvent(new Event('submit', {bubbles: true, cancelable: true}));", f"127.0.0.1:{port}")
    ok = wait(lambda: d.js(f"return {B}.browserState[arguments[0]]?.title === 'Kelpie Test Shop'", p["id"]), 20)
    run.check("page loads; its title reaches Kelpie", bool(ok))
    run.check("a bare host:port opens as http://", (info(label) or {}).get("url", "").startswith(f"{BASE}/"))
    run.check("pane header / tab shows the page title", wait(lambda: d.js("return [...document.querySelectorAll('.tab .title')].some(t => t.textContent.includes('Kelpie Test Shop'))"), 10))
    run.check("the page is kept in the layout", wait(lambda: (pane() or {}).get("url", "").startswith(BASE), 10))
    host = d.js("const r = document.querySelector('.browser-host').getBoundingClientRect(); return {x: r.left, y: r.top, width: r.width, height: r.height}")
    i1 = info(label)
    run.check("the webview sits exactly over the pane body", i1 and abs(i1["x"] - host["x"]) < 2 and abs(i1["y"] - host["y"]) < 2 and abs(i1["width"] - host["width"]) < 2, f"{i1} vs {host}")
    time.sleep(1)
    run.shot("01-browser-open")

    # ---------- navigation ----------
    js_await(f"{B}.browserNavigate(arguments[0], arguments[1])", label, f"{BASE}/page2")
    run.check("navigate: title follows", bool(wait(lambda: d.js(f"return {B}.browserState[arguments[0]]?.title === 'Second Page'", p["id"]), 15)))
    run.check("navigate: address bar follows", bool(wait(lambda: d.js("return document.querySelector('.browser-address input').value.endsWith('/page2')"), 10)))
    d.js("document.querySelector('.browser-bar .icon-btn[title=\"Back\"]').click()")
    run.check("back", bool(wait(lambda: d.js(f"return {B}.browserState[arguments[0]]?.title === 'Kelpie Test Shop'", p["id"]), 15)))
    d.js("document.querySelector('.browser-bar .icon-btn[title=\"Forward\"]').click()")
    run.check("forward", bool(wait(lambda: d.js(f"return {B}.browserState[arguments[0]]?.title === 'Second Page'", p["id"]), 15)))
    d.js("document.querySelector('.browser-bar .icon-btn[title=\"Back\"]').click()")
    wait(lambda: d.js(f"return {B}.browserState[arguments[0]]?.title === 'Kelpie Test Shop'", p["id"]), 15)
    d.js("document.querySelector('.browser-bar .icon-btn[title=\"Reload\"]').click()")
    run.check("reload keeps the page", bool(wait(lambda: d.js(f"return {B}.browserState[arguments[0]]?.title === 'Kelpie Test Shop'", p["id"]), 15)))

    # ---------- follows the layout; hides under dialogs ----------
    js_await(f"{S}.newSession(arguments[0], 'shell', {{ where: 'right' }})", pid)
    time.sleep(1)
    host2 = d.js("const r = document.querySelector('.browser-host').getBoundingClientRect(); return {x: r.left, y: r.top, width: r.width, height: r.height}")
    i2 = wait(lambda: (lambda i: i if i and abs(i["width"] - host2["width"]) < 2 else None)(info(label)), 5)
    run.check("splitting the tab shrinks the webview with its pane", bool(i2) and i2["width"] < i1["width"] - 100, f"{i1 and i1['width']} -> {i2 and i2['width']}")
    d.js(f"{S}.setOverlay('palette')")
    run.check("a dialog over the page hides it (placeholder shown)", bool(d.wait("return [...document.querySelectorAll('.browser-host .empty')].some(e => e.textContent.includes('hidden'))", 5)))
    d.js(f"{S}.setOverlay(null)")
    run.check("closing the dialog shows the page again", bool(d.wait("return ![...document.querySelectorAll('.browser-host .empty')].some(e => e.textContent.includes('hidden'))", 5)))
    # Back to one pane so the page has room.
    sh = d.js(f"const t = {S}.activeTab({S}.state.activeProject); return window.__kelpie.layout.allPanes(t.root).find(p => p.kind === 'terminal').id")
    js_await(f"{S}.closePane({S}.state.activeProject, {S}.activeTab({S}.state.activeProject).id, arguments[0])", sh)
    time.sleep(1)

    # ---------- a link meant for a new window opens in this pane ----------
    page_click(100, 208)
    run.check("a link with target=_blank opens in the same pane", bool(wait(lambda: d.js(f"return {B}.browserState[arguments[0]]?.title === 'Second Page'", p["id"]), 15)))
    js_await(f"{B}.browserNavigate(arguments[0], arguments[1])", label, BASE + "/")
    wait(lambda: d.js(f"return {B}.browserState[arguments[0]]?.title === 'Kelpie Test Shop'", p["id"]), 15)

    # ---------- design mode with real input ----------
    d.js("document.querySelector('.browser-bar .icon-btn[title^=\"Design mode\"]').click()")
    time.sleep(0.8)
    page_hover(140, 104)  # centre of #save
    time.sleep(0.4)
    run.shot("02-design-hover")
    page_click(140, 104)
    note = wait(lambda: d.js(f"return {B}.browserState[arguments[0]]?.notes?.[0] ?? null", p["id"]), 15)
    run.check("clicking an element in design mode picks it", bool(note))
    if note:
        pk = note["pick"]
        run.check("pick: unique selector", pk["selector"] == "#save", pk["selector"])
        run.check("pick: text, tag, accessible name", pk["text"] == "Save cart" and pk["tag"] == "button" and pk["name"] == "Save cart")
        run.check("pick: allowlisted attributes (data-testid, aria-label)", pk["attributes"].get("data-testid") == "save-btn" and pk["attributes"].get("aria-label") == "Save cart", str(pk["attributes"]))
        run.check("pick: box and key styles", abs(pk["rect"]["width"] - 160) < 2 and abs(pk["rect"]["height"] - 48) < 2 and pk["styles"]["background-color"] == "rgb(45, 127, 249)", str(pk["rect"]))
        run.check("pick: page url and title", pk["page"]["title"] == "Kelpie Test Shop" and pk["page"]["url"].startswith(BASE))
        w, h = png_size(note["shot"]["data"]) if note.get("shot") else (0, 0)
        dpr_page = pk["page"]["dpr"]
        run.check("pick: screenshot of the element (with a small margin)", note.get("shot") and abs(w - (160 + 16) * dpr_page) <= 4 and abs(h - (48 + 16) * dpr_page) <= 4, f"{w}x{h} dpr {dpr_page}")
        run.check("pick: screenshot saved under the data dir", note.get("shot") and os.path.isfile(note["shot"]["path"]))
    # Pick the password field: its value never leaves the page.
    page_click(80, 270)
    note2 = wait(lambda: d.js(f"return {B}.browserState[arguments[0]]?.notes?.[1] ?? null", p["id"]), 15)
    run.check("a password field's value is redacted from the picked HTML", note2 and "hunter2" not in note2["pick"]["html"] and "[redacted]" in note2["pick"]["html"], note2 and note2["pick"]["html"][:120])
    run.shot("03-design-picked")
    page_escape()  # Esc ends design mode
    run.check("Esc in the page ends design mode", bool(wait(lambda: d.js(f"return {B}.browserState[arguments[0]]?.design === false", p["id"]), 10)))

    # The prompt for both.
    d.js("const t = document.querySelector('.browser-note textarea'); t.value = 'Make this button green'; t.dispatchEvent(new InputEvent('input', {bubbles: true}));")
    prompt = d.js(f"return {B}.annotationPrompt({B}.browserState[arguments[0]].notes)", p["id"])
    run.check("agent prompt: request, selector, HTML, screenshot path", all(x in prompt for x in ["Make this button green", "`#save`", "```html", "Screenshot: ", "Kelpie Test Shop"]), prompt[:300])

    # The page stays usable after design mode: an ordinary link navigates.
    page_click(20, 9)
    run.check("after design mode the page works as usual (a link navigates)", bool(wait(lambda: d.js(f"return {B}.browserState[arguments[0]]?.title === 'Second Page'", p["id"]), 15)))

    # ---------- whole-page screenshot ----------
    d.js("document.querySelector('.browser-bar .icon-btn[title=\"Screenshot of the page\"]').click()")
    n3 = wait(lambda: d.js(f"const n = {B}.browserState[arguments[0]]?.notes ?? []; return n.length === 3 ? n[2] : null", p["id"]), 15)
    run.check("page screenshot added as a mark", n3 and n3["pick"] is None and n3["shot"])
    run.shot("04-marks")

    # ---------- link routing, close ----------
    run.check("localhost links count as local, others not", d.js(f"return {B}.isLocalUrl('http://localhost:5173/x') && {B}.isLocalUrl('http://127.0.0.1:8080') && !{B}.isLocalUrl('https://example.com')"))
    js_await(f"{S}.closePane({S}.state.activeProject, {S}.activeTab({S}.state.activeProject)?.id, arguments[0])", p["id"])
    run.check("closing the pane closes its webview", bool(wait(lambda: "err" in d.invoke("browser_info", {"label": label}), 10)))
    run.check("closing the pane forgets its marks", d.js(f"return !{B}.browserState[arguments[0]]", p["id"]))

    # ---------- a page never stays above a tab it no longer belongs to ----------
    # It is a native window over the whole app, and creating it takes around half a second: leaving
    # its tab meanwhile must still hide it, or the page covers the tab now shown.
    js_await(f"{S}.newSession(arguments[0], 'shell')", pid)
    term_tab = wait(lambda: d.js(f"return {S}.activeTab(arguments[0])?.id", pid), 10)

    def stays_hidden(path):
        if not wait(lambda: on_screen.get(path) == "hidden", 10):
            return False
        time.sleep(1.5)
        return on_screen.get(path) == "hidden"

    def browser_tab_of(path):
        return d.js(f"return {S}.workspace(arguments[0]).tabs.find(t => window.__kelpie.layout.allPanes(t.root).some(p => (p.url ?? '').endsWith(arguments[1])))?.id ?? null", pid, path)

    for delay in (0, 50, 200, 500):
        path = f"/watched/left-{delay}"
        d.js(f"{B}.openBrowser(arguments[0], 'tab', arguments[1]); setTimeout(() => {S}.selectTab(arguments[1], arguments[2]), arguments[3]);", BASE + path, pid, term_tab, delay)
        run.check(f"tab left {delay} ms after opening a page: the page is hidden", stays_hidden(path), str(on_screen.get(path)))
    path = "/watched/left-0"
    tab = browser_tab_of(path)
    d.js(f"{S}.selectTab(arguments[0], arguments[1])", pid, tab)
    run.check("back on its tab, the page is shown again", bool(wait(lambda: on_screen.get(path) == "visible", 10)), str(on_screen.get(path)))

    js_await(f"new Promise((done) => {{ let i = 0; const step = () => {{ {S}.selectTab(arguments[0], i % 2 ? arguments[2] : arguments[1]); ++i < 41 ? setTimeout(step, 40) : done(); }}; step(); }})", pid, term_tab, tab)
    run.check("quick switching that ends on another tab leaves the page hidden", d.js(f"return {S}.activeTab(arguments[0])?.id === arguments[1]", pid, term_tab) and stays_hidden(path), str(on_screen.get(path)))
except Exception:
    import traceback
    traceback.print_exc()
    run.check("scenario ran to completion", False)
finally:
    server.shutdown()
    run.finish()
