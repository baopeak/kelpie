"""Perf gate for the phone sync: what eight phones cost the daemon while sessions print.

    python -X utf8 scripts/e2e/mobile_perf.py <kelpied.exe> [--phones 8] [--sessions 3] [--rounds 3]
                                              [--seconds 20] [--mib 16]

Daemon only (own endpoint and data folder; phone access on 127.0.0.1; Windows). Use a release build:
the numbers of a dev build say little. Phones here only count what they are sent, so that the machine
the daemon runs on is not busy parsing for them (they do acknowledge frames, every second one, and answer
heartbeats: the daemon sends nothing past four unacknowledged frames).

1. Steady output: printing sessions (Python printers) run for `--seconds` with nobody connected and
   with `--phones` phones each following all of them, `--rounds` times each, alternating. What the
   printers manage varies run to run (ConPTY is the limit), and the daemon's CPU follows it, so the
   phones' cost is the daemon's CPU minus what the same output cost without phones.
   Gate: at most 5 % of one core.
2. Flat out: the same number of sessions each emit `--mib` MiB (`kelpied bench-emit`) as fast as ConPTY
   delivers them; time and daemon CPU until all are done, with and without the phones. Reported only:
   ConPTY, not the daemon, sets the pace (seconds vary by a factor of three between identical runs), so
   how much the phones slow the daemon's intake is gated by the in-process
   `cargo test --release -p kelpied --bin kelpied -- --ignored ingest_speed --nocapture` (within 5 %).
3. A further session prints too and nobody follows it. Gate: 0 frames for it.
4. The daemon's own event-loop lag (`mobile.load`) during the steady runs with phones, with no stress from
   outside. Reported, and the input to the runtime-split decision: p99 above 30 ms would move the phone
   server onto a runtime of its own.
"""
import asyncio
import ctypes
import json
import os
import statistics
import sys
import tempfile
import time

import websockets

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import phone_support as ps  # noqa: E402

if len(sys.argv) < 2:
    raise SystemExit(__doc__)
exe = os.path.abspath(sys.argv[1])


def option(name, default):
    return type(default)(sys.argv[sys.argv.index(name) + 1]) if name in sys.argv else default


SECONDS = option("--seconds", 20)
PHONES = option("--phones", 8)
SESSIONS = option("--sessions", 3)
ROUNDS = option("--rounds", 3)
MIB = option("--mib", 16)

ps.isolate("phone-perf")
PORT = 49000 + os.getpid() % 1000
BASE = f"http://127.0.0.1:{PORT}/"
work = tempfile.mkdtemp(prefix="kelpie-phone-perf-cwd-")
PRINTER = os.path.join(work, "printer.py")
open(PRINTER, "w", encoding="utf-8").write(
    "import os, sys, time\n"
    "seconds, tag = float(sys.argv[1]), sys.argv[2]\n"
    "out = sys.stdout\n"
    "end = time.time() + seconds\n"
    "n = 0\n"
    "while time.time() < end:\n"
    "    began = time.time()\n"
    "    for _ in range(500):\n"
    "        n += 1\n"
    "        out.write('%s %08d %s\\n' % (tag, n, os.urandom(32).hex()))\n"
    "    out.flush()\n"
    "    time.sleep(max(0, 0.05 - (time.time() - began)))\n"
    "print('PRINTER-DONE-' + tag, flush=True)\n")


def cpu_seconds(pid):
    """Kernel + user time of a process, in seconds."""
    k32 = ctypes.windll.kernel32
    k32.OpenProcess.restype = ctypes.c_void_p
    handle = ctypes.c_void_p(k32.OpenProcess(0x1000, False, pid))  # PROCESS_QUERY_LIMITED_INFORMATION
    created, exited, kernel, user = (ctypes.c_ulonglong() for _ in range(4))
    k32.GetProcessTimes(handle, ctypes.byref(created), ctypes.byref(exited), ctypes.byref(kernel), ctypes.byref(user))
    k32.CloseHandle(handle)
    return (kernel.value + user.value) / 1e7


