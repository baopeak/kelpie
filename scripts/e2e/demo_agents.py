"""Simulated agents for README pictures: small Python programs that speak Claude Code's stream format (chat view) or print a shell
prompt, about the demo projects of readme_demo.py. Never a real agent and never a model call; their model reports itself as
"simulated", and every picture made with them is marked so in the media list.

    from demo_agents import write_programs
    programs = write_programs(folder)   # {"chat_work": path, ..., "ticker": path, "menu": path, "lead": path, "helper": path}

A chat session runs `[python, "-u", programs["chat_work"]]` as its `chat_argv` (kind "claude").
"""
import os

_STREAM = r'''
import json, sys
def out(m):
    sys.stdout.write(json.dumps(m, ensure_ascii=False) + "\n"); sys.stdout.flush()
def init():
    out({"type": "system", "subtype": "init", "model": "simulated", "session_id": "demo-simulated", "permissionMode": "default"})
def user(text):
    out({"type": "user", "isReplay": True, "message": {"role": "user", "content": text}})
def say(mid, blocks):
    out({"type": "assistant", "message": {"id": mid, "role": "assistant", "content": blocks}})
def tool(mid, tid, name, inp, result):
    say(mid, [{"type": "tool_use", "id": tid, "name": name, "input": inp}])
    out({"type": "user", "message": {"role": "user", "content": [{"type": "tool_result", "tool_use_id": tid, "content": result}]}})
def done(ms, turns):
    out({"type": "result", "subtype": "success", "is_error": False, "result": "ok", "duration_ms": ms, "num_turns": turns})
def idle():
    for line in sys.stdin:
        pass
'''

CHAT_WORK = _STREAM + r'''
init()
user("Pasted coupon codes like \" welcome10 \" are rejected at checkout. Fix it without changing the discount rules, and add a smoke check.")
say("m1", [{"type": "thinking", "thinking": "The coupon lookup compares the raw string. Normalise it before the lookup and keep integer-cent rounding as it is."}])
tool("m2", "t1", "Read", {"file_path": "src/checkout.py"}, "COUPONS = {\"WELCOME10\": 10, \"SPRING15\": 15}\n...\n    if coupon and coupon not in COUPONS:\n        raise ValueError(f\"unknown coupon: {coupon}\")")
tool("m3", "t2", "Bash", {"command": "python smoke.py", "description": "Run the demo's smoke checks"}, "PASS atlas-commerce: local contract assertions")
tool("m4", "t3", "Edit", {"file_path": "src/checkout.py", "old_string": "    if coupon and coupon not in COUPONS:", "new_string": "    coupon = coupon.strip().upper()\n    if coupon and coupon not in COUPONS:"}, "The file src/checkout.py has been updated.")
tool("m5", "t4", "Edit", {"file_path": "smoke.py", "old_string": "print(\"PASS", "new_string": "assert quote([LineItem(\"kit\", 10000)], \" welcome10 \").discount_cents == 1000\n\nprint(\"PASS"}, "The file smoke.py has been updated.")
tool("m6", "t5", "Bash", {"command": "python smoke.py && git diff --stat", "description": "Check again and show the change"}, "PASS atlas-commerce: local contract assertions\n src/checkout.py | 1 +\n smoke.py        | 2 ++\n 2 files changed, 3 insertions(+)")
say("m7", [{"type": "text", "text": "Fixed. `quote()` now trims and upper-cases the coupon before the lookup, so ` welcome10 ` gets the same 10% as `WELCOME10`.\n\n- `src/checkout.py`: one line, discount rules unchanged\n- `smoke.py`: a check for a pasted, lower-case code\n\n`python smoke.py` passes. Want me to commit it on `feature/coupon-validation`?"}])
done(38000, 6)
idle()
'''

CHAT_APPROVAL = _STREAM + r'''
init()
user("Prepare the 0.4.0 release of beacon-api: update the changelog from the merged commits and tag it.")
tool("m1", "t1", "Bash", {"command": "git log --oneline v0.3.0..HEAD", "description": "List the commits since the last release"}, "4c1e9d2 feat(catalog): filter items by tag\n9a07b11 fix(catalog): return 404 for an unknown item\n2f3c8e0 docs: describe the local server")
tool("m2", "t2", "Edit", {"file_path": "CHANGELOG.md", "old_string": "# Changelog\n", "new_string": "# Changelog\n\n## 0.4.0\n- Filter catalog items by tag\n- 404 for an unknown item\n"}, "The file CHANGELOG.md has been updated.")
say("m3", [{"type": "text", "text": "The changelog has a 0.4.0 entry. Next I would tag the release and push the tag."}])
out({"type": "control_request", "request_id": "req-tag", "request": {"subtype": "can_use_tool", "tool_name": "Bash", "input": {"command": "git tag -a v0.4.0 -m 'beacon-api 0.4.0' && git push origin v0.4.0", "description": "Tag the release and push the tag"}}})
idle()
'''

