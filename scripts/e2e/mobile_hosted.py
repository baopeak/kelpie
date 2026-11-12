"""The hosted phone app end to end on this machine: the relay serving the phone app at its own HTTPS origin (`wrangler dev --local`), a daemon
with no Tailscale and no listener on this network, and a phone-sized Edge that pairs, unlocks, types and gets alerts through the relay alone.

    python -X utf8 scripts/e2e/mobile_hosted.py <debug kelpied.exe> [--edgedriver PATH] [--gui <debug kelpie.exe>] [--tauri-driver PATH]

Before it: `cd app && pnpm install --frozen-lockfile` and `cd relay && pnpm install --frozen-lockfile` (wrangler and Vite are run from there),
Python's `cryptography`, `websockets` and `psutil`, and an msedgedriver matching the installed Edge (`--edgedriver`, else `KELPIE_EDGEDRIVER`,
else `msedgedriver` on PATH). `--gui` also needs a debug app with its frontend (`cd app && pnpm tauri build --debug --no-bundle`), `tauri-driver`
and an msedgedriver matching WebView2: this script starts tauri-driver itself, with the app's own isolated endpoint, data folder and WebView2
profile, since the relay's port and certificate are made here.

Everything is this run's own: daemons on endpoints and data folders of their own (phone_support.isolate), started from the debug build given
(never an installed kelpied), with the e2e seams `KELPIE_RELAY_TEST_URL` / `KELPIE_RELAY_TEST_CA` (a debug daemon refuses any relay not on this
machine then) and the push stand-in `KELPIE_PUSH_TEST_BASE`; a certificate authority and a `localhost` certificate made for this run, which the
daemon is told to trust and the browser accepts by its key (`--ignore-certificate-errors-spki-list`); the hosted bundle built for
`https://localhost:<port>` into this run's folder and served from a copy of `relay/` (never the repository's `relay/public` or `.dev.vars`) by
`wrangler dev --local` with its secrets on its command line, metrics off and no login (the copy's configs say https for wrangler's own proxy, which
otherwise rewrites a page's `Origin` to `http://`). The phone's browser resolves no name but this machine's, and its push manager is a stand-in
from before each page's own scripts run. Every process started is stopped by its PID (process
tree), and the run's folder is removed. Nothing reaches Cloudflare.

Checks, in order:
- setup: a new store; phone access on with this network's listeners off, the relay on with no key; the room made; the hosted page found; no
  listener on this network (nothing on 4817).
- pairing: the page shows the install step; the hosted code pairs through the relay only (no socket but `wss://<origin>/v1/phone`); the person's
  digits approved at the daemon; the device is `page_kind: hosted` at the hosted origin and holds a relay token for as long as an unused phone
  stays paired; the app says what it trusts and the bundle's fingerprint.
- lock: a passkey (CDP virtual authenticator, rpId `localhost`) set up in the window approval opens; a reload is locked; unlocking brings the
  sessions; a program with the phone's own key copied out of the browser and no passkey gets no session data, also with the lock switch off.
- typing: the phone types into a shell; `mobile.set_permission control` is refused; a hosted device with its passkey is refused `tk_start`,
  `screens` and `sc_start`.
- alerts: the stand-in push service gets an alert whose link is the hosted page's `#s=`; the real service worker shows it, and a synthetic
  `notificationclick` opens that session on the hosted page.
- headers: what is served carries the headers of the bundle's `_headers`; no CSP violation; `scripts/hosted/verify_live.py` exits 0, and 1 once
  one byte of a served file changed.
- new version: version B replaces the assets and wrangler restarts on the same state; the open page A takes B's worker, still loads A's chat
  chunk (which the origin no longer has) from A's kept cache, reloads once into B at its next foreground, and keeps a draft.
- replace: a second daemon's hosted code; the phone asks before replacing; once it said yes the first computer's alerts end (the browser's
  subscription is ended, and the first daemon, sending its alert test, drops it at the push service's 410).
- limits: on a relay with `ROOMS_PER_NETWORK_DAY` 2 the third room is refused saying so; with `REGISTRATION_KEY` set, a room without it is refused
  saying a key is needed.
- relay state: nothing a session said, and no phone's name, is in wrangler's persisted state.
- GUI (`--gui`): a new user turns phone access on in Settings → Mobile; Pair a phone opens with Kelpie's relay first and recommended, the other ways
  saying why they cannot pair now; its guide: Step 1's install link, whose page says it is the app from Kelpie's relay and lists the other ways, Step 2's
  link opened in Edge pairs; the approval offers no control; the device is badged as the hosted app; "What this trusts" is there.
"""
import argparse
import asyncio
import base64
import datetime
import hashlib
import http.client
import ipaddress
import json
import os
import re
import secrets
import shutil
import socket
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

import psutil
import websockets

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import link_noise as ln  # noqa: E402
import phone_support as ps  # noqa: E402

ROOT = os.path.abspath(os.path.join(HERE, "..", ".."))
APP = os.path.join(ROOT, "app")
RELAY = os.path.join(ROOT, "relay")
MANIFEST_TOOL = os.path.join(ROOT, "scripts", "hosted", "bundle_manifest.py")
VERIFY_TOOL = os.path.join(ROOT, "scripts", "hosted", "verify_live.py")

parser = argparse.ArgumentParser(description="The hosted phone app, end to end.")
parser.add_argument("kelpied", help="a debug build of kelpied")
parser.add_argument("--edgedriver", default=os.environ.get("KELPIE_EDGEDRIVER") or shutil.which("msedgedriver"))
parser.add_argument("--gui", metavar="APP", help="also the desktop part, with this debug app (pnpm tauri build --debug --no-bundle)")
parser.add_argument("--tauri-driver", default=shutil.which("tauri-driver"))
args = parser.parse_args()

exe = os.path.abspath(args.kelpied)
INSTALLED = [os.path.join(os.environ.get("LOCALAPPDATA", "~"), "Kelpie"), os.environ.get("ProgramFiles", "C:\\Program Files"), os.environ.get("ProgramFiles(x86)", "C:\\Program Files (x86)")]
if any(os.path.normcase(exe).startswith(os.path.normcase(p) + os.sep) for p in INSTALLED if p) or not os.path.isfile(exe):
    raise SystemExit(f"give a debug kelpied from a build folder, never an installed one (got {exe})")
if not args.edgedriver or not os.path.isfile(args.edgedriver):
    raise SystemExit("no msedgedriver: pass --edgedriver (it must match the installed Edge)")
for name in ("KELPIE_TAILSCALE_BIN", "KELPIE_RELAY_TEST_URL", "KELPIE_RELAY_TEST_CA", "KELPIE_PUSH_TEST_BASE", "REGISTRATION_KEY", "TICKET_SECRET"):
    os.environ.pop(name, None)
ps.isolate("phone-hosted")
DEFAULT_ENDPOINT = re.compile(r"kelpie-[A-Za-z0-9_-]*-v\d+")


def refuse_default(endpoint):
    if not endpoint or DEFAULT_ENDPOINT.fullmatch(endpoint):
        raise SystemExit(f"refusing: {endpoint!r} is the default endpoint")


refuse_default(os.environ["KELPIE_ENDPOINT"])

PID = os.getpid()
RELAY_PORT = 47000 + PID % 1000
LIMIT_PORT = 46000 + PID % 1000
PHONE_DRIVER = 45000 + PID % 1000
INSPECTOR = 44000 + PID % 1000
TAURI_PORT = 43000 + PID % 1000
NATIVE_PORT = 42000 + PID % 1000
ORIGIN = f"https://localhost:{RELAY_PORT}"
RELAY_URL = f"wss://localhost:{RELAY_PORT}"
RUN = tempfile.mkdtemp(prefix="kelpie-hosted-run-")
WORK = os.path.join(RUN, "work")
os.makedirs(WORK)
MARKER = "hosted-marker-" + secrets.token_hex(6)
PHONE_NAME = "Hosted phone " + secrets.token_hex(3)
SHELL_NAME = "hosted-shell-" + secrets.token_hex(3)
DRAFT = "draft-" + secrets.token_hex(4)

results = []
STARTED = []  # every process this run started: (what, Popen)
SEEN_PIDS = {}  # pid -> (what, start time), for the check after the run (children too): a PID the system gave to another program since is not ours
ENDPOINTS = []  # (endpoint, data folder) of the other daemons this run made


def check(name, ok, detail=""):
    results.append((name, bool(ok)))
    detail = str(detail).encode("ascii", "replace").decode()
    print(("PASS " if ok else "FAIL ") + name + (f"  ({detail[:400]})" if detail and not ok else ""), flush=True)
    return bool(ok)


def section(title):
    print(f"\n--- {title} ---", flush=True)


def port_free(port):
    with socket.socket() as s:
        try:
            s.bind(("127.0.0.1", port))
            return True
        except OSError:
            return False


for p in (RELAY_PORT, LIMIT_PORT, PHONE_DRIVER, INSPECTOR, INSPECTOR + 1, TAURI_PORT, NATIVE_PORT):
    if not port_free(p):
        raise SystemExit(f"port {p} of this run is in use by another program: not taking another one")


def seen(pid, what):
    try:
        SEEN_PIDS[pid] = (what, psutil.Process(pid).create_time())
    except psutil.Error:
        pass


def note_tree(what, proc):
    """Records a process of this run and what it started so far, for the check after the run."""
    try:
        root = psutil.Process(proc.pid)
        seen(proc.pid, what)
        for child in root.children(recursive=True):
            seen(child.pid, f"{what} (child {child.name()})")
    except psutil.Error:
        pass


def start(what, argv, **kw):
    proc = subprocess.Popen(argv, **kw, **({"start_new_session": True} if os.name != "nt" else {}))
    STARTED.append((what, proc))
    seen(proc.pid, what)
    return proc


