#!/usr/bin/env python3
"""Measures the Kelpie relay from this machine's network. Random bytes only, never Kelpie data.

    python scripts/perf/relay_probe.py desktop --url URL --secret-file F --room HEX
    python scripts/perf/relay_probe.py phone   --url URL --secret-file F --room HEX --measure rtt
    python scripts/perf/relay_probe.py phone   --url URL --secret-file F --measure colo
    python scripts/perf/relay_probe.py phone   --url URL --secret-file F --measure connect
    python scripts/perf/relay_probe.py phone   --url URL --secret-file F --measure hibernation
    python scripts/perf/relay_probe.py phone   --url URL --secret-file F --measure abuse
    python scripts/perf/relay_probe.py soak    --url URL --secret-file F --log-dir DIR
    python scripts/perf/relay_probe.py soak    --log-dir DIR --summary
    python scripts/perf/relay_probe.py analytics --hours 24

--url takes https://... for a deployed relay or http://127.0.0.1:8787 for a local `workerd`.
--secret-file holds the relay's TICKET_SECRET, or set KELPIE_RELAY_TICKET_SECRET.
`analytics` reads CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID from the environment.

An echo RTT needs a `desktop` process on the same --room: phone to relay to desktop and back.
Needs the `websockets` package (pip install websockets).
"""
import argparse
import asyncio
import base64
import collections
import ctypes
import datetime
import hashlib
import hmac
import json
import os
import random
import secrets
import socket
import ssl
import struct
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path
from urllib.parse import urlsplit

try:
    from websockets.asyncio.client import connect
    from websockets.exceptions import ConnectionClosed
except ImportError:  # `--help` still works without the dependency
    connect = None
    ConnectionClosed = Exception

KEEPALIVE_S = 20
PONG_DEADLINE_S = 10
BACKOFF_CAP_S = 60
STABLE_RESET_S = 30
SLEEP_TICK_S = 5
SLEEP_MIN_S = 5
STALL_MIN_S = 30
AFTER_SLEEP_GRACE_S = 120
NET_CHECK_TTL_S = 30
LOG_CAP_BYTES = 45 * 1024 * 1024
ROOM_SOCKETS = 5  # one desktop and four phones


def ticket_for(secret, room):
    mac = hmac.new(secret.encode(), room.encode(), hashlib.sha256).digest()
    return f"{room}.{base64.urlsafe_b64encode(mac).rstrip(b'=').decode()}"


def load_secret(args):
    if getattr(args, "secret_file", None):
        return Path(args.secret_file).read_text(encoding="utf-8").strip()
    secret = os.environ.get("KELPIE_RELAY_TICKET_SECRET", "")
    if not secret:
        sys.exit("no ticket secret: use --secret-file or KELPIE_RELAY_TICKET_SECRET")
    return secret


def percentile(sorted_values, pct):
    """Nearest rank."""
    if not sorted_values:
        return None
    rank = max(1, min(len(sorted_values), round(pct / 100 * len(sorted_values) + 0.4999)))
    return sorted_values[rank - 1]


def spread(values):
    values = sorted(values)
    if not values:
        return {"n": 0}
    return {
        "n": len(values),
        "min": round(values[0], 1),
        "p50": round(percentile(values, 50), 1),
        "p95": round(percentile(values, 95), 1),
        "p99": round(percentile(values, 99), 1),
        "max": round(values[-1], 1),
    }


def ms_since(start):
    return round((time.perf_counter() - start) * 1000, 1)


class Relay:
    def __init__(self, url, secret):
        parts = urlsplit(url)
        self.secure = parts.scheme in ("https", "wss")
        self.host = parts.hostname
        self.port = parts.port or (443 if self.secure else 80)
        self.netloc = parts.netloc
        self.secret = secret
        self.tls = ssl.create_default_context() if self.secure else None

    async def tcp(self, timeout=10):
        """An open non-blocking socket, with the DNS and TCP times."""
        loop = asyncio.get_running_loop()
        start = time.perf_counter()
        infos = await asyncio.wait_for(
            loop.getaddrinfo(self.host, self.port, type=socket.SOCK_STREAM), timeout
        )
        dns_ms = ms_since(start)
        failure = OSError("no address")
        for family, kind, proto, _, address in infos:
            sock = socket.socket(family, kind, proto)
            sock.setblocking(False)
            try:
                begun = time.perf_counter()
                await asyncio.wait_for(loop.sock_connect(sock, address), timeout)
                sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                return sock, dns_ms, ms_since(begun)
            except (OSError, asyncio.TimeoutError) as error:
                sock.close()
                failure = error
        raise failure

    async def dial(self, role, room, timeout=15):
        """A connected socket and its timings, up to the first frame back from the relay."""
        start = time.perf_counter()
        sock, dns_ms, tcp_ms = await self.tcp()
        scheme = "wss" if self.secure else "ws"
        uri = f"{scheme}://{self.netloc}/v1/{role}?t={ticket_for(self.secret, room)}"
        options = {"ssl": self.tls, "server_hostname": self.host} if self.secure else {}
        try:
            ws = await connect(
                uri,
                sock=sock,
                compression=None,
                ping_interval=None,
                open_timeout=timeout,
                close_timeout=2,
                max_size=1 << 17,
                max_queue=16,
                proxy=None,
                user_agent_header="kelpie-relay-probe",
                **options,
            )
        except BaseException:
            sock.close()
            raise
        upgrade_ms = ms_since(start)
        await ws.send("ping")
        await expect_text(ws, "pong", timeout)
        timings = {
            "dns_ms": dns_ms,
            "tcp_ms": tcp_ms,
            "upgrade_ms": upgrade_ms,
            "first_frame_ms": ms_since(start),
        }
        return ws, timings


