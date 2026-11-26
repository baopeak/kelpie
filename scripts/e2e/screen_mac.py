#!/usr/bin/env python3
"""The phone's remote screen on a Mac, end to end: the real app (a debug Kelpie.app, with its daemon and screen helper inside), a daemon of this
script's own, and the phone page in Google Chrome as an iPhone (Playwright: 390×844 at 3×, touch, a virtual authenticator for the app lock).

    python3 scripts/e2e/screen_mac.py <scratch-dir> <Kelpie.app> [--json out.json] [--chromium] [--no-refusal] [--shots DIR] [--video DIR]

--shots and --video make the README's pictures and clip from the same run (see their help); the document then has a demo name. Whatever
else is on the Mac's screen is in them: leave only the desktop and the app.

<Kelpie.app> is a debug bundle: `cd app && pnpm build:phone`, then `cargo build -p kelpied -p kelpie-screen`, the two copied to
`app/src-tauri/binaries/<name>-<host triple>`, then `APPLE_SIGNING_IDENTITY=- pnpm tauri build --debug --bundles app --config
'{"bundle":{"externalBin":["binaries/kelpied","binaries/kelpie-screen"]}}'` (the speech helper left out). Needs Python with Playwright,
`cryptography` and `websockets`, and Google Chrome (`--chromium`: Playwright's own Chromium, which may not decode H.264).

macOS asks for two permissions, Screen Recording and Accessibility, and counts them for the process responsible for the app. Started from
this script, the app is this shell's: the permissions are the ones the shell's own app was given (Terminal, or sshd over SSH). Started
through LaunchServices (`open`), it is its own app, which has neither: that is how the refusal is checked, and nothing is reset. Over SSH,
unlock the login keychain first (the daemon keeps the phone keys there), and keep the Mac's own session signed in and unlocked: the app
must run in it, and the daemon alone cannot see the screen.

Isolation: this process's own endpoint and data folder (phone_support.isolate) before anything starts; a fake Tailscale CLI; phone access on
127.0.0.1 with a certificate made for this run, opened at `localhost` (a passkey needs a name). Only the processes this script started are
stopped, by PID, and TextEdit is quit only if this script started it. TextEdit is driven only through the phone: AppleScript would ask for
an automation permission nobody can answer over SSH.

Checks, each PASS or FAIL, then a summary (`--json` writes them too):
1. The app reaches this daemon and offers the screen (a phone's list is not refused as `no_app`).
2. The page pairs, its passkey is set up, it is given control; the picker lists the TextEdit window in the Mac's words.
3. Seeing it brings a live picture; the key bar is ⌘ ⌥ ⌃ ⇧ with ⌘Space and no menu key.
4. With Finder in front, a tap on the picture (direct touch) clicks the TextEdit window, which comes to the front.
5. Text typed on the phone arrives, and ⌘ then S saves it: the file holds it.
6. Stop ends it; seen again, Control+Option+Shift+K from the key bar (the stop hotkey) ends it with the hotkey's words.
7. Without Screen Recording (the app opened through LaunchServices, which then lists the display and no window): the start is refused and the
   picker says which permission is missing.
"""
import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import time
import traceback

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import phone_support as ps  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("scratch")
ap.add_argument("app")
ap.add_argument("--json", default=None)
ap.add_argument("--chromium", action="store_true", help="Playwright's Chromium instead of the installed Google Chrome")
ap.add_argument("--no-refusal", action="store_true", help="skip the check without Screen Recording (it opens the app through LaunchServices)")
ap.add_argument("--shots", default=None, help="a folder for README pictures: the phone page seeing the Mac (mac-screen-from-phone.png) and the "
                "Mac's whole screen while it is shared (mac-sharing-border.png)")
ap.add_argument("--video", default=None, help="a folder for the README clip mac-phone-control.mp4 (the phone page beside the Mac's screen while "
                "the phone taps into TextEdit, types and presses ⌘S; needs ffmpeg) and its poster still")
ARGS = ap.parse_args()
if sys.platform != "darwin":
    raise SystemExit(__doc__)

