"""README pictures and a clip of Kelpie with a phone, on Windows: the phone page in an iPhone-sized Chromium beside the real app.

    python -X utf8 scripts/e2e/readme_phone_win.py <scratch-dir> <app> <out-dir> --what screen|answer [--media DIR]

<app> is a debug build of the app with its frontend embedded, with the debug kelpied (built after `pnpm build:phone`) and kelpie-screen next
to it. Needs tauri-driver (PATH or ~/.cargo/bin), msedgedriver matching WebView2 (PATH or MSEDGEDRIVER), Playwright for Python with
Chromium, Python's `cryptography`, `websockets` and Pillow; `--what answer` also needs ffmpeg on PATH.

Isolation as screen_phone.py's (screen_support.prepare/start): this process's own endpoint and data folder, a fake Tailscale CLI, the app
started by this script's tauri-driver; only what this script started is stopped. The phone page is served by this daemon on 127.0.0.1 with a
certificate made for this run (opened at `localhost`); the app lock is set up with Chromium's virtual authenticator.

--what screen: run as a standard (non-administrator) Windows user, since Kelpie does not share the screen while elevated. The phone sees this
  computer's whole screen (the app with demo projects and a ticking shell, a demo Notepad in front) and gets control. Writes
  <out-dir>/phone-screen-windows.png (the phone page). The desktop's sharing border is not pictured: Windows keeps it out of screen captures.
--what answer: a shell session waits at a demo menu (a small Python program, never an agent); the phone opens it from its inbox and taps 1;
  the desktop pane carries on. The desktop (ffmpeg gdigrab, cut to the app's window) and the phone (Playwright's video) are put side by side:
  <media>/clip-phone-answer.mp4 (no sound, under 3 MB) and <out-dir>/clip-phone-answer-poster.png.
"""
import argparse
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import screen_support as ss  # noqa: E402
from screen_support import check, until  # noqa: E402

SETUP = ss.prepare("readme-phone", __doc__)
import phone_support as ps  # noqa: E402
import readme_shots as rs  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("scratch")
ap.add_argument("app")
ap.add_argument("out")
ap.add_argument("--what", choices=["screen", "answer"], required=True)
ap.add_argument("--media", default=None)
ARGS = ap.parse_args()
OUT = os.path.abspath(ARGS.out)
os.makedirs(OUT, exist_ok=True)
PORT = ss.free_port()
PHONE_NAME = "Demo iPhone"
S = "window.__kelpie.store"
IPHONE_UA = "Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Mobile/15E148 Safari/604.1"

NOTES = """atlas-commerce 0.9 - release notes (demo)

- Coupon codes pasted with spaces or in lower case now work.
- Free shipping from 10,000 discounted cents, checked at 9,999 and 10,000.
- The catalog returns 404 for an unknown item.

Sample text of the Kelpie demo projects.
"""

# A demo menu in the style of an agent's question; after the answer it carries on, so the desktop shows the phone's tap land.
MENU = r'''
import sys, time
print("Bash(python scripts/migrate.py --apply)")
print(" Apply 3 pending migrations to the demo database?")
print(" > 1. Yes")
print("   2. Yes, and don't ask again for this command")
print("   3. No, and say what to do differently (esc)")
# One key, as an agent's menu takes it (no Enter).
try:
    import msvcrt
    choice = msvcrt.getwch()
except ImportError:
    import termios, tty
    fd = sys.stdin.fileno()
    old = termios.tcgetattr(fd)
    tty.setraw(fd)
    choice = sys.stdin.read(1)
    termios.tcsetattr(fd, termios.TCSADRAIN, old)
print("")
print(f"  answered {choice!r}: applying")
for name in ("0007_coupon_codes", "0008_free_shipping_threshold", "0009_catalog_tags"):
    time.sleep(0.9)
    print(f"  * {name} ... ok", flush=True)
print("  3 migrations applied (demo database)", flush=True)
time.sleep(600)
'''