async def expect_text(ws, wanted, timeout):
    async with asyncio.timeout(timeout):
        while True:
            message = await ws.recv()
            if message == wanted:
                return


async def ask_json(ws, command, timeout=15):
    await ws.send(command)
    async with asyncio.timeout(timeout):
        while True:
            message = await ws.recv()
            if isinstance(message, str) and message.startswith("{"):
                return json.loads(message)


def close_details(error):
    """Close code, reason and the browser's `wasClean` for a socket that went down."""
    received = error.rcvd
    sent = error.sent
    code = received.code if received else 1006
    reason = received.reason if received else ""
    return code, reason, bool(received and sent)


def random_room():
    return secrets.token_hex(16)


# ---- desktop mode ----


async def run_desktop(args):
    relay = Relay(args.url, load_secret(args))
    deadline = time.monotonic() + args.seconds if args.seconds else None
    echoed = 0
    while deadline is None or time.monotonic() < deadline:
        try:
            ws, timings = await relay.dial("desktop", args.room)
            print(json.dumps({"event": "desktop_open", **timings}), flush=True)
            next_ping = time.monotonic() + KEEPALIVE_S
            while deadline is None or time.monotonic() < deadline:
                try:
                    wait = max(0.05, next_ping - time.monotonic())
                    message = await asyncio.wait_for(ws.recv(), wait)
                except asyncio.TimeoutError:
                    await ws.send("ping")
                    next_ping = time.monotonic() + KEEPALIVE_S
                    continue
                if isinstance(message, bytes):
                    await ws.send(message)  # the slot prefix routes it back to the sender
                    echoed += 1
            await ws.close()
        except (ConnectionClosed, OSError, asyncio.TimeoutError) as error:
            print(json.dumps({"event": "desktop_down", "error": repr(error)}), flush=True)
            await asyncio.sleep(random.uniform(0, 2))
    print(json.dumps({"event": "desktop_done", "echoed": echoed}), flush=True)


# ---- phone mode ----


async def measure_rtt(args, relay):
    ws, timings = await relay.dial("phone", args.room)
    print(json.dumps({"event": "phone_open", **timings}), flush=True)
    # An unanswered first frame means the desktop process is not attached yet.
    for _ in range(30):
        await ws.send(b"\0" * 8)
        try:
            async with asyncio.timeout(1):
                while not isinstance(await ws.recv(), bytes):
                    pass
            break
        except asyncio.TimeoutError:
            pass
    else:
        sys.exit("no echo after 30 s: start `desktop` on the same --room first")

    results = {}
    seq = 1
    for size in args.sizes:
        rtts, lost, corrupt = [], 0, 0
        for _ in range(args.samples):
            seq += 1
            frame = struct.pack(">Q", seq) + os.urandom(size - 8)
            start = time.perf_counter()
            await ws.send(frame)
            try:
                async with asyncio.timeout(args.sample_timeout):
                    while True:
                        reply = await ws.recv()
                        if isinstance(reply, bytes) and reply[:8] == frame[:8]:
                            break
                rtts.append((time.perf_counter() - start) * 1000)
                corrupt += reply != frame
            except asyncio.TimeoutError:
                lost += 1
            await asyncio.sleep(args.gap_ms / 1000)
        results[f"{size}B"] = {**spread(rtts), "lost": lost, "corrupt": corrupt}
    await ws.close()
    return {"measure": "rtt", "at": utc_now(), "results": results}