APP = os.path.abspath(ARGS.app)
APP_BIN = os.path.join(APP, "Contents", "MacOS", "kelpie")
KELPIED = os.path.join(APP, "Contents", "MacOS", "kelpied")
HELPER = os.path.join(APP, "Contents", "MacOS", "kelpie-screen")
for what, path in (("app", APP_BIN), ("daemon in the app", KELPIED), ("screen helper in the app", HELPER)):
    if not os.path.exists(path):
        raise SystemExit(f"no {what}: {path}")

DATA = ps.isolate("screen-mac")
WORK = os.path.join(os.path.abspath(ARGS.scratch), f"screen-mac-{os.getpid()}")
os.makedirs(WORK)
os.environ["KELPIE_DAEMON_PATH"] = KELPIED
os.environ["KELPIE_E2E"] = "1"
os.environ.pop("KELPIE_ALLOW_REAL_DAEMON", None)

PORT = 48900 + os.getpid() % 400
PHONE_NAME = "Chrome iPhone"
MEDIA = bool(ARGS.shots or ARGS.video)
# For the README the document has a demo name and plain words; otherwise a name of this run's. Words TextEdit's spelling correction leaves
# as they are.
DOC = os.path.join(WORK, "release-notes.txt" if MEDIA else f"kelpie-mac-e2e-{os.getpid()}.txt")
TYPED = "Release notes checked from my phone" if MEDIA else "KELPIE 4711 MAC TEXT"
VIDEO_RAW = os.path.join(WORK, "video")
clip = {}
results = []
procs = {}


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    print(("PASS " if ok else "FAIL ") + name + (f"  ({str(detail)[:300]})" if detail and not ok else ""), flush=True)
    return ok


def until(fn, timeout=30, step=0.2):
    end = time.time() + timeout
    while time.time() < end:
        v = fn()
        if v:
            return v
        time.sleep(step)
    return None


def sh(cmd):
    return subprocess.run(cmd, shell=True, capture_output=True, text=True).stdout.strip()


def front_app():
    return sh("lsappinfo info -only name `lsappinfo front`")


# ---------- what this script starts ----------


def app_env():
    return {k: os.environ[k] for k in ("KELPIE_ENDPOINT", "KELPIE_DATA_DIR", "KELPIE_DAEMON_PATH", "KELPIE_E2E")}


def start_app():
    """The app from this shell: macOS counts its permissions as this shell's."""
    procs["app"] = subprocess.Popen([APP_BIN], env=os.environ.copy(), stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)


def open_app():
    """The app through LaunchServices: its own app to macOS, with no permission given. Its PID is found by its path in this bundle.

    LaunchServices starts it in `/`, and this run's endpoint is a socket path relative to this script's folder: the app is given it whole,
    with the daemon's user token copied under that name's key (removed at the end)."""
    before = set(pids_of_app())
    env = app_env()
    env["KELPIE_ENDPOINT"] = os.path.abspath(env["KELPIE_ENDPOINT"])
    token = ps.kc.token_path()
    key = "".join(c if (c.isascii() and c.isalnum()) or c in "-_" else "_" for c in env["KELPIE_ENDPOINT"])
    procs["alias_token"] = os.path.join(os.path.dirname(token), key + ".token")
    shutil.copyfile(token, procs["alias_token"])
    subprocess.run(["open", "-n", "-g", APP] + [a for k, v in env.items() for a in ("--env", f"{k}={v}")], check=True)
    new = until(lambda: [p for p in pids_of_app() if p not in before], 20)
    procs["opened"] = new[0] if new else None
    return procs["opened"]


def pids_of_app():
    out = sh(f"pgrep -f '^{APP_BIN}'")
    return [int(p) for p in out.split()] if out else []


