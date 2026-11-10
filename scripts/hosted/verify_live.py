#!/usr/bin/env python3
"""Check what the hosted phone app's origin serves against a release's manifest (standard library only).

    python -I scripts/hosted/verify_live.py                       # production, the tag it says it runs
    python -I scripts/hosted/verify_live.py --tag v0.1.16 --manifest hosted-phone-manifest.json
    python -I scripts/hosted/verify_live.py --env staging --manifest hosted-phone-manifest.json
    python -I scripts/hosted/verify_live.py --tag v0.1.16 --rebuild          # also build the tag here and compare
    python -I scripts/hosted/verify_live.py --tag v0.1.16 --scripts          # maintainer: compare the deployed Worker scripts

Fetches every file the manifest lists (`Cache-Control: no-cache`) and compares its SHA-256 and size; checks `/version.json` (tag,
commit, `bundle_sha256`); checks each response's headers against the manifest's `_headers` rules; checks that the service worker's
precache list and the scripts `index.html` loads are all files of the manifest. The manifest (`hosted-phone-manifest.json`, made by
`bundle_manifest.py`) comes from the tag's GitHub release unless `--manifest` names one.

`--rebuild` checks out the tag in a temporary directory, builds the hosted bundle there (pnpm; the release builds on Linux with
Node 24, and another platform may build different bytes) and compares the two manifests. `--scripts` reads `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID` from the environment (never printed, never passed to the build), downloads the deployed router and room
scripts through the Cloudflare API and compares them with `wrangler deploy --dry-run` of the tag. `--bundle DIR` checks a local bundle
folder against the manifest instead of an origin (what CI does before it deploys the folder).

Prints paths and hashes only, never what the origin sent. Exit 0: everything matched; 1: any difference; 2: usage or network error
(1 when a difference was found before the error).
"""
import argparse
import email.parser
import email.policy
import hashlib
import html.parser
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import http.client

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bundle_manifest as bm  # noqa: E402

ENVS = {
    "production": {
        "origin": "https://kelpie-link-relay.baopeak.workers.dev",
        "router": "kelpie-link-relay",
        "room": "kelpie-link-relay-room",
        "wrangler_env": "",
    },
    "staging": {
        "origin": "https://kelpie-link-relay-staging.baopeak.workers.dev",
        "router": "kelpie-link-relay-staging",
        "room": "kelpie-link-relay-room-staging",
        "wrangler_env": "staging",
    },
}
REPO_SLUG = "baopeak/kelpie"
RELEASE_URL = "https://github.com/{repo}/releases/download/{tag}/" + bm.MANIFEST_FILE
CF_SCRIPT_URL = "https://api.cloudflare.com/client/v4/accounts/{account}/workers/scripts/{script}"
TAG_RE = re.compile(r"^v[0-9A-Za-z][0-9A-Za-z.+-]{0,63}$")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
MAX_BODY = 32 << 20
USER_AGENT = "kelpie-verify-live"
CANNOT_PROVE = (
    "What this cannot prove: it saw what the origin served to this machine at this moment. Another visitor, another time, or code a "
    "phone already cached may differ."
)


class NetworkError(Exception):
    """The check could not finish: no answer, a broken connection or a failing tool."""


class UsageError(Exception):
    pass


def safe(text, limit=200):
    """`text` with anything but printable ASCII escaped, for printing what a server chose."""
    out = "".join(c if 32 <= ord(c) < 127 else "\\x%02x" % ord(c) if ord(c) < 256 else "\\u%04x" % (ord(c) & 0xFFFF) for c in str(text))
    return out if len(out) <= limit else out[:limit] + "..."


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def origin_of(url):
    u = urllib.parse.urlsplit(url)
    return "%s://%s" % (u.scheme, u.netloc)


# --- Cloudflare's `_headers` -------------------------------------------------------------------------------------------------------

def parse_headers_file(text):
    """[(pattern, [(name lowercased, value)])] in file order. A line at column 0 is a URL pattern, an indented one `Name: value`."""
    rules = []
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if raw[0] in " \t":
            if not rules:
                raise ValueError("a header before any path in _headers")
            if line.startswith("!"):
                rules[-1][1].append(("!" + line[1:].strip().lower(), ""))
                continue
            name, sep, value = line.partition(":")
            if not sep:
                raise ValueError("a header line without ':' in _headers")
            rules[-1][1].append((name.strip().lower(), value.strip()))
        else:
            rules.append((line, []))
    return rules