async def measure_connect(args, relay):
    timings = collections.defaultdict(list)
    failures = collections.Counter()
    for _ in range(args.samples):
        try:
            ws, sample = await relay.dial("phone", random_room() if not args.room else args.room)
            for name, value in sample.items():
                timings[name].append(value)
            await ws.close()
        except (ConnectionClosed, OSError, asyncio.TimeoutError) as error:
            failures[type(error).__name__] += 1
    return {
        "measure": "connect",
        "at": utc_now(),
        "results": {name: spread(values) for name, values in timings.items()},
        "failures": dict(failures),
    }


async def measure_colo(args, relay):
    rows = []
    for _ in range(args.samples):
        ws, timings = await relay.dial("desktop", random_room())
        where = await ask_json(ws, "colo")
        await ws.close()
        rows.append({**where, "first_frame_ms": timings["first_frame_ms"]})
    return {
        "measure": "colo",
        "at": utc_now(),
        "rooms": len(rows),
        "object_colo": dict(collections.Counter(row["colo"] for row in rows)),
        "entry_colo": dict(collections.Counter(row["entry"] for row in rows)),
        "cold_first_frame_ms": spread([row["first_frame_ms"] for row in rows]),
    }


async def measure_hibernation(args, relay):
    """Whether a room with idle sockets restarted during an idle gap with only keepalives in it."""
    room = random_room()
    sockets = [(await relay.dial("phone", room))[0] for _ in range(args.sockets)]
    await asyncio.sleep(1)

    async def keepalives(gap_s):
        ending = time.monotonic() + gap_s
        while (left := ending - time.monotonic()) > 0:
            await asyncio.sleep(min(left, args.ping_every))
            if time.monotonic() >= ending:
                return
            for ws in sockets:
                await ws.send("ping")
                await expect_text(ws, "pong", PONG_DEADLINE_S)

    results = {}
    previous = await ask_json(sockets[0], "info")
    began = utc_now()
    object_id = previous["id"]
    for gap_s in args.idle_gaps:
        rows = []
        for _ in range(args.repeats):
            await keepalives(gap_s)
            start = time.perf_counter()
            current = await ask_json(sockets[0], "info")
            rows.append(
                {
                    "restarted": current["boot"] != previous["boot"],
                    "answer_ms": ms_since(start),
                    "sockets": current["sockets"],
                }
            )
            previous = current
            # Give the answer a moment, so the next gap starts from the object being idle.
            await asyncio.sleep(0.2)
        results[f"{gap_s}s"] = {
            "restarted": sum(row["restarted"] for row in rows),
            "samples": len(rows),
            "answer_ms_when_restarted": spread(
                [row["answer_ms"] for row in rows if row["restarted"]]
            ),
            "answer_ms_when_awake": spread(
                [row["answer_ms"] for row in rows if not row["restarted"]]
            ),
            "sockets_seen": min(row["sockets"] for row in rows),
        }
    for ws in sockets:
        await ws.close()
    return {
        "measure": "hibernation",
        "began": began,
        "ended": utc_now(),
        "object_id": object_id,
        "sockets": args.sockets,
        "results": results,
    }


async def flood_one_room(relay, count, rate):
    """`count` frames at `rate` per second into one room with no desktop attached."""
    ws, _ = await relay.dial("phone", random_room())
    frame = os.urandom(100)
    start = time.perf_counter()
    sent = 0
    closed = None
    try:
        for sent in range(1, count + 1):
            due = start + sent / rate
            if (pause := due - time.perf_counter()) > 0:
                await asyncio.sleep(pause)
            await ws.send(frame)
        elapsed = time.perf_counter() - start
        await ws.send("ping")
        await expect_text(ws, "pong", PONG_DEADLINE_S)
    except (ConnectionClosed, asyncio.TimeoutError) as error:
        elapsed = time.perf_counter() - start
        closed = close_details(error)[0] if isinstance(error, ConnectionClosed) else "pong timeout"
    if closed is None:
        await ws.close()
    return {
        "frames_sent": sent,
        "achieved_per_s": round(sent / elapsed, 1),
        "socket_closed_with": closed,
    }