CHAT_DONE = _STREAM + r'''
init()
user("Explain how relay-workers retries a failed job, in three bullets.")
tool("m1", "t1", "Read", {"file_path": "src/app.py"}, "def process(jobs):\n    ...")
say("m2", [{"type": "text", "text": "- A job that raises is retried up to **3 times**, with a delay that doubles each time.\n- After the last try it goes to the `failed` list with its error, and the batch carries on.\n- Results come back in the order the jobs were given, whatever order they finished in."}])
done(12000, 2)
idle()
'''

TICKER = r'''
import itertools, time
checks = ["checkout totals", "coupon boundaries", "free-shipping threshold", "catalog 404", "palette contrast"]
for i in itertools.count():
    print(f" ✓ {checks[i % len(checks)]} ({3 + i % 7} assertions) {20 + (i * 7) % 300}ms", flush=True)
    time.sleep(0.8)
'''

MENU = r'''
import sys
print("Bash(python scripts/migrate.py --apply)")
print(" Do you want to proceed?")
print(" ❯ 1. Yes")
print("   2. Yes, and don't ask again for this command")
print("   3. No, and say what to do differently (esc)")
sys.stdin.read(1)
'''

# A terminal agent that splits a task into two sub-sessions through Kelpie's CLI, as a real agent would use it.
LEAD = r'''
import os, subprocess, sys
cli = os.environ.get("KELPIE_CLI", "kelpied")
print("Demo agent (simulated, no model)  -  release prep for atlas-commerce\n", flush=True)
print("> Prepare the 0.9 release: check the coupon tests and draft the release notes.\n", flush=True)
print("* Two independent parts: opening a helper for each.", flush=True)
for name, task in (("coupon-tests", "Check the coupon boundary tests in smoke.py"), ("release-notes", "Draft release notes from the commits since 0.8")):
    print(f"  $ kelpie sub spawn --name {name} demo-helper \"{task}\"", flush=True)
    r = subprocess.run([cli, "sub", "spawn", "--name", name, "demo-helper", task], capture_output=True, text=True, encoding="utf-8", errors="replace")
    print("    " + ("opened " + name if r.returncode == 0 else (r.stderr or r.stdout).strip()[:160]), flush=True)
print("\n* Waiting for their reports...", flush=True)
for line in sys.stdin:
    pass
'''

# A sub-session (its task arrives typed, as Kelpie types it into an agent): prints its work; the release-notes one reports
# done with a check it owes, since it cannot open a phone.
HELPER = r'''
import os, subprocess, sys, time
print("Demo helper (simulated, no model)\n", flush=True)
task = sys.stdin.readline().strip() or "demo task"
print("> " + task + "\n", flush=True)
steps = ["Read smoke.py", "Read src/checkout.py", "python smoke.py  ->  PASS atlas-commerce"] if "coupon" in task else ["git log --oneline v0.8..HEAD  ->  6 commits", "Write docs/release-notes.md"]
for s in steps:
    time.sleep(1.2)
    print("  * " + s, flush=True)
if "notes" in task:
    time.sleep(1)
    cli = os.environ.get("KELPIE_CLI", "kelpied")
    subprocess.run([cli, "sub", "report", "--owes", "phone:open the release notes on a phone:a photo of the page", "done", "Release notes drafted in docs/release-notes.md (demo)"], capture_output=True)
    print("\n  Reported: done (one check owed: look at it on a phone)", flush=True)
for line in sys.stdin:
    pass
'''

PROGRAMS = {"chat_work": CHAT_WORK, "chat_approval": CHAT_APPROVAL, "chat_done": CHAT_DONE, "ticker": TICKER, "menu": MENU, "lead": LEAD, "helper": HELPER}


def write_programs(folder):
    """Writes the programs into `folder` and returns their paths by name."""
    os.makedirs(folder, exist_ok=True)
    paths = {}
    for name, text in PROGRAMS.items():
        paths[name] = os.path.join(folder, f"demo_{name}.py")
        with open(paths[name], "w", encoding="utf-8") as f:
            f.write(text)
    return paths
