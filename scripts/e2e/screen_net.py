"""How the computer's screen holds up on a bad network, measured: the picture's rate and age, a click's and the pointer's round trips, the
bitrate and its cuts, under delay, a model of TCP loss, a slow link and a dropped path (Windows).

    python -X utf8 scripts/e2e/screen_net.py <scratch-dir> <app> [--label after] [--compare before.json] [--only 2,6] [--seconds 60]

<app> is a debug build of the app with the debug kelpied and kelpie-screen next to it, as for screen.py (same tools, same isolation: this
process's own endpoint and data folder, a fake Tailscale CLI, its own tauri-driver; the user's daemon and data are never reached). The
emulated phone reaches the daemon through a TCP fault proxy (net_fault_proxy.py), shares the primary monitor, and acknowledges each video
frame 8 ms after it arrives, standing in for decoding. A window of this script's own (tkinter) scrolls text at 30 frames a second (the motion)
and writes down when it is clicked. The pointer moves (it is the computer's own pointer, inside that window).

Each condition streams for `--seconds` (60): 20 clicks a second apart, and a move every 100 ms whose cursor message is timed back.

  #   one-way delay              loss model (rto = max(200 ms, 2 x RTT))   down rate
  1-3 25 / 50 / 100 ms           0                                        unlimited
  4-5 50 ms                      1 %, 5 %                                 unlimited
  6   50 ms                      0                                        300 kbps
  7   50 ms, then 10 s blackhole 0                                        unlimited
  8   50 ms, then 60 s blackhole 0                                        unlimited

Writes <scratch-dir>/screen-net-<label>.json and .md; with --compare, a table of both. Loss is a model: TCP delays what it lost, so the proxy
holds a piece and everything behind it. Nothing typed is sent; nothing captured is kept beyond the frames' headers.
"""
import argparse
import asyncio
import ctypes
import json
import math
import os
import sys
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import screen_support as ss  # noqa: E402
from screen_support import check, now_ms, until  # noqa: E402

SETUP = ss.prepare("screen-net", __doc__)
import net_fault_proxy as nfp  # noqa: E402

ap = argparse.ArgumentParser()
ap.add_argument("scratch")
ap.add_argument("app")
ap.add_argument("--label", default="run")
ap.add_argument("--compare")
ap.add_argument("--only", help="condition numbers, comma-separated")
ap.add_argument("--seconds", type=int, default=60)
ARGS = ap.parse_args()

ctypes.windll.shcore.SetProcessDpiAwareness(2)
WARM_S = 5
ACK_DELAY = 0.008

CONDITIONS = [
    {"n": 1, "name": "25 ms one way", "delay": 0.025},
    {"n": 2, "name": "50 ms one way", "delay": 0.050},
    {"n": 3, "name": "100 ms one way", "delay": 0.100},
    {"n": 4, "name": "50 ms, 1 % loss model", "delay": 0.050, "loss": 0.01},
    {"n": 5, "name": "50 ms, 5 % loss model", "delay": 0.050, "loss": 0.05},
    {"n": 6, "name": "50 ms, 300 kbps down", "delay": 0.050, "down_kbps": 300},
    {"n": 7, "name": "50 ms, 10 s blackhole", "delay": 0.050, "blackhole": 10},
    {"n": 8, "name": "50 ms, 60 s blackhole", "delay": 0.050, "blackhole": 60},
]

MOTION_WINDOW = ss.Motion(SETUP)
area, clicks_seen = MOTION_WINDOW.area, MOTION_WINDOW.clicks_seen


def pct(values, q):
    v = sorted(x for x in values if x is not None and math.isfinite(x))
    if not v:
        return None
    return round(v[min(len(v) - 1, int(round((len(v) - 1) * q)))], 1)


class Phone:
    """The emulated phone and what it takes to keep it connected: a connection that closed is made again, and the screen bound to it."""

    def __init__(self, connect, passkey):
        self.connect, self.passkey = connect, passkey
        self.p = None

    async def ensure(self):
        if self.p is not None and self.p.closed is None:
            return self.p
        p = self.connect()
        p.ack_delay = ACK_DELAY
        hello = await p.open()
        if (hello or {}).get("locked"):
            challenge = (await p.ask(type="lock_challenge"))["lock"]["challenge"]
            await p.ask(type="lock_unlock", **self.passkey.get(challenge))
        self.p = p
        return p


async def start_screen(p, target):
    mark = p.mark()
    p.sc, p.early = None, 0
    started = await p.ask(type="sc_start", target=target, timeout=20)
    p.sc = (started or {}).get("sc")
    if p.sc and getattr(p, "early", 0):
        await p.send(type="sc_ack", sc=p.sc, seq=p.early)
    p.early = 0
    first = await p.media(mark, lambda m: m["kind"] == "video", 15)
    return started, first, mark


