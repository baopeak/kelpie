"""Tests for verify_live.py against a local fixture origin on 127.0.0.1. Standard library only:

    python -I scripts/hosted/test_verify_live.py

The fixture serves a small bundle the way Cloudflare's static assets do (`/index.html` redirects to `/`, `_headers` is not served,
an unknown path is a 404) with headers written out by hand here, not derived from verify_live's own `_headers` reader.
"""
import http.server
import io
import json
import os
import shutil
import socket
import sys
import tempfile
import threading
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bundle_manifest as bm  # noqa: E402
import verify_live as vl  # noqa: E402

CSP = "default-src 'self'; connect-src 'self' wss://example.test; script-src 'self'"
IMMUTABLE = "public, max-age=31536000, immutable"
NO_STORE = ("/", "/index.html", "/sw.js", "/version.json")
HEADERS = (
    "/*\n  Content-Security-Policy: %s\n  X-Content-Type-Options: nosniff\n" % CSP
    + "".join("%s\n  Cache-Control: no-store\n" % p for p in NO_STORE)
    + "/assets/*\n  Cache-Control: %s\n" % IMMUTABLE
)
SW = 'const a=["/","/icon.svg","/assets/index-abc.js","/assets/chat-def.js"];self.addEventListener("install",e=>e.waitUntil(caches.open("v").then(c=>c.addAll(a))));'
FILES = {
    "index.html": '<!doctype html><script type="module" src="/assets/index-abc.js"></script><link rel="modulepreload" href="/assets/chat-def.js"><link rel="icon" href="/icon.svg">',
    "sw.js": SW,
    "icon.svg": "<svg xmlns='http://www.w3.org/2000/svg'/>",
    "assets/index-abc.js": "console.log('index body marker')",
    "assets/chat-def.js": "export const chat = 1",
    "_headers": HEADERS,
}
TAG = "v9.9.9"
COMMIT = "c0ffee" * 6 + "abcd"


def served_headers(path):
    """What the fixture origin sends with `path`, written out by hand."""
    out = [("Content-Security-Policy", CSP), ("X-Content-Type-Options", "nosniff")]
    if path in NO_STORE:
        out.append(("Cache-Control", "no-store"))
    if path.startswith("/assets/"):
        out.append(("Cache-Control", IMMUTABLE))
    return out


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        srv = self.server
        path = self.path.split("?")[0]
        srv.requests.append((path, self.headers.get("Cache-Control")))
        if path in srv.redirects:
            self.send_response(302)
            self.send_header("Location", srv.redirects[path])
            self.end_headers()
            return
        if path == "/index.html":
            self.send_response(307)
            self.send_header("Location", "/")
            self.end_headers()
            return
        rel = "index.html" if path == "/" else path.lstrip("/")
        full = os.path.join(srv.root, *rel.split("/"))
        if rel == bm.HEADERS_FILE or not os.path.isfile(full):
            self.send_response(404)
            self.send_header("Content-Type", "text/html")
            self.end_headers()
            self.wfile.write(b"<h1>Not found</h1>")
            return
        with open(full, "rb") as f:
            body = f.read()
        self.send_response(200)
        headers = dict((k, v) for k, v in served_headers(path))
        headers.update(srv.overrides.get(path, {}))
        for k, v in headers.items():
            self.send_header(k, v)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class Fixture:
    def __init__(self, root):
        self.httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.httpd.root = root
        self.httpd.requests = []
        self.httpd.overrides = {}
        self.httpd.redirects = {}
        self.thread = threading.Thread(target=self.httpd.serve_forever, daemon=True)
        self.thread.start()
        self.origin = "http://127.0.0.1:%d" % self.httpd.server_address[1]

    def stop(self):
        self.httpd.shutdown()
        self.httpd.server_close()
        self.thread.join(5)


def write_bundle(root, files):
    for rel, text in files.items():
        path = os.path.join(root, *rel.split("/"))
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w", encoding="utf-8", newline="") as f:
            f.write(text)


