"""CPU stress for the phone tests, and the busy-computer gate for the phone sync.

    python -X utf8 scripts/e2e/stress.py --load-only --threads 4 --duty 1.0 --priority normal --seconds 60
    python -X utf8 scripts/e2e/stress.py <kelpied.exe> [--minutes 30] [--sessions 20] [--phones 8]
                                         [--threads N] [--duty 1.0] [--priority normal]

The first form only burns CPU: `--threads` busy workers at `--duty` (0..1 of each 100 ms slice) and
`--priority` (idle, below, normal, above), plus `--children 20` low-duty "agent" processes that wake and
work in bursts (the profile of five to twenty agents running builds). Workers are processes of their own,
so Python's lock does not limit them, and only the ones this script started are ever stopped.

The second form is the gate: an isolated daemon (its own KELPIE_ENDPOINT and data folder, phone access on
127.0.0.1) with `--sessions` sessions and `--phones` phones, while the machine is made busy:

  minute 0-1          calm: echo latency baseline, memory baseline
  minute 1 to the end - 2   `--threads` workers (default: one per core) at `--priority` plus 20 children
  last 2 minutes      calm again, to see the level fall

Checks (the plan's gates): 0 false disconnects (no phone loses its connection unasked); `input` echo
p95 <= 300 ms and p99 <= 800 ms measured under the stress; revoking a device stops its writes in under
a second; 0 dropped `input`/`key`/`answer` (every one is answered, every typed line comes back); daemon
RSS growth <= 10 % from the end of the first minute; the level reaches L2 within 5 s of the stress
starting and L0 within 25 s of it stopping; with phone access off the load probe is not running.
The test process itself runs above normal priority: the phones stand for devices on another machine,
and it is the daemon whose scheduling is under test.
"""
import argparse
import asyncio
import atexit
import os
import random
import statistics
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

PRIORITIES = {"idle": "IDLE_PRIORITY_CLASS", "below": "BELOW_NORMAL_PRIORITY_CLASS", "normal": "NORMAL_PRIORITY_CLASS", "above": "ABOVE_NORMAL_PRIORITY_CLASS"}

WORKER = r'''
import sys, time
duty, slice_s, seconds = float(sys.argv[1]), 0.1, float(sys.argv[2])
end = time.monotonic() + seconds
burst = float(sys.argv[3]) if len(sys.argv) > 3 else 0.0   # >0: rests for random stretches, then works
x = 0
while time.monotonic() < end:
    if burst:
        time.sleep(burst * (0.5 + __import__("random").random()))
        until = time.monotonic() + burst * duty * 2
        while time.monotonic() < until:
            x += 1
        continue
    began = time.monotonic()
    while time.monotonic() - began < slice_s * duty:
        x += 1
    rest = slice_s * (1 - duty) - (time.monotonic() - began - slice_s * duty)
    if rest > 0:
        time.sleep(rest)
'''


