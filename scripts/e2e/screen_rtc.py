"""The computer's screen peer to peer (WebRTC data channels), against the real app, its screen helper and a daemon of this script's own, with the
phone's own peer code (app/phone/src/screen-rtc.ts) running in real browsers (Windows).

    python -X utf8 scripts/e2e/screen_rtc.py <scratch-dir> <app> [--browsers chromium,firefox] [--seconds 20]

<app> is a debug build of the app with the debug kelpied and a kelpie-screen next to it, as for screen_net.py (same tools, same isolation: this
process's own endpoint and data folder, a fake Tailscale CLI, its own tauri-driver; the user's daemon and data are never reached). Also needs
Playwright for Python with its Chromium and Firefox (`python -m playwright install chromium firefox`) and the app's node_modules (TypeScript
turns screen-rtc.ts into the page's module).

The phone is split in two, as the real page is: an emulated phone holds the encrypted link (the screen's start, its JSON, the offer and its
answer), and a browser page runs the real ScreenPeer: it offers through the link, says the hello, takes the picture on "m" and "c" and sends
its acknowledgements (8 ms after each frame, standing in for decoding) and input on "i". STUN is off (`KELPIE_RTC_STUN=off`): this computer's
own addresses are the candidates. The impairment comes from a file read for each new peer (`KELPIE_RTC_TEST_IMPAIR_FILE`).

1. A screen of the primary monitor starts on the link (Automatic over a direct link: Tailscale).
2. Chromium offers: the computer moves the picture peer to peer (`sc_path p2p`), the link carries no more video, frames keep coming (the
   acknowledgements on "i" reach the helper), the browser's selected pair is P2P, and a click sent on "i" lands in this script's window.
3. Back to the link by choice (`sc_via tailscale`): the picture is back on the link within 3 s.
4. An offer whose hello carries another nonce is closed (`bad_hello`), and the picture stays on the link.
5. Peer to peer again (`sc_via auto`, a new offer): the picture is back within 3 s, from a keyframe.
6. 1 % loss and 50 ms each way on the computer's UDP (the impairment hook): a new peer, and the screen keeps going for the run.
7. The rules hold peer to peer: turning the peer to peer way off on the computer ends such a screen within a second; the phone locking ends
   one within a second, after which nothing more comes on the peer (and the phone unlocks again).
8. Firefox offers in turn: the computer moves to its peer, frames come, a click lands.
9. Stop ends it; the audit log has the starts.

Writes <scratch-dir>/screen-rtc.json with each step's numbers. Nothing typed is sent; nothing captured is kept beyond the frames' headers.

With --matrix: the screen on the link for --seconds (no impairment: the baseline here), then peer to peer under each condition of MATRIX,
a new peer each (the impairment is read when a peer is made), measured the same way (`screen_support.frame_stats`): the picture's rate, data
rate, age p50/p95, longest gap and freezes over a second. Writes <scratch-dir>/screen-rtc-matrix.json and .md.
"""
import argparse
import asyncio
import ctypes
import json
import os
import subprocess
import sys
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import screen_support as ss  # noqa: E402
from screen_support import check, now_ms, until  # noqa: E402

SETUP = ss.prepare("screen-rtc", __doc__)

ap = argparse.ArgumentParser()
ap.add_argument("scratch")
ap.add_argument("app")
ap.add_argument("--browsers", default="chromium,firefox")
ap.add_argument("--seconds", type=int, default=20)
ap.add_argument("--matrix", action="store_true", help="the impairment matrix (1e) instead of the checks: each condition its own peer, --seconds long")
ARGS = ap.parse_args()

ctypes.windll.shcore.SetProcessDpiAwareness(2)
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
IMPAIR = os.path.join(SETUP.work, "impair.txt")
os.environ["KELPIE_RTC_STUN"] = "off"
os.environ["KELPIE_RTC_TEST_IMPAIR_FILE"] = IMPAIR
MOTION = ss.Motion(SETUP)
ACK_DELAY = 0.008
NUMBERS = {}


def set_impair(spec):
    with open(IMPAIR, "w", encoding="utf-8") as f:
        f.write(spec)


