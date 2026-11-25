#!/usr/bin/env python3
"""Smoke test of the screen helper on a Mac, driven the way the app drives it (`app/src-tauri/src/screen.rs`).

It runs the built `kelpie-screen` binary, speaks its stdin JSON lines and reads its stdout frames (`crates/kelpie-screen/src/wire.rs`):

1. `hello` with the protocol, then `targets` with the displays and windows.
2. A display stream for a while, acknowledged as the daemon would: it starts, its first frame is a keyframe, the H.264 decodes (ffmpeg),
   still regions are painted with tiles and the cursor is sent; then the stop hotkey (Ctrl+Option+Shift+K, posted through the helper's own
   input) ends it with `hotkey`.
3. A TextEdit window stream: another app is put in front, a click through the helper brings TextEdit back, typed text and Cmd+S reach it
   (the saved file holds the text), and `stop` ends the stream.
4. Closing stdin ends the helper.

Needs Screen Recording and Accessibility for the process that runs it, ffmpeg/ffprobe on PATH and nothing else at the Mac's keyboard. Every
check prints ok or FAILED; the exit code is the number of failures.

    python3 scripts/e2e/screen_helper_mac.py --helper "$CARGO_TARGET_DIR/debug/kelpie-screen" [--seconds 10] [--out /tmp/kelpie-screen-smoke]
"""

import argparse
import json
import os
import queue
import struct
import subprocess
import sys
import threading
import time

PROTO = 1
OUT_JSON, OUT_MEDIA = 1, 2
KIND_VIDEO, KIND_TILE, KIND_CURSOR = 1, 2, 3
VIDEO_HEADER = 20

failures = []


def check(ok, what, detail=""):
    print(("ok      " if ok else "FAILED  ") + what + (f"  ({detail})" if detail else ""), flush=True)
    if not ok:
        failures.append(what)
    return ok


def now_ms():
    return int(time.time() * 1000)


class Helper:
    def __init__(self, path):
        self.p = subprocess.Popen([path], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=sys.stderr)
        self.events = queue.Queue()
        self.media = {}
        self.lock = threading.Lock()
        self.next_id = 100
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        f = self.p.stdout
        while True:
            head = f.read(5)
            if len(head) < 5:
                self.events.put({"event": "_eof"})
                return
            (n,) = struct.unpack("<I", head[:4])
            body = f.read(n - 1)
            if head[4] == OUT_JSON:
                self.events.put(json.loads(body))
            elif head[4] == OUT_MEDIA:
                sc_len = body[0]
                sc, m = body[1:1 + sc_len].decode(), body[1 + sc_len:]
                with self.lock:
                    self.media.setdefault(sc, []).append(m)

    def send(self, obj):
        self.p.stdin.write((json.dumps(obj) + "\n").encode())
        self.p.stdin.flush()

    def wait(self, pred, timeout):
        """The first JSON message `pred` takes within `timeout` s (others are kept in `seen`)."""
        end = time.time() + timeout
        while time.time() < end:
            try:
                m = self.events.get(timeout=max(0.01, end - time.time()))
            except queue.Empty:
                break
            self.seen.append(m)
            if pred(m):
                return m
        return None

    seen = []

    def ask(self, obj, timeout=10):
        self.next_id += 1
        obj = dict(obj, id=self.next_id)
        self.send(obj)
        return self.wait(lambda m: m.get("id") == obj["id"], timeout)

    def videos(self, sc):
        with self.lock:
            return [m for m in self.media.get(sc, []) if m[1] == KIND_VIDEO]

    def kinds(self, sc, kind):
        with self.lock:
            return [m for m in self.media.get(sc, []) if m[1] == kind]


