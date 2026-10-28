"""Stand-in agent for a task that runs on an SSH "host" (spec_panel.py). Started by a `claude` script on the
host's PATH with the task's prompt as its last argument; it keeps what it was given and starts in, ticks the box
the prompt names (`Task (line N)` of the file `@path`) in its own working folder, and exits: the daemon ends the
task with the ssh session. Needs KELPIE_E2E_CAPTURE (a folder) in its environment."""
import json
import os
import re
import sys
import time

capture = os.environ["KELPIE_E2E_CAPTURE"]
prompt = sys.argv[-1]
with open(os.path.join(capture, "host-prompt.txt"), "w", encoding="utf-8", newline="") as f:
    f.write(prompt)
with open(os.path.join(capture, "host-start.json"), "w") as f:
    json.dump({"cwd": os.getcwd(), "argc": len(sys.argv)}, f)
print("host agent ready", flush=True)
time.sleep(1)
line = re.search(r"Task \(line (\d+)\)", prompt)
path = re.search(r"@(\S+)", prompt)
if line and path:
    target = os.path.join(os.getcwd(), path.group(1))
    with open(target, "rb") as f:
        lines = f.read().split(b"\n")
    i = int(line.group(1)) - 1
    lines[i] = lines[i].replace(b"[ ]", b"[x]", 1)
    with open(target, "wb") as f:
        f.write(b"\n".join(lines))
    print("ticked", flush=True)