def stop_pid(pid):
    if not pid:
        return
    try:
        os.kill(pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    if not until(lambda: subprocess.run(["kill", "-0", str(pid)], capture_output=True).returncode != 0, 10):
        os.kill(pid, signal.SIGKILL)


def stop_app():
    p = procs.pop("app", None)
    if p is not None and p.poll() is None:
        p.terminate()
        try:
            p.wait(10)
        except subprocess.TimeoutExpired:
            p.kill()
    stop_pid(procs.pop("opened", None))
    alias = procs.pop("alias_token", None)
    if alias and os.path.exists(alias):
        os.remove(alias)


def open_document():
    """The script's own document in TextEdit, in front. TextEdit is quit at the end only if this script started it."""
    procs["textedit_was_running"] = bool(sh("pgrep -x TextEdit"))
    with open(DOC, "w", encoding="utf-8") as f:
        f.write("")
    subprocess.run(["open", "-a", "TextEdit", DOC], check=True)
    return until(lambda: "TextEdit" in front_app(), 15)


def close_document():
    if procs.get("textedit_was_running") is False:
        subprocess.run(["pkill", "-x", "TextEdit"])
    elif procs.get("textedit_was_running"):
        print(f"        TextEdit was running before: {os.path.basename(DOC)} is left open in it", flush=True)


# ---------- the phone ----------


class Phone:
    def __init__(self, pw):
        self.browser = pw.chromium.launch() if ARGS.chromium else pw.chromium.launch(channel="chrome")
        video = {"record_video_dir": VIDEO_RAW, "record_video_size": {"width": 390, "height": 844}} if ARGS.video else {}
        self.ctx = self.browser.new_context(viewport={"width": 390, "height": 844}, device_scale_factor=3, is_mobile=True, has_touch=True, ignore_https_errors=True,
                                           user_agent="Mozilla/5.0 (iPhone; CPU iPhone OS 26_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.0 Mobile/15E148 Safari/604.1",
                                           **video)
        self.page = self.ctx.new_page()
        # The page's video starts with the page: clip times are measured from here.
        self.started = time.time()
        self.errors = []
        self.page.on("pageerror", lambda e: self.errors.append(str(e)))
        self.cdp = self.ctx.new_cdp_session(self.page)

    def tap(self, x, y):
        self.cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": x, "y": y, "id": 0}]})
        time.sleep(0.05)
        self.cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})

    def js(self, script, arg=None):
        return self.page.evaluate(script, arg)

    def has(self, selector):
        return self.page.locator(selector).count() > 0

    def close(self):
        self.ctx.close()  # writes the page's video, if one is recorded
        self.browser.close()


def pair(phone, d):
    """The page paired at `localhost`, its passkey set up, given control on the desktop: True when it shows the inbox, unlocked."""
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


def picker_once_offered(phone, name, timeout=60):
    """The picker once the app offers the screen again (an app just opened may not have reached the daemon when the phone first asks): the
    page is opened again until its list comes."""
    page = phone.page
    end = time.time() + timeout
    while True:
        page.click("[data-see-screen]")
        got = page.wait_for_selector(f"[data-target]:has-text('{name}'), [data-lock-screen=unlock], .takeover-pick .banner.bad", timeout=30000)
        if got.get_attribute("data-lock-screen") == "unlock":
            page.click("[data-lock-go]")
            page.wait_for_selector("[data-lock-screen]", state="detached", timeout=20000)
            got = page.wait_for_selector(f"[data-target]:has-text('{name}'), .takeover-pick .banner.bad", timeout=30000)
        if got.get_attribute("data-target") is not None or time.time() > end:
            return got
        time.sleep(2)
        page.reload()
        page.wait_for_selector("[data-see-screen], [data-lock-go]", timeout=20000)
        if phone.has("[data-lock-go]"):
            page.click("[data-lock-go]")
            page.wait_for_selector("[data-see-screen]", timeout=20000)


