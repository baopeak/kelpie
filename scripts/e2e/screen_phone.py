"""The computer's screen from the real phone page: the phone app in Playwright's Chromium as an iPhone (390×844 at 3×, touch), against the real
app, its screen helper and a daemon of this script's own (Windows). Every function of the remote screen a person reaches with their fingers.

    python -X utf8 scripts/e2e/screen_phone.py <scratch-dir> <app> [--json out.json] [--load L2:cpu] [--chrome]

<app> is a debug build of the app with its frontend embedded (`cd app && pnpm tauri build --debug --no-bundle`), with the debug kelpied (built
after `pnpm build:phone`, which it embeds: the page tested is the daemon's own) and kelpie-screen next to it. Needs tauri-driver (PATH or
~/.cargo/bin), msedgedriver matching WebView2 (PATH or MSEDGEDRIVER), Playwright for Python with Chromium, Python's `cryptography`,
`websockets` and tkinter.

Isolation as screen.py's (screen_support.prepare/start): this process's own endpoint and data folder, set before anything starts; a fake
Tailscale CLI; the app started by this script's tauri-driver. The user's daemon, data, Tailscale and keyring are never reached, and only the
processes this script started are stopped, by PID. The phone page is served by this daemon on 127.0.0.1 with a certificate made for this run
(opened at `localhost`, which a passkey needs); the app lock is set up with Chromium's virtual authenticator. Safari's WebKit has no WebCodecs
on Windows, so the iPhone is Chromium with an iPhone's size, pixel ratio and touch.

A window of this script's own (tkinter) is what the phone sees: it notes each click, double click, right click, drag, wheel turn and key with
where it landed. The phone's touches are real touch events (CDP `Input.dispatchTouchEvent`).

Checks, each PASS or FAIL, then a summary (`--json` writes them too):
1. pairing, the passkey and control; the screen picker lists the window and the ways to connect; seeing it brings a live picture.
2. trackpad: a slide moves the computer's cursor and the phone's cursor ends where the computer's is; a tap clicks there; a double tap is a
   double click; a long press and a two-finger tap right-click; a long press then a move drags; two fingers scroll; the page's cost per move.
3. the gestures' feedback: rings for clicks, the long press's hint, the drag's chip; the help sheet.
4. direct touch: the same gestures under the finger, the magnifier while dragging.
5. pinch zoom keeps the cursor in the picture shown; Fit goes back. Zoomed in: a trackpad slide moves the cursor and the view only follows
   it near an edge, the cursor kept inside the picture shown; two fingers move the view and scroll nothing; direct touch's slide moves the
   cursor under the finger, a finger held at an edge pans the view and a tap there does not; the page itself never zooms or scrolls, and
   its touch and Safari gesture listeners cancel (Chromium honours touch-action anyway: the iPhone's own page zoom is not reproducible here).
6. text: shown in the field as typed, Hide masks it; text with Vietnamese diacritics arrives whole; Backspace, Enter, Tab, Esc; Ctrl with a
   letter is a shortcut.
7. Stop ends it, and it starts again; leaving the app (the page hidden) ends it within a second and nothing more lands.
`--chrome` runs the phone in the installed Google Chrome (Playwright's `chrome` channel, a throwaway profile) instead of Playwright's own
Chromium: the browser a person has. `--load` pins this script's daemon at a load level (`KELPIE_LOAD_FORCE`, calm by default). At L2 every check above runs on a screen that started
at its lowest rung. At L3 the start is refused instead: the picker says why and when it tries again, Stop trying stops it, and nothing else runs.
Not here: the relay and its Face ID check (screen_relay.py, with an emulated phone), peer to peer under impairment (screen_rtc.py).
"""
import argparse
import json
import os
import statistics
import sys
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import screen_support as ss  # noqa: E402
from screen_support import check, until  # noqa: E402

SETUP = ss.prepare("screen-phone", __doc__)
import phone_support as ps  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("scratch")
ap.add_argument("app")
ap.add_argument("--json", default=None)
ap.add_argument("--load", default=None, help="the daemon's pinned load level and reason, such as L2:cpu or L3:memory (memory alone never lowers the stream: L2:memory starts it as at L0)")
ap.add_argument("--chrome", action="store_true", help="the installed Google Chrome instead of Playwright's Chromium")
ARGS = ap.parse_args()
if ARGS.load:
    os.environ["KELPIE_E2E_LOAD"] = ARGS.load
OVERLOADED = (ARGS.load or "").upper().startswith(("L3", "3"))

TITLE = f"Kelpie phone screen {os.getpid()}"
EVENTS = os.path.join(SETUP.work, "target-events.txt")
PORT = ss.free_port()
PHONE_NAME = "Playwright iPhone"
TYPED = "Xin chào Việt Nam: ẩm thực, phở, bánh mì"

# ---------- the window the phone sees ----------