async def flood_forged(relay, count, rate, rooms, workers):
    """Upgrade requests with forged tickets, over reused connections, at `rate` per second."""
    room_ids = [random_room() for _ in range(rooms)]
    statuses = collections.Counter()
    errors = collections.Counter()
    claimed = 0
    start = None
    latencies = []

    async def open_connection():
        kwargs = {"ssl": relay.tls, "server_hostname": relay.host} if relay.secure else {}
        return await asyncio.open_connection(relay.host, relay.port, **kwargs)

    async def worker(reader, writer):
        nonlocal claimed
        while claimed < count:
            claimed += 1
            index = claimed
            due = start + index / rate
            if (pause := due - time.perf_counter()) > 0:
                await asyncio.sleep(pause)
            forged = f"{room_ids[index % rooms]}.{secrets.token_urlsafe(32)[:43]}"
            head = (
                f"GET /v1/phone?t={forged} HTTP/1.1\r\nHost: {relay.netloc}\r\n"
                "Upgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\n"
                "Sec-WebSocket-Key: a2VscGllLXJlbGF5LXByb2JlMQ==\r\nUser-Agent: kelpie-relay-probe\r\n\r\n"
            )
            begun = time.perf_counter()
            try:
                writer.write(head.encode())
                await writer.drain()
                header = await asyncio.wait_for(reader.readuntil(b"\r\n\r\n"), 15)
                lines = header.decode("latin-1").split("\r\n")
                statuses[lines[0].split(" ")[1]] += 1
                latencies.append((time.perf_counter() - begun) * 1000)
                length = 0
                for line in lines[1:]:
                    if line.lower().startswith("content-length:"):
                        length = int(line.split(":")[1])
                if length:
                    await asyncio.wait_for(reader.readexactly(length), 15)
                if any(line.lower() == "connection: close" for line in lines):
                    raise ConnectionError("server closed")
            except (OSError, EOFError, asyncio.TimeoutError, asyncio.IncompleteReadError) as error:
                errors[type(error).__name__] += 1
                writer.close()
                try:
                    reader, writer = await open_connection()
                except (OSError, asyncio.TimeoutError) as again:
                    errors[f"reopen {type(again).__name__}"] += 1
                    return
        writer.close()

    connections = await asyncio.gather(*(open_connection() for _ in range(workers)))
    start = time.perf_counter()
    await asyncio.gather(*(worker(reader, writer) for reader, writer in connections))
    elapsed = time.perf_counter() - start
    return {
        "requests": sum(statuses.values()),
        "achieved_per_s": round(sum(statuses.values()) / elapsed, 1),
        "statuses": dict(statuses),
        "errors": dict(errors),
        "response_ms": spread(latencies),
        "distinct_rooms": rooms,
    }


async def measure_abuse(args, relay):
    per_flood = int(args.rate * args.flood_seconds)
    if per_flood * 2 > args.max_requests:
        sys.exit(f"refusing: {per_flood * 2} requests is over --max-requests {args.max_requests}")
    began = utc_now()
    one_room = await flood_one_room(relay, per_flood, args.rate)
    forged = await flood_forged(relay, per_flood, args.rate, args.forged_rooms, args.workers)
    return {
        "measure": "abuse",
        "began": began,
        "ended": utc_now(),
        "cap_requests": args.max_requests,
        "one_room": one_room,
        "forged_tickets": forged,
    }


async def run_phone(args):
    relay = Relay(args.url, load_secret(args))
    measures = {
        "rtt": measure_rtt,
        "connect": measure_connect,
        "colo": measure_colo,
        "hibernation": measure_hibernation,
        "abuse": measure_abuse,
    }
    if args.measure == "rtt" and not args.room:
        sys.exit("--measure rtt needs --room, shared with a `desktop` process")
    result = await measures[args.measure](args, relay)
    text = json.dumps(result, indent=2)
    print(text, flush=True)
    if args.out:
        with open(args.out, "a", encoding="utf-8") as out:
            out.write(json.dumps(result) + "\n")


# ---- soak mode ----


def utc_now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds")


def awake_seconds():
    """Time this machine has been awake. Only Windows offers it directly."""
    if os.name != "nt":
        return time.monotonic()
    ticks = ctypes.c_ulonglong(0)
    ctypes.windll.kernel32.QueryUnbiasedInterruptTime(ctypes.byref(ticks))
    return ticks.value / 10_000_000


def pid_running(pid):
    if os.name != "nt":
        try:
            os.kill(pid, 0)
            return True
        except OSError:
            return False
    handle = ctypes.windll.kernel32.OpenProcess(0x1000, False, pid)  # query limited information
    if not handle:
        return False
    code = ctypes.c_ulong(0)
    ctypes.windll.kernel32.GetExitCodeProcess(handle, ctypes.byref(code))
    ctypes.windll.kernel32.CloseHandle(handle)
    return code.value == 259  # still active