def picker(phone, name=None):
    """From the inbox to the picker, with Face ID again if it asks; the row naming `name` (the TextEdit document's) once listed."""
    page = phone.page
    page.click("[data-see-screen]")
    # TextEdit's window is named after the file, without its extension.
    name = name or os.path.splitext(os.path.basename(DOC))[0]
    first = page.wait_for_selector(f"[data-target]:has-text('{name}'), [data-lock-screen=unlock], .takeover-pick .banner.bad", timeout=30000)
    if first.get_attribute("data-lock-screen") == "unlock":
        page.click("[data-lock-go]")
        page.wait_for_selector("[data-lock-screen]", state="detached", timeout=20000)
    try:
        return page.wait_for_selector(f"[data-target]:has-text('{name}')", timeout=30000)
    except Exception:
        print("        the picker shows: " + page.locator(".takeover-pick").inner_text().replace("\n", " | ")[:600], flush=True)
        for log in sorted(__import__("glob").glob(os.path.join(DATA, "**", "*.log"), recursive=True)):
            with open(log, errors="replace") as f:
                lines = [ln.rstrip() for ln in f if any(w in ln for w in ("WARN", "ERROR", "screen", "provider"))]
            print(f"        {os.path.basename(log)}: " + "\n          ".join(lines[-12:]), flush=True)
        raise


def frames(phone):
    return phone.js("() => window.__kelpieScreenTimes?.().presentInterval.n ?? -1")


def see(phone, row):
    page = phone.page
    row.query_selector("button").click()
    page.wait_for_selector(".screen-live[data-phase=live]", timeout=30000)
    shown = until(lambda: frames(phone) > 15, 20)
    if phone.has("[data-screen-help-close]"):
        page.click("[data-screen-help-close]")
    return shown


def type_on_phone(phone, text):
    page = phone.page
    box = page.locator("[data-screen-text]").first
    if ARGS.video and len(text) > 1:
        box.fill("")
        box.type(text, delay=55)  # typed as a person would, for the clip
    else:
        box.fill(text)
    page.click("[data-screen-type]")


# ---------- README pictures and the clip ----------


def pause(seconds=1.2):
    """A beat between steps, only while the clip is recorded, so a viewer can follow."""
    if ARGS.video:
        time.sleep(seconds)


def close_notifications():
    """The notifications on the Mac's screen closed (mac_window.swift), so the pictures and the clip show none of the Mac's own alerts."""
    helper = os.path.join(WORK, "mac_window")
    subprocess.run(["xcrun", "swiftc", "-O", os.path.join(HERE, "mac_window.swift"), "-o", helper], check=True, capture_output=True, timeout=300)
    subprocess.run([helper, "--close-notifications"], capture_output=True, timeout=30)


def take_shots(phone):
    os.makedirs(ARGS.shots, exist_ok=True)
    time.sleep(1.5)
    phone.page.screenshot(path=os.path.join(ARGS.shots, "mac-screen-from-phone.png"))
    # The Mac's whole screen while it is shared: the orange border and its badge.
    subprocess.run(["screencapture", "-x", os.path.join(ARGS.shots, "mac-sharing-border.png")], check=True, timeout=30)
    print("        pictures in " + ARGS.shots, flush=True)


