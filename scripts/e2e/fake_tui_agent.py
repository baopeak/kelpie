"""A stand-in agent terminal UI that records exactly what it is typed (Windows).

    python scripts/e2e/fake_tui_agent.py <log-file> [--codex] [--no-bracketed]

Like the agents' own UIs it puts the console in raw, virtual-terminal input mode, so that what the
terminal sends arrives byte for byte, and asks for bracketed paste (unless `--no-bracketed`). It
appends one token per input character to the log: the character itself, `<hh>` for a control
character (`<1b>` is Escape), and for Enter `<CR>` inside a bracketed paste, otherwise `<SUBMIT>`
(a line break after it). `--codex` makes an Enter that follows input by less than 120 ms an
inserted newline (`<NEWLINE>`), as Codex does after a paste. The log therefore shows what a program
behind a terminal would have read: whether a paste was cut short by an escape sequence in its
content, and whether the Enter submitted.
"""
import codecs
import ctypes
import os
import sys
import time

log = sys.argv[1]
codex = "--codex" in sys.argv

kernel32 = ctypes.windll.kernel32
stdin = kernel32.GetStdHandle(-10)
mode = ctypes.c_uint32()
kernel32.GetConsoleMode(stdin, ctypes.byref(mode))
# No line input, echo or Ctrl+C processing; ENABLE_VIRTUAL_TERMINAL_INPUT (0x200).
kernel32.SetConsoleMode(stdin, (mode.value & ~0x7) | 0x200)
kernel32.SetConsoleCP(65001)

if "--no-bracketed" not in sys.argv:
    sys.stdout.write("\x1b[?2004h")
print("fake agent ready", flush=True)


def put(text):
    with open(log, "a", encoding="utf-8") as f:
        f.write(text)


decoder = codecs.getincrementaldecoder("utf-8")("replace")
tail, in_paste, last = "", False, 0.0
while True:
    data = os.read(0, 4096)
    now = time.time()
    gap, last = now - last, now
    for i, ch in enumerate(decoder.decode(data)):
        # Characters of one read arrived together: only the first follows a pause.
        quiet = gap if i == 0 else 0.0
        tail = (tail + ch)[-6:]
        if ch in "\r\n":
            put("<CR>" if in_paste else "<NEWLINE>" if codex and quiet < 0.12 else "<SUBMIT>\n")
            continue
        put(f"<{ord(ch):02x}>" if ch < " " or ch == "\x7f" else ch)
        if tail == "\x1b[200~":
            in_paste = True
        elif tail == "\x1b[201~":
            in_paste = False