class Light:
    """A phone that only counts what it is sent (parsing every frame would cost more than the phone
    it stands for, and the machine is shared with the daemon being measured)."""

    def __init__(self, base, token, quiet_id):
        self.url = base.replace("http://", "ws://") + "ws"
        self.token, self.quiet_id = token, quiet_id
        self.frames = self.bytes = self.quiet = 0
        self.subs = set()
        self.unacked = {}

    async def open(self):
        self.ws = await websockets.connect(self.url, max_size=None, ping_interval=None)
        await self.ws.send(json.dumps({"token": self.token, "caps": ["ack", "hb"]}))
        await self.ws.recv()  # hello
        self.reader = asyncio.ensure_future(self._read())

    async def _read(self):
        try:
            async for raw in self.ws:
                if '"type":"screen"' in raw:
                    self.frames += 1
                    self.bytes += len(raw)
                    self.quiet += self.quiet_id in raw
                    # {"type":"screen","id":"...","rev":N,...}: read the two fields without parsing the frame.
                    start = raw.index('"id":"') + 6
                    sid = raw[start:raw.index('"', start)]
                    start = raw.index('"rev":', start) + 6
                    end = start
                    while raw[end].isdigit():
                        end += 1
                    self.unacked[sid] = self.unacked.get(sid, 0) + 1
                    if self.unacked[sid] >= 2:
                        self.unacked[sid] = 0
                        await self.ws.send('{"type":"ack","id":"%s","rev":%s}' % (sid, raw[start:end]))
                elif '"type":"hb"' in raw and '"echo"' not in raw:
                    await self.ws.send('{"type":"hb","echo":true}')
        except websockets.ConnectionClosed:
            pass

    async def follow(self, ids):
        for sid in ids:
            await self.ws.send(json.dumps({"type": "sub", "id": sid, "height": 200}))
        self.subs |= set(ids)

    async def let_go(self):
        for sid in self.subs:
            await self.ws.send(json.dumps({"type": "unsub", "id": sid}))
        self.subs.clear()

    async def close(self):
        self.reader.cancel()
        await self.ws.close()


def info(d, sid):
    return next((s for s in d.call("session.list") if s["id"] == sid), {})


def shell(d, name):
    s = d.call("session.create", {"kind": "shell", "cwd": work, "name": name})
    ps.wait_for(lambda: info(d, s["id"]).get("status") == "idle" and "PS " in d.call("session.read", {"handle": s["handle"]})["text"], 150)
    return s


def counters(phones):
    return [(p.frames, p.bytes, p.quiet) for p in phones]


async def steady(d, sessions, quiet, phones, label):
    """The sessions print for SECONDS; the daemon is measured over the middle of it."""
    for s in sessions + [quiet]:
        d.call("session.input", {"handle": info(d, s["id"])["handle"], "data": f'python "{PRINTER}" {SECONDS} {s["name"]}\r'})
    await asyncio.sleep(3)
    t0, c0, f0 = time.time(), cpu_seconds(d.pid), counters(phones)
    s0 = sum(info(d, s["id"])["seq"] for s in sessions)
    await asyncio.sleep(SECONDS - 6)
    t1, c1, f1 = time.time(), cpu_seconds(d.pid), counters(phones)
    s1 = sum(info(d, s["id"])["seq"] for s in sessions)
    for s in sessions + [quiet]:
        await ps.async_wait_for(lambda s=s: f"PRINTER-DONE-{s['name']}" in d.call("session.read", {"handle": info(d, s["id"])["handle"], "lines": 8})["text"], SECONDS + 60)
    wall = t1 - t0
    got = [sum(x[i] - y[i] for x, y in zip(f1, f0)) for i in range(3)]
    r = {"label": label, "cpu_pct": (c1 - c0) / wall * 100, "ingest_MBps": (s1 - s0) / wall / 1e6, "frames_per_phone_per_s": got[0] / max(1, len(phones)) / wall,
         "avg_frame_kB": got[1] / max(1, got[0]) / 1000, "quiet_frames": got[2]}
    print(json.dumps({k: round(v, 2) if isinstance(v, float) else v for k, v in r.items()}), flush=True)
    return r


async def flat_out(d, phones, label):
    """SESSIONS sessions emit MIB MiB each, as fast as the daemon takes it; time until they are done."""
    c0, f0 = cpu_seconds(d.pid), counters(phones)
    began = time.time()
    args = lambda: [exe, "bench-emit", str(MIB << 20)]  # noqa: E731
    made = [d.call("session.create", {"kind": "custom", "cwd": work, "name": f"emit{i}", "cols": 120, "rows": 30, "args": args()}) for i in range(SESSIONS)]
    ids = [m["id"] for m in made]
    ghost = d.call("session.create", {"kind": "custom", "cwd": work, "name": "emit-quiet", "cols": 120, "rows": 30, "args": args()})
    for p in phones:
        p.quiet_id = ghost["id"]
        await p.follow(ids)
    while any(info(d, i).get("alive") for i in ids):
        await asyncio.sleep(0.05)
    took = time.time() - began
    c1, f1 = cpu_seconds(d.pid), counters(phones)
    ps.wait_for(lambda: not info(d, ghost["id"]).get("alive"), 120)
    got = [sum(x[i] - y[i] for x, y in zip(f1, f0)) for i in range(3)]
    for p in phones:
        await p.let_go()
    for i in ids + [ghost["id"]]:
        d.call("session.close", {"handle": info(d, i)["handle"]})
    r = {"label": label, "seconds": took, "cpu_s": c1 - c0, "MBps": SESSIONS * MIB * 1.048576 / took, "frames_per_phone_per_s": got[0] / max(1, len(phones)) / took,
         "avg_frame_kB": got[1] / max(1, got[0]) / 1000, "quiet_frames": got[2]}
    print(json.dumps({k: round(v, 2) if isinstance(v, float) else v for k, v in r.items()}), flush=True)
    return r


