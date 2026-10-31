"""The phone protocol on a bad network and on a busy computer, against a real daemon.

    python -X utf8 scripts/e2e/mobile_resilience.py <kelpied.exe> [--quick]

Daemon only (no app, no browser): its own KELPIE_ENDPOINT and data folder, phone access on 127.0.0.1, and
phones that behave like the page (`phone_support.Phone`) connected through `net_fault_proxy.FaultProxy`.
Windows (PowerShell sessions, ConPTY). Takes about six minutes; `--quick` skips the waits for 90 s.

1. Heartbeats: a phone that never answers is asked at 20 s and again every 8 s and dropped at 45 s (4410);
   one that answers stays; a half-open path (phone -> daemon dead, the other way alive) is dropped the same
   way and the phone is told; high latency and jitter do not disturb a healthy connection.
2. Blackholes of 10, 30 and 90 s: the first two are survived and the view catches up; the third is dropped
   by the daemon, the phone keeps its cached view readable, input typed meanwhile is never executed, and the
   reconnected phone's rows equal the terminal's; an answer sent meanwhile is retried once and lands once.
3. An answer whose reply was lost: retried on a new connection it is a duplicate, not a second answer.
4. A reset storm, then 100 connections killed mid-stream: the rows are identical after every reconnect.
5. A 20 KB/s phone while a session prints about 1 MB/s: the daemon's memory stays flat, no frame is over
   64 KiB, and the phone has caught up within 2 s of the throttle being lifted.
6. Answer results: another device's answer is `stale` and names who and when; a read-only device is `refused`.
7. A loaded computer (the load level pinned by KELPIE_LOAD_FORCE): L2 and L3 shed frame rate and size, pause
   background sessions at L3, tell the phone (`load.status`, `level` in frames), stretch the heartbeat
   (40 s / 90 s at L2) and pace pairing.
8. A message sent to a chat whose reply was lost (or that never got there): retried under the same `cid` on a new
   connection it is a duplicate (or lands once), and the agent never gets a second turn.
"""
import asyncio
import os
import random
import shutil
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import net_fault_proxy as nfp  # noqa: E402
import phone_support as ps  # noqa: E402

if len(sys.argv) < 2:
    raise SystemExit(__doc__)
exe = os.path.abspath(sys.argv[1])
QUICK = "--quick" in sys.argv
data_dir = ps.isolate("phone-resilience")
PORT = 51000 + os.getpid() % 1000
BASE = f"http://127.0.0.1:{PORT}/"

results = []


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail})" if detail and not ok else ""), flush=True)


work = tempfile.mkdtemp(prefix="kelpie-phone-resilience-cwd-")


def script(name, body):
    path = os.path.join(work, name)
    open(path, "w", encoding="utf-8").write(body)
    return path


# One numbered line every 100 ms.
TICKER = script("ticker.py", "import time\nfor i in range(100000):\n    print('T=%06d' % i, flush=True)\n    time.sleep(0.1)\n")
# Prints 60 numbered lines when it is sent a line, and waits for the next.
REPEATER = script("repeater.py", "import sys\nk = 0\nprint('repeater ready', flush=True)\nfor line in sys.stdin:\n    k += 1\n    for i in range(60):\n        print('B%03d-%02d' % (k, i), flush=True)\n")
# About 0.8 MB/s for the given seconds.
PRINTER = script("printer.py", (
    "import os, sys, time\nend = time.time() + float(sys.argv[1])\nn = 0\nwhile time.time() < end:\n    began = time.time()\n"
    "    for _ in range(500):\n        n += 1\n        sys.stdout.write('P %08d %s\\n' % (n, os.urandom(32).hex()))\n    sys.stdout.flush()\n"
    "    time.sleep(max(0, 0.05 - (time.time() - began)))\nprint('PRINTER-DONE', flush=True)\n"))
