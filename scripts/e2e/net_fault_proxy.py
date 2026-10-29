"""A TCP fault proxy for the phone tests: sits between a test phone and the daemon and makes the network
bad on demand. Python and the standard library only.

    from net_fault_proxy import FaultProxy
    proxy = FaultProxy(target_port=4817)       # inside a running asyncio loop
    port = await proxy.start()                 # the phone connects to 127.0.0.1:<port>
    proxy.latency = 0.3                        # seconds each way (plus `jitter`)
    proxy.bandwidth["down"] = 20_000           # bytes a second, daemon -> phone
    proxy.blackhole(10)                        # nothing passes either way for 10 s, then all of it does
    proxy.stall("up", 30)                      # half-open: only phone -> daemon stops
    proxy.kill_all()                           # reset every connection now (RST)
    proxy.rst_storm(1.0, 10)                   # reset everything every second for 10 s
    await proxy.stop()

or from a shell, with the faults fixed for the run:

    python net_fault_proxy.py --target 4817 [--listen 4900] [--latency 0.2] [--jitter 0.05]
                              [--down-bps 20000] [--up-bps 20000] [--blackhole-after 5 --blackhole-for 30]

A blackhole is a stall, not a loss: the proxy stops reading, so the kernel buffers fill and the sender
backs up exactly as it does on a path that dropped packets and retransmits, and what was held is delivered
when the path comes back (or finds the other end closed). Directions are `up` (phone -> daemon) and `down`
(daemon -> phone). It forwards bytes; it knows nothing of WebSocket.
"""
import argparse
import asyncio
import random
import socket
import struct
import time

CHUNK = 16 * 1024


class _Link:
    """One proxied connection."""

    def __init__(self, client, upstream):
        self.client, self.upstream = client, upstream
        self.writers = (client[1], upstream[1])
        self.tasks = []

    def abort(self):
        """Closes both sockets with a reset, as a NAT that forgot the flow or a firewall does."""
        for w in self.writers:
            sock = w.get_extra_info("socket")
            try:
                if sock is not None:
                    sock.setsockopt(socket.SOL_SOCKET, socket.SO_LINGER, struct.pack("ii", 1, 0))
                w.transport.abort()
            except Exception:  # noqa: BLE001
                pass
        for t in self.tasks:
            t.cancel()


class FaultProxy:
    def __init__(self, target_port, target_host="127.0.0.1", listen_port=0, listen_host="127.0.0.1"):
        self.target = (target_host, target_port)
        self.listen = (listen_host, listen_port)
        self.latency = 0.0
        self.jitter = 0.0
        self.bandwidth = {"up": None, "down": None}  # bytes a second
        self._stalled_until = {"up": 0.0, "down": 0.0}
        self.links = []
        self.bytes = {"up": 0, "down": 0}
        self.connections = 0
        self.server = None
        self._storm = None

    async def start(self):
        self.server = await asyncio.start_server(self._accept, *self.listen)
        return self.server.sockets[0].getsockname()[1]

    async def stop(self):
        if self._storm:
            self._storm.cancel()
        if self.server:
            self.server.close()
        self.kill_all()

    # ---- faults ----

    def stall(self, direction, seconds):
        """Nothing passes in `direction` for `seconds` (then what was held does)."""
        self._stalled_until[direction] = time.monotonic() + seconds

    def blackhole(self, seconds):
        self.stall("up", seconds)
        self.stall("down", seconds)

    def heal(self):
        self._stalled_until = {"up": 0.0, "down": 0.0}

    def kill_all(self):
        links, self.links = self.links, []
        for link in links:
            link.abort()
        return len(links)

    def rst_storm(self, every, seconds):
        """Resets every connection each `every` seconds for `seconds`."""
        async def storm():
            end = time.monotonic() + seconds
            while time.monotonic() < end:
                await asyncio.sleep(every)
                self.kill_all()
        self._storm = asyncio.ensure_future(storm())
        return self._storm

    # ---- plumbing ----

    async def _accept(self, reader, writer):
        try:
            up_reader, up_writer = await asyncio.open_connection(*self.target)
        except OSError:
            writer.transport.abort()
            return
        for w in (writer, up_writer):
            sock = w.get_extra_info("socket")
            if sock is not None:
                sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        link = _Link((reader, writer), (up_reader, up_writer))
        self.links.append(link)
        self.connections += 1
        link.tasks = [asyncio.ensure_future(self._pump("up", reader, up_writer, link)), asyncio.ensure_future(self._pump("down", up_reader, writer, link))]

    async def _pump(self, direction, reader, writer, link):
        """Reads from one side, holds each piece for the latency and writes it to the other at the bandwidth."""
        queue = asyncio.Queue()

        async def deliver():
            while True:
                due, data = await queue.get()
                if data is None:
                    break
                wait = due - time.monotonic()
                if wait > 0:
                    await asyncio.sleep(wait)
                writer.write(data)
                try:
                    await writer.drain()
                    # At the bandwidth, in steps, so that lifting the limit takes effect at once.
                    owed = len(data) / self.bandwidth[direction] if self.bandwidth[direction] else 0
                    while owed > 0 and self.bandwidth[direction]:
                        step = min(0.05, owed)
                        await asyncio.sleep(step)
                        owed -= step
                except (ConnectionError, OSError):
                    break
            try:
                if writer.can_write_eof():
                    writer.write_eof()
            except Exception:  # noqa: BLE001
                pass

        sender = asyncio.ensure_future(deliver())
        link.tasks.append(sender)
        try:
            while True:
                # A stall stops reading: the sender's buffers fill, as on a path that lost the packets.
                while (left := self._stalled_until[direction] - time.monotonic()) > 0:
                    await asyncio.sleep(min(left, 0.05))
                data = await reader.read(CHUNK)
                if not data:
                    break
                # What was read just as the path went dead is held too, not delivered.
                while (left := self._stalled_until[direction] - time.monotonic()) > 0:
                    await asyncio.sleep(min(left, 0.05))
                self.bytes[direction] += len(data)
                delay = self.latency + (random.uniform(-self.jitter, self.jitter) if self.jitter else 0.0)
                await queue.put((time.monotonic() + max(0.0, delay), data))
        except (ConnectionError, OSError, asyncio.CancelledError):
            pass
        finally:
            await queue.put((0, None))


async def _main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--target", type=int, required=True)
    ap.add_argument("--listen", type=int, default=0)
    ap.add_argument("--latency", type=float, default=0.0)
    ap.add_argument("--jitter", type=float, default=0.0)
    ap.add_argument("--down-bps", type=int)
    ap.add_argument("--up-bps", type=int)
    ap.add_argument("--blackhole-after", type=float)
    ap.add_argument("--blackhole-for", type=float, default=30.0)
    args = ap.parse_args()
    proxy = FaultProxy(args.target, listen_port=args.listen)
    proxy.latency, proxy.jitter = args.latency, args.jitter
    proxy.bandwidth = {"up": args.up_bps, "down": args.down_bps}
    port = await proxy.start()
    print(f"proxying 127.0.0.1:{port} -> 127.0.0.1:{args.target}", flush=True)
    if args.blackhole_after is not None:
        await asyncio.sleep(args.blackhole_after)
        print(f"blackhole for {args.blackhole_for} s", flush=True)
        proxy.blackhole(args.blackhole_for)
    await asyncio.Event().wait()


if __name__ == "__main__":
    try:
        asyncio.run(_main())
    except KeyboardInterrupt:
        pass