async def main(d):
    d.call("mobile.enable", {"ip": "127.0.0.1", "port": PORT})
    tokens = [ps.pair(d, BASE, f"Phone {i}", False)[0] for i in range(PHONES)]
    sessions = [shell(d, f"printer{i}") for i in range(SESSIONS)]
    quiet = shell(d, "quiet")
    phones = [Light(BASE, t, quiet["id"]) for t in tokens]
    for p in phones:
        await p.open()

    lag = []
    steady_runs = {"without": [], "with": []}
    flat_runs = {"without": [], "with": []}
    for round_ in range(1, ROUNDS + 1):
        steady_runs["without"].append(await steady(d, sessions, quiet, [], f"steady {round_}: no phones"))
        for p in phones:
            await p.follow([s["id"] for s in sessions])
        await asyncio.sleep(1)
        d.call("mobile.load", {"reset": True})
        steady_runs["with"].append(await steady(d, sessions, quiet, phones, f"steady {round_}: {PHONES} phones"))
        lag.append(d.call("mobile.load"))
        for p in phones:
            await p.let_go()
        await asyncio.sleep(1)
        flat_runs["without"].append(await flat_out(d, [], f"flat out {round_}: no phones"))
        flat_runs["with"].append(await flat_out(d, phones, f"flat out {round_}: {PHONES} phones"))
    for p in phones:
        await p.close()

    print("---")
    per_mb = statistics.median(r["cpu_pct"] / r["ingest_MBps"] for r in steady_runs["without"])
    extra = [r["cpu_pct"] - per_mb * r["ingest_MBps"] for r in steady_runs["with"]]
    print(f"steady: the daemon takes in {statistics.median(r['ingest_MBps'] for r in steady_runs['without']):.2f} MB/s for {per_mb:.1f} % of a core per MB/s; "
          f"{PHONES} phones add {statistics.median(extra):+.1f} points (runs: {', '.join(f'{x:+.1f}' for x in extra)}); "
          f"{statistics.median(r['frames_per_phone_per_s'] for r in steady_runs['with']):.1f} frames/s/phone of {statistics.median(r['avg_frame_kB'] for r in steady_runs['with']):.1f} kB")
    base = statistics.median(r["seconds"] for r in flat_runs["without"])
    with_ = statistics.median(r["seconds"] for r in flat_runs["with"])
    slower = (with_ / base - 1) * 100
    print(f"flat out (not gated, ConPTY sets the pace): {SESSIONS} x {MIB} MiB in {base:.2f} s without phones ({statistics.median(r['MBps'] for r in flat_runs['without']):.1f} MB/s), {with_:.2f} s with {PHONES} ({slower:+.1f} %); "
          f"daemon CPU {statistics.median(r['cpu_s'] for r in flat_runs['without']):.1f} s -> {statistics.median(r['cpu_s'] for r in flat_runs['with']):.1f} s; "
          f"{statistics.median(r['frames_per_phone_per_s'] for r in flat_runs['with']):.1f} frames/s/phone of {statistics.median(r['avg_frame_kB'] for r in flat_runs['with']):.1f} kB")
    print("event-loop lag with the phones and the printers (no outside stress): " + "; ".join(
        f"run {i + 1}: p99 {x['lag_p99_ms']} ms, p99.9 {x['lag_p999_ms']} ms over {x['probes']} probes, level {x['level']}" for i, x in enumerate(lag)))
    quiet_frames = sum(r["quiet_frames"] for runs in (steady_runs, flat_runs) for r in runs["with"])
    print(f"frames for the sessions nobody follows: {quiet_frames}")
    ok = quiet_frames == 0 and statistics.median(extra) <= 5
    print("GATE " + ("PASS" if ok else "FAIL"))
    return ok


d = ps.Daemon(exe).start()
try:
    passed = asyncio.run(main(d))
finally:
    d.stop()
sys.exit(0 if passed else 1)