# A menu that waits for one answer.
MENU = script("menu.py", (
    "print('Bash(npm test)')\nprint(' Do you want to proceed?')\nprint(' \\u276f 1. Yes')\nprint('   2. Yes, and don\\'t ask again')\n"
    "print('   3. No (esc)')\nanswer = input()\nprint('ANSWERED=' + answer)\ninput()\n"))


def info(d, sid):
    return next((s for s in d.call("session.list") if s["id"] == sid), {})


def read(d, sid, lines=60):
    return d.call("session.read", {"handle": info(d, sid)["handle"], "lines": lines})["text"]


def custom(d, name, *args):
    return d.call("session.create", {"kind": "custom", "cwd": work, "name": name, "args": [sys.executable, *map(str, args)]})["id"]


def same_as_terminal(d, phone, sid, minimum=5):
    """The rows the phone holds are the end of the terminal's (blank lines aside)."""
    held = [line.rstrip() for line in phone.lines.get(sid, []) if line.strip()]
    truth = [line.rstrip() for line in read(d, sid, 200).split("\n") if line.strip()]
    return len(held) >= minimum and held == truth[-len(held):]


def ticker_of(text):
    ticks = [int(line[2:8]) for line in text.split("\n") if line.startswith("T=") and line[2:8].isdigit()]
    return ticks[-1] if ticks else None


async def connect(base, token, sid=None, old=None, **kw):
    """A phone on `base`; it carries on from what `old` (a connection that ended) had acknowledged for `sid`. A known device may
    handshake 5 times in 10 s (4429 after that, and the page tries again later): this waits for the daemon to let it in."""
    for attempt in range(20):
        phone = ps.Phone(base, token, **kw)
        try:
            await phone.open()
            break
        except ConnectionError:
            if phone.closed != 4429 or attempt == 19:
                raise
            await asyncio.sleep(1)
    if old is not None:
        phone.resume_from(old)
    if sid:
        await phone.ask(type="sub", id=sid, height=50, have_rev=phone.acked.get(sid))
    return phone