def acknowledge(h, sc, seconds, during=None):
    """Plays the daemon for `seconds`: acknowledges every frame (so the window never fills), calling `during(t)` once a second."""
    end, tick = time.time() + seconds, 0
    while time.time() < end:
        vids = h.videos(sc)
        acked = struct.unpack("<I", vids[-1][4:8])[0] if vids else 0
        h.send({"cmd": "flow", "sc": sc, "acked": acked, "rtt_ms": 30, "level": 0, "lost_tiles": False, "dropped_through": 0, "raw": 0, "wire": 0, "link_full_ms": 0, "win": False})
        if during and time.time() >= end - seconds + tick:
            during(tick)
            tick += 1
        time.sleep(0.1)


def sh(cmd):
    return subprocess.run(cmd, shell=True, capture_output=True, text=True)


def front_app():
    return sh("lsappinfo info -only name `lsappinfo front`").stdout


def display_stream(h, display, seconds, out):
    sc = "smoke-display"
    r = h.ask({"cmd": "start", "sc": sc, "target": display["id"], "device": "smoke", "hotkey": "Ctrl+Alt+Shift+K", "level": 0, "tiles": True}, 15)
    if not check(r is not None and "ok" in r, "a display stream starts", json.dumps(r)[:300]):
        return
    started = r["ok"]
    print(f"        started: {started['w']}x{started['h']} encoder {started['encoder']} {started['encoder_name']} refused {started['refused']}")
    src = (started["w"], started["h"])

    def wiggle(t):
        # The pointer moves (the cursor message follows it), and a corner of the screen changes now and then.
        h.send({"cmd": "input", "sc": sc, "ops": [{"k": "move", "x": src[0] * (0.3 + 0.05 * (t % 4)), "y": src[1] * 0.5}], "t": now_ms()})

    acknowledge(h, sc, seconds, wiggle)
    vids = h.videos(sc)
    check(len(vids) > 0, "video frames come", f"{len(vids)} frames")
    if not vids:
        for m in h.seen:
            if m.get("sc") == sc:
                print("        " + json.dumps(m)[:400])
        return
    check(vids[0][2] & 1 == 1, "the first frame is a keyframe")
    path = os.path.join(out, "display.h264")
    with open(path, "wb") as f:
        for m in vids:
            f.write(m[VIDEO_HEADER + m[3] * 8:])
    dec = sh(f"ffmpeg -v error -i {path} -f null - 2>&1")
    probe = sh(f"ffprobe -v error -count_frames -select_streams v:0 -show_entries stream=nb_read_frames,width,height,profile -of csv=p=0 {path}")
    check(dec.returncode == 0 and not dec.stdout.strip(), "the H.264 decodes without an error", (dec.stdout.strip() or probe.stdout.strip())[:200])
    coded = struct.unpack("<HH", vids[0][16:20])
    check(probe.stdout.strip().count(",") >= 2 and str(coded[0]) in probe.stdout, "ffprobe reads every frame at the coded size", f"{probe.stdout.strip()} for {len(vids)} sent, coded {coded}")
    tiles = h.kinds(sc, KIND_TILE)
    check(len(tiles) > 0, "still regions are painted with tiles", f"{len(tiles)} tiles")
    check(len(h.kinds(sc, KIND_CURSOR)) > 0, "the cursor is sent", f"{len(h.kinds(sc, KIND_CURSOR))} cursor messages")
    stats = [m for m in h.seen if m.get("event") == "stats" and m.get("sc") == sc]
    if stats:
        s = stats[-1]
        print("        last stats: " + json.dumps({k: s.get(k) for k in ("fps", "kbps", "w", "h", "encode_ms", "level", "software")}))
    # The stop hotkey, pressed through the helper's own input (a display target takes any key).
    h.send({"cmd": "input", "sc": sc, "ops": [{"k": "key", "key": "K", "mods": ["ctrl", "alt", "shift"]}], "t": now_ms()})
    ended = h.wait(lambda m: m.get("event") == "ended" and m.get("sc") == sc, 5)
    check(ended is not None and ended.get("reason") == "hotkey", "the stop hotkey ends the stream", json.dumps(ended))