def peer_module():
    """screen-rtc.ts as a browser module: its types taken out by the app's own TypeScript."""
    src = os.path.join(ROOT, "app", "phone", "src", "screen-rtc.ts")
    js = "const ts=require('typescript');const fs=require('fs');process.stdout.write(ts.transpileModule(fs.readFileSync(process.argv[1],'utf8'),{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ES2022}}).outputText)"
    out = subprocess.run(["node", "-e", js, src], cwd=os.path.join(ROOT, "app"), capture_output=True, text=True, check=True)
    return out.stdout


# The page: the real ScreenPeer, and what a phone does with what it gets (counted, acknowledged on "i" 8 ms after a frame).
DRIVER = r"""
window.startPeer = (sc) => {
  if (window.peer) window.peer.close();
  window.frames = []; window.cursors = 0; window.state = "offering"; window.fails = []; window.trace = [];
  const t0 = Date.now();
  const note = (...x) => window.trace.push([Date.now() - t0, ...x]);
  // The browser's own view of the peer, for a failure's report: its states and the candidates it gathered.
  class Traced extends RTCPeerConnection {
    constructor(c) {
      super(c);
      this.addEventListener("icegatheringstatechange", () => note("gathering", this.iceGatheringState));
      this.addEventListener("iceconnectionstatechange", () => note("ice", this.iceConnectionState));
      this.addEventListener("connectionstatechange", () => note("connection", this.connectionState));
      this.addEventListener("icecandidate", (e) => note("candidate", e.candidate ? e.candidate.candidate.split(" ").slice(4, 8).join(" ") : "end"));
    }
  }
  const env = { RTCPeerConnection: Traced, setTimeout: (f, ms) => setTimeout(f, ms), clearTimeout: (t) => clearTimeout(t) };
  const port = {
    offer: (s, sdp) => { note("offer", (sdp.match(/a=candidate:/g) || []).length); return window.kelpieOffer(s, sdp); },
    ice: (s, c) => { note("trickle", c.split(" ").slice(4, 8).join(" ")); return window.kelpieIce(s, c); },
  };
  const peer = new KelpieRtc.ScreenPeer(env, port, sc, {
    media: (b) => {
      if (b[1] === 1) {
        const v = new DataView(b.buffer, b.byteOffset, b.byteLength);
        const seq = v.getUint32(4, true);
        window.frames.push([seq, v.getUint32(8, true), Date.now(), b.length, b[2] & 1]);
        setTimeout(() => peer.send({ type: "sc_ack", seq }), ACK_MS);
      } else if (b[1] === 3) window.cursors++;
    },
    open: () => { window.state = "open"; },
    failed: (w) => { window.state = "failed"; window.fails.push(w); },
  });
  window.peer = peer;
  peer.start();
};
window.clickAt = (x, y) => window.peer.send({ type: "sc_input", ops: [{ k: "click", x, y, b: "left" }], st: Date.now() });
window.pathNow = async () => (window.peer ? await window.peer.stats() : null);
"""


# The 1e conditions on the computer's UDP, both ways: one-way delay, loss, a rate.
MATRIX = [
    {"n": 1, "name": "25 ms one way", "impair": "delay_ms:25"},
    {"n": 2, "name": "50 ms one way (100 ms RTT)", "impair": "delay_ms:50"},
    {"n": 3, "name": "100 ms one way", "impair": "delay_ms:100"},
    {"n": 4, "name": "100 ms RTT, 1 % loss", "impair": "delay_ms:50,loss:0.01"},
    {"n": 5, "name": "100 ms RTT, 5 % loss", "impair": "delay_ms:50,loss:0.05"},
    {"n": 6, "name": "100 ms RTT, 300 kbps", "impair": "delay_ms:50,kbps:300"},
]


def daemon_mb():
    """This script's daemon's memory (working set, MB): the kelpied next to the app under test. Another e2e running from the same build folder
    at the same time would be taken instead (the scripts here run one at a time)."""
    try:
        import psutil
    except ImportError:
        return None
    want = os.path.normcase(os.path.abspath(SETUP.kelpied))
    for proc in psutil.process_iter(["exe", "memory_info"]):
        if proc.info["exe"] and os.path.normcase(proc.info["exe"]) == want and proc.info["memory_info"]:
            return round(proc.info["memory_info"].rss / (1024 * 1024), 1)
    return None