def _pattern(pattern):
    out = []
    for part in re.split(r"(\*|:[A-Za-z_][A-Za-z0-9_]*)", pattern):
        if part == "*":
            out.append(".*")
        elif part.startswith(":") and len(part) > 1:
            out.append("[^/]+")
        else:
            out.append(re.escape(part))
    return re.compile("^" + "".join(out) + "$")


def expected_headers(rules, path):
    """What the rules set for `path`: every matching rule's headers, values of one name joined by ", ", `! name` removing one."""
    out = {}
    for pattern, headers in rules:
        if not _pattern(pattern).match(path):
            continue
        for name, value in headers:
            if name.startswith("!"):
                out.pop(name[1:].strip(), None)
            elif name in out:
                out[name] = out[name] + ", " + value
            else:
                out[name] = value
    return out


# --- Fetching ----------------------------------------------------------------------------------------------------------------------

class OffOrigin(Exception):
    pass


class _SameOrigin(urllib.request.HTTPRedirectHandler):
    def __init__(self, origin):
        super().__init__()
        self.origin = origin

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        if origin_of(urllib.parse.urljoin(req.full_url, newurl)) != self.origin:
            raise OffOrigin()
        return super().redirect_request(req, fp, code, msg, headers, newurl)


class Response:
    def __init__(self, status, path, headers, body):
        self.status, self.path, self.headers, self.body = status, path, headers, body

    def header(self, name):
        values = self.headers.get_all(name) or []
        return ", ".join(v.strip() for v in values) if values else None


def _read(resp):
    body = resp.read(MAX_BODY + 1)
    if len(body) > MAX_BODY:
        raise NetworkError("an answer larger than %d bytes" % MAX_BODY)
    return body


def open_url(opener, url, headers, timeout):
    """(status, final url, headers, body); HTTP error statuses are answers, not errors."""
    req = urllib.request.Request(url, headers=dict(headers, **{"User-Agent": USER_AGENT}))
    try:
        with opener.open(req, timeout=timeout) as resp:
            return resp.status, resp.geturl(), resp.headers, _read(resp)
    except urllib.error.HTTPError as e:
        with e:
            return e.code, e.geturl() or url, e.headers, _read(e)
    except OffOrigin:
        raise
    except (urllib.error.URLError, OSError, http.client.HTTPException, ValueError) as e:
        raise NetworkError(type(e).__name__)


class Origin:
    """GETs on one origin, following redirects only within it."""

    def __init__(self, origin, timeout=20):
        self.origin = origin
        self.timeout = timeout
        self.opener = urllib.request.build_opener(_SameOrigin(origin))

    def get(self, path):
        status, url, headers, body = open_url(self.opener, self.origin + path, {"Cache-Control": "no-cache"}, self.timeout)
        return Response(status, urllib.parse.urlsplit(url).path or "/", headers, body)


def download(url, timeout=20, headers=None):
    status, _, resp_headers, body = open_url(urllib.request.build_opener(), url, headers or {}, timeout)
    return status, resp_headers, body


# --- What the bundle says about itself ---------------------------------------------------------------------------------------------

class _Loads(html.parser.HTMLParser):
    """`src` of every script and `href` of every module preload or stylesheet."""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.urls = []

    def handle_starttag(self, tag, attrs):
        a = dict(attrs)
        if tag == "script" and a.get("src"):
            self.urls.append(a["src"])
        elif tag == "link" and a.get("href"):
            rel = (a.get("rel") or "").lower().split()
            if {"modulepreload", "preload", "stylesheet"} & set(rel):
                self.urls.append(a["href"])


def index_loads(body):
    p = _Loads()
    p.feed(body.decode("utf-8", errors="replace"))
    p.close()
    return p.urls


PRECACHE_RE = re.compile(r'\[\s*"/"\s*(?:,\s*"(?:[^"\\]|\\.)*"\s*)*\]')


def precache_lists(sw_body):
    """Every JSON list of paths starting with "/" in the service worker: the build writes its precache list that way."""
    return [json.loads(m.group(0)) for m in PRECACHE_RE.finditer(sw_body.decode("utf-8", errors="replace"))]