def stop_tree(proc, what=""):
    """Stops a process this run started and everything it started, by its PID."""
    if proc is None:
        return
    try:
        note_tree(what, proc)
    except Exception:  # noqa: BLE001
        pass
    if proc.poll() is None:
        if os.name == "nt":
            subprocess.run(["taskkill", "/T", "/F", "/PID", str(proc.pid)], capture_output=True, check=False)
        else:
            import signal
            try:
                os.killpg(proc.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        try:
            proc.wait(15)
        except subprocess.TimeoutExpired:
            proc.kill()


# ---------- certificates ----------

def make_certificates(folder):
    """A certificate authority of this run and a certificate for localhost and 127.0.0.1 it signed: (CA file, certificate, key, the
    certificate's key hash for the browser)."""
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.x509.oid import ExtendedKeyUsageOID, NameOID

    now = datetime.datetime.now(datetime.timezone.utc)
    ca_key = ec.generate_private_key(ec.SECP256R1())
    ca_name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "Kelpie e2e CA " + secrets.token_hex(4))])
    ca = (x509.CertificateBuilder().subject_name(ca_name).issuer_name(ca_name).public_key(ca_key.public_key()).serial_number(x509.random_serial_number())
          .not_valid_before(now - datetime.timedelta(minutes=5)).not_valid_after(now + datetime.timedelta(days=1))
          .add_extension(x509.BasicConstraints(ca=True, path_length=0), critical=True)
          .add_extension(x509.SubjectKeyIdentifier.from_public_key(ca_key.public_key()), critical=False)
          .add_extension(x509.KeyUsage(digital_signature=True, key_cert_sign=True, crl_sign=True, content_commitment=False, key_encipherment=False, data_encipherment=False, key_agreement=False, encipher_only=False, decipher_only=False), critical=True)
          .sign(ca_key, hashes.SHA256()))
    key = ec.generate_private_key(ec.SECP256R1())
    leaf = (x509.CertificateBuilder().subject_name(x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "localhost")])).issuer_name(ca_name).public_key(key.public_key())
            .serial_number(x509.random_serial_number()).not_valid_before(now - datetime.timedelta(minutes=5)).not_valid_after(now + datetime.timedelta(days=1))
            .add_extension(x509.SubjectAlternativeName([x509.DNSName("localhost"), x509.IPAddress(ipaddress.ip_address("127.0.0.1"))]), critical=False)
            .add_extension(x509.BasicConstraints(ca=False, path_length=None), critical=True)
            .add_extension(x509.ExtendedKeyUsage([ExtendedKeyUsageOID.SERVER_AUTH]), critical=False)
            .add_extension(x509.SubjectKeyIdentifier.from_public_key(key.public_key()), critical=False)
            .add_extension(x509.AuthorityKeyIdentifier.from_issuer_public_key(ca_key.public_key()), critical=False)
            .sign(ca_key, hashes.SHA256()))
    ca_file, cert_file, key_file = (os.path.join(folder, n) for n in ("ca.pem", "localhost.crt", "localhost.key"))
    with open(ca_file, "wb") as f:
        f.write(ca.public_bytes(serialization.Encoding.PEM))
    with open(cert_file, "wb") as f:
        f.write(leaf.public_bytes(serialization.Encoding.PEM))
    with open(key_file, "wb") as f:
        f.write(key.private_bytes(serialization.Encoding.PEM, serialization.PrivateFormat.PKCS8, serialization.NoEncryption()))
    spki = key.public_key().public_bytes(serialization.Encoding.DER, serialization.PublicFormat.SubjectPublicKeyInfo)
    return ca_file, cert_file, key_file, base64.b64encode(hashlib.sha256(spki).digest()).decode()


CA_FILE, CERT_FILE, KEY_FILE, SPKI = make_certificates(RUN)
CTX = ssl.create_default_context(cafile=CA_FILE)


def get(path, port=RELAY_PORT, headers=None):
    c = http.client.HTTPSConnection("localhost", port, timeout=15, context=CTX)
    try:
        c.request("GET", path, headers=headers or {})
        r = c.getresponse()
        return r.status, {k.lower(): v for k, v in r.getheaders()}, r.read()
    finally:
        c.close()


# ---------- the hosted bundle and the relay ----------

def git_commit():
    return subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True, check=True).stdout.strip()


APP_VERSION = json.load(open(os.path.join(APP, "package.json"), encoding="utf-8"))["version"]
NODE = shutil.which("node")


def build_bundle(name, version=None):
    """The hosted app for this run's origin, into `RUN/<name>/bundle`, with its version.json and manifest: (folder, manifest)."""
    out = os.path.join(RUN, name, "bundle")
    env = {k: v for k, v in os.environ.items() if not k.startswith("CLOUDFLARE_")}
    env["KELPIE_HOSTED_ORIGIN"] = ORIGIN
    env.pop("KELPIE_APP_VERSION", None)
    if version:
        env["KELPIE_APP_VERSION"] = version
    vite = os.path.join(APP, "node_modules", "vite", "bin", "vite.js")
    built = subprocess.run([NODE, vite, "build", "--config", "vite.phone.config.ts", "--mode", "hosted", "--outDir", out, "--emptyOutDir", "--logLevel", "error"], cwd=APP, env=env, capture_output=True, text=True)
    if built.returncode != 0:
        raise SystemExit("the hosted build failed:\n" + built.stdout[-2000:] + built.stderr[-2000:])
    tag = "v" + (version or APP_VERSION)
    subprocess.run([sys.executable, "-I", MANIFEST_TOOL, out, "--tag", tag, "--commit", git_commit()], check=True, capture_output=True)
    with open(os.path.join(RUN, name, "hosted-phone-manifest.json"), encoding="utf-8") as f:
        return out, json.load(f)


def relay_copy(name, bundle=None):
    """A copy of relay/ for one wrangler run, with `bundle` (or nothing) as its public folder. The repository's own relay/ is only read."""
    dest = os.path.join(RUN, name)
    shutil.copytree(RELAY, dest, ignore=shutil.ignore_patterns("node_modules", "public", ".dev.vars", ".wrangler", "dist", "rooms"))
    # Wrangler dev puts a proxy in front of the Worker that rewrites every header naming the served host to the Worker's inner URL, which is
    # plain http unless the config says otherwise: a page's `Origin: https://localhost:<port>` would reach the room as `http://…` and be refused.
    # Cloudflare does no such thing, so this run's copy of the configs says https.
    for name in ("wrangler.toml", "wrangler.room.toml"):
        with open(os.path.join(dest, name), "a", encoding="utf-8") as f:
            f.write('\n[dev]\nlocal_protocol = "https"\nupstream_protocol = "https"\n')
    put_public(dest, bundle)
    return dest


def put_public(copy, bundle):
    public = os.path.join(copy, "public")
    if os.path.isdir(public):
        shutil.rmtree(public)
    if bundle:
        shutil.copytree(bundle, public)
    else:
        os.makedirs(public)


