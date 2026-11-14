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
7. Firefox offers in turn: the computer moves to its peer, frames come, a click lands.
8. Stop ends it; the audit log has the start.

Writes <scratch-dir>/screen-rtc.json with each step's numbers. Nothing typed is sent; nothing captured is kept beyond the frames' headers.
"""
import argparse
import asyncio
import ctypes
import json
import math
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


def to_source(x, y, started):
    """A point on the primary monitor (physical pixels) in the picture's source pixels."""
    mw, mh = ctypes.windll.user32.GetSystemMetrics(0), ctypes.windll.user32.GetSystemMetrics(1)
    sw, sh = started.get("w") or mw, started.get("h") or mh
    return round(x * sw / max(mw, 1)), round(y * sh / max(mh, 1))


def pct(values, q):
    v = sorted(x for x in values if x is not None and math.isfinite(x))
    return round(v[min(len(v) - 1, int(round((len(v) - 1) * q)))], 1) if v else None


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
    """The page's frames over `seconds`: rate, age p50/p95 (from capture on the computer to arrival), longest gap."""
    before = len(await frames(page))
    began = now_ms()
    await asyncio.sleep(seconds)
    got = (await frames(page))[before:]
    ages = [at - (t0 + cap) for _, cap, at, _, _ in got] if t0 else []
    gaps = [b[2] - a[2] for a, b in zip(got, got[1:])]
    span = max(1, (now_ms() - began) / 1000)
    return {"frames": len(got), "fps": round(len(got) / span, 1), "kbps": round(sum(g[3] for g in got) * 8 / span / 1000), "age_p50": pct(ages, 0.5), "age_p95": pct(ages, 0.95), "longest_gap_ms": max(gaps) if gaps else None}


async def click_lands(page, started, x, y):
    seen = len(MOTION.clicks_seen())
    sx, sy = to_source(x, y, started)
    await page.evaluate(f"window.clickAt({sx}, {sy})")
    landed = await asyncio.to_thread(until, lambda: len(MOTION.clicks_seen()) > seen, 5)
    return bool(landed)


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
    p.sc, p.early = None, 0
    started = await p.ask(type="sc_start", target=monitor["id"], timeout=20) or {}
    p.sc = started.get("sc")
    sc, t0 = p.sc, started.get("t0")
    check("1. the screen starts on the link (Automatic, direct)", sc and started.get("via") == "tailscale", started)
    check("1. frames come on the link", await p.media(mark, lambda m: m["kind"] == "video", 15) is not None)
    module = await asyncio.to_thread(peer_module)
    ax, ay, aw, ah = MOTION.area()
    bridge = Bridge(p)
    async with async_playwright() as pw:
        kinds = [k.strip() for k in ARGS.browsers.split(",") if k.strip()]
        browser, page = await page_for(pw, kinds[0], module, bridge)
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
            await page.evaluate("window.peer && window.peer.close()")
        finally:
            await browser.close()
        # 7. Another browser engine.
        for kind in kinds[1:]:
            browser, page = await page_for(pw, kind, module, bridge)
            try:
                mark = p.mark()
                await page.evaluate(f"window.startPeer({json.dumps(sc)})")
                moved = await wait_path(p, mark, lambda m: m.get("path") == "p2p" and m.get("state") == "live", 20)
                check(f"7. {kind}: the computer moves the picture to its peer", moved is not None, [m for m in p.log[mark:] if m["type"] == "sc_path"])
                if moved is None:
                    print("browser trace:", json.dumps(await page.evaluate("window.trace")), flush=True)
                run_k = await run_for(page, 5, t0)
                NUMBERS[f"{kind}_clean"] = run_k
                check(f"7. {kind}: frames come peer to peer", run_k["frames"] >= 10, run_k)
                check(f"7. {kind}: a click lands", await click_lands(page, started, ax + aw // 3, ay + ah // 2))
            finally:
                await browser.close()
    # 8. Stop.
    mark = p.mark()
    await p.ask(type="sc_stop", sc=sc)
    await asyncio.sleep(0.5)
    audit = (await call("mobile.audit", {"limit": 50})).get("entries", [])
    check("8. the audit log has the start", any(e.get("action") == "screen_start" for e in audit), audit[-3:])
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
    out = os.path.join(SETUP.scratch, "screen-rtc.json")
    with open(out, "w", encoding="utf-8") as f:
        json.dump({"at": time.strftime("%Y-%m-%d %H:%M"), "numbers": NUMBERS, "checks": ss.results}, f, indent=1)
    print(json.dumps(NUMBERS, indent=1))
    failed = [r for r in ss.results if not r[1]]
    print(f"{len(ss.results) - len(failed)}/{len(ss.results)} passed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