async def matrix(p, page, sc, t0, mark, monitor):
    """The baseline on the link, then each condition peer to peer. A condition the screen does not survive is a finding: the row says so, a
    new screen starts, and the next condition runs on it."""
    rows = []
    await asyncio.sleep(3)
    began = now_ms()
    await asyncio.sleep(ARGS.seconds)
    link = [m for m in p.log[mark:] if m["type"] == "sc_media" and m["kind"] == "video" and m["_w"] >= began]
    rows.append({"n": 0, "name": "link, no impairment (this machine)", **ss.frame_stats([(m["seq"], m["capture_ms"], m["_w"], m["_size"]) for m in link], t0, (now_ms() - began) / 1000), "kelpied_mb": daemon_mb()})
    for c in MATRIX:
        set_impair(c["impair"])
        at = p.mark()
        began = time.monotonic()
        await page.evaluate(f"window.startPeer({json.dumps(sc)})")
        moved = await wait_path(p, at, lambda m: m.get("path") == "p2p" and m.get("state") == "live", 30)
        if moved is None:
            rows.append({"n": c["n"], "name": c["name"], "error": "no peer to peer", "paths": [m for m in p.log[at:] if m["type"] == "sc_path"][-2:]})
            continue
        opened = round((time.monotonic() - began) * 1000)
        await asyncio.sleep(3)
        r = await run_for(page, ARGS.seconds, t0)
        ended = [m for m in p.log[at:] if m["type"] == "sc_end"]
        rows.append({"n": c["n"], "name": c["name"], "open_ms": opened, **r, "survived": not ended, "kelpied_mb": daemon_mb()})
        print(json.dumps(rows[-1]), flush=True)
        if ended:
            sc, t0, _ = await start(p, monitor)
    set_impair("")
    return rows


def matrix_table(rows):
    cols = ["fps", "kbps", "age_p50", "age_p95", "longest_gap_ms", "freezes_over_1s", "open_ms", "survived", "kelpied_mb"]
    lines = ["| # | condition | " + " | ".join(cols) + " |", "|" + "---|" * (len(cols) + 2)]
    for r in rows:
        if "error" in r:
            lines.append(f"| {r['n']} | {r['name']} | {r['error']} |")
            continue
        lines.append(f"| {r['n']} | {r['name']} | " + " | ".join("–" if r.get(k) is None else str(r[k]) for k in cols) + " |")
    return "\n".join(lines)


def to_source(x, y, started):
    """A point on the primary monitor (physical pixels) in the picture's source pixels."""
    mw, mh = ctypes.windll.user32.GetSystemMetrics(0), ctypes.windll.user32.GetSystemMetrics(1)
    sw, sh = started.get("w") or mw, started.get("h") or mh
    return round(x * sw / max(mw, 1)), round(y * sh / max(mh, 1))




class Bridge:
    """The page's offers and candidates, carried on the emulated phone's link; `tamper` gives the page another nonce than the computer's."""

    def __init__(self, phone):
        self.p = phone
        self.tamper = False

    async def offer(self, sc, sdp):
        r = await self.p.ask(type="sc_rtc_offer", sc=sc, sdp=sdp, timeout=15) or {}
        if r.get("type") != "ack":
            return {"t": "error", "code": r.get("code", "offline"), "message": r.get("message", "")}
        return {"t": "ack", "sdp": r["sdp"], "nonce": "not-the-nonce" if self.tamper else r["nonce"], "ice_servers": r.get("ice_servers")}

    async def ice(self, sc, cand):
        await self.p.send(type="sc_rtc_ice", sc=sc, cand=cand)


async def page_for(pw, kind, module, bridge):
    browser = await getattr(pw, kind).launch()
    page = await browser.new_page()
    await page.expose_function("kelpieOffer", bridge.offer)
    await page.expose_function("kelpieIce", bridge.ice)
    # A module of its own: an inline module's exports are its own, so the page names them. Served at an origin of its own, as the phone's page
    # is: Firefox never starts gathering for the first peer of an about:blank page.
    body = module + "\nwindow.KelpieRtc = { ScreenPeer };\n"
    html = "<!doctype html><meta charset=utf-8><title>phone</title><script type=module>" + body + "</script>"
    await page.route("http://phone.test/", lambda r: r.fulfill(body=html, content_type="text/html"))
    await page.goto("http://phone.test/")
    await page.wait_for_function("window.KelpieRtc !== undefined", timeout=10_000)
    await page.evaluate(DRIVER.replace("ACK_MS", str(int(ACK_DELAY * 1000))))
    return browser, page