def make_bundle(tmp, files=FILES):
    """A bundle at `<tmp>/dist` with its version.json, and the manifest beside it; returns (root, manifest path)."""
    root = os.path.join(tmp, "dist")
    write_bundle(root, files)
    return root, bm.write(root, bm.build(root, TAG, COMMIT, api_min=40))


def run(argv):
    out = io.StringIO()
    code = vl.main(argv, out=out)
    return code, out.getvalue()


class LiveTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.root, self.manifest = make_bundle(self.tmp)
        self.site = Fixture(self.root)
        self.addCleanup(self.site.stop)

    def tearDown(self):
        shutil.rmtree(self.tmp, ignore_errors=True)

    def check(self, *extra):
        return run(["--origin", self.site.origin, "--tag", TAG, "--manifest", self.manifest] + list(extra))

    def edit(self, rel, text):
        write_bundle(self.root, {rel: text})

    def test_match_exits_0(self):
        code, out = self.check()
        self.assertEqual(code, 0, out)
        self.assertIn("ok    /assets/chat-def.js", out)
        self.assertIn("ok    /version.json", out)
        self.assertIn("ok    /sw.js precache", out)
        self.assertIn(vl.CANNOT_PROVE, out)
        self.assertNotIn("body marker", out)
        self.assertNotIn("/_headers", [p for p, _ in self.site.httpd.requests])
        self.assertTrue(all(cc == "no-cache" for _, cc in self.site.httpd.requests))

    def test_tag_comes_from_the_live_version_json(self):
        code, out = run(["--origin", self.site.origin, "--manifest", self.manifest])
        self.assertEqual(code, 0, out)
        self.assertIn("tag %s" % TAG, out)

    def test_changed_byte_exits_1(self):
        self.edit("assets/chat-def.js", "export const chat = 2")
        code, out = self.check()
        self.assertEqual(code, 1, out)
        self.assertIn("DIFF  /assets/chat-def.js", out)
        self.assertIn(vl.CANNOT_PROVE, out)

    def test_missing_file_exits_1(self):
        os.remove(os.path.join(self.root, "icon.svg"))
        code, out = self.check()
        self.assertEqual(code, 1, out)
        self.assertIn("DIFF  /icon.svg status 404", out)

    def test_changed_csp_exits_1(self):
        self.site.httpd.overrides["/"] = {"Content-Security-Policy": CSP.replace("script-src 'self'", "script-src *")}
        code, out = self.check()
        self.assertEqual(code, 1, out)
        self.assertIn("DIFF  / header content-security-policy", out)

    def test_missing_cache_header_exits_1(self):
        self.site.httpd.overrides["/assets/index-abc.js"] = {"Cache-Control": "public, max-age=60"}
        code, out = self.check()
        self.assertEqual(code, 1, out)
        self.assertIn("DIFF  /assets/index-abc.js header cache-control", out)

    def test_version_json_with_another_hash_exits_1(self):
        info = bm.version_info("0" * 64, TAG, COMMIT, 40)
        self.edit(bm.VERSION_FILE, json.dumps(info))
        code, out = self.check()
        self.assertEqual(code, 1, out)
        self.assertIn("DIFF  /version.json bundle_sha256 differs", out)

    def test_manifest_for_another_tag_exits_1(self):
        code, out = run(["--origin", self.site.origin, "--tag", "v1.0.0", "--manifest", self.manifest])
        self.assertEqual(code, 1, out)
        self.assertIn("DIFF  manifest is for %s" % TAG, out)

    def test_inconsistent_manifest_exits_1(self):
        with open(self.manifest, encoding="utf-8") as f:
            m = json.load(f)
        m["files"][0]["sha256"] = "1" * 64
        with open(self.manifest, "w", encoding="utf-8") as f:
            json.dump(m, f)
        code, out = self.check()
        self.assertEqual(code, 1, out)
        self.assertIn("does not match its own file list", out)

    def test_redirect_to_another_origin_exits_1(self):
        self.site.httpd.redirects["/icon.svg"] = "https://elsewhere.invalid/icon.svg"
        code, out = self.check()
        self.assertEqual(code, 1, out)
        self.assertIn("DIFF  /icon.svg redirected to another origin", out)

    def test_bad_tag_is_a_usage_error(self):
        code, out = run(["--origin", self.site.origin, "--tag", "1.0/../x", "--manifest", self.manifest])
        self.assertEqual(code, 2, out)