class Wrangler:
    """`wrangler dev --local` on a copy of relay/, HTTPS on this run's certificate, its state in `persist`, its secrets on its own command line."""

    def __init__(self, copy, port, persist, inspector, variables):
        self.copy, self.port, self.persist, self.inspector, self.variables = copy, port, persist, inspector, variables
        self.proc = None
        self.log = os.path.join(RUN, f"wrangler-{port}.log")

    def start(self):
        wrangler = os.path.join(RELAY, "node_modules", "wrangler", "bin", "wrangler.js")
        argv = [NODE, wrangler, "dev", "--local", "-c", "wrangler.toml", "-c", "wrangler.room.toml", "--ip", "127.0.0.1", "--port", str(self.port),
                "--local-protocol", "https", "--https-key-path", KEY_FILE, "--https-cert-path", CERT_FILE, "--persist-to", self.persist,
                "--inspector-port", str(self.inspector), "--show-interactive-dev-session=false", "--log-level", "warn"]
        for k, v in self.variables.items():
            argv += ["--var", f"{k}:{v}"]
        env = {k: v for k, v in os.environ.items() if not k.startswith(("CLOUDFLARE_", "KELPIE_", "WRANGLER_"))}
        env.update({"WRANGLER_SEND_METRICS": "false", "CLOUDFLARE_CF_FETCH_ENABLED": "false", "NO_UPDATE_NOTIFIER": "1", "CI": "1"})
        with open(self.log, "ab") as log:
            self.proc = start(f"wrangler :{self.port}", argv, cwd=self.copy, env=env, stdout=log, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
        end = time.time() + 90
        while time.time() < end:
            if self.proc.poll() is not None:
                break
            try:
                if get("/v1/health", self.port)[0] == 200:
                    note_tree(f"wrangler :{self.port}", self.proc)
                    return self
            except OSError:
                pass
            time.sleep(0.5)
        tail = open(self.log, encoding="utf-8", errors="replace").read()[-3000:]
        for v in self.variables.values():
            if len(str(v)) > 8:  # the secrets; a limit like "2" is no secret, and replacing it would garble the log
                tail = tail.replace(str(v), "<secret>")
        stop_tree(self.proc, "wrangler")
        raise SystemExit(f"wrangler did not start on {self.port}:\n{tail}")

    def stop(self):
        stop_tree(self.proc, f"wrangler :{self.port}")
        self.proc = None


# ---------- the phone ----------

class EdgePhone:
    """Headless Edge with a phone's screen and agent, through msedgedriver (W3C WebDriver), with its network and console logs kept."""

    def __init__(self, port, profile):
        self.port = port
        self.proc = start("msedgedriver (phone)", [args.edgedriver, f"--port={port}"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        end = time.time() + 20
        while time.time() < end and port_free(port):
            time.sleep(0.2)
        caps = {"capabilities": {"alwaysMatch": {"browserName": "MicrosoftEdge", "acceptInsecureCerts": True, "ms:loggingPrefs": {"performance": "ALL", "browser": "ALL"}, "ms:perfLoggingPrefs": {"enableNetwork": True}, "ms:edgeOptions": {
            "args": ["--headless=new", "--window-size=412,900", "--no-first-run", f"--user-data-dir={profile}", f"--ignore-certificate-errors-spki-list={SPKI}",
                     # The phone's browser reaches nothing but this machine: no push service, nothing of Microsoft's, nothing of Cloudflare's.
                     "--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE localhost , EXCLUDE 127.0.0.1"],
            "mobileEmulation": {"deviceMetrics": {"width": 412, "height": 900, "pixelRatio": 2.0}, "userAgent": "Mozilla/5.0 (Linux; Android 14; Pixel 7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0 Mobile Safari/537.36"},
        }}}}
        made = self.req("POST", "/session", caps)
        self.sid = made["sessionId"]
        self.debugger = made.get("capabilities", {}).get("ms:edgeOptions", {}).get("debuggerAddress")
        note_tree("msedgedriver (phone)", self.proc)
        self.events = []
        self.console = []

    def req(self, method, path, body=None):
        data = json.dumps(body).encode() if body is not None else None
        r = urllib.request.Request(f"http://127.0.0.1:{self.port}" + path, data=data, method=method, headers={"Content-Type": "application/json"})
        try:
            with urllib.request.urlopen(r, timeout=90) as resp:
                return json.loads(resp.read() or b"{}").get("value", {})
        except urllib.error.HTTPError as e:
            raise RuntimeError(json.loads(e.read() or b"{}").get("value", {}).get("message", "webdriver error")) from None

    def go(self, url):
        self.req("POST", f"/session/{self.sid}/url", {"url": url})

    def js(self, script, *a):
        return self.req("POST", f"/session/{self.sid}/execute/sync", {"script": script, "args": list(a)})

    def js_async(self, script, *a):
        """`script` ends by calling `done(value)`."""
        return self.req("POST", f"/session/{self.sid}/execute/async", {"script": "const done = arguments[arguments.length - 1];" + script, "args": list(a)})

    def wait(self, script, timeout=20, *a):
        end = time.time() + timeout
        while time.time() < end:
            try:
                v = self.js(script, *a)
            except RuntimeError:
                v = None  # a page that is loading
            if v:
                return v
            time.sleep(0.3)
        return None

    def read_logs(self):
        for entry in self.req("POST", f"/session/{self.sid}/log", {"type": "performance"}):
            self.events.append(json.loads(entry["message"])["message"])
        for entry in self.req("POST", f"/session/{self.sid}/log", {"type": "browser"}):
            self.console.append(entry.get("message", ""))

    def sockets(self):
        self.read_logs()
        return [e["params"].get("url", "") for e in self.events if e.get("method") == "Network.webSocketCreated"]

    def socket_story(self):
        """What became of the page's sockets: created, the handshake's status, errors and closes."""
        self.read_logs()
        out = []
        for e in self.events:
            m, p = e.get("method", ""), e.get("params", {})
            if m == "Network.webSocketCreated":
                out.append(("created", p.get("url", "")[:70]))
            elif m == "Network.webSocketHandshakeResponseReceived":
                out.append(("handshake", p.get("response", {}).get("status")))
            elif m in ("Network.webSocketFrameError", "Network.webSocketClosed"):
                out.append((m.rsplit(".", 1)[1], p.get("errorMessage", "")))
            elif m == "Network.webSocketFrameReceived" and p.get("response", {}).get("opcode") == 1:
                out.append(("text", p["response"].get("payloadData", "")[:80]))
        return out

    def responses(self):
        self.read_logs()
        return [(e["params"]["response"].get("url", ""), e["params"]["response"].get("status"), e["params"]["response"].get("fromServiceWorker")) for e in self.events if e.get("method") == "Network.responseReceived"]

    def csp_violations(self):
        self.read_logs()
        return [line for line in self.console if "Content Security Policy" in line and (line.startswith(ORIGIN) or not line.startswith(("http://", "https://")))]

    def quit(self):
        try:
            self.req("DELETE", f"/session/{self.sid}")
        except Exception:  # noqa: BLE001
            pass
        finally:
            stop_tree(self.proc, "msedgedriver (phone)")


class Cdp:
    """One DevTools target of the phone's browser (`page` or `service_worker`), over its own socket."""

    def __init__(self, debugger, kind, url_start):
        from websockets.sync.client import connect

        with urllib.request.urlopen(f"http://{debugger}/json/list", timeout=10) as resp:
            listed = json.loads(resp.read())
        target = next((t for t in listed if t["type"] == kind and t["url"].startswith(url_start)), None)
        if not target:
            raise RuntimeError(f"no {kind} at {url_start}: {[(t['type'], t['url'][:60]) for t in listed]}")
        # A session can stay open for the whole run with nobody reading it (the stand-in's script lives as long as its session): its events are
        # kept unbounded, as a bounded queue stops reading the socket once full, and then the keepalive's pong is never read and the session is
        # closed (1011), taking the stand-in off the next page load. DevTools is on this machine: no keepalive is needed.
        self.ws = connect(target["webSocketDebuggerUrl"], max_size=None, max_queue=None, ping_interval=None, open_timeout=10)
        self.n = 0
        self.events = []

    def call(self, method, params=None, timeout=15):
        self.n += 1
        self.ws.send(json.dumps({"id": self.n, "method": method, "params": params or {}}))
        end = time.time() + timeout
        while time.time() < end:
            m = json.loads(self.ws.recv(timeout=max(0.1, end - time.time())))
            if m.get("id") == self.n:
                if "error" in m:
                    raise RuntimeError(f"{method}: {m['error']}")
                return m.get("result", {})
            self.events.append(m)
        raise TimeoutError(method)

    def close(self):
        self.ws.close()


LOCK_SCREEN = "return document.querySelector('[data-lock-screen]')?.dataset.lockScreen"
STATE_JS = "return document.querySelector('.conn')?.dataset.state ?? null"
CONNECTED = "return ['live', 'busy', 'degraded'].includes(document.querySelector('.conn')?.dataset.state)"
OPEN_WITH = "return !document.querySelector('[data-lock-screen]') && [...document.querySelectorAll('.row')].some(r => r.textContent.includes(arguments[0]))"
SET_HIDDEN = "Object.defineProperty(document, 'hidden', {value: arguments[0], configurable: true}); Object.defineProperty(document, 'visibilityState', {value: arguments[0] ? 'hidden' : 'visible', configurable: true}); document.dispatchEvent(new Event('visibilitychange'));"
PAIRING_JS = """const req = indexedDB.open('kelpie-link');
req.onerror = () => done(null);
req.onsuccess = () => { try { const g = req.result.transaction('kv').objectStore('kv').get('pairing');
  g.onsuccess = () => { const p = g.result; const b = (a) => btoa(String.fromCharCode(...a));
    done(p ? {priv: b(p.priv), pub: b(p.pub), dpk: b(p.dpk), deviceId: p.deviceId, endpoints: p.endpoints, relayToken: p.relayToken ?? null, relayExpires: p.relayExpires ?? 0, computer: p.computer} : null); req.result.close(); };
  g.onerror = () => done(null); } catch (e) { done(null); } };"""
# The browser's own `pushManager` would reach a real push service, so every page of this origin gets one that hands out the stand-in's address, from before its
# own scripts run (a reload included). The subscription it holds is kept in the page's storage, as a browser keeps its own across reloads.
STAND_IN_PUSH = """(() => {
  if (typeof PushManager === "undefined") return;
  const [endpoint, p256dh, auth] = %s;
  const HELD = "e2e-push-held", ENDED = "e2e-push-ended";
  const bytes = (t) => Uint8Array.from(atob(t.replace(/-/g, "+").replace(/_/g, "/")), (c) => c.charCodeAt(0)).buffer;
  const sub = (key) => ({ endpoint, options: { applicationServerKey: bytes(key) }, getKey: (name) => bytes(name === "p256dh" ? p256dh : auth),
    unsubscribe: async () => { localStorage.removeItem(HELD); localStorage.setItem(ENDED, String(Number(localStorage.getItem(ENDED) || 0) + 1)); return true; } });
  PushManager.prototype.subscribe = async function (options) {
    const k = new Uint8Array(options.applicationServerKey.buffer ?? options.applicationServerKey);
    const key = btoa(String.fromCharCode(...k)).replace(/\\+/g, "-").replace(/\\//g, "_").replace(/=+$/, "");
    localStorage.setItem(HELD, key);
    return sub(key);
  };
  PushManager.prototype.getSubscription = async function () { const key = localStorage.getItem(HELD); return key ? sub(key) : null; };
  window.__standInPush = true;
})();"""


class StandInPush:
    """The stand-in push manager on every page load of the phone (`Page.addScriptToEvaluateOnNewDocument`, on a DevTools session kept open)."""

    def __init__(self, phone, sub):
        self.source = STAND_IN_PUSH % json.dumps([sub["endpoint"], sub["p256dh"], sub["auth"]])
        self.cdp = Cdp(phone.debugger, "page", ORIGIN)
        self.cdp.call("Page.enable")
        self.cdp.call("Page.addScriptToEvaluateOnNewDocument", {"source": self.source})
        self.cdp.call("Runtime.evaluate", {"expression": self.source})

    def close(self):
        self.cdp.close()


PASSKEYS = None


class VirtualPasskeys:
    """CDP's virtual authenticator on the phone's page, standing for the phone's own (internal, with user verification). DevTools can lose it with the
    page's renderer (a navigation may swap it): the credential is kept here, private key included, and put back into a new authenticator then, as the
    phone's own authenticator would still hold it."""

    def __init__(self, phone):
        self.phone, self.cdp, self.kept = phone, None, []
        self.attach()

    def attach(self):
        if self.cdp:
            try:
                self.cdp.close()
            except Exception:  # noqa: BLE001
                pass
        self.cdp = Cdp(self.phone.debugger, "page", ORIGIN)
        self.cdp.call("WebAuthn.enable", {"enableUI": False})
        self.id = self.cdp.call("WebAuthn.addVirtualAuthenticator", {"options": {"protocol": "ctap2", "transport": "internal", "hasResidentKey": True, "hasUserVerification": True, "isUserVerified": True, "automaticPresenceSimulation": True}})["authenticatorId"]
        for c in self.kept:
            self.cdp.call("WebAuthn.addCredential", {"authenticatorId": self.id, "credential": c})

    def credentials(self):
        try:
            return self.cdp.call("WebAuthn.getCredentials", {"authenticatorId": self.id})["credentials"]
        except (RuntimeError, OSError, websockets.exceptions.ConnectionClosed):
            self.attach()
            return self.cdp.call("WebAuthn.getCredentials", {"authenticatorId": self.id})["credentials"]

    def keep(self):
        """After a ceremony: what the authenticator holds now (its signature count too)."""
        self.kept = self.credentials()

    def close(self):
        self.cdp.close()


def unlock(phone, row, timeout=40):
    """The person presses Unlock on the lock screen (the virtual authenticator answers): the sessions come, with `row` among them. What stood in the way, if not."""
    if PASSKEYS:
        PASSKEYS.credentials()  # the authenticator is there before the press
    end = time.time() + timeout
    while time.time() < end:
        # A press made before the page heard the computer's challenge does nothing: it is pressed again, as a person would.
        phone.wait("return !!document.querySelector('[data-lock-go]:not([disabled])') || !document.querySelector('[data-lock-screen]')", 10)
        phone.js("document.querySelector('[data-lock-go]:not([disabled])')?.click()")
        if phone.wait(OPEN_WITH, 10, row):
            if PASSKEYS:
                PASSKEYS.keep()
            return None
    return (phone.js(LOCK_SCREEN), phone.js("return document.querySelector('[data-lock-error]')?.textContent ?? null"), phone.js(STATE_JS), phone.js("return document.querySelector('main')?.textContent?.slice(0, 200)"))


def pairing_of(phone):
    p = phone.js_async(PAIRING_JS)
    if not p:
        return None
    for k in ("priv", "pub", "dpk"):
        p[k] = base64.b64decode(p[k])
    return p


def relay_phone(device, ticket, token):
    """A phone-like program that comes through the relay with `token`, as the hosted page: its Origin, its token's hash in its greeting."""
    phone = ps.Phone(ORIGIN + "/", device, origin=ORIGIN, ssl=CTX)
    phone.url = f"{RELAY_URL}/v1/phone?t={urllib.parse.quote(ticket)}&k={urllib.parse.quote(token)}"
    phone.caps = ["ack", "hb", "relay", "pad"]
    phone.greeting = {"relay_token": hashlib.sha256(ln.unb64(token)).hexdigest()}
    return phone


def raw_answer(url, origin=ORIGIN):
    """What the relay answers a socket to `url` from `origin`: 101, or the refusal's status and body (for saying why a check failed)."""
    async def go():
        try:
            ws = await websockets.connect(url, origin=origin, ssl=CTX, open_timeout=10)
            await ws.close()
            return 101
        except websockets.exceptions.InvalidStatus as e:
            return (e.response.status_code, bytes(e.response.body or b"")[:80])
    return asyncio.run(go())


def daemon_log(data_dir, word="relay", n=15):
    """The last lines of a daemon's log that name `word`, long tokens left out (for saying why a check failed)."""
    try:
        lines = open(os.path.join(data_dir, "logs", "kelpied.log"), encoding="utf-8", errors="replace").read().splitlines()
    except OSError:
        return []
    return [re.sub(r"[A-Za-z0-9_\-+/=.]{24,}", "<...>", l)[-200:] for l in lines if word in l][-n:]


def refused(fn):
    """The error a daemon call ends with, or None."""
    try:
        fn()
        return None
    except SystemExit as e:  # kelpie-call's way of reporting an error
        return str(e)


def wait_for(fn, timeout, step=0.3):
    end = time.time() + timeout
    while time.time() < end:
        try:
            v = fn()
        except (OSError, RuntimeError):
            v = None
        if v:
            return v
        time.sleep(step)
    return None


def device(d, name):
    return next((x for x in d.call("mobile.status")["devices"] if x["name"] == name), None)


def relay_connected(d):
    r = d.call("mobile.status")["relay"]
    return r if r.get("on") and r.get("connected") else None


def hosted_ready(d):
    r = d.call("mobile.status")["relay"]
    return r if r.get("on") and r.get("connected") and (r.get("hosted") or {}).get("available") else None


class Endpoint:
    """Points this process at another daemon endpoint of this run while a daemon there is started or cleaned up."""

    def __init__(self, endpoint, data):
        self.endpoint, self.data = endpoint, data

    def __enter__(self):
        self.saved = (os.environ["KELPIE_ENDPOINT"], os.environ["KELPIE_DATA_DIR"])
        os.environ["KELPIE_ENDPOINT"], os.environ["KELPIE_DATA_DIR"] = self.endpoint, self.data
        return self

    def __exit__(self, *exc):
        os.environ["KELPIE_ENDPOINT"], os.environ["KELPIE_DATA_DIR"] = self.saved


def daemon_env(relay_url, push_base=None):
    # Pinned calm, as mobile.py does: these checks are about what the hosted app may do, not how a loaded machine paces it.
    env = {"KELPIE_E2E": "1", "KELPIE_RELAY_TEST_URL": relay_url, "KELPIE_RELAY_TEST_CA": CA_FILE, "KELPIE_LOAD_FORCE": "L0:calm"}
    if push_base:
        env["KELPIE_PUSH_TEST_BASE"] = push_base
    return env


def other_daemon(tag, env):
    """Another daemon of this run, on an endpoint and data folder of its own."""
    endpoint, data = f"kelpie-{tag}-{PID}", tempfile.mkdtemp(prefix=f"kelpie-{tag}-")
    refuse_default(endpoint)
    ps.MADE.add(endpoint)
    ENDPOINTS.append((endpoint, data))
    with Endpoint(endpoint, data):
        d = ps.Daemon(exe, env={**env, "KELPIE_ENDPOINT": endpoint, "KELPIE_DATA_DIR": data}).start()
    seen(d.pid, f"kelpied {tag}")
    return d


def hosted_setup(d):
    """What Settings does for a new user: phone access on with this network's listeners off (refused until a way in exists), then the relay with no key."""
    first = refused(lambda: d.call("mobile.enable", {"lan": False}))
    turned = refused(lambda: d.call("mobile.set_relay", {"on": True}))
    return first, turned


def payload_of(link):
    return json.loads(ln.unb64(link.split("#p=")[1]))


def approve_on(d, name, digits, permission="write"):
    request = ps.pending_request(d, name, 30)
    d.call("mobile.approve", {"id": request["id"], "allow": True, "code": digits, "permission": permission})
    return request


def pair_in_page(phone, d, link, name):
    """The hosted page pairs from `link` as a person would (pasted into the scanner screen, or opened), with the digits approved at `d`."""
    if phone.js("return !!document.querySelector('[data-scan]')"):
        phone.js("const i = document.querySelector('[data-paste]'); i.value = arguments[0]; i.dispatchEvent(new Event('input', {bubbles: true})); document.querySelector('[data-paste-go]').click();", link)
    else:
        phone.go(link)
    phone.wait("return !!document.querySelector('.card input.field') && !document.querySelector('[data-scan]')", 20)
    phone.js("const i = document.querySelector('.card input.field'); i.value = arguments[0]; i.dispatchEvent(new Event('input', {bubbles: true})); document.querySelector('.card .btn.primary').click();", name)
    digits = phone.wait("return document.querySelector('[data-pair-code]')?.textContent", 30)
    request = approve_on(d, name, digits) if digits else None
    return digits, request


TINY_CHAT_AGENT = """import json, sys
sys.stdout.reconfigure(encoding='utf-8')
print(json.dumps({'type': 'assistant', 'message': {'id': 'a-1', 'role': 'assistant', 'content': [{'type': 'text', 'text': 'hello from the hosted chat'}]}}), flush=True)
for line in sys.stdin:
    pass
"""


# ---------- the run ----------

def main(phone_box, push, dA, wrangler, bundle_a, manifest_a, bundle_b, manifest_b, copy_main):
    # ---------- setup ----------
    section("setup: a new user, relay only")
    st = dA.call("mobile.status")
    check("a new data folder is a new phone setup", st.get("new_store") is True, st.get("new_store"))
    first, turned = hosted_setup(dA)
    st = dA.call("mobile.status")
    check("phone access on with this network's listeners off (refused until something reaches this computer)", st["enabled"] and st.get("lan") is False and "nothing else reaches this computer" in (first or ""), (first, st.get("lan")))
    check("the relay turns on with no registration key", turned is None, turned)
    relay = wait_for(lambda: relay_connected(dA), 40)
    check("…makes its room and connects, on a relay of the second version", relay and relay.get("version") == 2, relay)
    ready = wait_for(lambda: hosted_ready(dA), 40)
    hosted = (ready or {}).get("hosted") or {}
    check("the relay's phone app is found serving, at the relay's own origin", hosted.get("available") and hosted.get("origin") == ORIGIN and hosted.get("version") == "v" + APP_VERSION, hosted)
    st = wait_for(lambda: (x := dA.call("mobile.status"))["running"] and x, 20) or dA.call("mobile.status")
    check("phone access runs on the relay alone", st["running"] and not st.get("error"), (st["running"], st.get("error")))
    listening = [c.laddr for c in psutil.Process(dA.pid).net_connections("tcp") if c.status == psutil.CONN_LISTEN]
    check("no listener on this network: nothing on 4817, nothing but loopback", not any(a.port == 4817 for a in listening) and all(ipaddress.ip_address(a.ip).is_loopback for a in listening), listening)

    # ---------- pairing ----------
    section("pairing through the relay only")
    shell = dA.call("session.create", {"kind": "shell", "cwd": WORK, "name": SHELL_NAME})
    phone = phone_box["phone"] = EdgePhone(PHONE_DRIVER, os.path.join(RUN, "phone-profile"))
    phone.go(ORIGIN + "/")
    check("the hosted page shows how to install first", phone.wait("return document.querySelector('[data-install]')?.dataset.install", 30) == "android", phone.js("return document.body.textContent.slice(0, 200)"))
    check("…on a page the browser trusts as secure, with no way yet to send a code", phone.js("return window.isSecureContext") is True and not phone.js("return !!document.querySelector('[data-scan]')"))
    phone.js("document.querySelector('[data-stay-in-browser]').click()")
    phone.wait("return !!document.querySelector('[data-scan]')", 10)
    made = dA.call("mobile.pair_start", {"hosted": True})
    ep = payload_of(made["url"])["ep"]
    check("a hosted code: the hosted page's link, with the relay as its only way", made.get("hosted") is True and made["url"].startswith(ORIGIN + "/#p=") and [e["t"] for e in ep] == ["relay"] and ep[0]["a"] == RELAY_URL, [(e["t"], e.get("a")) for e in ep])
    digits, request = pair_in_page(phone, dA, made["url"], PHONE_NAME)
    check("the phone shows six digits, and the request came through the relay from the hosted page", bool(re.fullmatch(r"\d{6}", digits or "")) and request and request["address"] == "the relay" and request.get("origin_kind") == "hosted", request or (phone.socket_story(), raw_answer(f"{RELAY_URL}/v1/phone?t={urllib.parse.quote(ep[0]['r'])}&p={ps.relay_pair_token(ln.unb64(payload_of(made['url'])['psk']))}"), daemon_log(os.environ["KELPIE_DATA_DIR"])))
    check("approved: the app says what it trusts", bool(phone.wait("return !!document.querySelector('[data-trust]')", 30)), phone.js("return document.querySelector('main')?.textContent?.slice(0, 200)"))
    bundle_shown = phone.wait("return document.querySelector('[data-trust-bundle]')?.textContent", 15)
    check("…with the fingerprint the relay says it serves, which is this bundle's", bundle_shown == manifest_a["bundle_sha256"], bundle_shown)
    phone.js("document.querySelector('[data-trust] .btn').click()")
    sockets = phone.sockets()
    check("the page opened no socket but the relay's own", sockets and all(u.startswith(f"{RELAY_URL}/v1/phone?") for u in sockets), sockets)
    dev = device(dA, PHONE_NAME)
    check("the device is a hosted page that may type", dev and dev.get("page_kind") == "hosted" and dev.get("page") == ORIGIN and dev.get("can_write") is True, dev)
    pairing = pairing_of(phone)
    days = ((pairing or {}).get("relayExpires", 0) / 1000 - time.time()) / 86400
    check("…and holds a relay token for as long as an unused phone stays paired (90 days)", pairing and pairing.get("relayToken") and 85 < days < 95, round(days, 1))

    # ---------- lock ----------
    section("the lock: mandatory for the hosted app")
    check("the hosted phone stops at the lock, with a passkey to set up", phone.wait(LOCK_SCREEN + " === 'enrol'", 30) is True, phone.js(LOCK_SCREEN))
    global PASSKEYS
    PASSKEYS = phone_box["webauthn"] = VirtualPasskeys(phone)
    passkeys = PASSKEYS.credentials
    phone.js("document.querySelector('[data-lock-go]').click()")
    check("a passkey set up in the window approval opened, checked by the computer: the sessions come", bool(phone.wait(OPEN_WITH, 30, SHELL_NAME)), phone.js("return document.querySelector('[data-lock-error]')?.textContent ?? null"))
    PASSKEYS.keep()
    dev = device(dA, PHONE_NAME)
    check("…one passkey, for localhost, on record and unlocked", len(passkeys()) == 1 and passkeys()[0].get("rpId") == "localhost" and dev.get("passkey") is True and dev.get("unlocked") is True, ([c.get("rpId") for c in passkeys()], dev))
    phone.go(ORIGIN + "/")
    check("a reload is locked, with no session on the page", phone.wait(LOCK_SCREEN + " === 'unlock'", 30) is True and not phone.js("return !!document.querySelector('.row')"), phone.js(LOCK_SCREEN))
    stuck = unlock(phone, SHELL_NAME)
    check("…and the passkey opens it", stuck is None, stuck)
    # Leaving and coming back within the lock's grace asks nothing; after it, the passkey again. The page's clock is moved on in place of waiting.
    signs = passkeys()[0].get("signCount")
    phone.js(SET_HIDDEN, True)
    check("hiding the hosted app covers it", phone.js("return !!document.querySelector('[data-lock-cover]')") is True)
    phone.js(SET_HIDDEN, False)
    check("…and coming back within the grace opens it with no passkey asked, the computer still unlocked", bool(phone.wait(OPEN_WITH, 15, SHELL_NAME)) and passkeys()[0].get("signCount") == signs and device(dA, PHONE_NAME).get("unlocked") is True, device(dA, PHONE_NAME))
    phone.js(SET_HIDDEN, True)
    phone.js("const real = Date.now; Date.now = () => real() + 6 * 60 * 1000; try { Object.defineProperty(document, 'hidden', {value: false, configurable: true}); Object.defineProperty(document, 'visibilityState', {value: 'visible', configurable: true}); document.dispatchEvent(new Event('visibilitychange')); } finally { Date.now = real; }")
    check("coming back after the grace shows the lock, and the computer locks", phone.wait(LOCK_SCREEN + " === 'unlock'", 15) is True and bool(wait_for(lambda: device(dA, PHONE_NAME).get("unlocked") is False, 15)), phone.js(LOCK_SCREEN))
    stuck = unlock(phone, SHELL_NAME)
    check("…which the passkey opens", stuck is None, stuck)
    # "Immediately": leaving locks at once, as it always did. The choice is read when the app starts, so it takes a reload; the key goes again after,
    # and the next reload below is back to the default.
    phone.js("localStorage.setItem('kelpie-lock-grace', '0')")
    phone.go(ORIGIN + "/")
    stuck = unlock(phone, SHELL_NAME)
    phone.js(SET_HIDDEN, True)
    check("with the grace set to Immediately, hiding the hosted app locks it at the computer", stuck is None and bool(wait_for(lambda: device(dA, PHONE_NAME).get("unlocked") is False, 15)), stuck or device(dA, PHONE_NAME))
    phone.js(SET_HIDDEN, False)
    check("…and coming back shows the lock", phone.wait(LOCK_SCREEN + " === 'unlock'", 15) is True, phone.js(LOCK_SCREEN))
    phone.js("localStorage.removeItem('kelpie-lock-grace')")
    stuck = unlock(phone, SHELL_NAME)
    check("…which the passkey opens", stuck is None, stuck)

    keys = ps.Device(pairing["priv"], pairing["pub"], pairing["dpk"], pairing["deviceId"])
    relay_ep = next(e for e in pairing["endpoints"] if e["t"] == "relay")

    async def copied_key(label):
        thief = relay_phone(keys, relay_ep["r"], pairing["relayToken"])
        hello = await thief.open()
        check(f"a program with the phone's key copied out of the browser and no passkey gets a locked hello with nothing in it ({label})", hello.get("locked") is True and hello["sessions"] == [] and hello["projects"] == {}, (hello.get("locked"), hello.get("lock")))
        replies = [await thief.ask(type=t, id=shell["id"], text="x") for t in ("list", "sub", "input", "chat_sub")]
        check(f"…and reading and typing are refused by the daemon ({label})", all((r or {}).get("code") == "locked" for r in replies), [(r or {}).get("code") for r in replies])
        await thief.close()

    # The page steps away meanwhile: the program uses its token, and the relay would hand the socket to whichever came last.
    phone.go("about:blank")
    asyncio.run(copied_key("lock on"))
    dA.call("mobile.set_lock", {"on": False})
    check("the lock switch is off", dA.call("mobile.status")["lock"]["on"] is False)
    asyncio.run(copied_key("lock switch off"))
    dA.call("mobile.set_lock", {"on": True})
    phone.go(ORIGIN + "/")
    stuck = unlock(phone, SHELL_NAME)
    check("back on the page, the passkey opens it again", stuck is None, stuck)

    # ---------- typing ----------
    section("typing, and nothing past it")
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes(arguments[0])).click()", SHELL_NAME)
    phone.wait("return !!document.querySelector('.send textarea')", 20)
    time.sleep(2.5)
    phone.js("const t = document.querySelector('.send textarea'); t.value = arguments[0]; t.dispatchEvent(new Event('input', {bubbles: true}));", f"echo {MARKER[::-1]}-$((6*7))")
    phone.js("document.querySelector('.send .btn.primary').click()")
    check("the hosted phone types into a shell, and the shell runs it", bool(wait_for(lambda: f"{MARKER[::-1]}-42" in dA.call("session.read", {"handle": shell["handle"], "lines": 60})["text"], 20)))
    phone.js("history.back()")
    phone.wait("return !!document.querySelector('.row')", 15)
    raised = refused(lambda: dA.call("mobile.set_permission", {"id": dev["id"], "permission": "control"}))
    check("raising it to control is refused, saying why", raised and "hosted app can read and type" in raised and device(dA, PHONE_NAME)["permission"] == "write", raised)

    async def control_refused():
        """A hosted device unlocked with its own passkey (a program, paired as the page would be) asks for what only control may do."""
        name = "Hosted program " + secrets.token_hex(3)
        link = dA.call("mobile.pair_start", {"hosted": True})["url"]
        load = payload_of(link)
        ep = next(e for e in load["ep"] if e["t"] == "relay")
        attempt = ps.PairAttempt(ORIGIN + "/", link, name, "write", origin=ORIGIN, ssl=CTX)
        attempt.url = f"{RELAY_URL}/v1/phone?t={urllib.parse.quote(ep['r'])}&p={ps.relay_pair_token(ln.unb64(load['psk']))}"
        await attempt.start()
        if attempt.session is None:
            return check("a program pairs as a hosted page through the relay", False, (attempt.closed, attempt.silent))
        approving = asyncio.ensure_future(asyncio.to_thread(approve_on, dA, name, attempt.code()))
        paired = await attempt.outcome(40)
        await approving
        if not (paired and paired.get("relay")):
            return check("a program pairs as a hosted page through the relay", False, paired)
        program = relay_phone(ps.Device(attempt.priv, attempt.pub, attempt.dpk, paired["device"]["id"]), ep["r"], paired["relay"]["token"])
        hello = await program.open()
        passkey = ps.SoftPasskey(ORIGIN)
        challenge = (await program.ask(type="lock_challenge"))["lock"]["challenge"]
        enrolled = await program.ask(type="lock_enrol", **passkey.create(challenge))
        check("a hosted device's own passkey opens it", hello.get("locked") is True and (enrolled or {}).get("result") == "ok", (hello.get("lock"), enrolled))
        replies = [await program.ask(**verb) for verb in ({"type": "tk_start", "pane": "browser-a"}, {"type": "screens"}, {"type": "sc_start", "target": "w:1:2"})]
        check("…and taking over a browser pane, listing screens and starting a screen are refused (no_control)", all((r or {}).get("code") == "no_control" for r in replies), [(r or {}).get("code") for r in replies])
        await program.close()
        dA.call("mobile.revoke", {"id": paired["device"]["id"]})

    asyncio.run(control_refused())

    # ---------- alerts ----------
    section("alerts lead to the hosted page")
    sub = ps.browser_subscription(push.base, "/push/hosted")
    phone_box["push"] = StandInPush(phone, sub)
    check("the page has the stand-in for the push service", phone.js("return window.__standInPush === true") is True)
    phone.req("POST", f"/session/{phone.sid}/permissions", {"descriptor": {"name": "notifications"}, "state": "granted"})
    worker = phone.js_async("navigator.serviceWorker.getRegistration().then((r) => done(r ? r.scope : null), () => done(null));")
    check("the app's service worker runs on the hosted origin", worker == ORIGIN + "/", worker)
    live_label = phone.wait("return document.querySelector('.conn')?.dataset.state === 'live' && document.querySelector('.conn .conn-label').textContent", 20)
    check("the live label says the connection goes through the relay", live_label == "Live · through the relay", live_label)
    phone.js("document.querySelector('[data-open-settings]').click()")
    card = phone.wait("const f = (n) => document.querySelector(`[data-setting=\"${n}\"] dd`)?.textContent; return f('paired-through') && [f('paired-through'), f('live-path'), f('screen')]", 15)
    check("the phone's Settings say the hosted app was paired through the relay, goes through it now, and why the screen is not here", bool(card) and card[0] == "The hosted app, through the relay" and card[1] == "the relay" and "comes from the relay" in card[2], card)
    check("Settings offers alerts", phone.wait("return document.querySelector('[data-alerts]')?.dataset.alerts", 20) == "off", phone.js("return document.querySelector('[data-alerts]')?.dataset.alerts"))
    phone.js("document.querySelector('[data-alerts-on]').click()")
    check("turning alerts on subscribes, and the computer has it", phone.wait("return document.querySelector('[data-alerts]')?.dataset.alerts === 'on'", 20) is True and bool(wait_for(lambda: device(dA, PHONE_NAME).get("push"), 10)))
    phone.js("document.querySelector('header .icon').click()")
    phone.wait("return !document.querySelector('[data-settings]')", 10)
    alert_shell = dA.call("session.create", {"kind": "shell", "cwd": WORK, "name": "alert-secret-" + secrets.token_hex(3)})
    wait_for(lambda: next((x for x in dA.call("session.list") if x["id"] == alert_shell["id"]), {}).get("status") == "idle", 60)
    hook = lambda d, sid, event: d.call("hook.report", {"session_id": sid, "agent": "claude", "payload": {"hook_event_name": event}})  # noqa: E731
    hook(dA, alert_shell["id"], "UserPromptSubmit")
    wait_for(lambda: next((x for x in dA.call("session.list") if x["id"] == alert_shell["id"]), {}).get("status") == "working", 15)
    before = len(push.at("/push/hosted"))
    hook(dA, alert_shell["id"], "PermissionRequest")
    got = wait_for(lambda: len(push.at("/push/hosted")) > before, 20)
    plain = ps.open_alert(push.at("/push/hosted")[before]["body"], sub) if got else b"{}"
    note = json.loads(plain).get("notification", {})
    check("a needs-you alert reaches the stand-in, its link the hosted page's own", note.get("navigate") == f"{ORIGIN}/#s={alert_shell['id']}&a=1", note.get("navigate"))
    page_cdp = Cdp(phone.debugger, "page", ORIGIN)
    page_cdp.call("ServiceWorker.enable")
    time.sleep(1)
    page_cdp.call("Runtime.evaluate", {"expression": "1"})
    registration = next((r["registrationId"] for e in page_cdp.events if e.get("method") == "ServiceWorker.workerRegistrationUpdated" for r in e["params"]["registrations"] if r["scopeURL"].startswith(ORIGIN) and not r["isDeleted"]), None)
    shown = None
    if registration is not None:
        sw = Cdp(phone.debugger, "service_worker", ORIGIN)
        # What the worker does with the push, from inside: whether it reached the handler, and what `showNotification` was given.
        sw.call("Runtime.evaluate", {"expression": """(() => {
            const seen = (self.__seen = {pushes: 0, shown: [], failed: []});
            self.addEventListener("push", () => void seen.pushes++);
            const real = self.registration.showNotification.bind(self.registration);
            self.registration.showNotification = (title, options) => real(title, options).then(() => void seen.shown.push({title, navigate: options.data && options.data.navigate}), (e) => { seen.failed.push(String(e)); throw e; });
        })()"""})
        page_cdp.call("ServiceWorker.deliverPushMessage", {"origin": ORIGIN, "registrationId": registration, "data": plain.decode()})
        seen = lambda: json.loads(sw.call("Runtime.evaluate", {"expression": "JSON.stringify(self.__seen ?? null)", "returnByValue": True})["result"]["value"] or "null")  # noqa: E731
        shown = wait_for(lambda: (x := seen()) and (x["shown"] or x["failed"]) and x, 15)
    check("the worker shows it, with the hosted page's link", bool(shown) and shown["pushes"] == 1 and len(shown["shown"]) == 1 and shown["shown"][0]["navigate"] == note.get("navigate"), shown or registration)
    if shown and shown["shown"]:
        hook(dA, alert_shell["id"], "Stop")
        # A person's tap cannot be made through DevTools: the event is dispatched in the worker, on the notification it shows (or one like it, when the
        # browser does not list it back), and the rest is the worker's own code.
        sw.call("Runtime.evaluate", {"expression": "self.registration.getNotifications().then((ns) => { const e = new ExtendableEvent('notificationclick'); e.notification = ns[0] ?? {data: {navigate: %s}, close() {}}; self.dispatchEvent(e); })" % json.dumps(note.get("navigate")), "awaitPromise": True})
        check("a tap on it opens that session on the hosted page", bool(phone.wait("return location.origin === arguments[0] && !!document.querySelector('.session')", 20, ORIGIN)), phone.js("return location.href"))
        phone.js("history.back()")
    if registration is not None:
        sw.close()
    page_cdp.close()
    dA.call("session.close", {"handle": alert_shell["handle"]})

    # ---------- headers and what is served ----------
    section("headers, CSP, and what is live against the manifest")
    rules = parse_headers(open(os.path.join(bundle_a, "_headers"), encoding="utf-8").read())
    asset = next("/" + f["path"] for f in manifest_a["files"] if f["path"].startswith("assets/") and f["path"].endswith(".js"))
    wrong = {}
    for path in ("/", "/sw.js", "/version.json", "/manifest.webmanifest", "/icon.svg", asset):
        status, headers, _ = get(path)
        if status != 200:
            wrong[path] = status
            continue
        for k, v in expected_headers(rules, path).items():
            if headers.get(k.lower()) != v:
                wrong.setdefault(path, {})[k] = headers.get(k.lower())
    check("every file comes with exactly the headers the bundle's _headers gives it", not wrong, wrong)
    status, headers, body = get("/no-such-file")
    check("an unknown path is the bundle's 404 page, not the Worker", status == 404 and headers.get("content-type", "").startswith("text/html") and b"Not found" in body, (status, headers.get("content-type")))
    violations = phone.csp_violations()
    check("no content security policy violation on the hosted page, from install through alerts", not violations, violations[:3])
    manifest_file_a = os.path.join(RUN, "a", "hosted-phone-manifest.json")
    live = verify_live(manifest_file_a)
    check("verify_live.py: what the origin serves is the manifest's (exit 0)", live.returncode == 0, live.stdout[-600:] + live.stderr[-600:])
    icon = os.path.join(copy_main, "public", "icon.svg")
    original = open(icon, "rb").read()
    changed = bytearray(original)
    changed[len(changed) // 2] ^= 0x01
    open(icon, "wb").write(bytes(changed))
    served = wait_for(lambda: get("/icon.svg")[2] == bytes(changed), 20)
    if not served:  # wrangler did not pick the change up by itself: it serves what it read at start
        print("note: wrangler restarted to serve the changed file", flush=True)
        wrangler.stop()
        wrangler.start()
    tampered = verify_live(manifest_file_a)
    check("…and exit 1 once one byte of a served file changed", tampered.returncode == 1, tampered.returncode)
    open(icon, "wb").write(original)
    if not wait_for(lambda: get("/icon.svg")[2] == original, 20):
        wrangler.stop()
        wrangler.start()
    # Wrangler reloads the Worker when a served file changes, which drops every socket: the computer and the page come back first.
    wait_for(lambda: relay_connected(dA), 90)
    phone.wait(CONNECTED, 60)

    # ---------- a new version while the app is open ----------
    section("a new version while the app is open")
    agent = os.path.join(WORK, "tiny_chat_agent.py")
    open(agent, "w", encoding="utf-8").write(TINY_CHAT_AGENT)
    chat = dA.call("session.create", {"kind": "claude", "cwd": WORK, "chat_argv": [sys.executable, "-u", agent], "name": "hosted-chat"})
    phone.go(ORIGIN + "/")
    stuck = unlock(phone, SHELL_NAME)
    rows = lambda: phone.js("return [...document.querySelectorAll('.row')].map(r => r.textContent.slice(0, 60))")  # noqa: E731
    reopened = stuck is None and bool(phone.wait(OPEN_WITH, 30, "hosted-chat"))
    if not reopened:
        print("note: after reopening:", {k: v for k, v in device(dA, PHONE_NAME).items() if k in ("unlocked", "passkey", "state")}, phone.socket_story()[-12:], daemon_log(os.environ["KELPIE_DATA_DIR"], "relay", 10), flush=True)
    check("the page opens again, listing the chat beside the shell", reopened, stuck or rows())
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes(arguments[0])).click()", SHELL_NAME)
    phone.wait("return !!document.querySelector('.send textarea')", 20)
    phone.js("const t = document.querySelector('.send textarea'); t.value = arguments[0]; t.dispatchEvent(new Event('input', {bubbles: true}));", DRAFT)
    phone.js("history.back()")
    phone.wait("return !!document.querySelector('.row')", 15)
    check("a draft is kept on the phone", bool(phone.wait("return Object.keys(localStorage).some(k => k.startsWith('kelpie-draft:') && localStorage.getItem(k).includes(arguments[0]))", 10, DRAFT)))
    loaded_a = phone.js("return [...performance.getEntriesByType('resource')].map(e => e.name).filter(n => n.includes('-adapter-'))")
    adapter_a = next("/" + f["path"] for f in manifest_a["files"] if re.match(r"assets/claude-adapter-.*\.js$", f["path"]))
    index_b = re.search(r'src="(/assets/index-[^"]+\.js)"', open(os.path.join(bundle_b, "index.html"), encoding="utf-8").read()).group(1)
    caches_before = phone.js_async("caches.keys().then((k) => done(k.filter((n) => n.startsWith('kelpie-phone-'))));")
    check("page A is open with no chat chunk loaded yet, its worker holding A's files", not loaded_a and len(caches_before or []) == 1, (loaded_a, caches_before))
    put_public(copy_main, bundle_b)
    wrangler.stop()
    wrangler.start()
    status_old = get(adapter_a)[0]
    check("version B is live: A's chat chunk is gone from the origin", status_old == 404 and adapter_a not in {"/" + f["path"] for f in manifest_b["files"]}, status_old)
    wait_for(lambda: relay_connected(dA), 90)
    phone.js("window.__pageA = true; window.__taken = 0; navigator.serviceWorker.addEventListener('controllerchange', () => { window.__taken++; });")
    # The browser looks for a new worker on its own now and then (each navigation, at most a day apart); here it is asked to look now.
    phone.js_async("navigator.serviceWorker.getRegistration().then((r) => r.update()).then(() => done(true), (e) => done(String(e)));")
    check("B's worker takes the open page over", bool(phone.wait("return window.__taken > 0", 40)), phone.js("return window.__taken"))
    kept = phone.js_async("caches.keys().then((k) => done(k.filter((n) => n.startsWith('kelpie-phone-'))));")
    check("…keeping A's files beside its own", len(kept or []) == 2 and set(caches_before or []) <= set(kept or []), kept)
    phone.wait(CONNECTED, 60)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes('hosted-chat')).click()")
    shown = phone.wait("return document.body.textContent.includes('hello from the hosted chat')", 30)
    entries = phone.js("return [...performance.getEntriesByType('resource')].filter(e => e.name.endsWith(arguments[0])).map(e => [e.name, e.responseStatus])", adapter_a)
    missing = [(u, s) for u, s, _ in phone.responses() if "/assets/" in u and s == 404]
    check("a chat opened on page A loads A's chunk from A's kept cache: no 404, the chat shows", bool(shown) and entries and all(s == 200 for _, s in entries) and not missing and phone.js("return window.__pageA === true"), (entries, missing, bool(shown)))
    phone.js("history.back()")
    phone.wait("return !!document.querySelector('.row')", 15)
    phone.js(SET_HIDDEN, True)
    time.sleep(1)
    phone.js(SET_HIDDEN, False)
    now_b = phone.wait("return !window.__pageA && document.querySelector('script[src*=\"/assets/index-\"]')?.getAttribute('src')", 30)
    check("at its next foreground the page loads again, into B", now_b == index_b, (now_b, index_b))
    unlock(phone, SHELL_NAME)
    phone.js("window.__pageB = true")
    phone.js(SET_HIDDEN, True)
    time.sleep(1)
    phone.js(SET_HIDDEN, False)
    time.sleep(4)
    check("…once: the next foreground does not load it again", phone.js("return window.__pageB === true") is True)
    unlock(phone, SHELL_NAME)
    phone.js("[...document.querySelectorAll('.row')].find(r => r.textContent.includes(arguments[0])).click()", SHELL_NAME)
    kept_draft = phone.wait("return document.querySelector('.send textarea')?.value", 20)
    check("the draft typed in A is there in B", kept_draft == DRAFT, kept_draft)
    phone.js("history.back()")
    phone.wait("return !!document.querySelector('.row')", 15)
    live_b = verify_live(os.path.join(RUN, "b", "hosted-phone-manifest.json"))
    check("verify_live.py: B is what is live now", live_b.returncode == 0, live_b.stdout[-400:])

    # ---------- replacing the computer ----------
    section("pairing another computer replaces this one")
    dB = phone_box["dB"] = other_daemon("phone-hosted-b", daemon_env(RELAY_URL, push.base))
    hosted_setup(dB)
    check("a second computer turns the relay on and finds the app", bool(wait_for(lambda: hosted_ready(dB), 40)))
    held = phone.js_async("navigator.serviceWorker.ready.then((r) => r.pushManager.getSubscription()).then((s) => done(s ? s.endpoint : null), () => done(null));")
    check("after the reload the page still holds the stand-in's subscription, and no other", phone.js("return window.__standInPush === true") is True and held == sub["endpoint"], held)
    phone.js("window.__asked = []; window.confirm = (q) => { window.__asked.push(q); return true; };")
    link_b = dB.call("mobile.pair_start", {"hosted": True})["url"]
    name_b = PHONE_NAME + " on B"
    phone.js("location.hash = arguments[0]", "#" + link_b.split("#", 1)[1])
    phone.wait("return !!document.querySelector('.card input.field')", 20)
    phone.js("const i = document.querySelector('.card input.field'); i.value = arguments[0]; i.dispatchEvent(new Event('input', {bubbles: true})); document.querySelector('.card .btn.primary').click();", name_b)
    digits_b = phone.wait("return document.querySelector('[data-pair-code]')?.textContent", 30)
    asked = phone.js("return window.__asked")
    check("the phone asks before it replaces the computer it is paired with", asked and len(asked) == 1 and "This app is paired with" in asked[0] and "loses this phone" in asked[0], asked)
    check("…and once told yes, ends the browser's subscription for the first computer's alerts", int(phone.js("return localStorage.getItem('e2e-push-ended') || '0'")) >= 1, phone.js("return localStorage.getItem('e2e-push-ended')"))
    if digits_b:
        approve_on(dB, name_b, digits_b)
    check("it pairs with the second computer, as a hosted page", bool(wait_for(lambda: (x := device(dB, name_b)) and x.get("page_kind") == "hosted", 30)))
    # The push service now answers the first computer's sends to that address with 410, as it does once the browser ended the subscription.
    push.answer("/push/hosted", 410, 410, 410)
    sent_before = len(push.at("/push/hosted"))
    # The first computer alerts that phone: its alert test (Settings' button), which goes to the phone's address as any alert does. (An event's
    # alert also depends on this machine's windows and on whether its person is idle, which a test cannot hold still.)
    tested = refused(lambda: dA.call("notify.test", {"channel": "webpush"}))
    gone = wait_for(lambda: not device(dA, PHONE_NAME).get("push"), 30)
    check("…and the first computer drops that phone's alerts at the push service's 410", bool(gone) and len(push.at("/push/hosted")) > sent_before, (device(dA, PHONE_NAME).get("push"), len(push.at("/push/hosted")) - sent_before, tested, dA.call("notify.get").get("webpush")))
    dA.call("session.close", {"handle": chat["handle"]})


def parse_headers(text):
    """Cloudflare's `_headers`: (path pattern, {name: value}) in order."""
    rules, current = [], None
    for line in text.splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        if not line[0].isspace():
            current = (line.strip(), {})
            rules.append(current)
        elif current:
            name, value = line.strip().split(":", 1)
            current[1][name.strip()] = value.strip()
    return rules


def expected_headers(rules, path):
    out = {}
    for pattern, headers in rules:
        if pattern == path or (pattern.endswith("*") and path.startswith(pattern[:-1])):
            out.update(headers)
    return out


def verify_live(manifest):
    env = {k: v for k, v in os.environ.items() if not k.startswith(("CLOUDFLARE_", "GH_", "GITHUB_"))}
    env["SSL_CERT_FILE"] = CA_FILE
    return subprocess.run([sys.executable, "-I", VERIFY_TOOL, "--origin", ORIGIN, "--manifest", manifest, "--timeout", "15"], env=env, capture_output=True, text=True, timeout=300)


def limits():
    section("limits: rooms per network, and a closed relay")
    copy = relay_copy("relay-limits")
    persist = os.path.join(RUN, "persist-limits")
    ticket = secrets.token_urlsafe(32)
    w = Wrangler(copy, LIMIT_PORT, persist, INSPECTOR + 1, {"TICKET_SECRET": ticket, "OPEN_REGISTRATION": "1", "ROOMS_PER_NETWORK_DAY": "2"}).start()
    try:
        dC = other_daemon("phone-hosted-c", daemon_env(f"wss://localhost:{LIMIT_PORT}"))
        try:
            _, turned = hosted_setup(dC)
            check("a room on the limited relay (1 of 2 today)", turned is None and bool(wait_for(lambda: relay_connected(dC), 40)), turned)
            second = refused(lambda: dC.call("mobile.relay_recreate"))
            check("…a second (2 of 2)", second is None, second)
            third = refused(lambda: dC.call("mobile.relay_recreate"))
            check("the third is refused, and the daemon says why", third and "too many rooms today" in third, third)
            w.stop()
            w.variables = {**w.variables, "REGISTRATION_KEY": secrets.token_urlsafe(24)}
            w.start()
            closed = refused(lambda: dC.call("mobile.relay_recreate"))
            check("with a registration key set, a room without it is refused saying a key is needed", closed and "needs a registration key" in closed, closed)
        finally:
            dC.stop()
    finally:
        w.stop()
    return persist


def gui():
    """The desktop: a new user turns phone access on in Settings and pairs the hosted app."""
    section("desktop: Settings for a new user")
    from smoke_support import daemon as app_call  # noqa: PLC0415
    from wd import Driver  # noqa: PLC0415

    app = os.path.abspath(args.gui)
    if not os.path.isfile(app) or any(os.path.normcase(app).startswith(os.path.normcase(p) + os.sep) for p in INSTALLED if p):
        return check("a debug app to drive", False, app)
    if not args.tauri_driver:
        return check("tauri-driver found", False)
    endpoint, data = f"kelpie-phone-hosted-gui-{PID}", tempfile.mkdtemp(prefix="kelpie-phone-hosted-gui-")
    refuse_default(endpoint)
    ps.MADE.add(endpoint)
    ENDPOINTS.append((endpoint, data))
    webview = os.path.join(RUN, "webview")
    env = {k: v for k, v in os.environ.items() if not k.startswith(("KELPIE_", "CLOUDFLARE_", "WEBVIEW2_"))}
    env.update({"KELPIE_ENDPOINT": endpoint, "KELPIE_DATA_DIR": data, "KELPIE_E2E": "1", "KELPIE_TEST_USER_PRESENCE": "approve", "KELPIE_LOAD_FORCE": "L0:calm",
                "KELPIE_RELAY_TEST_URL": RELAY_URL, "KELPIE_RELAY_TEST_CA": CA_FILE, "KELPIE_DAEMON_PATH": exe,
                # Its own WebView2 profile: the installed app's (by the same identifier) is never shared.
                "WEBVIEW2_USER_DATA_FOLDER": webview})
    driver_proc = start("tauri-driver", [args.tauri_driver, "--port", str(TAURI_PORT), "--native-port", str(NATIVE_PORT), "--native-driver", args.edgedriver], env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    wait_for(lambda: not port_free(TAURI_PORT), 20)
    d = None
    phone = None
    gui_daemon_pid = None
    try:
        d = Driver(app, url=f"http://127.0.0.1:{TAURI_PORT}")
        note_tree("tauri-driver", driver_proc)
        d.wait("return !!(window.__kelpie && window.__kelpie.store.loaded())", 90)
        d.wait("return window.__kelpie.store.connected()", 60)
        info = app_call(d, "daemon.info")
        gui_daemon_pid = info.get("pid")
        if gui_daemon_pid:
            seen(gui_daemon_pid, "kelpied (app's)")
        same = os.path.normcase(os.path.normpath(str(info.get("data_dir", "")))) == os.path.normcase(os.path.normpath(data))
        if not (str(info.get("endpoint", "")).endswith(endpoint) and same and info.get("test_user_presence") == "approve"):
            return check("the app talks to this run's own daemon, which answers verifications itself", False, (info.get("endpoint"), info.get("data_dir")))
        check("the app talks to this run's own daemon, which answers verifications itself", True)
        d.js("window.__kelpie.store.setSettings({ tips: false, language: 'en', shortcutHints: false })")
        d.set_window_size(1480, 920)
        d.js("window.confirm = () => true")
        d.js("window.__kelpie.store.openSettings('mobile')")
        d.wait("return !!document.querySelector('[data-setting=\"mobile-access\"] .switch')", 30)
        no_relay = d.js("return document.querySelector('[data-setting=\"mobile-hosted\"]')?.textContent ?? ''")
        check("before: the hosted row says what comes first, and offers no pairing", ("Turn on the relay first" in no_relay or "Turn on phone access first" in no_relay) and not d.js("return !!document.querySelector('[data-hosted-pair]')"), no_relay[:200])
        d.js("document.querySelector('[data-setting=\"mobile-access\"] .switch').click()")
        # A new setup lands on Pair a phone with the relay first; the relay's way is offered once Kelpie has found the app served.
        try:
            d.wait("return !!document.querySelector('[data-pair-chooser] [data-pair-method=relay][data-preselected]')", 60)
            d.wait("return !!document.querySelector('[data-hosted-pair]')", 60)
        except TimeoutError:
            pass
        ways = d.js("""return [...document.querySelectorAll('[data-pair-method]')].map((m) => ({id: m.dataset.pairMethod, available: m.dataset.available === 'true',
          recommended: m.hasAttribute('data-recommended'), preselected: m.hasAttribute('data-preselected'), why: m.querySelector('[data-pair-why]')?.textContent ?? null,
          label: m.querySelector('.pair-method-head b')?.textContent ?? ''}))""")
        by = {w["id"]: w for w in ways or []}
        check("a new setup lands on Pair a phone with Kelpie's relay first, offered now and recommended", by.get("relay", {}).get("preselected") and by["relay"]["available"] and by["relay"]["recommended"] and by["relay"]["label"] == "Kelpie's relay"
              and d.js("return document.querySelector('[data-hosted-pair]')?.classList.contains('primary') ?? false") is True, ways)
        check("…Tailscale and this network are listed, each saying why it cannot pair now", not by.get("tailscale", {}).get("available", True) and bool(by["tailscale"]["why"])
              and not by.get("network", {}).get("available", True) and "listeners are off" in (by["network"]["why"] or ""), ways)
        step1 = None
        try:
            d.js("document.querySelector('[data-hosted-pair]').click()")
            step1 = d.wait("return document.querySelector('[data-hosted-guide=\"1\"] [data-install-link]')?.dataset.installLink", 60)
        except Exception:  # noqa: BLE001
            pass
        status = app_call(d, "mobile.status")
        audit = app_call(d, "mobile.audit", {"limit": 100})
        entries = audit.get("entries", audit if isinstance(audit, list) else [])
        relay_on = [e for e in entries if e.get("action") == "relay_on" or "relay_on" in json.dumps(e)]
        check("turning phone access on sets up the relay, with this network's listeners off", status["enabled"] and status.get("lan") is False and status["relay"]["on"] and len(relay_on) == 1, (status.get("lan"), status["relay"].get("on"), len(relay_on), d.js("return [...document.querySelectorAll('.form-note.error')].map(e => e.textContent)")))
        check("…and the guide opens on the hosted app: Step 1 is its address, with no secret", step1 == ORIGIN + "/" and d.js("return document.querySelector('[data-pair-guide]')?.dataset.guideVia") == "hosted", step1)
        # A person installs the app between the two steps; by then the relay connection is up (the guide opens once the app is found).
        wait_for(lambda: (r := app_call(d, "mobile.status")["relay"]).get("connected") and r, 30)
        d.js("document.querySelector('[data-guide-next]').click()")
        try:
            link = d.wait("return document.querySelector('[data-hosted-guide=\"2\"] .mobile-qr-code')?.dataset.link", 30)
        except TimeoutError:
            link = None
        relay_now = app_call(d, "mobile.status")["relay"]
        check("Step 2 is a pairing code on the hosted app", bool(link) and link.startswith(ORIGIN + "/#p="), (d.js("return document.querySelector('[data-guide-error]')?.textContent ?? null"), {k: relay_now.get(k) for k in ("on", "connected", "version", "error")}, {k: (relay_now.get("hosted") or {}).get(k) for k in ("available", "reason", "origin")}))
        if not link:
            raise RuntimeError("no pairing code in Step 2")
        phone = EdgePhone(PHONE_DRIVER, os.path.join(RUN, "gui-phone-profile"))
        # Opened from Step 1's address, before any code: the app says which way it is, and the other ways to pair this phone.
        phone.go(ORIGIN + "/")
        hosted_ways = phone.wait("const w = document.querySelector('[data-pair-ways]'); return w && {way: w.dataset.pairWays, text: w.textContent}", 30)
        check("before pairing, the hosted app says it is the app from Kelpie's relay, and lists the other ways with where to choose them", bool(hosted_ways) and hosted_ways["way"] == "relay"
              and "This is the app from Kelpie's relay" in hosted_ways["text"] and "Tailscale:" in hosted_ways["text"] and "This network:" in hosted_ways["text"] and "Pair a phone" in hosted_ways["text"], hosted_ways)
        phone.go("about:blank")
        phone.go(link)
        if phone.wait("return !!document.querySelector('[data-stay-in-browser]')", 20):
            phone.js("document.querySelector('[data-stay-in-browser]').click()")
        phone.wait("return !!document.querySelector('.card input.field')", 20)
        name = "GUI phone " + secrets.token_hex(3)
        phone.js("const i = document.querySelector('.card input.field'); i.value = arguments[0]; i.dispatchEvent(new Event('input', {bubbles: true})); document.querySelector('.card .btn.primary').click();", name)
        digits = phone.wait("return document.querySelector('[data-pair-code]')?.textContent", 30)
        d.wait(f"return [...document.querySelectorAll('.mobile-approve h2')].some(h => h.textContent.includes('{name}'))", 30)
        approval = "[...document.querySelectorAll('.mobile-approve')].find(a => a.textContent.includes(arguments[0]))"
        offered = d.js(f"const a = {approval}; return a ? {{kind: a.dataset.originKind, allow: [...a.querySelectorAll('[data-allow]')].map(b => b.dataset.allow), page: a.querySelector('[data-request-page]')?.textContent ?? ''}} : null", name)
        check("the approval for a hosted phone says where it pairs and offers no control", offered and offered["kind"] == "hosted" and "control" not in offered["allow"] and set(offered["allow"]) <= {"read", "write"} and "hosted app" in offered["page"], offered)
        d.js("const i = document.querySelector('.mobile-approve-code'); i.value = arguments[0]; i.dispatchEvent(new Event('input', {bubbles: true}));", digits)
        d.js(f"{approval}.querySelector('[data-allow=write]').click()", name)
        check("the phone opened from Step 2's link pairs", bool(phone.wait("return !!document.querySelector('[data-trust]') || !!document.querySelector('[data-lock-screen]')", 40)))
        listed = wait_for(lambda: next((x for x in app_call(d, "mobile.status")["devices"] if x["name"] == name), None), 15)
        check("…as a hosted page", listed and listed.get("page_kind") == "hosted", listed)
        d.js("document.querySelector('[data-guide-close]')?.click()")
        d.js("const a = document.querySelector('[data-mobile-advanced]'); if (a) { a.open = true; a.dispatchEvent(new Event('toggle')); }")
        badge = None
        try:
            badge = d.wait("return document.querySelector('[data-page-kind=\"hosted\"]')?.textContent", 20)
        except TimeoutError:
            pass
        check("the device list badges it as the hosted app", badge == "hosted app", badge)
        trust = d.js("return document.querySelector('[data-hosted-trust]')?.textContent ?? ''")
        check("\"What this trusts\" is there, naming the relay's deploy", "What this trusts" in trust and "Cloudflare" in trust, trust[:300])
        try:
            app_call(d, "daemon.shutdown", {"kill_sessions": True})
        except Exception:  # noqa: BLE001
            pass
    except (Exception, SystemExit) as e:  # noqa: BLE001
        import traceback
        traceback.print_exc()
        check("the desktop part ran to completion", False, e)
    finally:
        if phone:
            phone.quit()
        if d:
            d.quit()
        stop_tree(driver_proc, "tauri-driver")
        if gui_daemon_pid and psutil.pid_exists(gui_daemon_pid):
            # The app's daemon outlives the app: stopped through its own endpoint, else by its PID.
            with Endpoint(endpoint, data):
                try:
                    ps.kc.Conn().call("daemon.shutdown", {"kill_sessions": True})
                except BaseException:  # noqa: BLE001
                    pass
            if not wait_for(lambda: not psutil.pid_exists(gui_daemon_pid), 15):
                try:
                    psutil.Process(gui_daemon_pid).kill()
                except psutil.Error:
                    pass


def search_state(folders, words):
    found = []
    files = 0
    for folder in folders:
        for base, _, names in os.walk(folder):
            for n in names:
                files += 1
                data = open(os.path.join(base, n), "rb").read()
                found += [(n, w) for w in words if w.encode() in data or w.encode("utf-16-le") in data]
    return files, found


def run():
    phone_box = {}
    push = ps.FakePushService()
    wrangler = None
    dA = None
    persist_main = os.path.join(RUN, "persist")
    persist_limits = None
    try:
        section("building version A and version B of the hosted app")
        bundle_a, manifest_a = build_bundle("a")
        bundle_b, manifest_b = build_bundle("b", APP_VERSION + "-b")
        print(f"A {manifest_a['bundle_sha256'][:16]}…, B {manifest_b['bundle_sha256'][:16]}…", flush=True)
        copy_main = relay_copy("relay-main", bundle_a)
        wrangler = Wrangler(copy_main, RELAY_PORT, persist_main, INSPECTOR, {"TICKET_SECRET": secrets.token_urlsafe(32), "OPEN_REGISTRATION": "1"}).start()
        dA = ps.Daemon(exe, env=daemon_env(RELAY_URL, push.base)).start()
        seen(dA.pid, "kelpied A")
        if dA.call("daemon.info").get("test_user_presence") != "approve":
            raise SystemExit("the daemon would raise a real verification prompt: use a debug build")
        try:
            main(phone_box, push, dA, wrangler, bundle_a, manifest_a, bundle_b, manifest_b, copy_main)
        except (Exception, SystemExit) as e:  # noqa: BLE001  (a refused daemon call is a SystemExit)
            import traceback
            traceback.print_exc()
            check("the phone scenario ran to completion", False, e)
        for k in ("webauthn", "push"):
            if phone_box.get(k):
                phone_box[k].close()
        if phone_box.get("phone"):
            phone_box.pop("phone").quit()
        if args.gui:
            gui()
        wrangler.stop()
        try:
            persist_limits = limits()
        except (Exception, SystemExit) as e:  # noqa: BLE001
            import traceback
            traceback.print_exc()
            check("the limits ran to completion", False, e)
        section("what the relay kept")
        files, found = search_state([p for p in (persist_main, persist_limits) if p], [MARKER, MARKER[::-1], PHONE_NAME, SHELL_NAME, DRAFT, "hosted-chat", "hello from the hosted chat"])
        check("nothing a session said, typed or was called, and no phone's name, is in the relay's state", files > 0 and not found, (files, found))
    finally:
        if phone_box.get("phone"):
            phone_box["phone"].quit()
        for d in (phone_box.get("dB"), dA):
            if d:
                try:
                    d.stop()
                except Exception:  # noqa: BLE001
                    pass
        if wrangler:
            wrangler.stop()
        push.stop()
        for what, proc in STARTED:
            stop_tree(proc, what)
        for endpoint, data in ENDPOINTS:
            ps.forget_secrets(endpoint)
            with Endpoint(endpoint, data):
                try:
                    ps.forget_token(endpoint)
                except (ValueError, OSError):
                    pass
            shutil.rmtree(data, ignore_errors=True)


try:
    run()
except SystemExit as e:
    check("the run was set up", False, e)
finally:
    time.sleep(1)
    still = {}
    for pid, (what, started) in SEEN_PIDS.items():
        try:
            p = psutil.Process(pid)
            if pid != PID and abs(p.create_time() - started) < 0.01:
                still[pid] = (what, p.name())
        except psutil.Error:
            pass
    print(f"\nprocesses of this run still running: {still or 'none'}", flush=True)
    check("no process of this run is left", not still, still)
    data_dir = os.environ.get("KELPIE_DATA_DIR")
    for folder in (RUN, data_dir):
        for _ in range(10):
            shutil.rmtree(folder, ignore_errors=True)
            if not os.path.exists(folder):
                break
            time.sleep(1)
    failed = [n for n, ok in results if not ok]
    print(f"\n{len(results) - len(failed)} of {len(results)} passed" + (f"; failed: {failed}" if failed else ""))
    sys.exit(1 if failed else 0)
