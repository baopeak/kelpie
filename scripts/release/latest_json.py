#!/usr/bin/env python3
"""Writes the updater manifest (`latest.json`) for a GitHub release.

    python scripts/release/latest_json.py --version 0.1.3 \
        --base-url https://github.com/baopeak/kelpie/releases/download/v0.1.3 \
        [--notes-file notes.md] \
        --artifact windows-x86_64=path/Kelpie_0.1.3_x64-setup.exe \
        --artifact darwin-aarch64=path/Kelpie.app.tar.gz  > latest.json

Each artifact needs its minisign signature next to it (`<file>.sig`, made by the Tauri build with
TAURI_SIGNING_PRIVATE_KEY). The app fetches
https://github.com/baopeak/kelpie/releases/latest/download/latest.json and verifies the download
against the public key in tauri.conf.json.
"""
import argparse
import datetime
import json
import os
import sys
import urllib.parse


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--version", required=True)
    ap.add_argument("--base-url", required=True, help="download URL of the release's assets")
    ap.add_argument("--notes-file")
    ap.add_argument("--artifact", action="append", default=[], metavar="PLATFORM=PATH", help="e.g. windows-x86_64=Kelpie_x64-setup.exe")
    a = ap.parse_args(argv)
    if not a.artifact:
        ap.error("at least one --artifact")
    platforms = {}
    for item in a.artifact:
        platform, _, path = item.partition("=")
        if not platform or not path:
            ap.error(f"bad --artifact {item!r}: expected PLATFORM=PATH")
        sig = path + ".sig"
        if not os.path.isfile(path) or not os.path.isfile(sig):
            ap.error(f"{platform}: {path} and its .sig must both exist")
        with open(sig, encoding="utf-8") as f:
            signature = f.read().strip()
        name = os.path.basename(path)
        platforms[platform] = {"signature": signature, "url": f"{a.base_url.rstrip('/')}/{urllib.parse.quote(name)}"}
    notes = ""
    if a.notes_file:
        with open(a.notes_file, encoding="utf-8") as f:
            notes = f.read().strip()
    manifest = {
        "version": a.version.lstrip("v"),
        "notes": notes,
        "pub_date": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    }
    json.dump(manifest, sys.stdout, indent=2)
    sys.stdout.write("\n")


if __name__ == "__main__":
    main()