class BundleShapeTest(unittest.TestCase):
    """Bundles whose manifest is self-consistent and served faithfully, but which load or keep files outside it."""

    def serve(self, files):
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp, True)
        root, manifest = make_bundle(tmp, files)
        site = Fixture(root)
        self.addCleanup(site.stop)
        return run(["--origin", site.origin, "--tag", TAG, "--manifest", manifest])

    def test_extra_precache_entry_exits_1(self):
        code, out = self.serve(dict(FILES, **{"sw.js": SW.replace('"/icon.svg"', '"/icon.svg","/evil.js"')}))
        self.assertEqual(code, 1, out)
        self.assertIn("DIFF  /sw.js precache /evil.js", out)

    def test_script_outside_the_manifest_exits_1(self):
        index = FILES["index.html"] + '<script src="/assets/other.js"></script><script src="https://cdn.example/x.js"></script>'
        code, out = self.serve(dict(FILES, **{"index.html": index}))
        self.assertEqual(code, 1, out)
        self.assertIn("DIFF  / loads /assets/other.js", out)
        self.assertIn("DIFF  / loads https://cdn.example/x.js", out)

    def test_no_precache_list_exits_1(self):
        code, out = self.serve(dict(FILES, **{"sw.js": "self.addEventListener('install',()=>{})"}))
        self.assertEqual(code, 1, out)
        self.assertIn("DIFF  /sw.js no precache list found", out)


class LocalBundleTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp, True)
        self.root, self.manifest = make_bundle(self.tmp)

    def check(self, tag=TAG):
        return run(["--bundle", self.root, "--tag", tag, "--manifest", self.manifest])

    def test_same_bundle_exits_0(self):
        code, out = self.check()
        self.assertEqual(code, 0, out)
        self.assertIn("ok    bundle bundle_sha256", out)

    def test_changed_byte_exits_1(self):
        write_bundle(self.root, {"assets/index-abc.js": "console.log('index body markeR')"})
        code, out = self.check()
        self.assertEqual(code, 1, out)
        self.assertIn("DIFF  /assets/index-abc.js", out)
        self.assertNotIn("body marker", out)

    def test_extra_file_exits_1(self):
        write_bundle(self.root, {"assets/extra.js": "1"})
        code, out = self.check()
        self.assertEqual(code, 1, out)
        self.assertIn("DIFF  /assets/extra.js expected none", out)

    def test_version_json_for_another_tag_exits_1(self):
        code, out = self.check("v1.0.0")
        self.assertEqual(code, 1, out)

    def test_bundle_needs_a_manifest(self):
        code, out = run(["--bundle", self.root, "--tag", TAG])
        self.assertEqual(code, 2, out)


class NetworkTest(unittest.TestCase):
    def test_nothing_listening_exits_2(self):
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
        s.close()
        tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, tmp, True)
        _, manifest = make_bundle(tmp)
        code, out = run(["--origin", "http://127.0.0.1:%d" % port, "--tag", TAG, "--manifest", manifest, "--timeout", "5"])
        self.assertEqual(code, 2, out)
        self.assertIn("result: not checked", out)
        self.assertIn(vl.CANNOT_PROVE, out)