def manifest_problems(manifest):
    """What makes a manifest unusable or inconsistent with itself (the hash covers its file list, the `_headers` entry its text)."""
    for key in ("version", "commit", "bundle_sha256", "files", "headers"):
        if key not in manifest:
            return ["manifest has no %s" % key]
    files = [(f["path"], f["sha256"], int(f["size"])) for f in manifest["files"]]
    out = []
    if bm.bundle_hash(files) != manifest["bundle_sha256"]:
        out.append("manifest bundle_sha256 does not match its own file list")
    entry = {p: h for p, h, _ in files}.get(bm.HEADERS_FILE)
    if "index.html" not in {p for p, _, _ in files}:
        out.append("manifest lists no index.html")
    if entry is None:
        out.append("manifest lists no %s" % bm.HEADERS_FILE)
    elif sha256(manifest["headers"].encode("utf-8")) != entry:
        out.append("manifest headers text does not match its %s entry" % bm.HEADERS_FILE)
    return out


# --- The live check ----------------------------------------------------------------------------------------------------------------

class Report:
    def __init__(self, out=sys.stdout):
        self.out = out
        self.diffs = 0

    def ok(self, what, digest=""):
        print(("ok    %s %s" % (what, digest)).rstrip(), file=self.out)

    def diff(self, what, detail=""):
        self.diffs += 1
        print(("DIFF  %s %s" % (what, detail)).rstrip(), file=self.out)

    def note(self, text):
        print(text, file=self.out)


def check_headers(report, rules, resp, label):
    want = expected_headers(rules, resp.path)
    bad = False
    for name, value in sorted(want.items()):
        got = resp.header(name)
        if got != value:
            bad = True
            report.diff("%s header %s" % (label, name), "expected %s served %s" % (sha256(value.encode())[:16], "none" if got is None else sha256(got.encode())[:16]))
    return not bad


def check_live(site, manifest, tag, report):
    """Compare what `site` serves with `manifest`. Differences go to `report`; NetworkError ends the check."""
    for problem in manifest_problems(manifest):
        report.diff("manifest", problem)
    if report.diffs:
        return
    if manifest["version"] != tag:
        report.diff("manifest", "is for %s, not %s" % (safe(manifest["version"]), tag))
    rules = parse_headers_file(manifest["headers"])
    files = {"/" + f["path"]: f for f in manifest["files"]}
    bodies = {}

    def fetch(path):
        try:
            return site.get(path)
        except OffOrigin:
            report.diff(path, "redirected to another origin")
            return None

    # `_headers` is not served (wrangler parses it and never uploads it): the headers of every answer below check it instead.
    for path in ["/"] + sorted(p for p in files if p != "/" + bm.HEADERS_FILE):
        want = files.get(path) or files.get("/index.html")
        resp = fetch(path)
        if resp is None:
            continue
        if resp.status != 200:
            report.diff(path, "status %d" % resp.status)
            continue
        digest = sha256(resp.body)
        if digest != want["sha256"] or len(resp.body) != int(want["size"]):
            report.diff(path, "expected %s served %s" % (want["sha256"], digest))
        elif check_headers(report, rules, resp, path):
            report.ok(path, digest)
        bodies[path] = resp.body

    resp = fetch("/" + bm.VERSION_FILE)
    if resp is not None:
        if resp.status != 200:
            report.diff("/" + bm.VERSION_FILE, "status %d" % resp.status)
        else:
            try:
                info = json.loads(resp.body)
            except ValueError:
                info = None
            want = bm.version_info(manifest["bundle_sha256"], tag, manifest["commit"], manifest.get("api_min"))
            if not isinstance(info, dict):
                report.diff("/" + bm.VERSION_FILE, "not a JSON object")
            else:
                bad = [k for k in ("app", "version", "commit", "bundle_sha256", "api_min") if info.get(k) != want[k]]
                for k in bad:
                    report.diff("/" + bm.VERSION_FILE, "%s differs" % k)
                if not bad and check_headers(report, rules, resp, "/" + bm.VERSION_FILE):
                    report.ok("/" + bm.VERSION_FILE, "bundle_sha256 %s" % manifest["bundle_sha256"])

    if "/sw.js" in bodies:
        lists = precache_lists(bodies["/sw.js"])
        if not lists:
            report.diff("/sw.js", "no precache list found")
        extra = sorted({p for lst in lists for p in lst if p != "/" and p not in files})
        for p in extra:
            report.diff("/sw.js precache", safe(p))
        if lists and not extra:
            report.ok("/sw.js precache", "%d paths" % len({p for lst in lists for p in lst}))
    if "/" in bodies:
        extra = []
        for url in index_loads(bodies["/"]):
            full = urllib.parse.urljoin(site.origin + "/", url)
            path = urllib.parse.urlsplit(full).path
            if origin_of(full) != site.origin or path not in files:
                extra.append(full if origin_of(full) != site.origin else path)
        for u in sorted(set(extra)):
            report.diff("/ loads", safe(u))
        if not extra:
            report.ok("/ loads", "only files of the manifest")


