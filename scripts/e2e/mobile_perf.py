"""Perf gate for the phone sync: what eight phones cost the daemon while sessions print.

    python -X utf8 scripts/e2e/mobile_perf.py <kelpied.exe> [--pair-with <debug kelpied.exe>] [--phones 8] [--sessions 3]
                                              [--rounds 3] [--seconds 20] [--mib 16] [--warm-up]

Daemon only (own endpoint and data folder; phone access on 127.0.0.1; Windows). Measure a release build: the numbers of a
dev build say little, the encryption above all. Pairing needs a daemon that takes this script for the app, which only a debug
build does, so with a release build name a debug one in `--pair-with`: it pairs the phones and stops, and the release daemon
starts on the same endpoint and data folder. No verification prompt can come up: the release daemon is only ever
asked for what the plain user may do. The daemons run pinned at L0 (`KELPIE_LOAD_FORCE`, which release builds honour too): a daemon
at L2 or L3 sheds frames, which lowers the very cost being measured. Phones here only count what they are sent, so that the machine
the daemon runs on is not busy parsing for them (they do acknowledge frames, every second one, and answer
heartbeats: the daemon sends nothing past four unacknowledged frames).

1. Steady output: printing sessions (Python printers) run for `--seconds` with nobody connected and
   with `--phones` phones each following all of them, `--rounds` times each, alternating. The phones' cost is the daemon's CPU with
   them minus its CPU without them, measured in cycles (`QueryProcessCycleTime`) and converted to points of one core with the printed
   clock rate. Gate: at most 5 points.
   What it can show: what serving these phones this much output costs the daemon in all, at L0: rendering, diffing, JSON, encryption
   and the socket. What it cannot show: which of those it is (a separate measurement put sealing at about 0.4 points), or what a
   person's phone costs, since 8 phones each following 3 flooding sessions is a worst case. CPU time in clock ticks reads low on these
   short bursts, and correcting it by the output taken in moved it by more than the limit between identical rounds, so both are
   printed as information only and gate nothing. Every round over the limit is a fail however far apart they are: they agree on that.
   Otherwise the result is reported as unstable, neither pass nor fail, exit 2, when identical rounds disagree by more than the limit, when
   the cost comes out below about -2 points (impossible, so the rounds without phones are no fair measure), or when the phones were
   not sent at least 70 % of the 5 frames a second per session. A busy machine cannot settle the limit: with the host at 100 %, rounds
   without phones have read from 15.2 to 41.1 points. The workload itself keeps a host of this size near 100 %, so closing other programs
   may not be enough, and it may take a bigger machine. A phone
   that stopped reading is a failure of the run. Every round is printed. The first round with phones counts on its own as well as
   in the median: no cause has been found that would make its cost one-time work, so the median may not hide it. `--warm-up` runs one
   more round with phones before anything else, which then is that first round, to look at the first time and every time apart.
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
import winreg

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
PAIR_WITH = os.path.abspath(sys.argv[sys.argv.index("--pair-with") + 1]) if "--pair-with" in sys.argv else None
# The limit, in points of one core.
LIMIT_POINTS = 5
# Identical rounds further apart than the limit say nothing about it. A cost below this is impossible, and says the same.
LOWEST_POINTS = -2
# Below this a with-phones run was not sending frames, whatever the CPU says (each of the sessions goes at 5 frames a second at L0).
MIN_FRAMES_PER_PHONE = 0.7 * SESSIONS * 5
if ROUNDS < 2 or SESSIONS < 1:
    raise SystemExit(f"--rounds must be at least 2 (one round cannot disagree with itself) and --sessions at least 1 (nothing prints otherwise).\n{__doc__}")
if SECONDS <= 6:
    raise SystemExit(f"--seconds must be more than 6: the measured window is what is left after the 6 s of settling.\n{__doc__}")
WARM_UP = "--warm-up" in sys.argv
# The full work: a daemon that sheds frames under load measures less than it costs.
CALM = {"KELPIE_LOAD_FORCE": "L0:calm"}

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


def clock_hz():
    """The rate cycle counts advance at: the processor's stated clock, which an invariant counter follows."""
    with winreg.OpenKey(winreg.HKEY_LOCAL_MACHINE, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0") as key:
        return winreg.QueryValueEx(key, "~MHz")[0] * 1e6


CLOCK_HZ = clock_hz()


def cycles(pid):
    """The cycles a process has used. A call that fails raises: read as none, every round would agree on a cost of nothing."""
    k32 = ctypes.windll.kernel32
    k32.OpenProcess.restype = ctypes.c_void_p
    opened = k32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not opened:
        raise OSError(f"cannot open process {pid} to read its cycles")
    handle = ctypes.c_void_p(opened)
    used = ctypes.c_ulonglong()
    ok = k32.QueryProcessCycleTime(handle, ctypes.byref(used))
    k32.CloseHandle(handle)
    if not ok or not used.value:
        raise OSError(f"no cycles could be read for process {pid}")
    return used.value


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

    def __init__(self, base, device, quiet_id):
        self.url = ps.link_url(base)
        self.device, self.quiet_id = device, quiet_id
        self.frames = self.bytes = self.quiet = 0
        self.subs = set()
        self.unacked = {}
        self.send_lock = asyncio.Lock()

    async def open(self):
        self.ws = await websockets.connect(self.url, max_size=None, ping_interval=None, compression=None)
        init = ps.ln.Initiator(ps.ln.RECONNECT, self.device.priv, self.device.pub, self.device.dpk)
        await self.ws.send(init.first)
        self.session, _ = init.finish(await self.ws.recv())
        await self.send('{"caps":["ack","hb"]}')
        while self.session.open(await self.ws.recv()) is None:  # hello
            pass
        self.reader = asyncio.ensure_future(self._read())

    async def send(self, text):
        async with self.send_lock:  # sealing numbers the frames: they go out in the order they were sealed
            for frame in self.session.seal(text):
                await self.ws.send(frame)

    async def _read(self):
        try:
            async for sealed in self.ws:
                whole = self.session.open(sealed)
                if whole is None:
                    continue
                raw = whole.decode()
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
                        await self.send('{"type":"ack","id":"%s","rev":%s}' % (sid, raw[start:end]))
                elif '"type":"hb"' in raw and '"echo"' not in raw:
                    await self.send('{"type":"hb","echo":true}')
        except websockets.ConnectionClosed:
            pass

    async def follow(self, ids):
        for sid in ids:
            await self.send(json.dumps({"type": "sub", "id": sid, "height": 200}))
        self.subs |= set(ids)

    async def let_go(self):
        for sid in self.subs:
            await self.send(json.dumps({"type": "unsub", "id": sid}))
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
    t0, c0, f0, y0 = time.time(), cpu_seconds(d.pid), counters(phones), cycles(d.pid)
    s0 = sum(info(d, s["id"])["seq"] for s in sessions)
    await asyncio.sleep(SECONDS - 6)
    t1, c1, f1, y1 = time.time(), cpu_seconds(d.pid), counters(phones), cycles(d.pid)
    s1 = sum(info(d, s["id"])["seq"] for s in sessions)
    for s in sessions + [quiet]:
        await ps.async_wait_for(lambda s=s: f"PRINTER-DONE-{s['name']}" in d.call("session.read", {"handle": info(d, s["id"])["handle"], "lines": 8})["text"], SECONDS + 60)
    wall = t1 - t0
    if y1 <= y0:
        raise OSError(f"the daemon used no cycles over {wall:.1f} s of {label}: nothing was measured")
    got = [sum(x[i] - y[i] for x, y in zip(f1, f0)) for i in range(3)]
    r = {"label": label, "cycles": y1 - y0, "seconds": wall, "points": (y1 - y0) / wall / CLOCK_HZ * 100, "cpu_pct": (c1 - c0) / wall * 100, "ingest_MBps": (s1 - s0) / wall / 1e6, "frames_per_phone_per_s": got[0] / max(1, len(phones)) / wall,
         "avg_frame_kB": got[1] / max(1, got[0]) / 1000, "quiet_frames": got[2], "level": d.call("mobile.load")["level"]}
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
    quiet_ids = [p.quiet_id for p in phones]
    for p in phones:
        p.quiet_id = ghost["id"]
        await p.follow(ids)
    while any(info(d, i).get("alive") for i in ids):
        await asyncio.sleep(0.05)
    took = time.time() - began
    c1, f1 = cpu_seconds(d.pid), counters(phones)
    ps.wait_for(lambda: not info(d, ghost["id"]).get("alive"), 120)
    got = [sum(x[i] - y[i] for x, y in zip(f1, f0)) for i in range(3)]
    for p, quiet_id in zip(phones, quiet_ids):
        await p.let_go()
        p.quiet_id = quiet_id  # the steady runs look at the session nobody follows there
    for i in ids + [ghost["id"]]:
        d.call("session.close", {"handle": info(d, i)["handle"]})
    r = {"label": label, "seconds": took, "cpu_s": c1 - c0, "MBps": SESSIONS * MIB * 1.048576 / took, "frames_per_phone_per_s": got[0] / max(1, len(phones)) / took,
         "avg_frame_kB": got[1] / max(1, got[0]) / 1000, "quiet_frames": got[2]}
    print(json.dumps({k: round(v, 2) if isinstance(v, float) else v for k, v in r.items()}), flush=True)
    return r


def unstable(without_points, extra, with_phones):
    """Why the steady runs say nothing about the limit, or None when they agree well enough and the phones were really served."""
    why = []
    if max(without_points) - min(without_points) > LIMIT_POINTS:
        why.append(f"the rounds without phones cost {min(without_points):.1f} to {max(without_points):.1f} points")
    if max(extra) - min(extra) > LIMIT_POINTS:
        why.append(f"the phones' cost ranged {min(extra):+.1f} to {max(extra):+.1f} points in identical rounds")
    if statistics.median(extra) < LOWEST_POINTS:
        why.append(f"the phones seem to save {-statistics.median(extra):.1f} points, so the rounds without them are not a fair measure")
    thin = [r["frames_per_phone_per_s"] for r in with_phones if r["frames_per_phone_per_s"] < MIN_FRAMES_PER_PHONE]
    if thin:
        why.append(f"phones were sent only {min(thin):.2f} frames a second in a round (at least {MIN_FRAMES_PER_PHONE:.1f} are needed to have measured the load)")
    shed = [r["level"] for r in with_phones if r["level"] > 0]
    if shed:
        why.append(f"the daemon was at level {max(shed)}, not pinned at 0, so it was shedding work")
    return "; ".join(why) or None


def verdict(without_points, extra, first, with_phones):
    """The gate on the steady rounds: PASS, FAIL or UNSTABLE, and what to say about it."""
    if min(extra) > LIMIT_POINTS:
        return "FAIL", f"every round is over the limit of {LIMIT_POINTS} points, the lowest at {min(extra):+.2f}, however far apart they are"
    why = unstable(without_points, extra, with_phones)
    if why:
        return "UNSTABLE", (f"UNSTABLE: {why}. The limit of {LIMIT_POINTS} points was neither met nor missed, and a busy machine cannot settle it: with the host at 100 %, "
                            "rounds without phones have read from 15.2 to 41.1 points. Close other programs and run it again; no figure from this run says anything about the limit.")
    ok = statistics.median(extra) <= LIMIT_POINTS and first <= LIMIT_POINTS
    return ("PASS" if ok else "FAIL"), f"the first round with phones adds {first:+.2f} points"


def paired_phones(pair_exe):
    """Pairs the phones on a daemon that takes this script for the app; their records and keys stay for the next daemon."""
    d = ps.Daemon(pair_exe, env=CALM).start()
    try:
        d.call("mobile.enable", {"ip": "127.0.0.1", "port": PORT})
        return [ps.pair(d, BASE, f"Phone {i}", False)[0] for i in range(PHONES)]
    finally:
        d.stop()


async def main(d, tokens):
    d.call("mobile.enable", {"ip": "127.0.0.1", "port": PORT})
    if tokens is None:
        tokens = [ps.pair(d, BASE, f"Phone {i}", False)[0] for i in range(PHONES)]
    sessions = [shell(d, f"printer{i}") for i in range(SESSIONS)]
    quiet = shell(d, "quiet")
    phones = [Light(BASE, t, quiet["id"]) for t in tokens]
    for p in phones:
        await p.open()

    lag = []
    steady_runs = {"without": [], "with": []}
    flat_runs = {"without": [], "with": []}
    warm = None
    if WARM_UP:
        for p in phones:
            await p.follow([s["id"] for s in sessions])
        await asyncio.sleep(1)
        warm = await steady(d, sessions, quiet, phones, f"warm-up: {PHONES} phones")
        for p in phones:
            await p.let_go()
        await asyncio.sleep(1)
    for round_ in range(1, ROUNDS + 1):
        steady_runs["without"].append(await steady(d, sessions, quiet, [], f"steady {round_}: no phones"))
        for p in phones:
            await p.follow([s["id"] for s in sessions])
        await asyncio.sleep(1)
        d.call("mobile.load", {"reset": True})
        steady_runs["with"].append(await steady(d, sessions, quiet, phones, f"steady {round_}: {PHONES} phones"))
        lag.append(d.call("mobile.load"))
        dead = sum(p.reader.done() for p in phones)
        if dead:
            for p in phones:
                if p.reader.done() and p.reader.exception():
                    print(f"a phone stopped reading: {p.reader.exception()!r}", flush=True)
            print(f"{dead} of {PHONES} phones stopped reading in round {round_}: nothing past that was measured.")
            print("GATE FAIL")
            return 1
        for p in phones:
            await p.let_go()
        await asyncio.sleep(1)
        flat_runs["without"].append(await flat_out(d, [], f"flat out {round_}: no phones"))
        flat_runs["with"].append(await flat_out(d, phones, f"flat out {round_}: {PHONES} phones"))
    for p in phones:
        await p.close()

    print("---")
    # Information only: CPU by clock ticks, corrected by the output taken in (see the top of this file).
    per_mb = statistics.median(r["cpu_pct"] / max(r["ingest_MBps"], 1e-6) for r in steady_runs["without"])
    without_points = [r["points"] for r in steady_runs["without"]]
    baseline = statistics.median(without_points)
    extra = [r["points"] - baseline for r in steady_runs["with"]]
    r = steady_runs["with"][0]
    print(f"cycles to points: cycles / seconds / {CLOCK_HZ / 1e6:.0f} MHz x 100; the first round with phones: {r['cycles']} / {r['seconds']:.2f} / {CLOCK_HZ / 1e6:.0f} MHz x 100 = {r['points']:.2f}")
    for i, (without, with_, added) in enumerate(zip(steady_runs["without"], steady_runs["with"], extra), 1):
        print(f"round {i}: no phones {without['points']:.2f} points, level {without['level']}; with {PHONES} {with_['points']:.2f} points, adds {added:+.2f} points, {with_['frames_per_phone_per_s']:.2f} frames/s/phone of {with_['avg_frame_kB']:.1f} kB, level {with_['level']}; "
              f"for information, ticks {without['cpu_pct']:.1f} and {with_['cpu_pct']:.1f} %, intake {without['ingest_MBps']:.2f} and {with_['ingest_MBps']:.2f} MB/s, ticks corrected by intake {with_['cpu_pct'] - per_mb * with_['ingest_MBps']:+.1f}")
    if warm:
        print(f"warm-up round, the first with phones: {warm['points']:.2f} points, adds {warm['points'] - baseline:+.2f} points, {warm['frames_per_phone_per_s']:.2f} frames/s/phone, level {warm['level']}")
    print(f"steady: {baseline:.2f} points without phones; {PHONES} phones add {statistics.median(extra):+.2f} points (rounds: {', '.join(f'{x:+.2f}' for x in extra)}); "
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
    if quiet_frames != 0:
        print("GATE FAIL")
        return 1
    gate, said = verdict(without_points, extra, warm["points"] - baseline if warm else extra[0], steady_runs["with"])
    print(said)
    print(f"GATE {gate}")
    return {"PASS": 0, "FAIL": 1, "UNSTABLE": 2}[gate]


# Paired on a debug daemon, measured on the one named first.
paired = paired_phones(PAIR_WITH) if PAIR_WITH else None
d = ps.Daemon(exe, env=CALM, as_app=PAIR_WITH is None).start()
try:
    verdict = asyncio.run(main(d, paired))
finally:
    d.stop()
sys.exit(verdict)