async def wait_path(p, since, pred, timeout):
    return await p.wait(lambda m: m["type"] == "sc_path" and pred(m), timeout, since)


def link_video(p, since):
    return [m for m in p.log[since:] if m["type"] == "sc_media" and m["kind"] == "video"]


async def frames(page):
    return await page.evaluate("window.frames")


async def run_for(page, seconds, t0):
    """The page's frames over `seconds` (`screen_support.frame_stats`)."""
    before = len(await frames(page))
    began = now_ms()
    await asyncio.sleep(seconds)
    got = (await frames(page))[before:]
    return ss.frame_stats([(seq, cap, at, size) for seq, cap, at, size, _ in got], t0, (now_ms() - began) / 1000)


async def click_lands(page, started, x, y):
    seen = len(MOTION.clicks_seen())
    sx, sy = to_source(x, y, started)
    await page.evaluate(f"window.clickAt({sx}, {sy})")
    landed = await asyncio.to_thread(until, lambda: len(MOTION.clicks_seen()) > seen, 5)
    return bool(landed)


async def start(p, monitor):
    """A screen of the primary monitor, from the emulated phone (Automatic, over its direct link): its id, its clock, the start's answer."""
    p.sc, p.early = None, 0
    started = await p.ask(type="sc_start", target=monitor["id"], timeout=20) or {}
    p.sc = started.get("sc")
    return p.sc, started.get("t0"), started