TARGET = r'''
import ctypes, sys, tkinter as tk
ctypes.windll.shcore.SetProcessDpiAwareness(2)
title, events = sys.argv[1], sys.argv[2]
def note(line):
    with open(events, "a", encoding="utf-8") as f:
        f.write(line + "\n")
root = tk.Tk()
root.title(title)
root.geometry("960x620+100+100")
root.attributes("-topmost", True)
pad = tk.Canvas(root, bg="#f2f2f2", highlightthickness=0)
pad.place(x=0, y=0, width=960, height=500)
for i in range(0, 960, 60):
    pad.create_line(i, 0, i, 500, fill="#d8d8d8")
for j in range(0, 500, 60):
    pad.create_line(0, j, 960, j, fill="#d8d8d8")
press = {}
def b1(e):
    press.update(at=(e.x_root, e.y_root), moved=False)
def m1(e):
    if "at" in press and abs(e.x_root - press["at"][0]) + abs(e.y_root - press["at"][1]) > 15:
        press["moved"] = True
def r1(e):
    if press.get("moved"):
        note("drag:%d,%d:%d,%d" % (press["at"][0], press["at"][1], e.x_root, e.y_root))
    else:
        note("click:%d,%d" % (e.x_root, e.y_root))
    press.clear()
pad.bind("<ButtonPress-1>", b1)
pad.bind("<B1-Motion>", m1)
pad.bind("<ButtonRelease-1>", r1)
pad.bind("<Double-Button-1>", lambda e: note("double:%d,%d" % (e.x_root, e.y_root)))
pad.bind("<Button-3>", lambda e: note("right:%d,%d" % (e.x_root, e.y_root)))
root.bind_all("<MouseWheel>", lambda e: note("wheel:%d" % e.delta))
field = tk.Entry(root, font=("Segoe UI", 14))
field.place(x=20, y=530, width=700, height=44)
def key(e):
    note("key:%s:%d" % (e.keysym, 1 if e.state & 0x4 else 0))
    root.after(1, lambda: note("text:" + field.get()))
field.bind("<KeyPress>", key)
field.bind("<FocusIn>", lambda e: note("focus"))
def where():
    root.update_idletasks()
    note("pad@%d,%d,%d,%d" % (pad.winfo_rootx(), pad.winfo_rooty(), pad.winfo_width(), pad.winfo_height()))
    note("field@%d,%d,%d,%d" % (field.winfo_rootx(), field.winfo_rooty(), field.winfo_width(), field.winfo_height()))
root.after(500, where)
root.mainloop()
'''


def events():
    try:
        with open(EVENTS, encoding="utf-8") as f:
            return [line.rstrip("\n") for line in f]
    except OSError:
        return []


def geometry(name):
    for line in events():
        if line.startswith(name + "@"):
            return tuple(int(v) for v in line.split("@", 1)[1].split(","))
    return None


def since(n, kind):
    """The events of `kind` after the first `n` lines, each as its fields."""
    return [(line.split(":", 1) + [""])[1] for line in events()[n:] if line.split(":", 1)[0] == kind]


def xy(text):
    return tuple(int(v) for v in text.split(","))


# ---------- the phone ----------

# Before the page's own scripts: each pointermove's wait and handling, and where the cursor is shown each frame (the page's cost per move).
PROBE = r'''
(() => {
  const rec = window.__probe = { moves: [], frames: [], marks: [] };
  // Each ring the page puts on the picture (they go after a moment), as it is added.
  new MutationObserver((list) => {
    for (const m of list) for (const n of m.addedNodes) if (n.dataset?.screenMark) rec.marks.push(n.dataset.screenMark);
  }).observe(document, { childList: true, subtree: true });
  addEventListener("pointermove", (e) => rec.moves.push({ ts: e.timeStamp, start: performance.now(), end: 0 }), { capture: true });
  addEventListener("pointermove", () => { const m = rec.moves[rec.moves.length - 1]; if (m) m.end = performance.now(); });
  const tick = () => {
    const el = document.querySelector("[data-screen-cursor]");
    rec.frames.push({ at: performance.now(), t: el && el.style.display !== "none" ? el.style.transform : null });
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
})();
'''


class Phone:
    def __init__(self, pw):
        self.browser = pw.chromium.launch(channel="chrome") if ARGS.chrome else pw.chromium.launch()
        self.ctx = self.browser.new_context(viewport={"width": 390, "height": 844}, device_scale_factor=3, is_mobile=True, has_touch=True, ignore_https_errors=True,
                                           user_agent="Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Mobile/15E148 Safari/604.1")
        self.ctx.add_init_script(PROBE)
        self.page = self.ctx.new_page()
        self.errors = []
        self.page.on("pageerror", lambda e: self.errors.append(str(e)))
        self.cdp = self.ctx.new_cdp_session(self.page)

    def touch(self, kind, pts):
        self.cdp.send("Input.dispatchTouchEvent", {"type": kind, "touchPoints": [{"x": x, "y": y, "id": i} for i, (x, y) in enumerate(pts)]})

    def tap(self, x, y, hold=0.05):
        self.touch("touchStart", [(x, y)])
        time.sleep(hold)
        self.touch("touchEnd", [])

    def slide(self, x, y, dx, dy, steps=30, hold=0.0, fps=60):
        self.touch("touchStart", [(x, y)])
        time.sleep(hold)
        for i in range(1, steps + 1):
            self.touch("touchMove", [(x + dx * i / steps, y + dy * i / steps)])
            time.sleep(1 / fps)
        self.touch("touchEnd", [])

    def two(self, x, y, dx=0, dy=0, spread=0, steps=15):
        a, b = (x - 50, y), (x + 50, y)
        self.touch("touchStart", [a, b])
        time.sleep(0.03)
        for i in range(1, steps + 1):
            f = i / steps
            self.touch("touchMove", [(a[0] + dx * f - spread * f, a[1] + dy * f), (b[0] + dx * f + spread * f, b[1] + dy * f)])
            time.sleep(1 / 60)
        self.touch("touchEnd", [])

    def js(self, script, arg=None):
        return self.page.evaluate(script, arg)

    def has(self, selector):
        return self.page.locator(selector).count() > 0

    def close(self):
        self.browser.close()


