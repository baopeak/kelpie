"""Tests for bundle_manifest.py over a small fixture bundle. Standard library only:

    python -I scripts/hosted/test_bundle_manifest.py
"""
import hashlib
import json
import os
import shutil
import sys
import tempfile
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import bundle_manifest as bm  # noqa: E402

FILES = {
    "index.html": b"<!doctype html><script type=module src=/assets/index-abc.js></script>",
    "sw.js": b"self.addEventListener('install',()=>{})",
    "_headers": b"/*\n  X-Content-Type-Options: nosniff\n/assets/*\n  Cache-Control: public, max-age=31536000, immutable\n",
    "404.html": b"<!doctype html><h1>Not found</h1>",
    "assets/index-abc.js": b"console.log(1)",
    "assets/chat-def.js": b"export const a = 2",
}


class BundleManifestTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp()
        self.root = self.bundle("one")

    def tearDown(self):
        shutil.rmtree(self.tmp)

    def bundle(self, name, files=FILES):
        root = os.path.join(self.tmp, name, "dist")
        for rel, data in files.items():
            path = os.path.join(root, *rel.split("/"))
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "wb") as f:
                f.write(data)
        return root

    def hash_of(self, root):
        return bm.build(root, "v1.2.3", "abc123", api_min=34)["bundle_sha256"]

    def test_lists_every_file_sorted_with_its_hash_and_size(self):
        m = bm.build(self.root, "v1.2.3", "abc123", api_min=34)
        self.assertEqual([f["path"] for f in m["files"]], ["404.html", "_headers", "assets/chat-def.js", "assets/index-abc.js", "index.html", "sw.js"])
        index = next(f for f in m["files"] if f["path"] == "index.html")
        self.assertEqual(index["size"], len(FILES["index.html"]))
        self.assertEqual(len(index["sha256"]), 64)
        self.assertEqual(m["headers"], FILES["_headers"].decode())
        self.assertEqual({k: m[k] for k in ("app", "version", "commit", "api_min")}, {"app": "kelpie-phone", "version": "v1.2.3", "commit": "abc123", "api_min": 34})

    def test_the_same_files_give_the_same_hash_wherever_they_are(self):
        self.assertEqual(self.hash_of(self.root), self.hash_of(self.bundle("two")))

    def test_one_byte_changes_the_hash(self):
        changed = dict(FILES, **{"assets/chat-def.js": b"export const a = 3"})
        self.assertNotEqual(self.hash_of(self.root), self.hash_of(self.bundle("two", changed)))

    def test_one_header_line_changes_the_hash(self):
        changed = dict(FILES, _headers=FILES["_headers"].replace(b"immutable", b"no-cache"))
        self.assertNotEqual(self.hash_of(self.root), self.hash_of(self.bundle("two", changed)))

    def test_a_renamed_or_added_file_changes_the_hash(self):
        renamed = {("assets/chat-xyz.js" if k == "assets/chat-def.js" else k): v for k, v in FILES.items()}
        self.assertNotEqual(self.hash_of(self.root), self.hash_of(self.bundle("two", renamed)))
        self.assertNotEqual(self.hash_of(self.root), self.hash_of(self.bundle("three", dict(FILES, extra=b"x"))))

    def test_version_json_is_left_out_so_writing_it_changes_nothing(self):
        before = self.hash_of(self.root)
        bm.write(self.root, bm.build(self.root, "v1.2.3", "abc123", api_min=34))
        self.assertTrue(os.path.isfile(os.path.join(self.root, "version.json")))
        self.assertEqual(self.hash_of(self.root), before)
        m = bm.build(self.root, "v1.2.3", "abc123", api_min=34)
        self.assertNotIn("version.json", [f["path"] for f in m["files"]])

    def test_the_hash_is_that_of_the_lines(self):
        files = bm.bundle_files(self.root)
        text = "".join("%s %s %d\n" % f for f in files)
        self.assertEqual(bm.bundle_hash(files), hashlib.sha256(text.encode()).hexdigest())

    def test_writes_version_json_in_the_bundle_and_the_manifest_beside_it_with_no_time_in_either(self):
        out = bm.write(self.root, bm.build(self.root, "v1.2.3", "abc123", api_min=34))
        self.assertEqual(out, os.path.join(os.path.dirname(self.root), "hosted-phone-manifest.json"))
        with open(os.path.join(self.root, "version.json"), encoding="utf-8") as f:
            version = json.load(f)
        self.assertEqual(sorted(version), ["api_min", "app", "bundle_sha256", "commit", "version"])
        with open(out, encoding="utf-8") as f:
            manifest = json.load(f)
        self.assertEqual(manifest["bundle_sha256"], version["bundle_sha256"])
        # Nothing that says when: written twice, a second later, the files are byte for byte the same.
        with open(out, "rb") as f:
            first = f.read()
        bm.write(self.root, bm.build(self.root, "v1.2.3", "abc123", api_min=34))
        with open(out, "rb") as f:
            self.assertEqual(f.read(), first)
        for key in ("time", "date", "built", "generated", "timestamp"):
            self.assertNotIn(key, json.dumps(manifest).lower().replace("bundle_sha256", ""))

    def test_reads_the_lowest_api_from_the_app(self):
        self.assertGreaterEqual(bm.hosted_api(), 34)

    def test_refuses_a_folder_that_is_not_a_hosted_bundle(self):
        no_headers = {k: v for k, v in FILES.items() if k != "_headers"}
        with self.assertRaises(SystemExit):
            bm.build(self.bundle("two", no_headers), "v1", "c")
        with self.assertRaises(SystemExit):
            bm.build(os.path.join(self.tmp, "missing"), "v1", "c")


if __name__ == "__main__":
    unittest.main()