def start_clip(phone):
    """The Mac's screen recorded from now (ffmpeg's AVFoundation input: `screencapture -v` does not record when started without a
    terminal); the phone page's video has run since the page opened."""
    clip["mac"] = os.path.join(WORK, "mac-screen.mkv")
    launched = time.time()
    procs["screen_video"] = subprocess.Popen(["ffmpeg", "-y", "-loglevel", "error", "-f", "avfoundation", "-capture_cursor", "1", "-framerate", "30",
                                              "-pixel_format", "uyvy422", "-i", "Capture screen 0:none", "-t", "60", "-c:v", "libx264", "-preset", "ultrafast",
                                              "-crf", "18", clip["mac"]], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    time.sleep(2.5)  # the capture takes a moment to start
    clip["start"] = time.time() - phone.started
    # Where this moment is in the Mac's recording: its first frame comes about a second after ffmpeg starts.
    clip["mac_start"] = max(0.0, time.time() - launched - 1.0)


def stop_clip(phone):
    clip["end"] = time.time() - phone.started
    p = procs.pop("screen_video")
    p.send_signal(signal.SIGINT)
    try:
        p.wait(20)
    except subprocess.TimeoutExpired:
        p.kill()


def make_clip(phone_video):
    """The phone page (390×844) beside the Mac's screen scaled to the same height: H.264, no sound, small enough for a README."""
    os.makedirs(ARGS.video, exist_ok=True)
    out = os.path.join(ARGS.video, "mac-phone-control.mp4")
    length = clip["end"] - clip["start"]
    graph = ("[0:v]trim=start={s:.2f}:duration={n:.2f},setpts=PTS-STARTPTS,fps=30,scale=390:844[p];"
             "[1:v]trim=start={m:.2f}:duration={n:.2f},setpts=PTS-STARTPTS,fps=30,scale=-2:844[m];"
             "[p][m]hstack=inputs=2,pad=ceil(iw/2)*2:ceil(ih/2)*2[v]").format(s=clip["start"], m=clip["mac_start"], n=length)
    for crf in (26, 30, 34):
        subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-i", phone_video, "-i", clip["mac"], "-filter_complex", graph, "-map", "[v]",
                        "-an", "-c:v", "libx264", "-preset", "slow", "-crf", str(crf), "-pix_fmt", "yuv420p", "-movflags", "+faststart", out], check=True)
        if os.path.getsize(out) <= 3 * 1024 * 1024:
            break
    # The poster: the last moment, the text saved.
    subprocess.run(["ffmpeg", "-y", "-loglevel", "error", "-sseof", "-1", "-i", out, "-frames:v", "1", os.path.join(ARGS.video, "mac-phone-control-poster.png")], check=True)
    print(f"        clip: {out} ({os.path.getsize(out) // 1024} KB, {length:.1f} s)", flush=True)


# ---------- the run ----------