def pair(phone, d):
    """The page paired on the secure origin, its passkey set up, given control on the desktop: True when it shows the inbox, unlocked."""
    page = phone.page
    page.goto(f"https://localhost:{PORT}/")
    page.wait_for_selector("[data-stay-in-browser], [data-scan]", timeout=20000)
    if phone.has("[data-stay-in-browser]"):
        page.click("[data-stay-in-browser]")
    page.wait_for_selector("[data-scan]", timeout=10000)
    page.click("[data-scan-start]")
    page.wait_for_selector("[data-paste]", timeout=15000)
    link = d.call("mobile.pair_start")["url"].replace(f"https://127.0.0.1:{PORT}/", f"https://localhost:{PORT}/")
    page.fill("[data-paste]", link)
    page.click("[data-paste-go]")
    page.wait_for_selector(".card input.field", timeout=10000)
    page.fill(".card input.field", PHONE_NAME)
    page.click(".card .btn.primary")
    code = page.wait_for_selector("[data-pair-code]", timeout=15000).text_content()
    request = ps.pending_request(d, PHONE_NAME)
    d.call("mobile.approve", {"id": request["id"], "allow": True, "code": code, "permission": "write"})
    page.wait_for_selector("[data-lock-screen]", timeout=20000)
    phone.cdp.send("WebAuthn.enable", {"enableUI": False})
    phone.cdp.send("WebAuthn.addVirtualAuthenticator", {"options": {"protocol": "ctap2", "transport": "internal", "hasResidentKey": True, "hasUserVerification": True, "isUserVerified": True, "automaticPresenceSimulation": True}})
    page.click("[data-lock-go]")
    page.wait_for_selector("[data-lock-screen]", state="detached", timeout=20000)
    device = next(x for x in d.call("mobile.status")["devices"] if x["name"] == PHONE_NAME)
    d.call("mobile.set_permission", {"id": device["id"], "permission": "control"})
    return bool(page.wait_for_selector("[data-see-screen]", timeout=20000))


class Picture:
    """Where the window's pixels are on the phone, at the view shown (zoom 1): the picture is the window's frame (WGC), fitted in the box."""

    def __init__(self, phone, frame):
        self.phone, self.frame = phone, frame

    def box(self):
        return self.phone.page.locator(".screen-canvas").bounding_box()

    def at(self, sx, sy):
        """A point of the computer's screen (physical pixels) → the phone's screen (CSS pixels)."""
        fx, fy, fw, fh = self.frame
        b = self.box()
        s = min(b["width"] / fw, b["height"] / fh)
        return b["x"] + b["width"] / 2 + (sx - fx - fw / 2) * s, b["y"] + b["height"] / 2 + (sy - fy - fh / 2) * s

    def back(self, cx, cy):
        """The phone's screen (CSS pixels) → the computer's screen (physical pixels)."""
        fx, fy, fw, fh = self.frame
        b = self.box()
        s = min(b["width"] / fw, b["height"] / fh)
        return fx + fw / 2 + (cx - b["x"] - b["width"] / 2) / s, fy + fh / 2 + (cy - b["y"] - b["height"] / 2) / s

    def scale(self):
        b = self.box()
        return min(b["width"] / self.frame[2], b["height"] / self.frame[3])

    def under(self, cx, cy, v):
        """The phone's screen (CSS pixels) → the computer's screen (physical pixels), at the page's view `v` (zoomed and panned)."""
        fx, fy = self.frame[0], self.frame[1]
        b = self.box()
        s = self.scale() * v["zoom"]
        return fx + v["cx"] + (cx - b["x"] - b["width"] / 2) / s, fy + v["cy"] + (cy - b["y"] - b["height"] / 2) / s


def shown_cursor(phone):
    """The phone's cursor (CSS pixels, viewport), or None when the page draws none of its own layer."""
    return phone.js("""() => {
      const el = document.querySelector("[data-screen-cursor]"), box = document.querySelector(".screen-canvas");
      if (!el || !box || el.style.display === "none") return null;
      const m = /translate3d\\(\\s*(-?[\\d.]+)px,\\s*(-?[\\d.]+)px/.exec(el.style.transform);
      const r = box.getBoundingClientRect();
      return m ? [r.left + Number(m[1]), r.top + Number(m[2])] : null;
    }""")