async def main(d):
    d.call("mobile.enable", {"ip": "127.0.0.1", "port": PORT})
    devices = {name: ps.pair(d, BASE, name, True) for name in ("Silent", "Half", "Latent", "Legacy", "B10", "B30", "B90", "Kill", "Slow", "Alice", "Bob", "ChatA", "ChatB")}
    ticker = custom(d, "ticker", TICKER)
    await ps.async_wait_for(lambda: "T=0000" in read(d, ticker), 60)

    async def proxy(**faults):
        p = nfp.FaultProxy(PORT)
        port = await p.start()
        for k, v in faults.items():
            setattr(p, k, v)
        return p, f"http://127.0.0.1:{port}/"

    started = time.monotonic()
    # ---------- 1 and 2. heartbeats and blackholes, side by side ----------
    async def silent():
        token, _ = devices["Silent"]
        p = await connect(BASE, token, None, auto_hb=False)  # follows nothing: it has nothing to acknowledge
        t0 = time.monotonic()
        await ps.async_wait_for(lambda: p.closed is not None, 60)
        beats = [round(m["_t"] - t0) for m in p.log if m["type"] == "hb"]
        check("a phone that never answers is asked at 20 s and every 8 s after", len(beats) >= 3 and all(abs(a - b) <= 1 for a, b in zip(beats[:3], [20, 28, 36])), beats)
        check("…and dropped at 45 s with 4410", p.closed == 4410 and 44 <= time.monotonic() - t0 <= 49, (p.closed, round(time.monotonic() - t0, 1)))
        await p.close()

    async def old_page():
        # A page from before frames were acknowledged: no caps, no acknowledgements, no heartbeat answers.
        token, _ = devices["Legacy"]
        p = await connect(BASE, token, ticker, auto_ack=False, auto_hb=False, caps=False)
        await asyncio.sleep(14 if QUICK else 56)
        frames = [m for m in p.log if m["type"] == "screen"]
        check("a page that never acknowledges keeps getting frames and is not dropped", p.closed is None and len(frames) > 30 and not p.lost, (p.closed, len(frames), len(p.lost)))
        check("…and shows the session as it is", ticker_of(p.text(ticker)) is not None and ticker_of(p.text(ticker)) >= ticker_of(read(d, ticker)) - 30)
        await p.close()

    async def half_open():
        token, _ = devices["Half"]
        px, base = await proxy()
        p = await connect(base, token, ticker)
        px.stall("up", 300)
        t0 = time.monotonic()
        await ps.async_wait_for(lambda: p.closed is not None, 60)
        check("a half-open path (the phone cannot be heard) is dropped at 45 s and the phone is told", p.closed == 4410 and 44 <= time.monotonic() - t0 <= 49, (p.closed, round(time.monotonic() - t0, 1)))
        await p.close()
        await px.stop()

    async def latent():
        token, _ = devices["Latent"]
        px, base = await proxy(latency=0.3, jitter=0.1)
        p = await connect(base, token, ticker)
        t0 = time.monotonic()
        await asyncio.sleep(20 if QUICK else 58)
        # It acknowledges the ticker's frames all along, so the daemon never has to ask it anything.
        check("300 ms latency with jitter: a healthy connection is not dropped and loses no frame", p.closed is None and not p.lost, (p.closed, len(p.lost)))
        check("…and still shows the session", ticker_of(p.text(ticker)) is not None and ticker_of(p.text(ticker)) >= ticker_of(read(d, ticker)) - 30, (ticker_of(p.text(ticker)), ticker_of(read(d, ticker))))
        await p.close()
        await px.stop()

    async def blackhole(name, seconds, drops):
        token, _ = devices[name]
        px, base = await proxy()
        p = await connect(base, token, ticker)
        await ps.async_wait_for(lambda: ticker_of(p.text(ticker)) is not None, 5)
        await asyncio.sleep(1)
        px.blackhole(seconds)
        await asyncio.sleep(0.5)
        before = p.text(ticker)
        stale_input = f"STALE-{name}"
        await p.send(type="input", id=ticker, text=stale_input, enter=True)
        await asyncio.sleep(seconds - 2)
        readable = p.text(ticker) == before and bool(before.strip())
        await asyncio.sleep(2.5)
        if not drops:
            survived = p.closed is None
            caught_up = await ps.async_wait_for(lambda: ticker_of(p.text(ticker)) and ticker_of(p.text(ticker)) >= ticker_of(read(d, ticker)) - 15, 4)
            check(f"a {seconds} s blackhole is survived and the view catches up within seconds", survived and caught_up, (p.closed, ticker_of(p.text(ticker)), ticker_of(read(d, ticker))))
            await p.close()
            await px.stop()
            return
        check(f"during a {seconds} s blackhole the cached view stays readable", readable)
        await ps.async_wait_for(lambda: p.closed is not None, 10)
        check("…the daemon dropped the silent phone (4410) and the phone learns it when the path returns", p.closed == 4410, p.closed)
        # The sent input was held by the dead path and the connection it was meant for was gone.
        await asyncio.sleep(1)
        check("input typed into a blackhole is never executed afterwards", stale_input not in read(d, ticker, 200))
        # Reconnect: carries on from the last acknowledged frame, and shows the terminal.
        q = await connect(BASE, token, ticker, old=p)
        ok = await ps.async_wait_for(lambda: ticker_of(q.text(ticker)) and ticker_of(q.text(ticker)) >= ticker_of(read(d, ticker)) - 10, 5)
        check("the reconnected phone's rows are the terminal's", ok and not q.lost, (ticker_of(q.text(ticker)), ticker_of(read(d, ticker)), len(q.lost)))
        await q.close()
        await p.close()
        await px.stop()

    async def lost_answer_in_blackhole():
        # An answer sent into a blackhole is lost; the phone retries it on a new connection and it lands once.
        token, _ = devices["Kill"]
        menu = custom(d, "menu-lost", MENU)
        px, base = await proxy()
        p = await connect(base, token, menu)
        w = await p.wait(lambda m: m["type"] == "screen" and m["id"] == menu and m["prompt"] and m["waiting_since"], 30)
        since = w["waiting_since"]
        px.blackhole(300)
        await p.send(type="answer", id=menu, waiting_since=since, text="1", enter=True, req="lost")
        await asyncio.sleep(1)
        check("an answer sent into a blackhole has not reached the program", "ANSWERED=" not in read(d, menu))
        px.kill_all()
        await px.stop()
        q = await connect(BASE, token, menu, old=p)
        r = await q.ask(type="answer", id=menu, waiting_since=since, text="1", enter=True)
        check("the retry on a new connection lands", r["type"] == "ack" and r["result"] == "ok", r)
        await ps.async_wait_for(lambda: "ANSWERED=1" in read(d, menu), 15)
        check("…and the program got exactly one answer", read(d, menu).count("ANSWERED=") == 1)
        again = await q.ask(type="answer", id=menu, waiting_since=since, text="1", enter=True)
        check("a second retry is a duplicate", again["type"] == "error" and again["code"] == "dup" and again["result"] == "dup", again)
        await q.close()
        await p.close()

    async def reply_lost():
        # The answer reached the daemon but its reply did not: the phone cannot tell, retries, and is told it is a duplicate.
        token, _ = devices["Alice"]
        menu = custom(d, "menu-reply", MENU)
        px, base = await proxy()
        p = await connect(base, token, menu)
        w = await p.wait(lambda m: m["type"] == "screen" and m["id"] == menu and m["prompt"] and m["waiting_since"], 30)
        since = w["waiting_since"]
        px.stall("down", 300)
        await p.send(type="answer", id=menu, waiting_since=since, text="1", enter=True, req="r1")
        await ps.async_wait_for(lambda: "ANSWERED=1" in read(d, menu), 15)
        check("an answer whose reply is lost has reached the program", "ANSWERED=1" in read(d, menu))
        px.kill_all()
        await px.stop()
        q = await connect(BASE, token, menu, old=p)
        retry = await q.ask(type="answer", id=menu, waiting_since=since, text="1", enter=True)
        check("its retry is a duplicate, not a second answer", retry["type"] == "error" and retry["result"] == "dup" and read(d, menu).count("ANSWERED=") == 1, retry)
        await q.close()
        await p.close()

    def new_turn_chat(name):
        # A chat whose agent takes one turn at a time and traces what it is given (phone_support.TURN_AGENT_SOURCE).
        folder = os.path.join(work, name)
        os.makedirs(folder, exist_ok=True)
        agent, trace, hold = ps.write_turn_agent(folder)
        chat = d.call("session.create", {"kind": "claude", "cwd": work, "chat_argv": [sys.executable, "-u", agent, trace, hold], "name": name})
        ps.wait_for(lambda: info(d, chat["id"]).get("alive"), 20)
        arrived = lambda: [t[len("ARRIVE "):] for t in (open(trace, encoding="utf-8").read().splitlines() if os.path.exists(trace) else []) if t.startswith("ARRIVE ")]  # noqa: E731
        return chat, arrived

    async def chat_reply_lost():
        # A message reached the daemon but its reply did not: the phone cannot tell, retries under the same cid on a new
        # connection, and is told it is a duplicate; the agent was given the message once.
        token, _ = devices["ChatA"]
        chat, arrived = new_turn_chat("send-reply-lost")
        px, base = await proxy()
        p = await connect(base, token)
        await p.ask(type="chat_sub", id=chat["id"])
        px.stall("down", 300)
        await p.send(type="chat_send", id=chat["id"], cid="lost-reply", text="hello once", req="c1")
        await ps.async_wait_for(lambda: arrived() == ["hello once"], 15)
        check("a chat message whose reply is lost has reached the agent", arrived() == ["hello once"], arrived())
        px.kill_all()
        await px.stop()
        q = await connect(BASE, token)
        mark = q.mark()
        await q.ask(type="chat_sub", id=chat["id"])
        seen = await q.wait(lambda m: m["type"] == "chat" and m["id"] == chat["id"] and any('"kelpie_sent"' in line and "lost-reply" in line for line in m["lines"]), 10, mark)
        check("the log shows the phone it was sent, even though the reply was lost", seen is not None)
        retry = await q.ask(type="chat_send", id=chat["id"], cid="lost-reply", text="hello once")
        check("its retry is a duplicate that says it was sent", retry["type"] == "ack" and retry["result"] == "dup" and retry["state"] == "sent", retry)
        await asyncio.sleep(1)
        check("…and the agent was given one turn", arrived() == ["hello once"], arrived())
        await q.close()
        await p.close()

    async def chat_send_in_blackhole():
        # A message sent into a blackhole never gets there: the retry on a new connection lands once.
        token, _ = devices["ChatB"]
        chat, arrived = new_turn_chat("send-blackhole")
        px, base = await proxy()
        p = await connect(base, token)
        await p.ask(type="chat_sub", id=chat["id"])
        px.blackhole(300)
        await p.send(type="chat_send", id=chat["id"], cid="lost-send", text="into the void", req="c1")
        await asyncio.sleep(1.5)
        check("a chat message sent into a blackhole has not reached the agent", arrived() == [], arrived())
        px.kill_all()
        await px.stop()
        q = await connect(BASE, token)
        first = await q.ask(type="chat_send", id=chat["id"], cid="lost-send", text="into the void")
        check("the retry on a new connection lands", first["type"] == "ack" and first["result"] == "ok", first)
        again = await q.ask(type="chat_send", id=chat["id"], cid="lost-send", text="into the void")
        # The agent is a process of its own: on a busy machine it reads what it was written later than a fixed pause.
        await ps.async_wait_for(arrived, 20)
        await asyncio.sleep(1)
        check("a second retry is a duplicate and the agent got one turn", again["result"] == "dup" and arrived() == ["into the void"], (again, arrived()))
        await q.close()
        await p.close()

    quick_note = " (quick: 90 s blackhole skipped)" if QUICK else ""
    jobs = [old_page(), latent(), blackhole("B10", 10, False), blackhole("B30", 30, False), lost_answer_in_blackhole(), reply_lost(), chat_reply_lost(), chat_send_in_blackhole()]
    if not QUICK:
        jobs += [silent(), half_open(), blackhole("B90", 90, True)]
    for outcome in await asyncio.gather(*jobs, return_exceptions=True):
        if isinstance(outcome, BaseException):
            import traceback
            traceback.print_exception(outcome)
            check("a scenario ran to completion", False, repr(outcome))
    print(f"part 1 and 2 took {time.monotonic() - started:.0f} s{quick_note}", flush=True)

    # ---------- 4. reset storm, then 100 kills ----------
    token, _ = devices["Kill"]
    repeater = custom(d, "repeater", REPEATER)
    await ps.async_wait_for(lambda: "repeater ready" in read(d, repeater), 30)
    px, base = await proxy()
    p = await connect(base, token, repeater)
    await ps.async_wait_for(lambda: p.lines.get(repeater), 5)
    storm = px.rst_storm(0.5, 10)
    reconnects = 0
    while not storm.done():
        if p.closed is not None:
            try:
                p = await connect(base, token, repeater, old=p)
                reconnects += 1
            except Exception:  # noqa: BLE001
                await asyncio.sleep(0.2)
        await asyncio.sleep(0.1)
    await asyncio.sleep(0.5)
    p = p if p.closed is None else await connect(base, token, repeater, old=p)
    d.call("session.input", {"handle": info(d, repeater)["handle"], "data": "go\r"})
    ok = await ps.async_wait_for(lambda: same_as_terminal(d, p, repeater) and "B001-59" in p.text(repeater), 8)
    check(f"a reset storm ({reconnects} reconnects in 10 s) leaves the phone with the terminal's rows", ok and not p.lost, (reconnects, len(p.lost)))

    mismatches, lost = 0, 0
    cycles = 30 if QUICK else 100
    for n in range(2, cycles + 2):
        if p.closed is not None:
            p = await connect(base, token, repeater, old=p)
        await ps.async_wait_for(lambda: p.acked.get(repeater) == p.rev.get(repeater), 3)
        d.call("session.input", {"handle": info(d, repeater)["handle"], "data": "go\r"})
        await asyncio.sleep(random.uniform(0, 0.15))
        px.kill_all()
        await asyncio.sleep(0.05)
        p = await connect(base, token, repeater, old=p)
        if not await ps.async_wait_for(lambda: same_as_terminal(d, p, repeater) and f"B{n:03d}-59" in p.text(repeater), 8):
            mismatches += 1
            print(f"cycle {n}: held {p.lines.get(repeater, [])[-3:]} / terminal {read(d, repeater, 3)!r}", flush=True)
        lost += len(p.lost)
        p.lost.clear()
    check(f"{cycles} connections killed mid-stream: the rows are identical after every reconnect", mismatches == 0 and lost == 0, (mismatches, lost))
    await p.close()
    await px.stop()

    # ---------- 5. a slow phone ----------
    token, _ = devices["Slow"]
    printer = custom(d, "flood", PRINTER, 30)
    px, base = await proxy()
    px.bandwidth["down"] = 20_000
    # The session's own buffers fill first; what the phone adds is measured after that.
    await asyncio.sleep(6)
    p = await connect(base, token, printer)
    rss0 = ps.rss_mb(d.pid)
    await asyncio.sleep(10)
    rss_mid = ps.rss_mb(d.pid)
    await ps.async_wait_for(lambda: "PRINTER-DONE" in read(d, printer), 40)
    await asyncio.sleep(1)
    rss1 = ps.rss_mb(d.pid)
    biggest = max((m["_size"] for m in p.log if m["type"] == "screen"), default=0)
    ingested = info(d, printer)["seq"] / 1e6
    check("a 20 KB/s phone under ~1 MB/s of output: the daemon's memory stays flat", rss1 <= rss0 * 1.10 + 5, f"{rss0:.0f} -> {rss_mid:.0f} -> {rss1:.0f} MiB after {ingested:.1f} MB")
    check("…and no frame is over 64 KiB", 0 < biggest < 64 * 1024, biggest)
    px.bandwidth["down"] = None
    lifted = time.monotonic()
    ok = await ps.async_wait_for(lambda: same_as_terminal(d, p, printer) and "PRINTER-DONE" in p.text(printer), 10)
    took = time.monotonic() - lifted
    check("…and it has caught up within 2 s of the throttle being lifted", ok and took <= 2.0, round(took, 2))
    await p.close()
    await px.stop()

    # ---------- 6. answer results across devices ----------
    menu = custom(d, "menu-two", MENU)
    (alice_token, _), (bob_token, _) = devices["Alice"], devices["Bob"]
    alice = await connect(BASE, alice_token, menu)
    w = await alice.wait(lambda m: m["type"] == "screen" and m["id"] == menu and m["prompt"] and m["waiting_since"], 30)
    since = w["waiting_since"]
    # One time: "2" here is "Yes, and don't ask again", which only the computer may choose (`one_shot`).
    first = await alice.ask(type="answer", id=menu, waiting_since=since, text="1")
    bob = await connect(BASE, bob_token, menu)
    second = await bob.ask(type="answer", id=menu, waiting_since=since, text="1")
    check("another device's late answer is stale and names who answered and when",
          first["result"] == "ok" and second["result"] == "stale" and second["answered_by"] == "Alice" and abs(second["at"] - time.time() * 1000) < 60000, (first, second))
    reader_token, _ = ps.pair(d, BASE, "Reader", False)
    reader = await connect(BASE, reader_token)
    refused = await reader.ask(type="answer", id=menu, waiting_since=since, text="1")
    check("a read-only device's answer is refused", refused["type"] == "error" and refused["result"] == "refused" and refused["code"] == "read_only", refused)
    for x in (alice, bob, reader):
        await x.close()

    d.call("mobile.disable")
    await asyncio.sleep(1.5)
    check("with phone access off the load probe is not running", d.call("mobile.load")["running"] is False)