def scenario(d, pw):
    cert, key = ps.certificate_for_loopback(WORK)
    d.call("mobile.enable", {"ip": "127.0.0.1", "port": PORT, "cert": cert, "key": key})
    if MEDIA:
        close_notifications()
    start_app()
    check("the TextEdit document is open, in front", open_document())
    phone = Phone(pw)
    try:
        check("the page pairs, sets up its passkey and is given control", pair(phone, d))
        page = phone.page
        row = picker(phone)
        check("1. the app reached this daemon and the picker lists the TextEdit window", row is not None)
        hint = page.locator(".takeover-pick p.muted").first.inner_text()
        check("2. the picker speaks of the Mac (its lock screen and login window)", "login window" in hint, hint)
        check("3. seeing the window brings a live picture", see(phone, row), frames(phone))
        mods = page.locator(".keybar [data-mod]").all_inner_texts()
        check("…the key bar's modifiers are ⌘ ⌥ ⌃ ⇧", mods == ["⌘", "⌥", "⌃", "⇧"], mods)
        keys = phone.js("() => [...document.querySelectorAll('.keybar [data-key]')].map((b) => b.dataset.key)")
        check("…with ⌘Space for Spotlight and no menu key", "Spotlight" in keys and "ContextMenu" not in keys and "Meta" not in keys, keys)
        if ARGS.shots:
            take_shots(phone)

        # 4. A tap on the window, with Finder in front, brings TextEdit to the front.
        subprocess.run(["open", "-a", "Finder"])
        until(lambda: "Finder" in front_app(), 5)
        before = front_app()
        if ARGS.video:
            start_clip(phone)
        page.click("[data-screen-mode]")
        check("…the mode switches to direct touch", "Direct" in page.locator("[data-screen-mode]").inner_text())
        pause()
        box = page.locator(".screen-canvas").bounding_box()
        phone.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
        after = until(lambda: "TextEdit" in front_app() and front_app(), 5) or front_app()
        check("4. a tap on the picture clicks the window: TextEdit comes to the front", "TextEdit" in after and "TextEdit" not in before, f"{before} -> {after}")
        pause()

        # 5. Text, then ⌘ then S.
        type_on_phone(phone, TYPED)
        check("5. the phone says the text was typed", bool(until(lambda: phone.has("[data-text-result=sent]"), 10)), page.locator("[data-text-result]").all_inner_texts())
        pause()
        page.click(".keybar [data-mod=meta]")
        pause(0.8)
        type_on_phone(phone, "s")

        def saved():
            with open(DOC, encoding="utf-8") as f:
                return TYPED in f.read()

        check("…and ⌘ then S saves it: the file holds the text", bool(until(saved, 10)), open(DOC, encoding="utf-8").read()[:80])
        if ARGS.video:
            pause(2.5)
            stop_clip(phone)
        stats = phone.js("() => window.__kelpieScreenTimes?.()")
        print("        phone: " + json.dumps({"frames_drawn": (stats or {}).get("presentInterval", {}).get("n")}))
        print("        quality: " + page.locator(".screen-quality").inner_text().strip())

        # 6. Stop, then the hotkey.
        page.click("[data-screen-stop]")
        check("6. Stop ends the screen", bool(page.wait_for_selector("[data-screen-ended]", timeout=5000)))
        page.click("[data-screen-ended] .btn")
        row = picker(phone)
        check("…it is seen again", see(phone, row))
        for m in ("ctrl", "alt", "shift"):
            page.click(f".keybar [data-mod={m}]")
        type_on_phone(phone, "k")
        ended = page.wait_for_selector("[data-screen-ended]", timeout=8000)
        text = ended.inner_text() if ended else ""
        check("…and Control+Option+Shift+K from the key bar ends it as the stop hotkey", "hotkey" in text, text)
        audit = json.dumps(d.call("mobile.audit", {"limit": 50})["entries"])
        check("…and the audit names the program shared, not the window's title", "screen_start" in audit and os.path.splitext(os.path.basename(DOC))[0] not in audit, audit[:300])
        page.click("[data-screen-ended] .btn")

        # 7. Without Screen Recording.
        if not ARGS.no_refusal:
            stop_app()
            pid = open_app()
            check("the app opens through LaunchServices", pid is not None)
            until(lambda: phone.has("[data-see-screen]"), 10)
            # Without Screen Recording macOS lists no window to it: the display is what is offered.
            row = picker_once_offered(phone, "main screen")
            if check("…and offers the screen to the phone", row.get_attribute("data-target") is not None, row.inner_text()):
                row.query_selector("button").click()
                banner = page.wait_for_selector(".takeover-pick .banner.bad", timeout=30000)
                said = banner.inner_text() if banner else ""
                check("7. without Screen Recording the start is refused, and the phone says which permission", "Screen Recording" in said and "System Settings" in said, said)
                check("…and no picture starts", page.locator(".screen-live").count() == 0)
    finally:
        check("no error on the page", not phone.errors, phone.errors[:3])
        video = phone.page.video.path() if ARGS.video and phone.page.video else None
        phone.close()
        if video and "end" in clip:
            make_clip(video)


def main():
    from playwright.sync_api import sync_playwright

    ts_bin, ts_state, _ = ps.write_fake_tailscale(WORK)
    ps.fake_tailscale_says(ts_state, status=ps.FAKE_TAILSCALE_READY)
    d = ps.Daemon(KELPIED, env={"KELPIE_LOAD_FORCE": "L0:calm", "KELPIE_E2E": "1", "KELPIE_TAILSCALE_BIN": ts_bin})
    try:
        d.start()
        with sync_playwright() as pw:
            scenario(d, pw)
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False)
    finally:
        recording = procs.pop("screen_video", None)
        if recording is not None and recording.poll() is None:
            recording.send_signal(signal.SIGINT)
            recording.wait(20)
        stop_app()
        close_document()
        assert os.environ["KELPIE_ENDPOINT"].startswith("kelpie-screen-mac-")
        if d.proc is not None:
            d.stop()
        shutil.rmtree(WORK, ignore_errors=True)
        shutil.rmtree(DATA, ignore_errors=True)
    failed = [r for r in results if not r[1]]
    summary = {"passed": len(results) - len(failed), "failed": len(failed), "checks": [{"name": n, "ok": ok} for n, ok in results]}
    if ARGS.json:
        with open(ARGS.json, "w", encoding="utf-8") as f:
            json.dump(summary, f, indent=1, ensure_ascii=False)
    print(json.dumps({k: summary[k] for k in ("passed", "failed")}))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