class Phone:
    def __init__(self, pw, video_dir=None):
        self.browser = pw.chromium.launch()
        opts = dict(viewport={"width": 390, "height": 844}, device_scale_factor=3, is_mobile=True, has_touch=True, ignore_https_errors=True,
                    user_agent=IPHONE_UA, color_scheme="dark")
        if video_dir:
            opts.update(record_video_dir=video_dir, record_video_size={"width": 390, "height": 844})
        self.ctx = self.browser.new_context(**opts)
        self.t0 = time.time()
        self.page = self.ctx.new_page()
        self.errors = []
        self.page.on("pageerror", lambda e: self.errors.append(str(e)))
        self.cdp = self.ctx.new_cdp_session(self.page)

    def tap(self, selector):
        box = self.page.locator(selector).first.bounding_box()
        x, y = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
        self.cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": x, "y": y}]})
        time.sleep(0.08)
        self.cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})

    def has(self, selector):
        return self.page.locator(selector).count() > 0


def pair(phone, d, permission):
    """The page paired on the secure origin, its passkey set up, then given `permission`; it shows the inbox, unlocked."""
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
    phone.cdp.send("WebAuthn.addVirtualAuthenticator", {"options": {"protocol": "ctap2", "transport": "internal", "hasResidentKey": True, "hasUserVerification": True,
                                                                    "isUserVerified": True, "automaticPresenceSimulation": True}})
    page.click("[data-lock-go]")
    page.wait_for_selector("[data-lock-screen]", state="detached", timeout=20000)
    if permission != "write":
        device = next(x for x in d.call("mobile.status")["devices"] if x["name"] == PHONE_NAME)
        d.call("mobile.set_permission", {"id": device["id"], "permission": permission})
    page.wait_for_selector("[data-view], .view-switch, [data-see-screen]", timeout=20000)


def demo_projects(run, names):
    """Some of the demo kit's projects in the app (readme_demo.py's kit next to the scratch folder, else none): (ids, paths)."""
    root = os.environ.get("KELPIE_DEMO_ROOT", r"C:\kelpie-demo-projects")
    ids = []
    for name in names:
        path = os.path.join(root, name)
        if os.path.isdir(path):
            ids.append((run.d.js(f"return {S}.addProject(arguments[0], {{open: false}})", path), path))
    if ids:
        run.d.js(f"{S}.switchProject(arguments[0]); {S}.showRightPanel('files')", ids[0][0])
    return ids


def shell(run, pid, name, command):
    info = run.d.js_async(f"const done=arguments[arguments.length-1]; {S}.newSession(arguments[0],'shell',{{name:arguments[1],where:'tab'}}).then(done)", pid, name)
    time.sleep(2)
    ss.app_daemon(run.d, "session.input", {"handle": info["handle"], "data": rs.PROMPT + "\r"})
    time.sleep(0.5)
    if command:
        ss.app_daemon(run.d, "session.input", {"handle": info["handle"], "data": command + "\r"})
    return info


def app_pid():
    return rs.native_app_pid(ss.procs["driver"].pid, SETUP.app)