class HeadersFileTest(unittest.TestCase):
    def test_rules_merge_and_match(self):
        rules = vl.parse_headers_file("# c\n/*\n  A: 1\n/assets/*\n  B: 2\n/u/:id/x\n  A: 3\n  ! C\n/u/*\n  C: 4\n")
        self.assertEqual(vl.expected_headers(rules, "/"), {"a": "1"})
        self.assertEqual(vl.expected_headers(rules, "/assets/x.js"), {"a": "1", "b": "2"})
        self.assertEqual(vl.expected_headers(rules, "/u/7/x"), {"a": "1, 3", "c": "4"})
        self.assertEqual(vl.expected_headers(rules, "/u/7/8/x"), {"a": "1", "c": "4"})

    def test_the_real_headers_file_shape(self):
        rules = vl.parse_headers_file(HEADERS)
        self.assertEqual(vl.expected_headers(rules, "/sw.js"), dict((k.lower(), v) for k, v in served_headers("/sw.js")))
        self.assertEqual(vl.expected_headers(rules, "/assets/a.js"), dict((k.lower(), v) for k, v in served_headers("/assets/a.js")))

    def test_precache_list_in_minified_code(self):
        self.assertEqual(vl.precache_lists(b'x=1;const n=["/","/a.js","/b\\"c"];f(n)'), [["/", "/a.js", '/b"c']])
        self.assertEqual(vl.precache_lists(b'x=["/a"]'), [])


class FakeTools:
    """Stands in for git, pnpm and wrangler: records each call's environment and makes what the real tool would."""

    def __init__(self, bundle_files=FILES, modules=None):
        self.calls = []
        self.bundle_files = bundle_files
        self.modules = modules or {}

    def __call__(self, argv, cwd, env):
        self.calls.append((argv, env))
        if argv[:2] == ["git", "-c"] and "clone" in argv:
            dest = argv[-1]
            os.makedirs(os.path.join(dest, "app", "src", "lib"))
            os.makedirs(os.path.join(dest, "relay"))
            with open(os.path.join(dest, "app", "src", "lib", "attention.ts"), "w") as f:
                f.write("export const HOSTED_API = 40;\n")
        elif argv[:2] == ["git", "rev-parse"]:
            return COMMIT + "\n"
        elif argv[:2] == ["pnpm", "build:phone:hosted"]:
            write_bundle(os.path.join(cwd, "dist-phone-hosted"), self.bundle_files)
        elif "--dry-run" in argv:
            out = argv[argv.index("--outdir") + 1]
            config = argv[argv.index("-c") + 1]
            os.makedirs(out)
            name = "entry-router.js" if config == "wrangler.toml" else "entry-room.js"
            for fname, data in ((name, self.modules.get(name, b"// " + name.encode())), (name + ".map", b"{}"), ("README.md", b"x")):
                with open(os.path.join(out, fname), "wb") as f:
                    f.write(data)
        return ""


class RebuildTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp, True)
        _, path = make_bundle(os.path.join(self.tmp, "release"))
        with open(path, encoding="utf-8") as f:
            self.manifest = json.load(f)

    def rebuild(self, tools):
        work = os.path.join(self.tmp, "work")
        os.makedirs(work)
        os.environ["CLOUDFLARE_API_TOKEN"] = "token-marker-123"
        self.addCleanup(os.environ.pop, "CLOUDFLARE_API_TOKEN", None)
        checkout = vl.Checkout(TAG, "/nowhere", work, run=tools)
        report = vl.Report(io.StringIO())
        vl.compare_manifests(report, vl.rebuild(checkout, TAG, "https://relay.example"), self.manifest)
        return report

    def test_same_build_matches(self):
        tools = FakeTools()
        report = self.rebuild(tools)
        self.assertEqual(report.diffs, 0, report.out.getvalue())
        build_env = [env for argv, env in tools.calls if argv[:2] == ["pnpm", "build:phone:hosted"]][0]
        self.assertEqual(build_env["KELPIE_HOSTED_ORIGIN"], "https://relay.example")
        self.assertTrue(all("CLOUDFLARE_API_TOKEN" not in env for _, env in tools.calls))
        clone = tools.calls[0][0]
        self.assertIn("core.autocrlf=false", clone)
        self.assertEqual(clone[clone.index("--branch") + 1], TAG)

    def test_different_build_differs(self):
        report = self.rebuild(FakeTools(dict(FILES, **{"assets/chat-def.js": "export const chat = 3"})))
        self.assertGreaterEqual(report.diffs, 2)
        self.assertIn("DIFF  rebuild /assets/chat-def.js", report.out.getvalue())
        self.assertIn("DIFF  rebuild bundle_sha256 differs", report.out.getvalue())


