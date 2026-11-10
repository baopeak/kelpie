#!/usr/bin/env python3
"""The hosted phone app's manifest and fingerprint (standard library only).

    python -I scripts/hosted/bundle_manifest.py app/dist-phone-hosted --tag v0.1.16 --commit <sha>

Lists every file of the built bundle (`pnpm build:phone:hosted`) as `path sha256 size`, sorted by path, `_headers` among them (it sets
what the origin sends with each file, so it is part of what is served); `bundle_sha256` is the SHA-256 of those lines. Writes the
bundle's `version.json` (left out of the list: it carries the hash) and `hosted-phone-manifest.json` beside the bundle's folder, which
is published with the release and never deployed. Nothing in either depends on when or where it ran: two builds of one tag give one
hash. `scripts/hosted/verify_live.py` imports these functions to check what the origin serves.
"""
import argparse
import hashlib
import json
import os
import re
import sys

APP = "kelpie-phone"
VERSION_FILE = "version.json"
HEADERS_FILE = "_headers"
MANIFEST_FILE = "hosted-phone-manifest.json"
REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# The lowest daemon API the hosted app serves (app/src/lib/attention.ts).
ATTENTION_TS = os.path.join(REPO, "app", "src", "lib", "attention.ts")


def hosted_api(path=ATTENTION_TS):
    """`HOSTED_API` as app/src/lib/attention.ts says it."""
    with open(path, encoding="utf-8") as f:
        m = re.search(r"export const HOSTED_API = (\d+);", f.read())
    if not m:
        raise SystemExit("HOSTED_API not found in %s" % path)
    return int(m.group(1))


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for block in iter(lambda: f.read(1 << 16), b""):
            h.update(block)
    return h.hexdigest()


def bundle_files(root):
    """Every file under `root` but `version.json`: (path relative to it with `/`, sha256, size), sorted by path."""
    out = []
    for folder, dirs, files in os.walk(root):
        dirs.sort()
        for name in files:
            full = os.path.join(folder, name)
            rel = os.path.relpath(full, root).replace(os.sep, "/")
            if rel == VERSION_FILE:
                continue
            out.append((rel, sha256_file(full), os.path.getsize(full)))
    out.sort(key=lambda f: f[0].encode("utf-8"))
    return out


def manifest_lines(files):
    return ["%s %s %d" % f for f in files]


def bundle_hash(files):
    """SHA-256 of the manifest lines, each ended with a newline."""
    return hashlib.sha256("".join(line + "\n" for line in manifest_lines(files)).encode("utf-8")).hexdigest()


def version_info(bundle_sha256, tag, commit, api_min):
    """What `/version.json` says (the daemon reads `app` and `api_min` before it offers the hosted app)."""
    return {"app": APP, "version": tag, "commit": commit, "api_min": api_min, "bundle_sha256": bundle_sha256}


def build(root, tag, commit, api_min=None):
    """The manifest of the bundle at `root`. Raises when it is not a hosted bundle."""
    if not os.path.isfile(os.path.join(root, "index.html")):
        raise SystemExit("%s has no index.html: build it with `pnpm build:phone:hosted`" % root)
    if not os.path.isfile(os.path.join(root, HEADERS_FILE)):
        raise SystemExit("%s has no %s: only the hosted build makes one" % (root, HEADERS_FILE))
    files = bundle_files(root)
    digest = bundle_hash(files)
    with open(os.path.join(root, HEADERS_FILE), encoding="utf-8", newline="") as f:
        headers = f.read()
    info = version_info(digest, tag, commit, hosted_api() if api_min is None else api_min)
    return dict(info, files=[{"path": p, "sha256": h, "size": s} for p, h, s in files], headers=headers)


def dumps(value):
    return json.dumps(value, indent=2, ensure_ascii=False) + "\n"


def write(root, manifest):
    """Writes `<root>/version.json` and `hosted-phone-manifest.json` beside `root`; returns the manifest's path."""
    info = {k: manifest[k] for k in ("app", "version", "commit", "api_min", "bundle_sha256")}
    with open(os.path.join(root, VERSION_FILE), "w", encoding="utf-8", newline="\n") as f:
        f.write(dumps(info))
    out = os.path.join(os.path.dirname(os.path.abspath(root)), MANIFEST_FILE)
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write(dumps(manifest))
    return out


def main(argv=None):
    p = argparse.ArgumentParser(description="Write the hosted phone app's version.json and manifest, and print its bundle hash.")
    p.add_argument("dir", help="the built bundle (app/dist-phone-hosted)")
    p.add_argument("--tag", required=True, help="the release tag, e.g. v0.1.16")
    p.add_argument("--commit", required=True, help="the commit the bundle was built from")
    args = p.parse_args(argv)
    manifest = build(args.dir, args.tag, args.commit)
    out = write(args.dir, manifest)
    print("bundle_sha256 %s" % manifest["bundle_sha256"])
    print("manifest %s" % out)
    return 0


if __name__ == "__main__":
    sys.exit(main())