def view(phone):
    """How the page shows the picture now: {zoom, cx, cy} (source pixels at the box's centre). A page without the probe (an older build)
    gives NaNs, new ones each time (a dict holding the same NaN object would compare equal): every check that compares them fails, and
    the checks that need nothing of it still run."""
    return phone.js("() => window.__kelpieScreenView?.() ?? null") or {k: float("nan") for k in ("zoom", "cx", "cy")}


def page_still(phone):
    """The page itself is neither zoomed nor scrolled (only the app's view of the picture moves)."""
    return phone.js("""() => ({ scale: window.visualViewport?.scale ?? 1, x: window.scrollX, y: window.scrollY,
      main: document.querySelector("main")?.scrollTop ?? 0, body: document.body.scrollTop })""")


def still(state):
    return state["scale"] == 1 and not state["x"] and not state["y"] and not state["main"] and not state["body"]


def wait_event(n, kind, timeout=5):
    return until(lambda: since(n, kind), timeout) and since(n, kind)


def near(a, b, tol):
    return a is not None and b is not None and abs(a[0] - b[0]) <= tol and abs(a[1] - b[1]) <= tol


def start_screen(phone):
    """From the inbox: the picker, then the test window: the picture live. Returns the picker's ways (ids and whether usable)."""
    page = phone.page
    page.click("[data-see-screen]")
    row = page.wait_for_selector(f"[data-target]:has-text('{TITLE}')", timeout=30000)
    ways = phone.js("() => [...document.querySelectorAll('[data-via]')].map(r => [r.dataset.via, r.dataset.ok])")
    row.query_selector("button").click()
    page.wait_for_selector(".screen-live[data-phase=live]", timeout=30000)
    # Frames drawn: the page's own measurement.
    until(lambda: (phone.js("() => window.__kelpieScreenTimes?.().presentInterval.n") or 0) > 15, 20)
    if phone.has("[data-screen-help-close]"):
        page.click("[data-screen-help-close]")
    return ways


def scenario(d, run, pw):
    run.boot()
    ss.spawn("target", [sys.executable, "-c", TARGET, TITLE, EVENTS])
    check("the test window is up", until(lambda: geometry("pad") and geometry("field"), 20))
    hwnd = until(lambda: __import__("ctypes").windll.user32.FindWindowW(None, TITLE), 10)
    ss.to_front(hwnd)
    cert, key = ps.certificate_for_loopback(SETUP.work)
    d.call("mobile.enable", {"ip": "127.0.0.1", "port": PORT, "cert": cert, "key": key})
    phone = Phone(pw)
    try:
        run_phone(d, phone, hwnd)
    finally:
        check("no error on the page", not phone.errors, phone.errors[:3])
        phone.close()


def refused_for_load(phone):
    """At L3: the start is refused, the picker says why and when it asks again, and Stop trying stops the tries."""
    page = phone.page
    page.click("[data-see-screen]")
    row = page.wait_for_selector(f"[data-target]:has-text('{TITLE}')", timeout=30000)
    row.query_selector("button").click()
    banner = page.wait_for_selector(".takeover-pick .banner.bad", timeout=30000)
    text = banner.inner_text()
    check("an overloaded computer refuses the screen and the phone says why", "memory" in text and "Close some programs" in text, text)
    retry = page.wait_for_selector("[data-busy-retry]", timeout=10000)
    check("…and says when it tries again on its own", "Trying again in" in retry.inner_text(), retry.inner_text())
    check("no picture starts", page.locator(".screen-live").count() == 0)
    retry.query_selector("button").click()
    check("Stop trying stops the tries", until(lambda: page.locator("[data-busy-retry]").count() == 0, 5), page.locator("[data-busy-retry]").count())