class ScriptsTest(unittest.TestCase):
    TOKEN = "token-marker-456"

    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.tmp, True)
        for k in ("CLOUDFLARE_API_TOKEN", "CLOUDFLARE_ACCOUNT_ID"):
            self.addCleanup(os.environ.pop, k, None)
            os.environ.pop(k, None)

    def multipart(self, parts):
        b = "kelpieboundary"
        body = b"".join(
            b"--" + b.encode() + b'\r\nContent-Disposition: form-data; name="' + n.encode() + b'"; filename="' + n.encode() + b'"\r\n'
            b"Content-Type: application/javascript+module\r\n\r\n" + data + b"\r\n" for n, data in parts.items()
        ) + b"--" + b.encode() + b"--\r\n"
        return "multipart/form-data; boundary=" + b, body

    def test_multipart_download_is_split_into_modules(self):
        ct, body = self.multipart({"entry-room.js": b"export class Room {}\r\n", "dep.js": b"x"})
        self.assertEqual(vl.script_modules(ct, body), {"entry-room.js": b"export class Room {}\r\n", "dep.js": b"x"})
        self.assertEqual(vl.script_modules("application/javascript", b"y"), {None: b"y"})

    def test_needs_credentials(self):
        work = os.path.join(self.tmp, "w")
        os.makedirs(work)
        checkout = vl.Checkout(TAG, "/nowhere", work, run=FakeTools())
        with self.assertRaises(vl.UsageError):
            vl.check_scripts(vl.Report(io.StringIO()), checkout, vl.ENVS["staging"], work, fetch=lambda *a: {})

    def run_scripts(self, live):
        os.environ["CLOUDFLARE_API_TOKEN"] = self.TOKEN
        os.environ["CLOUDFLARE_ACCOUNT_ID"] = "account-marker"
        work = os.path.join(self.tmp, "w")
        os.makedirs(work)
        tools = FakeTools(modules={"entry-router.js": b"router", "entry-room.js": b"room"})
        checkout = vl.Checkout(TAG, "/nowhere", work, run=tools)
        seen = []

        def fetch(account, script, token):
            seen.append((account, script, token))
            return live[script]

        report = vl.Report(io.StringIO())
        vl.check_scripts(report, checkout, vl.ENVS["staging"], work, fetch=fetch)
        out = report.out.getvalue()
        self.assertNotIn(self.TOKEN, out)
        self.assertNotIn("account-marker", out)
        self.assertTrue(all("CLOUDFLARE_API_TOKEN" not in env and "CLOUDFLARE_ACCOUNT_ID" not in env for _, env in tools.calls))
        dry = [argv for argv, _ in tools.calls if "--dry-run" in argv]
        self.assertEqual([a[-1] for a in dry], ["--env=staging", "--env=staging"])
        self.assertEqual([s for _, s, _ in seen], ["kelpie-link-relay-staging", "kelpie-link-relay-room-staging"])
        return report

    def test_same_scripts_match(self):
        report = self.run_scripts({"kelpie-link-relay-staging": {"entry-router.js": b"router"}, "kelpie-link-relay-room-staging": {None: b"room"}})
        self.assertEqual(report.diffs, 0, report.out.getvalue())

    def test_changed_script_differs(self):
        report = self.run_scripts({
            "kelpie-link-relay-staging": {"entry-router.js": b"router", "extra.js": b"z"},
            "kelpie-link-relay-room-staging": {"entry-room.js": b"r00m"},
        })
        out = report.out.getvalue()
        self.assertEqual(report.diffs, 2, out)
        self.assertIn("DIFF  kelpie-link-relay-staging extra.js", out)
        self.assertIn("DIFF  kelpie-link-relay-room-staging entry-room.js", out)


if __name__ == "__main__":
    unittest.main(verbosity=2)
