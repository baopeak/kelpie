"""How smoothly the phone shows the cursor a finger moves on the computer's screen, measured in a browser (no computer, link or daemon).

    python -X utf8 scripts/perf/phone_cursor.py [--out file.json] [--rtt 400] [--throttle 1,4] [--strokes 3] [--port 1447]

Serves scripts/perf/phone-cursor (the real app/phone/src/screen-view.tsx and screen.ts, with the phone's state stubbed) with the app's Vite on
127.0.0.1:<port>, opens it in Playwright's Chromium as an emulated iPhone (390×844 at 3×, touch) with a non-desynchronized canvas as Safari
has, and lets the page's fake computer send 1080p H.264 at 30 frames a second and its cursor a round trip (--rtt) behind. Then, for each CPU
slowdown in --throttle (Chromium's own emulation; 4 is closer to a phone than a desktop is), --strokes trackpad strokes of 1.5 s with real touch
events at 60 a second, each followed by 2 s with no finger.

Per stroke it reports: the moves handled, each move's wait before handling and its handling time (`handlerMs`: what a move costs the page,
the number that tells a cursor drawn into the canvas from one on its own layer), the move to the next animation frame (the earliest the cursor
can be seen there; both ways show it at the next composited frame, so this moves with the main thread's load, not with the way), the frame
rate and long frames (> 25 ms), how often the cursor's place changed a second (read from what the page set: the transform it drew the cursor
with, or the cursor layer's transform, not from pixels), the video pictures drawn a second and their drawImage time; and after the finger
stops, how far the cursor went from where the finger left it (a snap back to an older place) and for how many frames. Only the canvas's
`desynchronized` flag is made Safari's; the compositor is Chromium's. The fake computer moves its cursor to the places the page sends and
answers in order, with no loss, reordering or desk mouse. Needs Playwright for Python with Chromium and the app's node_modules. Stops the
Vite it started (by PID).
"""
import argparse
import json
import os
import socket
import subprocess
import sys
import time

from playwright.sync_api import sync_playwright

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
APP = os.path.join(ROOT, "app")
HARNESS = os.path.join(ROOT, "scripts", "perf", "phone-cursor")

ap = argparse.ArgumentParser()
ap.add_argument("--out", default=None)
ap.add_argument("--rtt", type=int, default=400)
ap.add_argument("--throttle", default="1,4")
ap.add_argument("--strokes", type=int, default=3)
ap.add_argument("--port", type=int, default=1447)
ARGS = ap.parse_args()


def port_open(port: int) -> bool:
    with socket.socket() as s:
        s.settimeout(0.3)
        return s.connect_ex(("127.0.0.1", port)) == 0


def main() -> int:
    if port_open(ARGS.port):
        print(f"port {ARGS.port} is in use: pass another --port", file=sys.stderr)
        return 2
    env = dict(os.environ, PHONE_CURSOR_PORT=str(ARGS.port))
    vite = subprocess.Popen(["node", os.path.join(APP, "node_modules", "vite", "bin", "vite.js"), "--config", os.path.join(HARNESS, "vite.config.mjs")],
                            cwd=HARNESS, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        deadline = time.time() + 30
        while not port_open(ARGS.port):
            if vite.poll() is not None or time.time() > deadline:
                print("vite did not start:", vite.stderr.read().decode(errors="replace") if vite.poll() is not None else "timeout", file=sys.stderr)
                return 1
            time.sleep(0.2)
        results = run()
    finally:
        if os.name == "nt":
            subprocess.run(["taskkill", "/PID", str(vite.pid), "/T", "/F"], capture_output=True)
        else:
            vite.terminate()
        vite.wait(10)
    text = json.dumps(results, indent=1)
    if ARGS.out:
        with open(ARGS.out, "w", encoding="utf-8") as f:
            f.write(text)
    print(text)
    return 0


def stroke(cdp, page, cx: float, cy: float) -> dict:
    """One trackpad stroke: down, 1.5 s of moves at 60 a second (a loop with turns), up; then 2 s with no finger."""
    cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": cx, "y": cy, "id": 1}]})
    start = time.perf_counter()
    from_ms = page.evaluate("() => window.__harness.now()")
    n = 90
    for i in range(1, n + 1):
        target = start + i / 60
        wait = target - time.perf_counter()
        if wait > 0:
            time.sleep(wait)
        # Out and back along a zigzag: the cursor turns as a finger's does.
        x = cx + 110 * (i / n if i <= n // 2 else (n - i) / n) * 2
        y = cy + 60 * ((i % 30) / 30 if (i // 30) % 2 == 0 else 1 - (i % 30) / 30)
        cdp.send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": [{"x": x, "y": y, "id": 1}]})
    cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
    to_ms = page.evaluate("() => window.__harness.now()")
    time.sleep(2.0)
    settle = page.evaluate("() => window.__harness.now()")
    return page.evaluate("([a, b, c]) => window.__harness.stats(a, b, c)", [from_ms, to_ms, settle])


def run() -> dict:
    out: dict = {"rtt_ms": ARGS.rtt, "runs": {}}
    with sync_playwright() as p:
        browser = p.chromium.launch(args=["--enable-features=WebCodecs"])
        try:
            ctx = browser.new_context(viewport={"width": 390, "height": 844}, device_scale_factor=3, is_mobile=True, has_touch=True)
            page = ctx.new_page()
            errors: list[str] = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            page.on("console", lambda m: errors.append(f"{m.type}: {m.text}") if m.type in ("error", "warning") else None)
            page.goto(f"http://127.0.0.1:{ARGS.port}/?rtt={ARGS.rtt}")
            try:
                page.wait_for_function("() => window.__harness && window.__harness.ready()", timeout=30000)
            except Exception:
                print("the page did not get ready:", errors[:10], file=sys.stderr)
                raise
            # A first screen opens with the gestures' sheet over the picture: closed, as a person would.
            if page.locator("[data-screen-help-close]").count():
                page.click("[data-screen-help-close]")
            box = page.locator(".screen-canvas").bounding_box()
            cx, cy = box["x"] + box["width"] / 2 - 110, box["y"] + box["height"] / 2 - 30
            cdp = ctx.new_cdp_session(page)
            for rate in [float(r) for r in ARGS.throttle.split(",")]:
                cdp.send("Emulation.setCPUThrottlingRate", {"rate": rate})
                time.sleep(1.0)
                out["runs"][f"cpu_x{rate:g}"] = [stroke(cdp, page, cx, cy) for _ in range(ARGS.strokes)]
            cdp.send("Emulation.setCPUThrottlingRate", {"rate": 1})
            out["errors"] = errors[:10]
        finally:
            browser.close()
    return out


if __name__ == "__main__":
    sys.exit(main())