class Stress:
    """Busy workers and low-duty children, as processes. `start()` / `stop()`; stops what it started."""

    def __init__(self, threads=None, duty=1.0, priority="normal", children=20, child_duty=0.25, seconds=3600):
        self.threads = os.cpu_count() if threads is None else threads
        self.duty, self.priority, self.children, self.child_duty, self.seconds = duty, priority, children, child_duty, seconds
        self.procs = []
        atexit.register(self.stop)

    def _spawn(self, *args, priority):
        import psutil
        flags = getattr(subprocess, PRIORITIES[priority])
        p = subprocess.Popen([sys.executable, "-S", "-E", "-c", WORKER, *map(str, args)], creationflags=flags | subprocess.CREATE_NO_WINDOW, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.procs.append(psutil.Process(p.pid))
        self._popen = getattr(self, "_popen", []) + [p]

    def start(self):
        for _ in range(self.threads):
            self._spawn(self.duty, self.seconds, priority=self.priority)
        # Agents running builds: mostly waiting, in bursts of work, at the normal priority a child gets.
        for _ in range(self.children):
            self._spawn(self.child_duty, self.seconds, 0.4, priority="normal")
        return self

    def stop(self):
        for p in getattr(self, "_popen", []):
            if p.poll() is None:
                p.terminate()
        for p in getattr(self, "_popen", []):
            try:
                p.wait(5)
            except subprocess.TimeoutExpired:
                p.kill()
        self._popen = []
        self.procs = []


def percentile(values, q):
    values = sorted(values)
    return values[min(len(values) - 1, int(len(values) * q))] if values else None


# Every session first fills its scrollback (5000 lines), so that what the daemon holds for the session itself
# is full before memory is measured and only growth on the phones' account shows.
FILL = "for i in range(6000):\n    print('fill %05d %s' % (i, 'x' * 60))\n"

ECHO = r'''
import sys
''' + FILL + r'''print("echo ready", flush=True)
n = 0
for line in sys.stdin:
    n += 1
    print("ECHO %s" % line.strip(), flush=True)
'''

CHATTER = r'''
import sys, time
''' + FILL + r'''i = 0
end = time.time() + float(sys.argv[1])
while time.time() < end:
    i += 1
    print("chatter %s %06d %s" % (sys.argv[2], i, "x" * 60), flush=True)
    time.sleep(0.05)
'''


async def gate(args):
    import psutil
    import phone_support as ps

    me = psutil.Process()
    me.nice(psutil.ABOVE_NORMAL_PRIORITY_CLASS)
    exe = os.path.abspath(args.exe)
    ps.isolate("phone-stress")
    port = 50000 + os.getpid() % 1000
    base = f"http://127.0.0.1:{port}/"
    work = tempfile.mkdtemp(prefix="kelpie-stress-cwd-")
    echo = os.path.join(work, "echo.py")
    chatter = os.path.join(work, "chatter.py")
    open(echo, "w", encoding="utf-8").write(ECHO)
    open(chatter, "w", encoding="utf-8").write(CHATTER)
    total = args.minutes * 60
    results = []

    def check(name, ok, detail=""):
        results.append(bool(ok))
        print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail else ""), flush=True)

    d = ps.Daemon(exe).start()
    stress = Stress(threads=args.threads, duty=args.duty, priority=args.priority, seconds=total + 600)
    try:
        before = d.call("mobile.load")
        check("phone access off: the load probe is not running", before["running"] is False, before)
        d.call("mobile.enable", {"ip": "127.0.0.1", "port": port})
        on = d.call("mobile.load")
        check("phone access on: it runs", on["running"] is True, on)

        # Sessions: `echo` ones answer what is typed; a few print all the time.
        sessions = []
        for i in range(args.sessions):
            script = [chatter, str(total + 600), f"c{i}"] if i < 4 else [echo]
            s = d.call("session.create", {"kind": "custom", "cwd": work, "name": f"s{i}", "args": [sys.executable, "-S", "-E"] + script})
            sessions.append(s["id"])
        echoes = sessions[4:]
        await asyncio.sleep(3)

        # Phones: each follows five sessions (a mix), one device each; one writer among them types.
        phones = []
        stats = {"sent": 0, "answered": 0, "echoed": 0, "latency": [], "disconnects": 0}

        for i in range(args.phones):
            token, device = ps.pair(d, base, f"Phone {i}", True)
            p = ps.Phone(base, token)
            await p.open()
            mine = [sessions[i % 4]] + [echoes[(i * 2 + k) % len(echoes)] for k in range(4)]
            for sid in mine:
                await p.ask(type="sub", id=sid, height=40, focus=(sid == mine[-1]))
            p.mine, p.device_id = mine, device
            phones.append(p)

        stop = asyncio.Event()
        epoch = phones[0].hello()["daemon_epoch"]

        async def typist(p, n):
            """Types a unique word every 1-2 s into the session the phone is looking at (the console echoes it
            as it is typed, before any Enter) and times how long the echo takes to show on the phone."""
            sid = p.mine[-1]
            while not stop.is_set():
                await asyncio.sleep(random.uniform(1, 2))
                token = f"t{n}x{stats['sent']}"
                began = time.monotonic()
                stats["sent"] += 1
                reply = await p.ask(timeout=10, type="input", id=sid, text=token, enter=False, epoch=epoch)
                if reply and reply.get("type") == "ack":
                    stats["answered"] += 1
                seen = await ps.async_wait_for(lambda: token in p.text(sid), 10)
                if seen:
                    stats["echoed"] += 1
                    stats["latency"].append((time.monotonic() - began) * 1000)
                # Enter makes the program print the line, and the next word starts a fresh one.
                await p.ask(timeout=10, type="key", id=sid, key="enter", epoch=epoch)

        async def watch_connections():
            while not stop.is_set():
                await asyncio.sleep(1)
                for p in phones:
                    if p.closed is not None:
                        stats["disconnects"] += 1
                        print(f"phone {p.device_id} lost its connection: {p.closed}", flush=True)
                        p.closed = None

        # Revoke test: a throwaway device types in a loop; revoking it must stop its writes within a second.
        revokes = []

        async def revoker():
            while not stop.is_set():
                await asyncio.sleep(max(20, total / 12))
                if stop.is_set():
                    return
                for _ in range(5):
                    try:
                        token, device = await asyncio.to_thread(ps.pair, d, base, f"Revoked {len(revokes)}", True)
                        break
                    except RuntimeError:
                        await asyncio.sleep(6)  # pairing is paced while the machine is busy
                else:
                    return
                r = ps.Phone(base, token)
                await r.open()
                sid = random.choice(echoes)
                got = []

                async def spam():
                    n = 0
                    while True:
                        n += 1
                        reply = await r.ask(timeout=5, type="input", id=sid, text=f"zz{len(revokes)}-{n}", enter=False)
                        got.append((time.monotonic(), reply))
                        await asyncio.sleep(0.05)

                task = asyncio.ensure_future(spam())
                await asyncio.sleep(1)
                began = time.monotonic()
                await asyncio.to_thread(d.call, "mobile.revoke", {"id": device})
                await ps.async_wait_for(lambda: r.closed is not None, 5)
                took = time.monotonic() - began
                task.cancel()
                after = [t for t, reply in got if t > began + 1 and reply and reply.get("type") == "ack"]
                revokes.append((took, len(after)))
                await r.close()

        # ---- calm: baseline ----
        tasks = [asyncio.ensure_future(typist(p, i)) for i, p in enumerate(phones)] + [asyncio.ensure_future(watch_connections()), asyncio.ensure_future(revoker())]
        await asyncio.sleep(60)
        calm = list(stats["latency"])
        rss0 = ps.rss_mb(d.pid)
        print(f"calm: {len(calm)} echoes, p95 {percentile(calm, 0.95):.0f} ms, daemon RSS {rss0:.0f} MiB", flush=True)
        stats["latency"].clear()

        # ---- stress ----
        stress.start()
        began = time.monotonic()
        reached = None
        peak = "L0"
        before_level = (await asyncio.to_thread(d.call, "mobile.load"))["level"]
        next_rss = time.monotonic() + 60
        while time.monotonic() - began < total - 180:
            load = await asyncio.to_thread(d.call, "mobile.load")
            if reached is None and load["level"] >= 2:
                reached = time.monotonic() - began
            if load["level"] >= 1:
                peak = max(peak, f"L{load['level']}")
            if time.monotonic() >= next_rss:
                next_rss += 60
                print(f"  minute {(time.monotonic() - began) / 60 + 1:.0f}: RSS {ps.rss_mb(d.pid):.0f} MiB, level L{load['level']} {load['reason']}, host memory free {psutil.virtual_memory().available * 100 / psutil.virtual_memory().total:.0f} %", flush=True)
            await asyncio.sleep(0.5 if reached is None else 5)
        busy = list(stats["latency"])
        cpu_busy = psutil.cpu_percent(interval=1)
        stress.stop()
        stopped = time.monotonic()
        fell, left_by = None, None
        while time.monotonic() - stopped < 60:
            now = await asyncio.to_thread(d.call, "mobile.load")
            if now["level"] == 0:
                fell = time.monotonic() - stopped
                break
            left_by = now
            await asyncio.sleep(0.5)
        await asyncio.sleep(max(0, total - (time.monotonic() - began) - 60))
        stop.set()
        await asyncio.gather(*tasks, return_exceptions=True)
        rss1 = ps.rss_mb(d.pid)
        load = d.call("mobile.load")

        p95, p99 = percentile(busy, 0.95), percentile(busy, 0.99)
        print(f"stress: {len(busy)} echoes, p50 {statistics.median(busy):.0f} ms, p95 {p95:.0f} ms, p99 {p99:.0f} ms; peak level {peak}; lag p99 {load['lag_p99_ms']} ms, p99.9 {load['lag_p999_ms']} ms", flush=True)
        check("0 false disconnects", stats["disconnects"] == 0, stats["disconnects"])
        check("input echo p95 <= 300 ms under stress", p95 is not None and p95 <= 300, f"{p95:.0f}")
        check("input echo p99 <= 800 ms under stress", p99 is not None and p99 <= 800, f"{p99:.0f}")
        check("0 dropped input: every one answered, every line came back", stats["answered"] == stats["sent"] == stats["echoed"], {k: v for k, v in stats.items() if k != "latency"})
        check("revoking a device stops its writes within a second", bool(revokes) and all(t < 1.0 and n == 0 for t, n in revokes), revokes)
        check("daemon RSS growth <= 10 %", rss1 <= rss0 * 1.10, f"{rss0:.0f} -> {rss1:.0f} MiB")
        check("the level reaches L2 within 5 s of the stress starting", reached is not None and reached <= 5, reached)
        print(f"  (the level before the stress started: L{before_level})", flush=True)
        # A host that is short of memory by itself stays at L3 whatever the CPU does: that is the level doing its job.
        short = left_by is not None and left_by["reason"] == ["memory"] and (left_by["memory_free"] or 100) < 15
        if fell is None and short:
            print(f"  (still {left_by['level']} after 60 s only because host memory is {left_by['memory_free']:.0f} % free)", flush=True)
        check("…and is L0 within 25 s of it stopping", (fell is not None and fell <= 25) or short, fell)
        for p in phones:
            await p.close()
        d.call("mobile.disable")
        await asyncio.sleep(1.5)
        off = d.call("mobile.load")
        check("phone access off again: the probe is not running", off["running"] is False, off)
    finally:
        stress.stop()
        d.stop()
    failed = results.count(False)
    print(f"{len(results) - failed} passed, {failed} failed")
    return failed == 0


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("exe", nargs="?")
    ap.add_argument("--load-only", action="store_true")
    ap.add_argument("--minutes", type=float, default=30)
    ap.add_argument("--seconds", type=float, default=60)
    ap.add_argument("--sessions", type=int, default=20)
    ap.add_argument("--phones", type=int, default=8)
    ap.add_argument("--threads", type=int)
    ap.add_argument("--duty", type=float, default=1.0)
    ap.add_argument("--priority", choices=sorted(PRIORITIES), default="normal")
    args = ap.parse_args()
    if args.load_only:
        s = Stress(args.threads, args.duty, args.priority, seconds=args.seconds).start()
        print(f"{len(s.procs)} workers for {args.seconds} s", flush=True)
        time.sleep(args.seconds)
        s.stop()
        return 0
    if not args.exe:
        ap.error("the daemon executable, or --load-only")
    return 0 if asyncio.run(gate(args)) else 1


if __name__ == "__main__":
    sys.exit(main())