async def loaded(d, level):
    """The daemon restarted with the load pinned at `level`: what a phone sees."""
    for _ in range(20):  # the previous daemon may still be letting go of the port
        try:
            d.call("mobile.enable", {"ip": "127.0.0.1", "port": PORT})
            break
        except (Exception, SystemExit):  # noqa: BLE001  (the call helper exits on an error reply)
            await asyncio.sleep(1)
    token, _ = ps.pair(d, BASE, f"Loaded {level}", True)
    printer = custom(d, f"flood-{level}", PRINTER, 40)
    other = custom(d, f"other-{level}", PRINTER, 40)
    p = ps.Phone(BASE, token)
    hello = await p.open()
    want = int(level[1])
    check(f"{level}: hello and load.status say the level and why", hello["load"]["level"] == want and hello["load"]["reason"] == ["thermal"], hello["load"])
    await p.ask(type="sub", id=printer, height=50, focus=True)
    await p.ask(type="sub", id=other, height=50, focus=False)
    mark = p.mark()
    await asyncio.sleep(6)
    frames = [m for m in p.log[mark:] if m["type"] == "screen"]
    mine = [m for m in frames if m["id"] == printer]
    background = [m for m in frames if m["id"] == other]
    status = [m for m in p.log[mark:] if m["type"] == "load.status"]
    rate = len(mine) / 6
    limit = {2: 4.5, 3: 1.5}[want]
    check(f"{level}: the focused session gets at most {limit:g} frames a second", 0 < rate <= limit, round(rate, 2))
    if want == 3:
        check("L3: a background session is paused", len(background) <= 1, len(background))
    else:
        check("L2: a background session gets at most one frame a second", len(background) / 6 <= 1.5, round(len(background) / 6, 2))
    cap = {2: 32, 3: 16}[want] * 1024
    check(f"{level}: no frame is over {cap // 1024} KiB", mine and max(m["_size"] for m in frames) <= cap, max((m["_size"] for m in frames), default=0))
    check(f"{level}: frames carry the level and the server time", all(m["level"] == want and m["server_ts"] > 0 for m in frames))
    check(f"{level}: load.status arrives every second while loaded", len(status) >= 4 and all(m["level"] == want and m["reason"] == ["thermal"] for m in status), len(status))
    ack = await p.ask(type="input", id=printer, text="x", enter=False, force=True)
    check(f"{level}: typing is never shed", ack["type"] == "ack", ack)
    # Pairing is paced: the second handshake within the gap is refused without spending its code.
    links = [d.call("mobile.pair_start")["url"] for _ in range(2)]
    first = await ps.PairAttempt(BASE, links[1], "Pace").start()
    await asyncio.sleep(0.4)
    second = await ps.PairAttempt(BASE, links[0], "Pace too").start()
    check(f"{level}: a second pairing handshake within {1 if want == 2 else 5} s is refused as busy", second.closed == 4503, (first.closed, second.closed, second.silent))
    pending = d.call("mobile.status")["pending"]
    for item in pending:
        d.call("mobile.approve", {"id": item["id"], "allow": False})
    if first.session:
        await first.outcome(10)
    if want == 2 and not QUICK:
        # The heartbeat is twice as patient: nothing for 40 s, then the first.
        await asyncio.sleep(1.2)  # pairing is paced at L2
        quiet = ps.Phone(BASE, ps.pair(d, BASE, "Patient", True)[0], auto_hb=False)
        await quiet.open()
        t0 = time.monotonic()
        beat = await quiet.wait(lambda m: m["type"] == "hb", 50)
        check("L2: the first heartbeat comes after 40 s, not 20", beat is not None and 39 <= beat["_t"] - t0 <= 43, beat and round(beat["_t"] - t0, 1))
        await ps.async_wait_for(lambda: quiet.closed is not None, 60)
        check("L2: and the silent phone is dropped at 90 s, not 45", quiet.closed == 4410 and 88 <= time.monotonic() - t0 <= 93, (quiet.closed, round(time.monotonic() - t0, 1)))
        await quiet.close()
    await p.close()