def to_source(x, y, started):
    """A point on the primary monitor (physical pixels) in the picture's source pixels."""
    mw, mh = ctypes.windll.user32.GetSystemMetrics(0), ctypes.windll.user32.GetSystemMetrics(1)
    sw, sh = started.get("w") or mw, started.get("h") or mh
    return round(x * sw / max(mw, 1)), round(y * sh / max(mh, 1))


async def measure(phone, proxy, target, c, call):
    """One condition: the proxy set, a fresh screen, the run, and its numbers."""
    proxy.heal()
    proxy.latency = c["delay"]
    rtt = 2 * c["delay"]
    proxy.loss = c.get("loss", 0.0)
    proxy.rto = max(0.2, 2 * rtt)
    proxy.bandwidth = {"up": None, "down": c["down_kbps"] * 1000 // 8 if c.get("down_kbps") else None}
    p = await phone.ensure()
    started, first, mark = await start_screen(p, target)
    sc = p.sc
    if not sc or first is None:
        return {"n": c["n"], "name": c["name"], "error": f"no picture: {started}"}
    t0 = started.get("t0")
    conns = [(p, mark)]
    ax, ay, aw, ah = area()
    cx, cy = ax + aw // 2, ay + ah // 2
    began = time.monotonic()
    began_w = now_ms()
    moves, clicks, i = [], [], 0
    hole = c.get("blackhole")
    hole_at = hole_end = None
    last_touch = time.monotonic()
    while time.monotonic() - began < ARGS.seconds + (hole or 0):
        t = time.monotonic() - began
        if hole and hole_at is None and t >= 15:
            hole_at = now_ms()
            proxy.blackhole(hole)
        if hole_at and hole_end is None and now_ms() - hole_at >= hole * 1000:
            hole_end = now_ms()
        if p.closed is not None:
            # The link went: as the page does, a new connection, and the screen asked for again on it.
            try:
                p = await asyncio.wait_for(phone.ensure(), 12)
                conns.append((p, 0))
                p.sc = sc
                await p.ask(type="sc_rebind", sc=sc)
            except Exception:  # noqa: BLE001 - the path is still down
                await asyncio.sleep(2)
            continue
        if time.monotonic() - last_touch > 30:
            last_touch = time.monotonic()
            await p.touch()
        if not hole:
            i += 1
            if 20 <= t < 40 and i % 10 == 0:
                # A click a second, each at its own place so the window's note says which it was.
                x, y = ax + 40 + (len(clicks) * 37) % (aw - 80), cy + 60
                sx, sy = to_source(x, y, started)
                clicks.append((now_ms(), x, y))
                await p.send(type="sc_input", sc=sc, ops=[{"k": "click", "x": sx, "y": sy, "b": "left"}], st=now_ms())
            else:
                a = i * 2 * math.pi / 37
                x, y = cx + round(150 * math.cos(a)), cy - 60 + round(100 * math.sin(a))
                sx, sy = to_source(x, y, started)
                moves.append((now_ms(), sx, sy))
                await p.send(type="sc_input", sc=sc, ops=[{"k": "move", "x": sx, "y": sy}], st=now_ms())
        await asyncio.sleep(0.1)
    end_w = now_ms()
    if hole and hole_end is None:
        hole_end = hole_at + hole * 1000
    # Every connection's log since the start: a reconnect starts a new one.
    log = [m for q, at in conns for m in q.log[at:]]
    slow = sum(1 for q, _ in conns if q.closed == 4411)
    if p.closed is None:
        await p.ask(type="sc_stop", sc=sc)
    await asyncio.sleep(0.5)
    audit = (await call("mobile.audit", {"limit": 50})).get("entries", [])
    return summarise(c, log, sc, t0, began_w, end_w, moves, clicks, hole_at, hole_end, audit, slow, proxy, len(conns))


def summarise(c, log, sc, t0, began_w, end_w, moves, clicks, hole_at, hole_end, audit, slow, proxy, connections):
    rtt = 2 * c["delay"] * 1000
    video = [m for m in log if m["type"] == "sc_media" and m["kind"] == "video"]
    steady = [m for m in video if m["_w"] >= began_w + WARM_S * 1000]
    span = max(1.0, (end_w - (began_w + WARM_S * 1000)) / 1000)
    if hole_at:
        span = max(1.0, span - (hole_end - hole_at) / 1000)
    fps = round(len(steady) / span, 1)
    # Picture age: from the capture (the stream's start + its capture time) to the frame's arrival; without the start's clock, only the part over
    # the best frame of the run (the queue it waited in).
    ages = [m["_w"] - (t0 + m["capture_ms"]) for m in steady] if t0 else []
    lag = [m["_w"] - m["capture_ms"] for m in steady]
    floor = min(lag) if lag else 0
    queued = [v - floor for v in lag]
    kbps = round(sum(m["_size"] for m in steady) * 8 / span / 1000)
    keys = [m["_size"] for m in steady if m["key"]]
    gaps = []
    for a, b in zip(video, video[1:]):
        if hole_at and a["_w"] <= hole_end + 500 and b["_w"] >= hole_at:
            continue
        if b["_w"] - a["_w"] > 2000:
            gaps.append(round(b["_w"] - a["_w"]))
    # The pointer: each move's round trip, to the first cursor message that shows it or a later move (the computer sends only the pointer's
    # latest place, so a move overtaken by the next one is never shown itself).
    cursors = [m for m in log if m["type"] == "sc_media" and m["kind"] == "cursor" and m["x"] is not None]
    cursor_rt = []
    for i, (at, _, _) in enumerate(moves):
        later = moves[i:i + 10]
        back = next((m for m in cursors if m["_w"] >= at and any(abs(m["x"] - x) <= 2 and abs(m["y"] - y) <= 2 for t, x, y in later if t <= m["_w"])), None)
        if back:
            cursor_rt.append(back["_w"] - at)
    # A click and the window's note of it, by where it landed.
    seen = clicks_seen()
    click_rt = []
    for at, x, y in clicks:
        hit = next((w for w, wx, wy in seen if w >= at - 5 and abs(wx - x) <= 3 and abs(wy - y) <= 3), None)
        if hit is not None:
            click_rt.append(hit - at)
    stats = [m for m in log if m["type"] == "sc_stats"]
    stages = [m.get("stages") or {} for m in stats]
    last = stats[-1] if stats else {}
    kbps_stats = [(m["_w"] - began_w, m.get("kbps")) for m in stats]
    under = None
    if c.get("down_kbps"):
        cap = c["down_kbps"]
        for k, (t, v) in enumerate(kbps_stats):
            if all((vv or 0) <= cap for _, vv in kbps_stats[k:]):
                under = round(t / 1000, 1)
                break
    ends = [(m.get("reason"), m.get("server_ts")) for m in log if m["type"] == "sc_end"]
    resumed = None
    if hole_at and c["blackhole"] < 30:
        after = [m for m in video if m["_w"] >= hole_end]
        if after:
            resumed = {"after_s": round((after[0]["_w"] - hole_end) / 1000, 2), "key_first": after[0]["key"]}
    lost_at = None
    if hole_at and c["blackhole"] >= 30:
        stop = next((e for e in audit if e.get("action") == "screen_stop" and "link_lost" in json.dumps(e)), None)
        when = None
        if stop:
            for k in ("at", "ts", "time", "when"):
                if isinstance(stop.get(k), (int, float)):
                    when = stop[k] * (1000 if stop[k] < 1e11 else 1)
        if when is None and ends:
            when = next((ts for r, ts in ends if r == "link_lost"), None)
        lost_at = round((when - hole_at) / 1000, 1) if when else None
    out = {
        "n": c["n"], "name": c["name"], "rtt_ms": rtt,
        "fps": fps, "frames": len(steady),
        "age_ms_p50": pct(ages, 0.5), "age_ms_p95": pct(ages, 0.95),
        "queued_ms_p50": pct(queued, 0.5), "queued_ms_p95": pct(queued, 0.95),
        "click_ms_p50": pct(click_rt, 0.5), "click_ms_p95": pct(click_rt, 0.95), "clicks": f"{len(click_rt)}/{len(clicks)}",
        "cursor_ms_p50": pct(cursor_rt, 0.5), "cursor_ms_p95": pct(cursor_rt, 0.95), "cursors": f"{len(cursor_rt)}/{len(moves)}",
        "kbps": kbps, "stats_kbps": last.get("kbps"), "size": f"{round(last.get('w') or 0)}x{round(last.get('h') or 0)}", "stats_fps": last.get("fps"), "reason": last.get("reason"),
        "cuts": max((s.get("link_cuts") or 0 for s in stages), default=0),
        "cuts_by_rule": {r: max((s.get(f"link_cuts_{r}") or 0 for s in stages), default=0) for r in ("queueing", "window", "size")},
        "inflight_limit": max((s["inflight_limit"] for s in stages if s.get("inflight_limit") is not None), default=None),
        "key_bytes_max": max(keys) if keys else None,
        "freezes_over_2s": gaps,
        "slow_consumer": slow, "connections": connections,
        "ends": [r for r, _ in ends], "proxy_held": dict(proxy.held),
        "under_cap_after_s": under, "resumed": resumed, "link_lost_after_s": lost_at,
        "last_stats": {k: v for k, v in last.items() if not k.startswith("_")},
    }
    return out


COLS = [("fps", "fps"), ("age p50/p95", ("age_ms_p50", "age_ms_p95")), ("queued p50/p95", ("queued_ms_p50", "queued_ms_p95")), ("click p50/p95", ("click_ms_p50", "click_ms_p95")),
        ("cursor p50/p95", ("cursor_ms_p50", "cursor_ms_p95")), ("kbps", "kbps"), ("size", "size"), ("cuts", "cuts"), ("inflight", "inflight_limit"), ("key max B", "key_bytes_max"),
        ("freezes>2s", "freezes_over_2s"), ("other", None)]


def cell(r, key):
    if key is None:
        bits = []
        if r.get("under_cap_after_s") is not None:
            bits.append(f"under cap {r['under_cap_after_s']} s")
        if r.get("resumed"):
            bits.append(f"resumed {r['resumed']['after_s']} s{' key first' if r['resumed']['key_first'] else ''}")
        if r.get("link_lost_after_s") is not None:
            bits.append(f"link_lost at {r['link_lost_after_s']} s")
        if r.get("ends"):
            bits.append("ends " + ",".join(str(e) for e in r["ends"]))
        if r.get("slow_consumer"):
            bits.append("slow_consumer")
        return "; ".join(bits)
    if isinstance(key, tuple):
        return " / ".join("–" if r.get(k) is None else str(round(r[k])) for k in key)
    v = r.get(key)
    if isinstance(v, list):
        return str(len(v)) if v else "0"
    return "–" if v is None else str(v)


def table(rows, label):
    head = "| # | condition | " + " | ".join(c for c, _ in COLS) + " |"
    lines = [f"### {label}", "", head, "|" + "---|" * (len(COLS) + 2)]
    for r in rows:
        if "error" in r:
            lines.append(f"| {r['n']} | {r['name']} | {r['error']} |")
            continue
        lines.append(f"| {r['n']} | {r['name']} | " + " | ".join(cell(r, k) for _, k in COLS) + " |")
    return "\n".join(lines)


async def scenario(run, d):
    async def call(method, params=None):
        return await asyncio.to_thread(d.call, method, params or {})

    run.boot()
    await asyncio.sleep(1)
    serve_port = await ss.phones_on(SETUP, call)
    proxy = nfp.FaultProxy(serve_port)
    proxy_port = await proxy.start()
    p, device, hello, passkey, connect = await ss.control_phone(SETUP, d, call, serve_port, "Net phone", via_port=proxy_port)
    check("the phone has control, through the proxy", hello.get("locked") is False and hello["device"]["permission"] == "control", hello.get("device"))
    phone = Phone(connect, passkey)
    p.ack_delay = ACK_DELAY
    phone.p = p
    MOTION_WINDOW.start()
    check("the motion window is up", await asyncio.to_thread(until, area, 20))
    listed = await p.ask(type="screens", timeout=20)
    rows = (listed or {}).get("targets", [])
    monitor = next((r for r in rows if r["kind"] == "monitor" and r.get("primary")), None) or next((r for r in rows if r["kind"] == "monitor"), None)
    check("the phone lists the primary monitor", monitor is not None, [r.get("kind") for r in rows])
    hwnd = MOTION_WINDOW.hwnd()
    ss.to_front(hwnd)
    only = {int(x) for x in ARGS.only.split(",")} if ARGS.only else None
    results = []
    for c in CONDITIONS:
        if only and c["n"] not in only:
            continue
        print(f"condition {c['n']}: {c['name']}", flush=True)
        try:
            r = await measure(phone, proxy, monitor["id"], c, call)
        except Exception as e:  # noqa: BLE001 - one condition's failure is that condition's row
            traceback.print_exc()
            r = {"n": c["n"], "name": c["name"], "error": repr(e)[:200]}
        print(json.dumps(r), flush=True)
        results.append(r)
        proxy.heal()
        await asyncio.sleep(2)
    await proxy.stop()
    return results


def main():
    d = run = None
    rows = []
    try:
        d, run = ss.start(SETUP)
        rows = asyncio.run(scenario(run, d))
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False)
    finally:
        ss.finish(SETUP, d, run, "motion")
    out = os.path.join(SETUP.scratch, f"screen-net-{ARGS.label}")
    with open(out + ".json", "w", encoding="utf-8") as f:
        json.dump({"label": ARGS.label, "at": time.strftime("%Y-%m-%d %H:%M"), "rows": rows}, f, indent=1)
    md = [table(rows, ARGS.label)]
    if ARGS.compare:
        with open(ARGS.compare, encoding="utf-8") as f:
            other = json.load(f)
        md.insert(0, table(other["rows"], other["label"]))
    with open(out + ".md", "w", encoding="utf-8") as f:
        f.write("\n\n".join(md) + "\n")
    print("\n\n".join(md))
    failed = [r for r in ss.results if not r[1]]
    sys.exit(1 if failed or any("error" in r for r in rows) else 0)


if __name__ == "__main__":
    main()