class SoakLog:
    """JSON lines. Past the cap only the bounded hourly and lifecycle lines are still written."""

    BOUNDED = {"start", "hour", "log_capped", "stop", "fatal", "room_info"}

    def __init__(self, path, cap_bytes):
        self.path = path
        self.cap = cap_bytes
        self.capped = False
        self.size = path.stat().st_size if path.exists() else 0
        self.file = open(path, "a", encoding="utf-8", buffering=1)
        self.counts = collections.Counter()

    def write(self, event, **fields):
        self.counts[event] += 1
        if self.size >= self.cap and event not in self.BOUNDED:
            if not self.capped:
                self.capped = True
                self.write("log_capped", size=self.size)
            return
        line = json.dumps({"ts": utc_now(), "event": event, **fields}, separators=(",", ":"))
        self.file.write(line + "\n")
        self.size += len(line) + 1


class NetworkCheck:
    """Whether this machine reached the internet at all, so a local outage is not blamed on the relay."""

    def __init__(self):
        self.checked = 0
        self.result = None
        self.lock = asyncio.Lock()

    async def ok(self):
        async with self.lock:
            if time.monotonic() - self.checked < NET_CHECK_TTL_S:
                return self.result
            self.result = await self.probe()
            self.checked = time.monotonic()
            return self.result

    async def probe(self):
        for host in ("1.1.1.1", "8.8.8.8"):
            try:
                _, writer = await asyncio.wait_for(asyncio.open_connection(host, 443), 3)
                writer.close()
                return True
            except (OSError, asyncio.TimeoutError):
                continue
        return False


class Soak:
    def __init__(self, args, relay, log):
        self.args = args
        self.relay = relay
        self.log = log
        self.net = NetworkCheck()
        self.last_wake = 0.0  # monotonic time the machine last came back from sleep
        self.wall, self.awake = time.time(), awake_seconds()
        self.up = set()
        self.pings = 0
        self.pongs = 0
        self.stopping = False

    def after_sleep(self):
        return self.last_wake > 0 and time.monotonic() - self.last_wake < AFTER_SLEEP_GRACE_S

    def check_sleep(self):
        """Also called on every drop, because a socket can notice the wake before the next tick."""
        now_wall, now_awake = time.time(), awake_seconds()
        gap = now_wall - self.wall
        slept = gap - (now_awake - self.awake)
        if slept > SLEEP_MIN_S:
            self.last_wake = time.monotonic()
            self.log.write("sleep", slept_s=round(slept, 1))
        elif gap > STALL_MIN_S:
            self.log.write("stall", gap_s=round(gap, 1))
        self.wall, self.awake = now_wall, now_awake

    async def watch_sleep(self):
        while not self.stopping:
            await asyncio.sleep(SLEEP_TICK_S)
            self.check_sleep()

    async def hourly(self):
        while not self.stopping:
            await asyncio.sleep(3600)
            self.log.write(
                "hour",
                sockets_up=len(self.up),
                pings=self.pings,
                pongs=self.pongs,
                counts=dict(self.log.counts),
                log_bytes=self.log.size,
            )

    async def run_socket(self, index, role, room, active):
        await asyncio.sleep(random.uniform(0, self.args.start_spread))
        failures = 0
        attempt = 0
        down_since = None
        while not self.stopping:
            attempt += 1
            self.log.write("attempt", s=index, n=attempt)
            try:
                ws, timings = await self.relay.dial(role, room)
            except Exception as error:  # includes a refused upgrade, which is a logged outcome too
                self.log.write("connect_fail", s=index, n=attempt, err=repr(error)[:200])
                delay = random.uniform(0, min(BACKOFF_CAP_S, 2**failures))
                failures += 1
                await asyncio.sleep(delay)
                continue
            outage_ms = None if down_since is None else round((time.monotonic() - down_since) * 1000)
            self.log.write("open", s=index, n=attempt, outage_ms=outage_ms, **timings)
            opened = time.monotonic()
            self.up.add(index)
            try:
                await self.hold(ws, role, active)
                return  # only returns when the run is stopping
            except ConnectionClosed as error:
                code, reason, clean = close_details(error)
                by = "peer" if error.rcvd else "abnormal"
            except asyncio.TimeoutError:
                code, reason, clean, by = 1006, "", False, "pong_timeout"
                ws.transport.abort()
            except OSError as error:
                code, reason, clean, by = 1006, repr(error)[:120], False, "os_error"
                ws.transport.abort()
            self.up.discard(index)
            self.check_sleep()
            down_since = time.monotonic()
            held_s = down_since - opened
            self.log.write(
                "drop",
                s=index,
                role=role,
                code=code,
                reason=reason[:120],
                was_clean=clean,
                by=by,
                held_s=round(held_s, 1),
                net_ok=await self.net.ok(),
                after_sleep=self.after_sleep(),
            )
            if held_s >= STABLE_RESET_S:
                failures = 0
            delay = random.uniform(0, min(BACKOFF_CAP_S, 2**failures))
            failures += 1
            await asyncio.sleep(delay)

    async def hold(self, ws, role, active):
        """Keepalives, plus frames in an active room; raises when the socket goes down."""
        next_ping = time.monotonic() + random.uniform(1, KEEPALIVE_S)
        pong_due = None
        sends_frames = active and role == "phone"
        next_frame = time.monotonic() + self.args.active_every if sends_frames else None
        while not self.stopping:
            now = time.monotonic()
            deadlines = [next_ping]
            if pong_due is not None:
                deadlines.append(pong_due)
            if next_frame is not None:
                deadlines.append(next_frame)
            try:
                message = await asyncio.wait_for(ws.recv(), max(0.05, min(deadlines) - now))
            except asyncio.TimeoutError:
                now = time.monotonic()
                if pong_due is not None and now >= pong_due:
                    raise
                if now >= next_ping:
                    await ws.send("ping")
                    self.pings += 1
                    pong_due = now + PONG_DEADLINE_S
                    next_ping = now + KEEPALIVE_S
                if next_frame is not None and now >= next_frame:
                    await ws.send(os.urandom(64))
                    next_frame = now + self.args.active_every
                continue
            if message == "pong":
                self.pongs += 1
                pong_due = None
            elif isinstance(message, bytes) and role == "desktop":
                await ws.send(message)
        await ws.close()

    async def room_info(self, rooms):
        """A short extra socket per room: which code is running and who is still attached."""
        for index, room in enumerate(rooms):
            try:
                ws, _ = await self.relay.dial("phone", room)
                self.log.write("room_info", room=index, **await ask_json(ws, "info"))
                await ws.close()
            except (ConnectionClosed, OSError, asyncio.TimeoutError) as error:
                self.log.write("room_info", room=index, err=repr(error)[:200])