def check_bundle(root, manifest, tag, report):
    """Compare a bundle folder (what a deploy is about to upload) with `manifest`, `version.json` included."""
    for problem in manifest_problems(manifest):
        report.diff("manifest", problem)
    if report.diffs:
        return
    if manifest["version"] != tag:
        report.diff("manifest", "is for %s, not %s" % (safe(manifest["version"]), tag))
    mine = {p: (h, s) for p, h, s in bm.bundle_files(root)}
    theirs = {f["path"]: (f["sha256"], int(f["size"])) for f in manifest["files"]}
    for path in sorted(set(mine) | set(theirs)):
        if mine.get(path) != theirs.get(path):
            report.diff("/" + path, "expected %s found %s" % ((theirs.get(path) or ("none",))[0], (mine.get(path) or ("none",))[0]))
    try:
        with open(os.path.join(root, bm.VERSION_FILE), encoding="utf-8") as f:
            info = json.load(f)
    except (OSError, ValueError):
        info = None
    want = bm.version_info(manifest["bundle_sha256"], tag, manifest["commit"], manifest.get("api_min"))
    if info != want:
        report.diff("/" + bm.VERSION_FILE, "does not say what the manifest says")
    if not report.diffs:
        report.ok("bundle", "bundle_sha256 %s" % manifest["bundle_sha256"])


# --- Rebuilding the tag ------------------------------------------------------------------------------------------------------------

def child_env(extra=None):
    """The environment for builds: never the Cloudflare credentials (an install runs package scripts)."""
    env = {k: v for k, v in os.environ.items() if k not in ("CLOUDFLARE_API_TOKEN", "CLOUDFLARE_ACCOUNT_ID")}
    env.pop("KELPIE_APP_VERSION", None)
    env["WRANGLER_SEND_METRICS"] = "false"
    env.update(extra or {})
    return env