def what_screen(d, run, pw):
    run.d.set_window_size(1280, 820)
    rs.move_window(app_pid(), 40, 40)
    run.d.js(f"{S}.setSettings({{theme: 'dark', usageMeters: false, notifications: false, tips: false, sidebarWidth: 260}})")
    ids = demo_projects(run, ["atlas-commerce", "beacon-api", "relay-workers"])
    if ids:
        # Something on the screen keeps changing, so the phone's picture shows a live frame rate.
        from demo_agents import write_programs
        ticker = write_programs(os.path.join(SETUP.work, "agents"))["ticker"]
        shell(run, ids[0][0], "Checks (watch)", f'{"Clear-Host" if os.name == "nt" else "clear"}; python -u "{ticker}"')
    notes = os.path.join(SETUP.work, "release-notes.txt")
    with open(notes, "w", encoding="utf-8") as f:
        f.write(NOTES)
    ss.spawn("notepad", ["notepad.exe", notes])
    hwnd = until(lambda: __import__("ctypes").windll.user32.FindWindowW(None, "release-notes.txt - Notepad") or __import__("ctypes").windll.user32.FindWindowW(None, "release-notes - Notepad"), 15)
    if hwnd:
        __import__("ctypes").windll.user32.MoveWindow(hwnd, 760, 380, 900, 520, True)
        ss.to_front(hwnd)
    cert, key = ps.certificate_for_loopback(SETUP.work)
    d.call("mobile.enable", {"ip": "127.0.0.1", "port": PORT, "cert": cert, "key": key})
    phone = Phone(pw)
    try:
        pair(phone, d, "control")
        page = phone.page
        page.click("[data-see-screen]")
        first = page.wait_for_selector("[data-target]:has-text('Screen'), [data-lock-screen=unlock]", timeout=30000)
        if first.get_attribute("data-lock-screen") == "unlock":
            page.click("[data-lock-go]")
            page.wait_for_selector("[data-lock-screen]", state="detached", timeout=20000)
        row = page.wait_for_selector("[data-target]:has-text('Screen')", timeout=30000)
        row.query_selector("button").click()
        page.wait_for_selector(".screen-live[data-phase=live]", timeout=30000)
        until(lambda: (phone.page.evaluate("() => window.__kelpieScreenTimes?.().presentInterval.n") or 0) > 15, 20)
        if phone.has("[data-screen-help-close]"):
            page.click("[data-screen-help-close]")
        if hwnd:
            ss.to_front(hwnd)
        time.sleep(3)
        page.screenshot(path=os.path.join(OUT, "phone-screen-windows.png"))
        check("the phone shows this computer's screen, live", phone.has(".screen-live[data-phase=live]"))
        print("SHOT phone-screen-windows", flush=True)
    finally:
        check("no error on the phone page", not phone.errors, phone.errors[:3])
        phone.browser.close()