async def run_soak(args):
    log_dir = Path(args.log_dir)
    log_dir.mkdir(parents=True, exist_ok=True)
    log = SoakLog(log_dir / "soak.jsonl", args.log_cap_mb * 1024 * 1024)
    pid_file = log_dir / "soak.pid"
    if pid_file.exists():
        running = int(pid_file.read_text(encoding="utf-8").strip() or 0)
        if running and pid_running(running):
            sys.exit(f"soak already running as PID {running}")
    pid_file.write_text(str(os.getpid()), encoding="ascii")

    relay = Relay(args.url, load_secret(args))
    soak = Soak(args, relay, log)
    rooms = [
        hashlib.sha256(f"{args.room_prefix}-{i}".encode()).hexdigest()[:32]
        for i in range((args.sockets + ROOM_SOCKETS - 1) // ROOM_SOCKETS)
    ]
    log.write(
        "start",
        pid=os.getpid(),
        url=args.url,
        sockets=args.sockets,
        rooms=len(rooms),
        active_rooms=args.active_rooms,
        duration_s=args.duration_s,
    )
    tasks = []
    for index in range(args.sockets):
        room_index = index // ROOM_SOCKETS
        role = "desktop" if index % ROOM_SOCKETS == 0 else "phone"
        active = room_index < args.active_rooms
        tasks.append(asyncio.create_task(soak.run_socket(index, role, rooms[room_index], active)))
    helpers = [asyncio.create_task(soak.watch_sleep()), asyncio.create_task(soak.hourly())]
    try:
        await asyncio.sleep(args.duration_s)
        await soak.room_info(rooms)
    finally:
        soak.stopping = True
        for task in tasks + helpers:
            task.cancel()
        await asyncio.gather(*tasks, *helpers, return_exceptions=True)
        log.write("stop", counts=dict(log.counts), pings=soak.pings, pongs=soak.pongs)
        pid_file.unlink(missing_ok=True)


def soak_summary(log_dir):
    events = []
    with open(Path(log_dir) / "soak.jsonl", encoding="utf-8") as file:
        for line in file:
            try:
                events.append(json.loads(line))
            except json.JSONDecodeError:
                continue  # a process stopped in the middle of a line
    if not events:
        sys.exit("empty log")

    def at(event):
        return datetime.datetime.fromisoformat(event["ts"]).timestamp()

    start = next((e for e in events if e["event"] == "start"), events[0])
    sockets = start.get("sockets", 20)
    # A restart appends to the same log; the time between runs was not measured.
    wall_s = 0.0
    run_began = at(events[0])
    for previous, event in zip(events, events[1:]):
        if event["event"] == "start":
            wall_s += at(previous) - run_began
            run_began = at(event)
    wall_s += at(events[-1]) - run_began
    slept_s = sum(e["slept_s"] for e in events if e["event"] == "sleep")
    awake_s = max(0.0, wall_s - slept_s)
    socket_days = sockets * awake_s / 86400

    drops = [e for e in events if e["event"] == "drop"]
    unplanned = [e for e in drops if e["net_ok"] and not e["after_sleep"]]
    opens = [e for e in events if e["event"] == "open"]
    outages = [e["outage_ms"] for e in opens if e.get("outage_ms") is not None]
    return {
        "first": start["ts"],
        "last": events[-1]["ts"],
        "wall_h": round(wall_s / 3600, 2),
        "slept_h": round(slept_s / 3600, 2),
        "socket_days_awake": round(socket_days, 2),
        "attempts": sum(e["event"] == "attempt" for e in events),
        "connect_fails": sum(e["event"] == "connect_fail" for e in events),
        "drops_total": len(drops),
        "drops_after_sleep": sum(e["after_sleep"] for e in drops),
        "drops_local_network": sum(not e["net_ok"] and not e["after_sleep"] for e in drops),
        "drops_unplanned": len(unplanned),
        "unplanned_per_socket_day": round(len(unplanned) / socket_days, 3) if socket_days else None,
        "close_codes": dict(collections.Counter(str(e["code"]) for e in drops)),
        "was_clean": dict(collections.Counter(str(e["was_clean"]) for e in drops)),
        "by": dict(collections.Counter(e["by"] for e in drops)),
        "reconnect_outage_ms": spread(outages),
        "first_frame_ms": spread([e["first_frame_ms"] for e in opens]),
        "runs": sum(e["event"] == "start" for e in events),
        "stalls": sum(e["event"] == "stall" for e in events),
        "sleeps": sum(e["event"] == "sleep" for e in events),
        "log_capped": any(e["event"] == "log_capped" for e in events),
        "room_info": [e for e in events if e["event"] == "room_info"],
        "stopped": events[-1]["event"] == "stop",
    }


# ---- analytics mode ----

ANALYTICS_QUERY = """
query($account: String!, $since: Time!, $until: Time!) {
  viewer { accounts(filter: {accountTag: $account}) {
    workers: workersInvocationsAdaptive(
      filter: {datetime_geq: $since, datetime_leq: $until}, limit: 100
    ) {
      dimensions { scriptName status }
      sum { requests errors subrequests cpuTimeUs }
    }
    invocations: durableObjectsInvocationsAdaptiveGroups(
      filter: {datetime_geq: $since, datetime_leq: $until}, limit: 5000
    ) {
      dimensions { objectId status type }
      sum { requests errors wallTime }
    }
    periodic: durableObjectsPeriodicGroups(
      filter: {datetimeMinute_geq: $since, datetimeMinute_leq: $until}, limit: 10000
    ) {
      dimensions { coloCode objectId }
      sum {
        activeTime cpuTime duration subrequests
        inboundWebsocketMsgCount outboundWebsocketMsgCount
        rowsRead rowsWritten
      }
    }
  } }
}
"""


def run_analytics(args):
    token = os.environ.get("CLOUDFLARE_API_TOKEN", "")
    account = os.environ.get("CLOUDFLARE_ACCOUNT_ID", "")
    if not token or not account:
        sys.exit("analytics needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID in the environment")
    until = datetime.datetime.fromisoformat(args.until) if args.until else datetime.datetime.now(
        datetime.timezone.utc
    )
    since = (
        datetime.datetime.fromisoformat(args.since)
        if args.since
        else until - datetime.timedelta(hours=args.hours)
    )
    variables = {
        "account": account,
        "since": since.isoformat(timespec="seconds").replace("+00:00", "Z"),
        "until": until.isoformat(timespec="seconds").replace("+00:00", "Z"),
    }
    request = urllib.request.Request(
        "https://api.cloudflare.com/client/v4/graphql",
        data=json.dumps({"query": ANALYTICS_QUERY, "variables": variables}).encode(),
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(request, timeout=30) as response:
            body = json.load(response)
    except urllib.error.HTTPError as error:
        sys.exit(f"analytics request refused: HTTP {error.code}")
    if body.get("errors"):
        print(json.dumps(body["errors"], indent=2))
        sys.exit("analytics query failed")
    data = body["data"]["viewer"]["accounts"][0]
    periodic = [row["sum"] for row in data["periodic"]]
    print(
        json.dumps(
            {
                "since": variables["since"],
                "until": variables["until"],
                "worker_requests": sum(row["sum"]["requests"] for row in data["workers"]),
                "object_requests": sum(row["sum"]["requests"] for row in data["invocations"]),
                "object_active_seconds": round(sum(row["activeTime"] for row in periodic) / 1e6, 3),
                "object_gb_seconds": round(sum(row["duration"] for row in periodic), 3),
                "inbound_ws_messages": sum(row["inboundWebsocketMsgCount"] for row in periodic),
                "outbound_ws_messages": sum(row["outboundWebsocketMsgCount"] for row in periodic),
                "workers": data["workers"],
                "invocations": data["invocations"],
                "periodic": data["periodic"],
            },
            indent=2,
        )
    )


# ---- command line ----


def parse_args():
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    modes = parser.add_subparsers(dest="mode", required=True)

    def relay_options(mode, url_required=True):
        mode.add_argument("--url", required=url_required, help="relay base URL")
        mode.add_argument("--secret-file", help="file holding TICKET_SECRET")

    desktop = modes.add_parser("desktop", help="attach to a room and echo every frame")
    relay_options(desktop)
    desktop.add_argument("--room", required=True, help="room id, 32 hex")
    desktop.add_argument("--seconds", type=float, default=0, help="stop after this long (0: never)")

    phone = modes.add_parser("phone", help="measure from the client's side")
    relay_options(phone)
    phone.add_argument(
        "--measure", required=True, choices=["rtt", "connect", "colo", "hibernation", "abuse"]
    )
    phone.add_argument("--room", help="room id, 32 hex")
    phone.add_argument("--samples", type=int, default=1000, help="rtt per size; colo and connect in all")
    phone.add_argument("--sizes", type=int, nargs="+", default=[100, 1024], help="rtt payload bytes")
    phone.add_argument("--gap-ms", type=float, default=20, help="rtt pause between samples")
    phone.add_argument("--sample-timeout", type=float, default=5, help="rtt lost after this long")
    phone.add_argument("--sockets", type=int, default=8, help="hibernation: idle sockets")
    phone.add_argument(
        "--idle-gaps", type=int, nargs="+", default=[5, 8, 10, 12, 15, 20, 30], help="hibernation: seconds"
    )
    phone.add_argument("--repeats", type=int, default=3, help="hibernation: samples per gap")
    phone.add_argument("--ping-every", type=float, default=3, help="hibernation: keepalive gap")
    phone.add_argument("--rate", type=float, default=1000, help="abuse: per second")
    phone.add_argument("--flood-seconds", type=float, default=3, help="abuse: length of each flood")
    phone.add_argument("--forged-rooms", type=int, default=1000, help="abuse: random room ids")
    phone.add_argument("--workers", type=int, default=64, help="abuse: connections")
    phone.add_argument("--max-requests", type=int, default=20000, help="abuse: hard cap")
    phone.add_argument("--out", help="also append the result to this file")

    soak = modes.add_parser("soak", help="hold sockets open for days and log every drop")
    relay_options(soak, url_required=False)
    soak.add_argument("--log-dir", required=True, help="outside the repository")
    soak.add_argument("--summary", action="store_true", help="summarise the log and exit")
    soak.add_argument("--sockets", type=int, default=20)
    soak.add_argument("--duration-s", type=float, default=7 * 86400)
    soak.add_argument("--start-spread", type=float, default=5, help="seconds to stagger the start over")
    soak.add_argument("--active-rooms", type=int, default=0, help="rooms kept awake by frames")
    soak.add_argument("--active-every", type=float, default=2, help="seconds between active frames")
    soak.add_argument("--room-prefix", default="soak", help="rooms are derived from this")
    soak.add_argument("--log-cap-mb", type=int, default=LOG_CAP_BYTES // (1024 * 1024))

    analytics = modes.add_parser("analytics", help="read Cloudflare's billing analytics")
    analytics.add_argument("--hours", type=float, default=24)
    analytics.add_argument("--since", help="ISO time with offset, instead of --hours")
    analytics.add_argument("--until", help="ISO time with offset, default now")

    return parser.parse_args()


def main():
    args = parse_args()
    if args.mode == "analytics":
        return run_analytics(args)
    if args.mode == "soak" and args.summary:
        return print(json.dumps(soak_summary(args.log_dir), indent=2))
    if connect is None:
        sys.exit("this mode needs the websockets package: pip install websockets")
    if args.mode == "desktop":
        return asyncio.run(run_desktop(args))
    if args.mode == "phone":
        return asyncio.run(run_phone(args))
    if not args.url:
        sys.exit("soak needs --url")
    try:
        asyncio.run(run_soak(args))
    except Exception as error:  # a detached process has nowhere else to say why it stopped
        with open(Path(args.log_dir) / "soak.jsonl", "a", encoding="utf-8") as log:
            log.write(json.dumps({"ts": utc_now(), "event": "fatal", "err": repr(error)[:500]}) + "\n")
        raise


if __name__ == "__main__":
    main()