async def resume_under(d, level, reason, resumes):
    """A phone that reconnects under load (pinned `level:reason`): from the last frame it acknowledged, or,
    when the shortage is memory, from a whole screen (what was kept for away phones was dropped)."""
    for _ in range(20):
        try:
            d.call("mobile.enable", {"ip": "127.0.0.1", "port": PORT})
            break
        except (Exception, SystemExit):  # noqa: BLE001
            await asyncio.sleep(1)
    token, _ = ps.pair(d, BASE, f"Resume {level} {reason}", True)
    ticker = custom(d, f"ticker-{level}-{reason}", TICKER)
    await ps.async_wait_for(lambda: "T=0000" in read(d, ticker), 60)
    first = await connect(BASE, token, ticker, auto_ack=True)
    await ps.async_wait_for(lambda: first.rev.get(ticker) and first.acked.get(ticker) == first.rev.get(ticker), 15)
    # No more acknowledgements from here, and the daemon has read the last one: its answer to a
    # heartbeat comes after it has handled everything sent before.
    first.auto_ack = False
    await first.ask(type="hb")
    last = first.acked.get(ticker)
    await first.close()
    # Longer than a second: the connection's tick, where the daemon clears what it keeps for away phones.
    await asyncio.sleep(2.5)
    again = await connect(BASE, token, ticker, old=first)
    frame = await again.wait(lambda m: m["type"] == "screen" and m["id"] == ticker, 15)
    if resumes:
        check(f"{level} ({reason}): a reconnect carries on from the last frame it acknowledged", frame is not None and last is not None and frame["base"] == last, frame and (frame["base"], last))
    else:
        check(f"{level} ({reason}): the saved positions are gone and the whole screen comes back", frame is not None and frame["base"] is None, frame and frame["base"])
    await again.close()