def run_tool(argv, cwd, env):
    """Run a build tool; its output is shown only when it fails."""
    exe = shutil.which(argv[0])
    if not exe:
        raise NetworkError("%s is not installed" % argv[0])
    p = subprocess.run([exe] + argv[1:], cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    if p.returncode != 0:
        sys.stderr.write(p.stdout.decode("utf-8", errors="replace")[-4000:])
        raise NetworkError("%s failed (exit %d)" % (" ".join(argv[:3]), p.returncode))
    return p.stdout.decode("utf-8", errors="replace")


class Checkout:
    """The tag, cloned once into a temporary directory (line endings as committed)."""

    def __init__(self, tag, source, tmp, run=run_tool):
        self.dir = os.path.join(tmp, "src")
        self.run = run
        run(["git", "-c", "core.autocrlf=false", "clone", "--quiet", "--depth", "1", "--branch", tag, source, self.dir], tmp, child_env())
        self.commit = run(["git", "rev-parse", "HEAD"], self.dir, child_env()).strip()


def rebuild(checkout, tag, origin):
    """The manifest of the hosted bundle built from `checkout`, built for `origin`."""
    app = os.path.join(checkout.dir, "app")
    env = child_env({"KELPIE_HOSTED_ORIGIN": origin})
    checkout.run(["pnpm", "install", "--frozen-lockfile"], app, env)
    checkout.run(["pnpm", "build:phone:hosted"], app, env)
    api = bm.hosted_api(os.path.join(app, "src", "lib", "attention.ts"))
    return bm.build(os.path.join(app, "dist-phone-hosted"), tag, checkout.commit, api)


def compare_manifests(report, built, manifest):
    mine = {f["path"]: f["sha256"] for f in built["files"]}
    theirs = {f["path"]: f["sha256"] for f in manifest["files"]}
    for path in sorted(set(mine) | set(theirs)):
        if mine.get(path) != theirs.get(path):
            report.diff("rebuild /" + path, "built %s manifest %s" % (mine.get(path, "none"), theirs.get(path, "none")))
    for key in ("commit", "api_min", "bundle_sha256"):
        if built.get(key) != manifest.get(key):
            report.diff("rebuild", "%s differs" % key)
    if not report.diffs:
        report.ok("rebuild", "bundle_sha256 %s" % built["bundle_sha256"])


# --- The deployed Worker scripts ---------------------------------------------------------------------------------------------------

def script_modules(content_type, body):
    """{module name: bytes} of a script download: a module Worker comes back as multipart/form-data, one part per module."""
    if not (content_type or "").lower().startswith("multipart/"):
        return {None: body}
    msg = email.parser.BytesParser(policy=email.policy.HTTP).parsebytes(b"Content-Type: " + content_type.encode("latin-1") + b"\r\n\r\n" + body)
    out = {}
    for part in msg.iter_parts():
        name = part.get_filename() or part.get_param("name", header="content-disposition")
        out[name] = part.get_payload(decode=True) or b""
    return out


def built_modules(outdir):
    """{file name: bytes} of the modules `wrangler deploy --dry-run --outdir` wrote (no source maps or README)."""
    out = {}
    for name in sorted(os.listdir(outdir)):
        if name.endswith((".js", ".mjs", ".wasm")):
            with open(os.path.join(outdir, name), "rb") as f:
                out[name] = f.read()
    return out


def compare_scripts(report, script, built, live):
    if list(live) == [None] and len(built) == 1:
        live = {next(iter(built)): live[None]}
    for name in sorted(set(built) | {n for n in live if n is not None} | ({"(main)"} if None in live else set())):
        b = built.get(name)
        l = live.get(None if name == "(main)" else name)
        if b is None or l is None or sha256(b) != sha256(l):
            report.diff("%s %s" % (script, safe(name)), "built %s live %s" % (sha256(b) if b is not None else "none", sha256(l) if l is not None else "none"))
        else:
            report.ok("%s %s" % (script, safe(name)), sha256(b))


def fetch_script(account, script, token, timeout=30):
    status, headers, body = download(CF_SCRIPT_URL.format(account=account, script=script), timeout, {"Authorization": "Bearer " + token})
    if status != 200:
        raise NetworkError("Cloudflare API answered %d for %s" % (status, script))
    return script_modules(headers.get("Content-Type"), body)


def check_scripts(report, checkout, target, tmp, fetch=fetch_script):
    token = os.environ.get("CLOUDFLARE_API_TOKEN", "")
    account = os.environ.get("CLOUDFLARE_ACCOUNT_ID", "")
    if not token or not account:
        raise UsageError("--scripts needs CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID in the environment")
    relay = os.path.join(checkout.dir, "relay")
    env = child_env()
    checkout.run(["pnpm", "install", "--frozen-lockfile"], relay, env)
    # The router's config names the assets folder; its content is not part of the script.
    os.makedirs(os.path.join(relay, "public"), exist_ok=True)
    for config, script in (("wrangler.toml", target["router"]), ("wrangler.room.toml", target["room"])):
        out = os.path.join(tmp, "dry-" + script)
        checkout.run(["pnpm", "exec", "wrangler", "deploy", "--dry-run", "--outdir", out, "-c", config, "--env=" + target["wrangler_env"]], relay, env)
        compare_scripts(report, script, built_modules(out), fetch(account, script, token))


# --- Command line ------------------------------------------------------------------------------------------------------------------

def load_manifest(path, tag, repo):
    if path:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    status, _, body = download(RELEASE_URL.format(repo=repo, tag=tag))
    if status != 200:
        raise NetworkError("no %s on the %s release (status %d)" % (bm.MANIFEST_FILE, tag, status))
    return json.loads(body)


def main_bundle(args, report):
    if not args.manifest or not args.tag or not TAG_RE.match(args.tag) or args.rebuild or args.scripts:
        report.note("error: --bundle takes --manifest and --tag v1.2.3, and nothing that reaches the network")
        return 2
    try:
        with open(args.manifest, encoding="utf-8") as f:
            manifest = json.load(f)
        if not isinstance(manifest, dict) or not HEX64.match(str(manifest.get("bundle_sha256", ""))):
            raise ValueError
        if not os.path.isdir(args.bundle):
            raise OSError
    except (OSError, ValueError):
        report.note("error: cannot read the manifest or the bundle folder")
        return 2
    report.note("tag %s" % args.tag)
    report.note("manifest bundle_sha256 %s" % manifest["bundle_sha256"])
    check_bundle(args.bundle, manifest, args.tag, report)
    report.note("result: %s" % ("DIFFERENT" if report.diffs else "match"))
    return 1 if report.diffs else 0


def main(argv=None, out=None):
    out = out or sys.stdout
    p = argparse.ArgumentParser(description="Check what the hosted phone app's origin serves against a release's manifest.")
    p.add_argument("--env", choices=sorted(ENVS), default="production", help="which deployment (default production)")
    p.add_argument("--origin", help="the origin to check (default: the deployment's)")
    p.add_argument("--tag", help="the release tag (default: what the origin's /version.json says)")
    p.add_argument("--manifest", help="a local hosted-phone-manifest.json (default: the tag's GitHub release asset)")
    p.add_argument("--repo", default=REPO_SLUG, help="GitHub owner/name for the release (default %s)" % REPO_SLUG)
    p.add_argument("--rebuild", action="store_true", help="also build the tag in a temporary directory and compare")
    p.add_argument("--scripts", action="store_true", help="also compare the deployed Worker scripts (needs Cloudflare credentials)")
    p.add_argument("--source", default=bm.REPO, help="where --rebuild/--scripts clone the tag from (default: this repository)")
    p.add_argument("--bundle", metavar="DIR", help="check this local bundle folder against --manifest instead of an origin (no network)")
    p.add_argument("--timeout", type=float, default=20.0)
    args = p.parse_args(argv)
    target = ENVS[args.env]
    origin = (args.origin or target["origin"]).rstrip("/")
    report = Report(out)
    if args.bundle:
        return main_bundle(args, report)
    code = 0
    try:
        if origin_of(origin) != origin or not origin.startswith(("https://", "http://127.0.0.1", "http://localhost")):
            raise UsageError("--origin must be https://<host>[:port]")
        site = Origin(origin, args.timeout)
        tag = args.tag
        if not tag:
            resp = site.get("/" + bm.VERSION_FILE)
            try:
                tag = json.loads(resp.body).get("version") if resp.status == 200 else None
            except (ValueError, AttributeError):
                tag = None
            if not isinstance(tag, str) or not TAG_RE.match(tag):
                raise NetworkError("the origin's /version.json names no release tag; pass --tag")
        if not TAG_RE.match(tag):
            raise UsageError("--tag must look like v1.2.3")
        manifest = load_manifest(args.manifest, tag, args.repo)
        if not isinstance(manifest, dict) or not HEX64.match(str(manifest.get("bundle_sha256", ""))):
            raise UsageError("the manifest is not a hosted-phone-manifest.json")
        report.note("origin %s" % origin)
        report.note("tag %s" % tag)
        report.note("manifest bundle_sha256 %s" % manifest["bundle_sha256"])
        check_live(site, manifest, tag, report)
        if args.rebuild or args.scripts:
            tmp = tempfile.mkdtemp(prefix="kelpie-verify-")
            try:
                checkout = Checkout(tag, args.source, tmp)
                if checkout.commit != manifest["commit"]:
                    report.diff("checkout", "%s is commit %s, the manifest says %s" % (tag, checkout.commit, safe(manifest["commit"])))
                if args.rebuild:
                    if sys.platform != "linux":
                        report.note("note: the release builds on Linux; a build on %s may differ for that reason alone" % sys.platform)
                    compare_manifests(report, rebuild(checkout, tag, origin_of(target["origin"])), manifest)
                if args.scripts:
                    check_scripts(report, checkout, target, tmp)
            finally:
                shutil.rmtree(tmp, ignore_errors=True)
        code = 1 if report.diffs else 0
    except UsageError as e:
        report.note("error: %s" % e)
        code = 1 if report.diffs else 2
    except (NetworkError, OffOrigin) as e:
        report.note("error: %s" % (e or "redirected to another origin"))
        code = 1 if report.diffs else 2
    except (OSError, ValueError) as e:
        report.note("error: %s" % type(e).__name__)
        code = 1 if report.diffs else 2
    report.note("result: %s" % ("match" if code == 0 else "DIFFERENT" if code == 1 else "not checked"))
    report.note(CANNOT_PROVE)
    return code


if __name__ == "__main__":
    sys.exit(main())