def what_answer(d, run, pw):
    media = os.path.abspath(ARGS.media or OUT)
    os.makedirs(media, exist_ok=True)
    run.d.set_window_size(1280, 860)
    rs.move_window(app_pid(), 0, 0)
    run.d.js(f"{S}.setSettings({{theme: 'dark', usageMeters: false, notifications: false, tips: false, sidebarWidth: 260, fontSize: 14}})")
    # The session runs in a plain folder (no Git remote, so the phone has no pull request to look up), beside two demo projects.
    ops = os.path.join(SETUP.work, "ops-scripts")
    os.makedirs(os.path.join(ops, "scripts"), exist_ok=True)
    with open(os.path.join(ops, "scripts", "migrate.py"), "w", encoding="utf-8") as f:
        f.write("# Demo migration runner (sample file of the Kelpie demo).\n")
    # A repository with an origin of its own on this computer (a bare copy), as a project usually has one.
    g = lambda *a, cwd=ops: subprocess.run(["git", *a], cwd=cwd, check=True, capture_output=True)  # noqa: E731
    g("init", "-q", "-b", "main")
    g("add", "-A")
    g("-c", "user.name=Kelpie demo", "-c", "user.email=demo@example.com", "commit", "-q", "-m", "chore: demo migrations")
    g("clone", "-q", "--bare", ops, ops + ".git", cwd=SETUP.work)
    g("remote", "add", "origin", ops + ".git")
    g("push", "-q", "-u", "origin", "main")
    ids = [(run.d.js(f"return {S}.addProject(arguments[0], {{open: false}})", ops), ops)] + demo_projects(run, ["atlas-commerce", "beacon-api"])
    run.d.js(f"{S}.switchProject(arguments[0])", ids[0][0])
    menu = os.path.join(SETUP.work, "demo_menu.py")
    with open(menu, "w", encoding="utf-8") as f:
        f.write(MENU)
    cert, key = ps.certificate_for_loopback(SETUP.work)
    d.call("mobile.enable", {"ip": "127.0.0.1", "port": PORT, "cert": cert, "key": key})
    d.call("mobile.set_projects", {"projects": {pid: os.path.basename(path) for pid, path in ids}})
    video_dir = os.path.join(SETUP.work, "phone-video")
    phone = Phone(pw, video_dir)
    try:
        pair(phone, d, "write")
        info = shell(run, ids[0][0], "Migration", f"{'Clear-Host' if os.name == 'nt' else 'clear'}; python -u \"{menu}\"")
        run.d.js(f"{S}.revealSession(arguments[0]); {S}.setRightPanel(null)", info["id"])
        until(lambda: next((s for s in d.call("session.list") if s["handle"] == info["handle"]), {}).get("status") == "waiting", 30)
        time.sleep(1.5)
        rec = rs.Recorder(os.path.join(SETUP.work, "desktop.mp4"))
        start_desktop = time.time()
        start_phone = start_desktop - phone.t0
        time.sleep(2.5)
        phone.page.locator(".row, [data-id]").filter(has_text="Migration").first.click()
        phone.page.wait_for_selector("[data-prompt] .key-answer", timeout=20000)
        time.sleep(2.0)
        tapped_at = time.time() - start_desktop
        phone.tap("[data-prompt] .key-answer >> nth=0")
        check("the desktop pane carries on after the phone's answer",
              until(lambda: "migrations applied" in ss.app_daemon(run.d, "session.read", {"handle": info["handle"], "lines": 40})["text"], 20))
        time.sleep(2.5)
        length = time.time() - start_desktop
        rec.stop()
        phone.ctx.close()
        webm = next(os.path.join(video_dir, f) for f in os.listdir(video_dir) if f.endswith(".webm"))
        desk = os.path.join(SETUP.work, "desktop-cut.mp4")
        rec.finish(desk, rs.window_rect(app_pid()), start_desktop - rec.t0, 1.0, 844, limit=50_000_000)
        out = os.path.join(media, "clip-phone-answer.mp4")
        for crf in (28, 31, 34, 37):
            subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", desk, "-ss", f"{start_phone:.2f}", "-i", webm, "-t", f"{length:.2f}", "-an",
                            "-filter_complex", "[0:v]setpts=PTS-STARTPTS,fps=15[a];[1:v]setpts=PTS-STARTPTS,fps=15,scale=-2:844,pad=iw+24:844:24:0:color=0x0f141c[b];[a][b]hstack=inputs=2,format=yuv420p",
                            "-c:v", "libx264", "-preset", "slow", "-crf", str(crf), "-movflags", "+faststart", out], check=True)
            if os.path.getsize(out) <= 3_000_000:
                break
        rs.poster(out, min(length - 0.5, tapped_at + 2.5), os.path.join(OUT, "clip-phone-answer-poster.png"))
        check("the clip is under 3 MB", os.path.getsize(out) <= 3_000_000, os.path.getsize(out))
        print(f"CLIP phone-answer {length:.1f} s, {os.path.getsize(out)} bytes", flush=True)
    finally:
        check("no error on the phone page", not phone.errors, phone.errors[:3])
        phone.browser.close()


def main():
    from playwright.sync_api import sync_playwright

    d = run = None
    try:
        d, run = ss.start(SETUP)
        with sync_playwright() as pw:
            (what_screen if ARGS.what == "screen" else what_answer)(d, run, pw)
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        import traceback
        traceback.print_exc()
        check("ran to completion", False)
    finally:
        ss.finish(SETUP, d, run, "notepad")
    failed = [r for r in ss.results if not r[1]]
    print(f"{len(ss.results) - len(failed)} passed, {len(failed)} failed", flush=True)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
