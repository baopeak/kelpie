"""A stand-in for tauri-driver on Windows that attaches to the app instead of having msedgedriver launch it.

    python scripts/e2e/attach_driver.py --port 4444 --native-driver <msedgedriver.exe>

Same command line and the same W3C endpoint as tauri-driver, so the e2e scripts run unchanged when this is what `tauri-driver` names
(e.g. a `tauri-driver.cmd` first on PATH that runs it). Where msedgedriver cannot open a session on the app it launches itself
("DevToolsActivePort file doesn't exist", seen on GitHub's hosted Windows runners), this starts the app itself with WebView2's
remote debugging on a free port (`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`) and asks msedgedriver to attach to it (`debuggerAddress`).

- `POST /session` with `tauri:options.application` (and optional `args`): starts that program with this process's environment (what the
  script gave tauri-driver), waits for its DevTools endpoint, opens an msedgedriver session attached to it.
- Every other request goes to msedgedriver unchanged.
- `DELETE /session/<id>`: closes the session and ends the program (and what it started) the way tauri-driver does.
"""
import argparse
import json
import os
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def call(url, method="GET", body=None, timeout=120):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers={"Content-Type": "application/json"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()


class State:
    native = ""
    native_url = ""
    apps = {}  # session id -> Popen
    lock = threading.Lock()


def start_app(app, args):
    port = free_port()
    env = dict(os.environ)
    extra = env.get("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", "")
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = (extra + f" --remote-debugging-port={port}").strip()
    proc = subprocess.Popen([app, *args], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    deadline = time.time() + 90
    while time.time() < deadline:
        if proc.poll() is not None:
            raise RuntimeError(f"the app exited at start (code {proc.returncode})")
        try:
            status, _ = call(f"http://127.0.0.1:{port}/json/version", timeout=2)
            if status == 200:
                # The page target appears a moment after the browser endpoint.
                status, body = call(f"http://127.0.0.1:{port}/json/list", timeout=2)
                if status == 200 and any(t.get("type") == "page" for t in json.loads(body or b"[]")):
                    return proc, port
        except OSError:
            pass
        time.sleep(0.3)
    end(proc)
    raise RuntimeError("the app's WebView2 never opened its DevTools port")


def end(proc):
    if proc.poll() is None:
        subprocess.run(["taskkill", "/T", "/F", "/PID", str(proc.pid)], capture_output=True)


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # quiet, like tauri-driver
        pass

    def reply(self, status, body):
        self.send_response(status)
        self.send_header("Content-Type", "application/json; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def body(self):
        n = int(self.headers.get("Content-Length") or 0)
        return self.rfile.read(n) if n else b""

    def forward(self, method):
        data = self.body()
        req = urllib.request.Request(State.native_url + self.path, data=data if method != "GET" else None, method=method,
                                     headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(req, timeout=300) as r:
                return r.status, r.read()
        except urllib.error.HTTPError as e:
            return e.code, e.read()

    def do_POST(self):
        if self.path.rstrip("/") == "/session":
            caps = json.loads(self.body() or b"{}").get("capabilities", {})
            opts = {**caps.get("alwaysMatch", {}), **(caps.get("firstMatch") or [{}])[0]}.get("tauri:options", {})
            try:
                proc, port = start_app(opts["application"], opts.get("args", []))
            except Exception as e:  # noqa: BLE001
                return self.reply(500, json.dumps({"value": {"error": "session not created", "message": str(e)}}).encode())
            status, out = call(State.native_url + "/session", "POST", {"capabilities": {"alwaysMatch": {
                "browserName": "webview2", "ms:edgeOptions": {"debuggerAddress": f"127.0.0.1:{port}"}}}})
            if status == 200:
                with State.lock:
                    State.apps[json.loads(out)["value"]["sessionId"]] = proc
            else:
                end(proc)
            return self.reply(status, out)
        self.reply(*self.forward("POST"))

    def do_GET(self):
        self.reply(*self.forward("GET"))

    def do_DELETE(self):
        status, out = self.forward("DELETE")
        parts = self.path.strip("/").split("/")
        if len(parts) == 2 and parts[0] == "session":
            with State.lock:
                proc = State.apps.pop(parts[1], None)
            if proc:
                end(proc)
        self.reply(status, out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, default=4444)
    ap.add_argument("--native-port", type=int, default=0)
    ap.add_argument("--native-driver", default=os.environ.get("MSEDGEDRIVER") or "msedgedriver")
    args, _ = ap.parse_known_args()
    native_port = args.native_port or free_port()
    State.native = subprocess.Popen([args.native_driver, f"--port={native_port}"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    State.native_url = f"http://127.0.0.1:{native_port}"
    deadline = time.time() + 30
    while time.time() < deadline:
        try:
            if call(State.native_url + "/status", timeout=2)[0] == 200:
                break
        except OSError:
            time.sleep(0.2)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), Handler)
    try:
        server.serve_forever()
    finally:
        for proc in State.apps.values():
            end(proc)
        State.native.kill()


if __name__ == "__main__":
    sys.exit(main())