def window_stream(h, out):
    path = os.path.join(out, "kelpie-smoke.txt")
    with open(path, "w") as f:
        f.write("")
    sh("pkill -x TextEdit; sleep 1")
    sh(f"open -a TextEdit {path}")
    target = None
    for _ in range(20):
        time.sleep(0.5)
        r = h.ask({"cmd": "targets"})
        ts = (r or {}).get("ok", {}).get("targets", [])
        target = next((t for t in ts if t["kind"] == "window" and "kelpie-smoke" in t["title"]), None)
        if target:
            break
    if not check(target is not None, "the TextEdit window is offered", json.dumps(target)):
        return
    sc = "smoke-window"
    r = h.ask({"cmd": "start", "sc": sc, "target": target["id"], "device": "smoke", "hotkey": "Ctrl+Alt+Shift+K", "level": 0, "tiles": True}, 15)
    if not check(r is not None and "ok" in r, "a window stream starts", json.dumps(r)[:300]):
        return
    w, hgt = r["ok"]["w"], r["ok"]["h"]
    acknowledge(h, sc, 1.5)
    # Another app in front; a click through the helper on the shared window brings it back.
    sh("open -a Finder")
    time.sleep(1.5)
    before = front_app()
    h.send({"cmd": "input", "sc": sc, "ops": [{"k": "click", "x": w / 2, "y": hgt / 2, "b": "left", "n": 1}], "t": now_ms()})
    acknowledge(h, sc, 1.5)
    after = front_app()
    check("TextEdit" in after and "TextEdit" not in before, "a click reaches the shared window", f"front {before.strip()} -> {after.strip()}")
    typed = "hello from kelpie é😀"
    r = h.ask({"cmd": "text", "sc": sc, "text": typed, "t": now_ms()}, 10)
    check(r is not None and r.get("ok") is True, "typed text is taken", json.dumps(r))
    h.send({"cmd": "input", "sc": sc, "ops": [{"k": "key", "key": "S", "mods": ["meta"]}], "t": now_ms()})
    acknowledge(h, sc, 2)
    with open(path, encoding="utf-8") as f:
        saved = f.read()
    check(typed in saved, "the text and Cmd+S reach TextEdit (the saved file holds the text)", repr(saved[:80]))
    refused = [m for m in h.seen if m.get("sc") == sc and m.get("event") in ("input_refused", "input_blocked")]
    check(not refused, "no input was refused", json.dumps(refused)[:200])
    check(len(h.videos(sc)) > 0, "the window's frames come", f"{len(h.videos(sc))} frames")
    h.send({"cmd": "stop", "sc": sc, "reason": "stop"})
    ended = h.wait(lambda m: m.get("event") == "ended" and m.get("sc") == sc, 5)
    check(ended is not None and ended.get("reason") == "stop", "stop ends the stream", json.dumps(ended))
    sh("pkill -x TextEdit")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--helper", default=os.path.join(os.environ.get("CARGO_TARGET_DIR", "target"), "debug", "kelpie-screen"))
    ap.add_argument("--seconds", type=float, default=10)
    ap.add_argument("--out", default="/tmp/kelpie-screen-smoke")
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    h = Helper(a.helper)
    hello = h.wait(lambda m: m.get("event") == "hello", 5)
    check(hello is not None and hello.get("proto") == PROTO and hello.get("available") is True, "hello with the protocol", json.dumps(hello))
    r = h.ask({"cmd": "targets"})
    ts = (r or {}).get("ok", {}).get("targets", [])
    display = next((t for t in ts if t["kind"] == "monitor"), None)
    check(display is not None and display["id"].startswith("m:display:"), "targets lists the display first", ", ".join(f"{t['id']} {t['title']!r}" for t in ts[:6]))
    if display:
        display_stream(h, display, a.seconds, a.out)
    window_stream(h, a.out)
    h.p.stdin.close()
    try:
        h.p.wait(5)
        check(True, "closing stdin ends the helper")
    except subprocess.TimeoutExpired:
        h.p.kill()
        check(False, "closing stdin ends the helper")
    print(f"{len(failures)} failed" if failures else "all ok")
    sys.exit(len(failures))


if __name__ == "__main__":
    main()