def run():
    # Pinned calm: the heartbeat times below are the calm ones, whatever else this machine is doing.
    d = ps.Daemon(exe, env={"KELPIE_LOAD_FORCE": "L0:calm"}).start()
    try:
        asyncio.run(main(d))
    except Exception:
        import traceback
        traceback.print_exc()
        check("calm scenarios ran to completion", False)
    finally:
        d.stop()
    for level in ("L2", "L3"):
        d = ps.Daemon(exe, env={"KELPIE_LOAD_FORCE": f"{level}:thermal"}).start()
        try:
            asyncio.run(loaded(d, level))
            asyncio.run(resume_under(d, level, "thermal", True))
        except Exception:
            import traceback
            traceback.print_exc()
            check(f"{level} scenarios ran to completion", False)
        finally:
            d.stop()
    # L3 because memory ran out: the one load that clears what is kept for phones that are away.
    d = ps.Daemon(exe, env={"KELPIE_LOAD_FORCE": "L3:memory"}).start()
    try:
        asyncio.run(resume_under(d, "L3", "memory", False))
    except Exception:
        import traceback
        traceback.print_exc()
        check("L3 (memory) scenario ran to completion", False)
    finally:
        d.stop()


try:
    run()
finally:
    shutil.rmtree(work, ignore_errors=True)
    shutil.rmtree(data_dir, ignore_errors=True)
failed = [r for r in results if not r[1]]
print(f"{len(results) - len(failed)} passed, {len(failed)} failed")
sys.exit(1 if failed else 0)