async def scenario(run, d):
    from playwright.async_api import async_playwright

    async def call(method, params=None):
        return await asyncio.to_thread(d.call, method, params or {})

    run.boot()
    await asyncio.sleep(1)
    serve_port = await ss.phones_on(SETUP, call)
    p, device, hello, passkey, connect = await ss.control_phone(SETUP, d, call, serve_port, "RTC phone")
    check("the phone has control", hello.get("locked") is False and hello["device"]["permission"] == "control", hello.get("device"))
    p.ack_delay = ACK_DELAY
    MOTION.start()
    check("the motion window is up", await asyncio.to_thread(until, MOTION.area, 20))
    ss.to_front(MOTION.hwnd())
    listed = await p.ask(type="screens", timeout=20)
    rows = (listed or {}).get("targets", [])
    paths = {r["id"]: r for r in (listed or {}).get("paths", [])}
    check("the computer lists peer to peer as a way", paths.get("p2p", {}).get("ok") is True, paths)
    monitor = next((r for r in rows if r["kind"] == "monitor" and r.get("primary")), None) or next((r for r in rows if r["kind"] == "monitor"), None)
    mark = p.mark()
    sc, t0, started = await start(p, monitor)
    check("1. the screen starts on the link (Automatic, direct)", sc and started.get("via") == "tailscale", started)
    check("1. frames come on the link", await p.media(mark, lambda m: m["kind"] == "video", 15) is not None)
    module = await asyncio.to_thread(peer_module)
    ax, ay, aw, ah = MOTION.area()
    bridge = Bridge(p)
    async with async_playwright() as pw:
        kinds = [k.strip() for k in ARGS.browsers.split(",") if k.strip()]
        browser, page = await page_for(pw, kinds[0], module, bridge)
        if ARGS.matrix:
            try:
                NUMBERS["matrix"] = await matrix(p, page, sc, t0, mark, monitor)
                rows = NUMBERS["matrix"]
                check("the matrix ran every condition", len(rows) == len(MATRIX) + 1 and all("error" not in r for r in rows), [r for r in rows if "error" in r])
                check("the screen survived every condition", all(r.get("survived", True) for r in rows), [r["name"] for r in rows if r.get("survived") is False])
            finally:
                await browser.close()
            await p.ask(type="sc_stop", sc=sc)
            return NUMBERS
        try:
            # 2. Peer to peer.
            mark = p.mark()
            began = time.monotonic()
            await page.evaluate(f"window.startPeer({json.dumps(sc)})")
            moved = await wait_path(p, mark, lambda m: m.get("path") == "p2p" and m.get("state") == "live", 15)
            NUMBERS["p2p_open_ms"] = round((time.monotonic() - began) * 1000)
            check("2. the computer moves the picture peer to peer", moved is not None, [m for m in p.log[mark:] if m["type"] == "sc_path"])
            if moved is None:
                print("browser trace:", json.dumps(await page.evaluate("window.trace")), flush=True)
            await asyncio.sleep(1)
            got = await frames(page)
            check("2. the first frame peer to peer is a keyframe", bool(got) and got[0][4] == 1, got[:2])
            after = p.mark()
            steady = await run_for(page, 5, t0)
            NUMBERS["p2p_clean"] = steady
            check("2. frames keep coming peer to peer (the acknowledgements on \"i\" reach the helper)", steady["frames"] >= 15, steady)
            check("2. the link carries no more video", len(link_video(p, after)) == 0, len(link_video(p, after)))
            path = await page.evaluate("window.pathNow()")
            check("2. the browser's selected pair is P2P", (path or {}).get("path") == "p2p", path)
            check("2. a click sent on \"i\" lands in the window", await click_lands(page, started, ax + aw // 2, ay + ah // 2))
            # 3. Back to the link.
            mark = p.mark()
            began = time.monotonic()
            r = await p.ask(type="sc_via", sc=sc, via="tailscale")
            back = await p.media(mark, lambda m: m["kind"] == "video", 10)
            NUMBERS["to_link_ms"] = round((time.monotonic() - began) * 1000)
            check("3. back on the link by choice within 3 s", (r or {}).get("type") == "ack" and back is not None and NUMBERS["to_link_ms"] <= 3000, NUMBERS["to_link_ms"])
            check("3. the computer says the link", await wait_path(p, mark, lambda m: m.get("path") == "tailscale" and m.get("state") == "live", 5) is not None)
            # 4. Another nonce.
            mark = p.mark()
            bridge.tamper = True
            await page.evaluate(f"window.startPeer({json.dumps(sc)})")
            refused = await wait_path(p, mark, lambda m: m.get("state") == "failed", 15)
            bridge.tamper = False
            check("4. a hello with another nonce is closed", (refused or {}).get("why") == "bad_hello", refused)
            stay = p.mark()
            await asyncio.sleep(1.5)
            check("4. and the picture stays on the link", len(link_video(p, stay)) > 0)
            # 5. Peer to peer again: the person picks Automatic, the page offers.
            mark = p.mark()
            began = time.monotonic()
            await p.ask(type="sc_via", sc=sc, via="auto")
            await page.evaluate(f"window.startPeer({json.dumps(sc)})")
            moved = await wait_path(p, mark, lambda m: m.get("path") == "p2p" and m.get("state") == "live", 15)
            back = await asyncio.wait_for(page.wait_for_function("window.frames.length > 0", timeout=10_000), 12) if moved else None
            NUMBERS["to_p2p_ms"] = round((time.monotonic() - began) * 1000)
            got = await frames(page)
            check("5. peer to peer again within 3 s, a keyframe first", moved is not None and back is not None and NUMBERS["to_p2p_ms"] <= 3000 and got and got[0][4] == 1, (NUMBERS["to_p2p_ms"], got[:1]))
            # 6. A lossy, slow network on the computer's UDP: a new peer under it.
            set_impair("loss:0.01,delay_ms:50")
            mark = p.mark()
            await page.evaluate(f"window.startPeer({json.dumps(sc)})")
            moved = await wait_path(p, mark, lambda m: m.get("path") == "p2p" and m.get("state") == "live", 20)
            check("6. a peer opens under 1 % loss and 100 ms round trip", moved is not None)
            lossy = await run_for(page, ARGS.seconds, t0)
            NUMBERS["p2p_loss1_rtt100"] = lossy
            ended = [m for m in p.log[mark:] if m["type"] == "sc_end"]
            check("6. the screen keeps going under loss", not ended and lossy["frames"] > 0, (ended, lossy))
            set_impair("")
            # 7. The rules hold peer to peer: the way turned off on the computer ends the screen within a second; the phone locking ends it
            # at once (both through the hold's task, as on the link).
            mark = p.mark()
            await page.evaluate(f"window.startPeer({json.dumps(sc)})")
            check("7. peer to peer again, impairment off", await wait_path(p, mark, lambda m: m.get("path") == "p2p" and m.get("state") == "live", 15) is not None)
            at = time.time()
            await call("mobile.set_screen", {"methods": {"p2p": False}})
            ended = await p.wait(lambda m: m["type"] == "sc_end", 5, mark)
            NUMBERS["p2p_off_end_ms"] = round((time.time() - at) * 1000)
            check("7. turning peer to peer off ends a screen going that way within a second", (ended or {}).get("reason") == "revoked" and NUMBERS["p2p_off_end_ms"] <= 1000, (ended, NUMBERS["p2p_off_end_ms"]))
            await call("mobile.set_screen", {"methods": {"p2p": True}})
            sc, t0, started = await start(p, monitor)
            mark = p.mark()
            await page.evaluate(f"window.startPeer({json.dumps(sc)})")
            check("7. a new screen goes peer to peer (the way on again, verified)", await wait_path(p, mark, lambda m: m.get("path") == "p2p" and m.get("state") == "live", 15) is not None)
            # A locked phone hears nothing of the screen: the desktop's indicator says it ended, as in screen.py.
            w = run.d
            shared = "return !!document.querySelector('[data-screen-shared]')"
            check("7. the desktop shows the screen shared", bool(await asyncio.to_thread(until, lambda: w.js(shared), 5)))
            at = time.time()
            await p.send(type="lock_now")
            gone = await asyncio.to_thread(until, lambda: not w.js(shared), 5, 0.05)
            NUMBERS["lock_end_ms"] = round((time.time() - at) * 1000)
            check("7. the phone locking ends a peer to peer screen within a second", bool(gone) and NUMBERS["lock_end_ms"] <= 1000, NUMBERS["lock_end_ms"])
            # And the peer stops sending: no frame arrives a second after the end.
            await asyncio.sleep(1)
            held = len(await frames(page))
            await asyncio.sleep(1.5)
            check("7. …and nothing more comes peer to peer", len(await frames(page)) == held, len(await frames(page)) - held)
            challenge = (await p.ask(type="lock_challenge"))["lock"]["challenge"]
            unlocked = await p.ask(type="lock_unlock", **passkey.get(challenge))
            check("7. …and the phone unlocks again", (unlocked or {}).get("result") == "ok", unlocked and unlocked.get("type"))
            sc, t0, started = await start(p, monitor)
            await page.evaluate("window.peer && window.peer.close()")
        finally:
            await browser.close()
        # 8. Another browser engine.
        for kind in kinds[1:]:
            browser, page = await page_for(pw, kind, module, bridge)
            try:
                mark = p.mark()
                await page.evaluate(f"window.startPeer({json.dumps(sc)})")
                moved = await wait_path(p, mark, lambda m: m.get("path") == "p2p" and m.get("state") == "live", 20)
                check(f"8. {kind}: the computer moves the picture to its peer", moved is not None, [m for m in p.log[mark:] if m["type"] == "sc_path"])
                if moved is None:
                    print("browser trace:", json.dumps(await page.evaluate("window.trace")), flush=True)
                run_k = await run_for(page, 5, t0)
                NUMBERS[f"{kind}_clean"] = run_k
                check(f"8. {kind}: frames come peer to peer", run_k["frames"] >= 10, run_k)
                check(f"8. {kind}: a click lands", await click_lands(page, started, ax + aw // 3, ay + ah // 2))
            finally:
                await browser.close()
    # 9. Stop.
    mark = p.mark()
    await p.ask(type="sc_stop", sc=sc)
    await asyncio.sleep(0.5)
    audit = (await call("mobile.audit", {"limit": 50})).get("entries", [])
    check("9. the audit log has the starts", sum(1 for e in audit if e.get("action") == "screen_start") >= 3, audit[-3:])
    return NUMBERS


def main():
    d = run = None
    try:
        set_impair("")
        d, run = ss.start(SETUP)
        asyncio.run(scenario(run, d))
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False)
    finally:
        ss.finish(SETUP, d, run, "motion")
    out = os.path.join(SETUP.scratch, "screen-rtc-matrix.json" if ARGS.matrix else "screen-rtc.json")
    with open(out, "w", encoding="utf-8") as f:
        json.dump({"at": time.strftime("%Y-%m-%d %H:%M"), "numbers": NUMBERS, "checks": ss.results}, f, indent=1)
    if ARGS.matrix and NUMBERS.get("matrix"):
        table = matrix_table(NUMBERS["matrix"])
        with open(out[:-5] + ".md", "w", encoding="utf-8") as f:
            f.write(table + "\n")
        print(table)
    print(json.dumps(NUMBERS, indent=1))
    failed = [r for r in ss.results if not r[1]]
    print(f"{len(ss.results) - len(failed)}/{len(ss.results)} passed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
