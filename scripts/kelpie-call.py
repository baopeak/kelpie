#!/usr/bin/env python3
"""Dev/automation client for kelpied that does not depend on the Rust binary being built.

    python scripts/kelpie-call.py <method> [json-params]
    python scripts/kelpie-call.py watch <handle> <seconds>   # print status changes
    python scripts/kelpie-call.py attach <handle>            # dump the session's replay/snapshot

Honours KELPIE_ENDPOINT like the daemon (Windows named pipe name / Unix socket path) and
authenticates with the user token the daemon writes for that endpoint.
"""
import json
import os
import socket
import struct
import sys
import time

KIND_JSON = 1
# Calls that stop sessions or the daemon (see Conn.call).
DESTRUCTIVE = {"daemon.shutdown", "session.kill", "session.close", "session.restart"}


def endpoint_key():
    """The endpoint as the daemon names it: pipe name (Windows) or socket path."""
    custom = os.environ.get("KELPIE_ENDPOINT")
    if os.name == "nt":
        return custom or "kelpie-%s-v1" % "".join(
            c for c in os.environ.get("USERNAME", "user") if c.isalnum() or c in "-_")
    if custom:
        return custom
    raise SystemExit("set KELPIE_ENDPOINT to the daemon socket path")


def endpoint():
    key = endpoint_key()
    return r"\\.\pipe" + "\\" + key if os.name == "nt" else key


def user_token():
    """kelpie_proto::auth_token_path: <data base>/Kelpie/auth/<endpoint key>.token."""
    if os.name == "nt":
        base = os.environ.get("LOCALAPPDATA") or os.environ.get("TEMP", ".")
    elif sys.platform == "darwin":
        base = os.path.join(os.path.expanduser("~"), "Library", "Application Support")
    else:
        base = os.environ.get("XDG_DATA_HOME") or os.path.join(os.path.expanduser("~"), ".local", "share")
    key = "".join(c if (c.isascii() and c.isalnum()) or c in "-_" else "_" for c in endpoint_key())
    try:
        with open(os.path.join(base, "Kelpie", "auth", key + ".token"), encoding="utf-8") as f:
            return f.read().strip() or None
    except OSError:
        return None


class Conn:
    def __init__(self):
        ep = endpoint()
        if os.name == "nt":
            self.f = open(ep, "r+b", buffering=0)
            self.read = self.f.read
            self.write = self.f.write
        else:
            s = socket.socket(socket.AF_UNIX)
            s.connect(ep)
            self.read = lambda n: s.recv(n, socket.MSG_WAITALL)
            self.write = s.sendall
        self.next_id = 1
        token = user_token()
        if token:
            self.call("auth", {"token": token})

    def frame(self):
        hdr = self.read(5)
        (length,) = struct.unpack("<I", hdr[:4])
        kind = hdr[4]
        body = b""
        while len(body) < length - 1:
            body += self.read(length - 1 - len(body))
        return kind, body

    def call(self, method, params=None):
        # Test and dev scripts run next to the user's real Kelpie: a call that stops sessions
        # must name its own endpoint (KELPIE_ENDPOINT), or opt in explicitly. A cleanup step
        # that forgot the endpoint once shut the real daemon down with every session in it.
        if method in DESTRUCTIVE and not os.environ.get("KELPIE_ENDPOINT") and os.environ.get("KELPIE_ALLOW_REAL_DAEMON") != "1":
            raise SystemExit("refusing %s on the default daemon (the real Kelpie): set KELPIE_ENDPOINT to a test daemon, "
                             "or KELPIE_ALLOW_REAL_DAEMON=1 if you really mean it" % method)
        rid = self.next_id
        self.next_id += 1
        body = json.dumps({"id": rid, "method": method, "params": params or {}}).encode()
        self.write(struct.pack("<I", len(body) + 1) + bytes([KIND_JSON]) + body)
        while True:
            kind, payload = self.frame()
            if kind != KIND_JSON:
                continue
            msg = json.loads(payload)
            if msg.get("id") == rid:
                if "error" in msg:
                    raise SystemExit("%s: %s" % (method, msg["error"]["message"]))
                return msg.get("result")


def main():
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    c = Conn()
    if sys.argv[1] == "watch":
        handle, secs = int(sys.argv[2]), float(sys.argv[3])
        last = None
        end = time.time() + secs
        while time.time() < end:
            for s in c.call("session.list"):
                if s["handle"] == handle:
                    cur = (s["status"], s["unread"], s.get("title"))
                    if cur != last:
                        print("%6.1fs %s unread=%s title=%r" % (secs - (end - time.time()), *cur), flush=True)
                        last = cur
            time.sleep(0.25)
        return
    if sys.argv[1] == "attach":
        import base64
        r = c.call("session.attach", {"handle": int(sys.argv[2]), "interactive": False})
        sys.stdout.write(base64.b64decode(r["data"]).decode("utf8", "replace"))
        return
    params = json.loads(sys.argv[2]) if len(sys.argv) > 2 else {}
    print(json.dumps(c.call(sys.argv[1], params), indent=2, ensure_ascii=False))


if __name__ == "__main__":
    main()
