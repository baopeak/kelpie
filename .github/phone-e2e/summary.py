"""The phone e2e run's table: per script, ran or skipped, its PASS/FAIL lines, exit code and time.

    python .github/phone-e2e/summary.py <results-dir>...

Each results dir is an uploaded e2e-out folder (<name>.log, <name>.exit). Written to stdout and GITHUB_STEP_SUMMARY.
"""
import glob
import os
import re
import sys

SKIPPED = {
    "remote_screen_real": "needs the production relay (wss://kelpie-link-relay...), Cloudflare TURN and the internet; no local stand-in",
    "remote_screen_target": "a helper window for remote_screen_real.py, not a test",
    "phone_merge_page": "really merges a sample pull request on GitHub (needs a real PR and a token able to merge); no local stand-in",
    "mobile_hosted --real-agent": "needs a signed-in Claude Code and model calls",
}

rows = []
for folder in sys.argv[1:]:
    for log in sorted(glob.glob(os.path.join(folder, "*.log"))):
        name = os.path.basename(log)[:-4]
        if name.startswith("probe-") or name.endswith("tauri-driver"):
            continue
        text = open(log, encoding="utf-8", errors="replace").read()
        passed = len(re.findall(r"^PASS\b", text, re.M))
        failed = re.findall(r"^FAIL\b.*$", text, re.M)
        exit_file = os.path.join(folder, name + ".exit")
        if os.path.exists(exit_file):
            code, seconds = (open(exit_file).read().split() + ["?", "?"])[:2]
        else:
            code, seconds = "timeout", "?"
        rows.append((name, code, seconds, passed, len(failed), failed))

lines = ["| script | exit | seconds | PASS | FAIL |", "|---|---|---|---|---|"]
for name, code, seconds, passed, nfail, _ in rows:
    lines.append(f"| {name} | {code} | {seconds} | {passed} | {nfail} |")
for name, why in SKIPPED.items():
    lines.append(f"| {name} | skipped | - | - | - ({why}) |")
lines.append("")
for name, *_, failed in rows:
    if failed:
        lines.append(f"### {name}: failed checks")
        lines += ["- " + f[:300] for f in failed[:40]]
        lines.append("")
text = "\n".join(lines)
print(text)
if os.environ.get("GITHUB_STEP_SUMMARY"):
    with open(os.environ["GITHUB_STEP_SUMMARY"], "a", encoding="utf-8") as f:
        f.write(text + "\n")