def run_phone(d, phone, hwnd):
    page = phone.page
    # ---------- 1. pairing, control, the picker, the picture ----------
    check("the page pairs on the secure origin, sets up its passkey and is given control", pair(phone, d))
    if OVERLOADED:
        return refused_for_load(phone)
    ways = start_screen(phone)
    if (ARGS.load or "").upper().startswith(("L2", "2")):
        quality = until(lambda: page.locator(".screen-quality").inner_text().strip(), 10) or ""
        check("a loaded computer starts the screen low, reduced for the load", "computer busy" in quality and int(quality.split(" fps")[0].split("· ")[-1] or 99) <= 8, quality)
    check("the picker lists the test window and starts it: the picture is live", page.locator(".screen-live[data-phase=live]").count() == 1)
    # This computer offers the screen on its own network only: Automatic and peer to peer, or no choice at all (an older daemon).
    check("Connect through lists the ways this computer offers, Automatic usable", not ways or ["auto", "true"] in ways, ways)
    frame = ss.frame_bounds(hwnd)
    pic = Picture(phone, frame)
    pad = geometry("pad")
    field = geometry("field")
    centre = (pad[0] + pad[2] // 2, pad[1] + pad[3] // 2)
    tol = max(6, int(3 / pic.scale()))  # three phone pixels, in the computer's

    # ---------- 2. trackpad ----------
    check("the screen starts in trackpad mode", "Trackpad" in page.locator("[data-screen-mode]").inner_text())
    # Put the computer's cursor in the pad first: a tap at the pad in direct mode, then back to trackpad.
    page.click("[data-screen-mode]")
    n = len(events())
    phone.tap(*pic.at(*centre))
    got = wait_event(n, "click")
    check("direct touch: a tap clicks under the finger", bool(got) and near(xy(got[0]), centre, tol + 4), (got, centre))
    page.click("[data-screen-mode]")
    time.sleep(0.6)
    b = pic.box()
    start = (b["x"] + 60, b["y"] + b["height"] * 0.6)
    before = ss.cursor()
    n_moves = phone.js("() => window.__probe.moves.length")
    phone.js("() => (window.__probe.frames.length = 0)")
    phone.slide(*start, 120, -40, steps=60)
    stroke = phone.js("() => window.__probe.frames.length")
    time.sleep(1.2)
    after = ss.cursor()
    moved = (after[0] - before[0], after[1] - before[1])
    check("trackpad: a slide right and up moves the computer's cursor right and up", moved[0] > 40 and moved[1] < -5, moved)
    shown = shown_cursor(phone)
    check("…and the phone's cursor ends where the computer's is", shown is not None and near(pic.back(*shown), after, tol + 4), (shown and pic.back(*shown), after))
    probe = phone.js("(n) => window.__probe.moves.slice(n).map(m => m.end - m.start)", n_moves)
    frames = phone.js("(n) => window.__probe.frames.slice(0, n)", stroke)
    changed = sum(1 for a, c in zip(frames, frames[1:]) if a["t"] != c["t"])
    secs = (frames[-1]["at"] - frames[0]["at"]) / 1000 if len(frames) > 1 else 1
    COST.update(moves=len(probe), handler_ms_p50=round(statistics.median(probe), 2) if probe else None, handler_ms_max=round(max(probe), 2) if probe else None,
                cursor_updates_per_s=round(changed / secs, 1) if frames and frames[0]["t"] is not None else None)
    check("…the page handles a move in under 4 ms (median), the cursor on its own layer", bool(probe) and statistics.median(probe) < 4, COST)
    cursor_now = ss.cursor()
    n = len(events())
    phone.tap(b["x"] + 40, b["y"] + 40)
    got = wait_event(n, "click")
    check("trackpad: a tap clicks where the cursor is, not under the finger", bool(got) and near(xy(got[0]), cursor_now, 3), (got, cursor_now))
    time.sleep(0.6)
    n = len(events())
    phone.tap(b["x"] + 40, b["y"] + 40)
    time.sleep(0.15)
    phone.tap(b["x"] + 70, b["y"] + 60)
    got = wait_event(n, "double")
    check("trackpad: a double tap is a double click there", bool(got) and near(xy(got[0]), cursor_now, 3), (got, events()[n:]))
    check("…shown as a double click on the picture", "double" in phone.js("() => window.__probe.marks"))
    time.sleep(0.6)
    n = len(events())
    phone.tap(b["x"] + 40, b["y"] + 40, hold=0.8)
    got = wait_event(n, "right")
    check("trackpad: a long press right-clicks at the cursor", bool(got) and near(xy(got[0]), cursor_now, 3), got)
    check("…shown as a right click", "right" in phone.js("() => window.__probe.marks"))
    n = len(events())
    phone.two(b["x"] + 200, b["y"] + 200, steps=0)
    got = wait_event(n, "right")
    check("trackpad: a two-finger tap right-clicks", bool(got), events()[n:])
    # A drag: the hint while held, the chip while dragging, the window sees press, move and release.
    n = len(events())
    phone.touch("touchStart", [(b["x"] + 60, b["y"] + 200)])
    time.sleep(0.7)
    check("…a long press shows what it will do before it does it", phone.has("[data-screen-held]"))
    for i in range(1, 31):
        phone.touch("touchMove", [(b["x"] + 60 + 3 * i, b["y"] + 200 + i)])
        time.sleep(1 / 60)
    check("…and moving then shows the drag going on", phone.has("[data-screen-dragging]"))
    phone.touch("touchEnd", [])
    got = wait_event(n, "drag")
    if got:
        a, z = got[0].split(":")
        dist = (xy(z)[0] - xy(a)[0], xy(z)[1] - xy(a)[1])
    check("trackpad: a long press then a move drags in the window", bool(got) and dist[0] > 40, (got, events()[n:]))
    check("…and the drag's chip goes when the finger lifts", not phone.has("[data-screen-dragging]"))
    # The wheel turns where the cursor is, which the drag may have taken out of the window (a wheel there is refused by design): back in the
    # window first, with a tap in direct mode.
    page.click("[data-screen-mode]")
    phone.tap(*pic.at(*centre))
    page.click("[data-screen-mode]")
    time.sleep(0.6)
    n = len(events())
    phone.two(b["x"] + 190, b["y"] + 150, dy=-120)
    got = wait_event(n, "wheel")
    check("trackpad: two fingers scroll", bool(got), (events()[n:], ss.cursor()))

    # ---------- 3. the help sheet ----------
    if phone.has("[data-screen-help]"):
        page.click("[data-screen-help]")
    sheet = phone.js("() => document.querySelector('[data-screen-help-sheet]')?.textContent ?? ''")
    check("the ? button shows the gestures of the mode in use", "Tap twice" in sheet and "two fingers" in sheet, sheet[:120])
    if phone.has("[data-screen-help-close]"):
        page.click("[data-screen-help-close]")

    # ---------- 4. direct touch ----------
    page.click("[data-screen-mode]")
    check("the mode switches to direct touch", "Direct" in page.locator("[data-screen-mode]").inner_text())
    target = (pad[0] + 200, pad[1] + 150)
    time.sleep(0.6)
    n = len(events())
    p = pic.at(*target)
    phone.tap(*p)
    time.sleep(0.15)
    phone.tap(p[0] + 5, p[1] + 4)
    got = wait_event(n, "double")
    check("direct touch: a double tap is a double click at the first tap's point", bool(got) and near(xy(got[0]), target, tol + 4), (got, target, events()[n:]))
    time.sleep(0.6)
    n = len(events())
    phone.tap(*pic.at(target[0] + 120, target[1]), hold=0.8)
    got = wait_event(n, "right")
    check("direct touch: a long press right-clicks under the finger", bool(got) and near(xy(got[0]), (target[0] + 120, target[1]), tol + 4), got)
    n = len(events())
    p = pic.at(target[0], target[1] + 120)
    phone.touch("touchStart", [p])
    time.sleep(0.7)
    for i in range(1, 25):
        phone.touch("touchMove", [(p[0] + 2 * i, p[1])])
        time.sleep(1 / 60)
    check("direct touch: a magnifier shows above the finger while dragging", phone.has("[data-screen-loupe]"))
    phone.touch("touchEnd", [])
    got = wait_event(n, "drag")
    check("direct touch: a long press then a move drags from under the finger", bool(got) and near(xy(got[0].split(":")[0]), (target[0], target[1] + 120), tol + 4), got)
    check("…and the magnifier goes when the finger lifts", not phone.has("[data-screen-loupe]"))
    n = len(events())
    phone.slide(*pic.at(*centre), 0, -80, steps=12)
    got = wait_event(n, "wheel")
    check("direct touch: a quick slide scrolls", bool(got), events()[n:])

    # ---------- 5. pinch zoom ----------
    page.click("[data-screen-mode]")  # trackpad: the zoom goes around the cursor
    time.sleep(0.3)
    b = pic.box()
    phone.two(b["x"] + b["width"] / 2, b["y"] + b["height"] / 2, spread=90)
    zoomed = phone.has("[data-screen-fit]") or page.locator(".takeover-head button:has-text('Fit')").count() > 0
    check("a pinch zooms the phone's view in (Fit shows)", zoomed)
    shown = shown_cursor(phone)
    b = pic.box()
    check("…and the cursor stays in the picture shown", shown is not None and b["x"] <= shown[0] <= b["x"] + b["width"] and b["y"] <= shown[1] <= b["y"] + b["height"], (shown, b))
    phone.slide(b["x"] + 40, b["y"] + 60, 150, 0, steps=40)
    time.sleep(0.4)
    shown = shown_cursor(phone)
    check("…also after a trackpad slide toward the edge (the view follows it)", shown is not None and b["x"] <= shown[0] <= b["x"] + b["width"], (shown, b))
    zoomed_fingers(phone, pic, pad, centre)
    if zoomed:
        page.locator(".takeover-head button:has-text('Fit')").click()
    check("Fit goes back to the whole picture", not page.locator(".takeover-head button:has-text('Fit')").count())

    # ---------- 6. text ----------
    page.click("[data-screen-mode]")  # direct: tap the field
    time.sleep(0.6)
    n = len(events())
    phone.tap(*pic.at(field[0] + 40, field[1] + field[3] // 2))
    check("a tap focuses the window's text field", bool(wait_event(n, "focus")), events()[n:])
    box = page.locator("[data-screen-text], .takeover-text input").first
    box.fill(TYPED)
    check("the text field shows what is typed (not dots)", box.get_attribute("type") == "text", box.get_attribute("type"))
    if phone.has("[data-screen-text-hide]"):
        page.click("[data-screen-text-hide]")
        check("…and Hide masks it, for a password", box.get_attribute("type") == "password")
        page.click("[data-screen-text-hide]")
    else:
        check("…and Hide masks it, for a password", False, "no Hide button")
    n = len(events())
    box.press("Enter")
    check("text with Vietnamese diacritics arrives whole", bool(until(lambda: TYPED in since(n, "text"), 10)), since(n, "text")[-1:])
    check("…and the phone says it was typed", bool(until(lambda: page.locator("[data-text-result=sent]").count(), 10)))
    check("…and the keyboard stays on the field", phone.js("() => document.activeElement?.matches('[data-screen-text], .takeover-text input') ?? false"))
    for label, keysym in (("Backspace", "BackSpace"), ("Enter", "Return"), ("Tab", "Tab"), ("Escape", "Escape")):
        n = len(events())
        page.click(f".keybar button[aria-label='{label}']")
        got = until(lambda: any(e.startswith(keysym + ":") for e in since(n, "key")), 5)
        check(f"the key bar's {label} reaches the window", bool(got), since(n, "key"))
        if label == "Backspace":
            check("…and takes one letter off", bool(until(lambda: since(n, "text") and since(n, "text")[-1] == TYPED[:-1], 5)), since(n, "text")[-1:])
        if label == "Tab":
            # Tab moves the window's focus out of the field: it is given back for the next key.
            phone.tap(*pic.at(field[0] + 40, field[1] + field[3] // 2))
            time.sleep(0.5)
    n = len(events())
    page.locator(".keybar button:has-text('Ctrl')").click()
    box.fill("a")
    box.press("Enter")
    got = until(lambda: [e for e in since(n, "key") if e.lower().startswith("a:1")], 5)
    check("Ctrl then a letter is a shortcut (Ctrl+A), not text", bool(got), since(n, "key"))

    # ---------- 7. Stop, again, leaving the app ----------
    page.click("[data-screen-stop]")
    check("Stop ends the screen", bool(page.wait_for_selector("[data-screen-ended]", timeout=5000)))
    page.click("[data-screen-ended] .btn")
    stops = json.dumps(d.call("mobile.audit", {"limit": 50})["entries"]).count("screen_stop")
    start_screen(phone)
    check("…and it starts again", page.locator(".screen-live[data-phase=live]").count() == 1)
    phone.js("""() => {
      Object.defineProperty(document, "visibilityState", { configurable: true, get: () => "hidden" });
      Object.defineProperty(document, "hidden", { configurable: true, get: () => true });
      document.dispatchEvent(new Event("visibilitychange"));
    }""")
    t0 = time.time()
    gone = until(lambda: page.locator(".screen-live").count() == 0, 3)
    check("leaving the app ends the screen within a second", bool(gone) and time.time() - t0 < 1.5, time.time() - t0)
    ended = lambda: json.dumps(d.call("mobile.audit", {"limit": 50})["entries"]).count("screen_stop")  # noqa: E731
    check("…and the computer ended it too (its audit says so)", bool(until(lambda: ended() >= stops + 1, 5)), (stops, ended()))


def zoomed_fingers(phone, pic, pad, centre):
    """Zoomed in, what the owner's iPhone got wrong: one finger moves the cursor and the view follows it only near an edge; the page itself
    never moves."""
    page = phone.page
    b = pic.box()
    v0 = view(phone)
    check("zoomed: the page shows its view to a test driver", v0["zoom"] > 1.2, v0)
    check("zoomed: the page itself is not zoomed or scrolled", still(page_still(phone)), page_still(phone))
    # The page's guards are wired: a finger moving on the picture and Safari's pinch events are cancelled.
    guarded = phone.js("""() => {
      const box = document.querySelector(".screen-canvas");
      const t = new Touch({ identifier: 9, target: box, clientX: 10, clientY: 10 });
      const move = new TouchEvent("touchmove", { cancelable: true, bubbles: true, touches: [t], targetTouches: [t], changedTouches: [t] });
      box.dispatchEvent(move);
      const g = new Event("gesturestart", { cancelable: true });
      document.dispatchEvent(g);
      return [move.defaultPrevented, g.defaultPrevented];
    }""")
    check("…a touch moving on the picture and Safari's gesturestart are cancelled (non-passive guards)", guarded == [True, True], guarded)

    # Trackpad, the cursor first put in the middle of the picture shown (a slide from wherever it is, then back to the middle on the glass).
    mid = (b["x"] + b["width"] / 2, b["y"] + b["height"] / 2)
    for _ in range(3):
        c = shown_cursor(phone)
        if c and abs(c[0] - mid[0]) < 30 and abs(c[1] - mid[1]) < 30:
            break
        if c:
            phone.slide(mid[0], mid[1], (mid[0] - c[0]) / 1.2, (mid[1] - c[1]) / 1.2, steps=40, fps=30)
            time.sleep(0.5)
    c = shown_cursor(phone)
    check("zoomed trackpad: the cursor is put near the middle of the picture shown first", near(c, mid, 60), (c, mid))
    v1 = view(phone)
    before = ss.cursor()
    n = len(events())
    phone.slide(mid[0] - 40, mid[1] + 40, 30, 0, steps=20, fps=30)
    time.sleep(0.8)
    after = ss.cursor()
    v2 = view(phone)
    check("zoomed trackpad: a short slide moves the computer's cursor", after[0] - before[0] > 3, (before, after))
    check("…and the view stays where it is (the cursor moves, not the picture)", v2 == v1, (v1, v2))
    check("…and sends no scroll", not since(n, "wheel"), since(n, "wheel"))
    # A long slide right: the cursor reaches the inner line, then the view follows, the cursor kept inside the picture shown.
    n = len(events())
    phone.slide(b["x"] + 30, mid[1] + 40, b["width"] - 60, 0, steps=60, fps=30)
    phone.slide(b["x"] + 30, mid[1] + 40, b["width"] - 60, 0, steps=60, fps=30)
    time.sleep(0.8)
    v3 = view(phone)
    shown = shown_cursor(phone)
    check("zoomed trackpad: a long slide right makes the view follow the cursor", v3["cx"] > v2["cx"] + 5, (v2, v3))
    check("…the cursor kept inside the picture shown, off the edge", shown is not None and b["x"] + 30 <= shown[0] <= b["x"] + b["width"] - 30, (shown, b))
    check("…the view never moved up or down for a slide right", abs(v3["cy"] - v2["cy"]) < 1, (v2, v3))
    check("…and still no scroll, and the page itself never moved", not since(n, "wheel") and still(page_still(phone)), (since(n, "wheel"), page_still(phone)))

    # Two fingers moving together, zoomed in: the view moves, nothing reaches the computer.
    n = len(events())
    cur = ss.cursor()
    # Right, back the way the view just went (it may stand at the picture's right edge now).
    phone.two(mid[0], mid[1], dx=80, steps=20)
    time.sleep(0.8)
    v4 = view(phone)
    check("zoomed: two fingers moving together move the phone's view", v4["cx"] < v3["cx"] - 5, (v3, v4))
    check("…and send nothing: no scroll, no click, the cursor where it was", not events()[n:] and near(ss.cursor(), cur, 2), (events()[n:], cur, ss.cursor()))

    # Direct touch, zoomed: a one-finger slide moves the cursor under the finger.
    page.click("[data-screen-mode]")
    time.sleep(0.4)
    n = len(events())
    end = (mid[0] + 40, mid[1] + 20)
    phone.slide(mid[0] - 40, mid[1] - 20, 80, 40, steps=20, fps=30)
    time.sleep(0.8)
    v5 = view(phone)
    shown = shown_cursor(phone)
    tol = max(6, int(3 / (pic.scale() * (v4["zoom"] if v4["zoom"] > 1 else 1))))
    check("zoomed direct touch: a slide moves the cursor, which ends under the finger", near(shown, end, 3) and near(ss.cursor(), pic.under(*end, v4), tol + 4), (shown, end, ss.cursor(), pic.under(*end, v4)))
    check("…and sends no scroll, the view where it was", not since(n, "wheel") and v5 == v4, (since(n, "wheel"), v4, v5))
    # A tap near the right edge: a click there (where the computer's cursor went), the view where it was.
    edge = (b["x"] + b["width"] - 8, mid[1])
    phone.tap(*edge)
    time.sleep(0.8)
    check("zoomed direct touch: a tap near an edge clicks under the finger and does not pan the view", view(phone) == v5 and near(ss.cursor(), pic.under(*edge, v5), tol + 4), (v5, view(phone), ss.cursor(), pic.under(*edge, v5)))
    time.sleep(0.6)
    # A finger slid to an edge and held there (the side with more picture beyond it): the view pans that way, the cursor under the finger.
    half = b["width"] / (pic.scale() * v5["zoom"]) / 2
    left = v5["cx"] - half > pic.frame[2] - (v5["cx"] + half)
    at_edge = (b["x"] + 6 if left else b["x"] + b["width"] - 6, mid[1])
    phone.touch("touchStart", [mid])
    for i in range(1, 16):
        phone.touch("touchMove", [(mid[0] + (at_edge[0] - mid[0]) * i / 15, mid[1])])
        time.sleep(1 / 30)
    time.sleep(1.0)
    phone.touch("touchEnd", [])
    time.sleep(0.5)
    # Read once the finger is up: the pan stops with it, and the cursor was put under the finger at the view's last frame.
    v6 = view(phone)
    shown = shown_cursor(phone)
    moved = (v5["cx"] - v6["cx"]) if left else (v6["cx"] - v5["cx"])
    check("zoomed direct touch: a finger held at an edge pans the view that way", moved > 5, (left, v5, v6))
    check("…the cursor still under the finger", near(shown, at_edge, 4) and near(ss.cursor(), pic.under(*at_edge, v6), tol + 4), (shown, ss.cursor(), pic.under(*at_edge, v6)))
    check("…and the page itself never zoomed or scrolled", still(page_still(phone)), page_still(phone))
    page.click("[data-screen-mode]")  # back to trackpad, as the rest expects
    time.sleep(0.3)


COST = {}


def main():
    from playwright.sync_api import sync_playwright

    d = run = None
    try:
        d, run = ss.start(SETUP)
        with sync_playwright() as pw:
            scenario(d, run, pw)
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False)
    finally:
        ss.finish(SETUP, d, run, "target")
    failed = [r for r in ss.results if not r[1]]
    summary = {"passed": len(ss.results) - len(failed), "failed": len(failed), "cost": COST, "checks": [{"name": n, "ok": ok} for n, ok in ss.results]}
    if ARGS.json:
        with open(ARGS.json, "w", encoding="utf-8") as f:
            json.dump(summary, f, indent=1, ensure_ascii=False)
    print(json.dumps({k: summary[k] for k in ("passed", "failed", "cost")}))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
