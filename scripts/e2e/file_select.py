"""Selecting text in the file viewer with a mouse, in the real app (Windows): a local project's file open in the editor (CodeMirror), then
what a person does to pick text out of it, again and again.

    python -X utf8 scripts/e2e/file_select.py <scratch-dir> <app>

<app> is a debug build of the app with its frontend embedded, with kelpied and kelpie-screen next to it (as screen_phone.py takes). The
isolation is screen_support's: this process's own endpoint and data folder, its own daemon, tauri-driver and app; nothing of the user's
is reached. The mouse and the keys are WebDriver's (input the page cannot tell from a person's). What Ctrl+C copies is read from the page's
own `copy` event: the computer's clipboard, which the person at it may be using, is never touched.

Checks:
1. a press and a drag across three lines selects them; Ctrl+C copies exactly that; a double click selects a word; a click then
   Shift+click selects from one to the other;
2. a drag that starts inside the text already selected selects anew from there (before the fix the browser began dragging the selected
   text away instead, and cancelled the press after a few pixels);
3. the same after Ctrl+A (everything selected, so every press is inside it);
4. the same after a click elsewhere in the window took the focus from the editor.
"""
import os
import sys
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import screen_support as ss  # noqa: E402
from screen_support import check  # noqa: E402

SETUP = ss.prepare("file-select", __doc__)

LINES = [
    "// Kelpie selection check",
    "export function alpha(value: number): number {",
    "  const beta = value * 2;",
    "  return beta + 1;",
    "}",
    "",
    "export const gamma = alpha(20);",
]

# What the page put on the clipboard at its last copy (CodeMirror fills the event's data itself), or what was selected then.
COPY_PROBE = """
window.__kelpieCopied = null;
if (!window.__kelpieCopyProbe) {
  window.__kelpieCopyProbe = true;
  window.addEventListener('copy', (e) => { window.__kelpieCopied = e.clipboardData?.getData('text/plain') || String(window.getSelection() || ''); });
}
"""
# The press's own fate: a drag of the selected text starts, and the browser cancels the press.
CANCEL_PROBE = """
window.__kelpieCancelled = 0;
if (!window.__kelpieCancelProbe) {
  window.__kelpieCancelProbe = true;
  for (const t of ['dragstart', 'pointercancel']) document.addEventListener(t, () => window.__kelpieCancelled++, true);
}
"""


def copied(d):
    """Ctrl+C in the page, and what it copied."""
    d.js(COPY_PROBE)
    d.keys("ctrl+c")
    time.sleep(0.3)
    return (d.js("return window.__kelpieCopied") or "").replace("\r\n", "\n")


def line_box(d, n):
    """Line `n` (1-based) of the editor: its text's left and right edges, its top and height, in viewport CSS pixels."""
    return d.js("""
      const line = document.querySelectorAll('.cm-content .cm-line')[arguments[0] - 1];
      if (!line) return null;
      const r = document.createRange(); r.selectNodeContents(line);
      const t = r.getBoundingClientRect(), b = line.getBoundingClientRect();
      return { left: t.width ? t.left : b.left, right: t.width ? t.right : b.left, top: b.top, h: b.height };
    """, n)


def drag_lines(d, first, last):
    """A press at the start of line `first`, moved in several steps to the end of line `last`, then let go. Returns the presses the
    browser cancelled meanwhile."""
    a, c = line_box(d, first), line_box(d, last)
    d.js(CANCEL_PROBE)
    d.drag(a["left"] + 1, a["top"] + a["h"] / 2, c["right"] - 1, c["top"] + c["h"] / 2, steps=12)
    time.sleep(0.3)
    return d.js("return window.__kelpieCancelled")


def text(first, last):
    return "\n".join(LINES[first - 1:last])


def shift_click(d, x, y):
    """A click with Shift held, as one WebDriver action chain (key and mouse together)."""
    d._req("POST", d.s("/actions"), {"actions": [
        {"type": "key", "id": "kb", "actions": [{"type": "keyDown", "value": ""}, {"type": "pause", "duration": 0}, {"type": "pause", "duration": 0}, {"type": "pause", "duration": 0}, {"type": "keyUp", "value": ""}]},
        {"type": "pointer", "id": "mouse", "parameters": {"pointerType": "mouse"}, "actions": [
            {"type": "pointerMove", "origin": "viewport", "x": int(x), "y": int(y), "duration": 50},
            {"type": "pointerDown", "button": 0}, {"type": "pointerUp", "button": 0}, {"type": "pause", "duration": 0}, {"type": "pause", "duration": 0}]},
    ]})
    d._req("DELETE", d.s("/actions"))


def scenario(run):
    run.boot()
    d = run.d
    proj = os.path.join(SETUP.work, "select-project")
    os.makedirs(proj, exist_ok=True)
    with open(os.path.join(proj, "sample.ts"), "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(LINES) + "\n")
    pid = d.js("const s = window.__kelpie.store; const id = s.addProject(arguments[0], { open: true }); s.switchProject(id); return id;", proj)
    time.sleep(1.5)
    d.js("window.__kelpie.store.openFile(arguments[0], 'sample.ts')", pid)
    run.check("a local project's file opens in the editor", bool(d.wait("return document.querySelector('.cm-content')?.innerText.includes('alpha') ?? false", 20)))
    time.sleep(0.8)

    # 1. The first selection, and what a person does with it.
    cancelled = drag_lines(d, 2, 4)
    got = copied(d)
    run.check("a drag across three lines selects them, and Ctrl+C copies exactly that", got == text(2, 4) and not cancelled, (repr(got[:100]), cancelled))
    run.shot("1-first-drag")
    b = line_box(d, 3)
    x = d.js("""
      const line = document.querySelectorAll('.cm-content .cm-line')[2];
      const node = [...line.childNodes].flatMap(n => n.nodeType === 3 ? [n] : [...n.childNodes]).find(n => n.textContent.includes('beta'));
      const r = document.createRange(); const i = node.textContent.indexOf('beta'); r.setStart(node, i + 1); r.setEnd(node, i + 2);
      return r.getBoundingClientRect().left;
    """)
    d.pointer([("move", x, b["top"] + b["h"] / 2), ("down",), ("up",), ("pause", 60), ("down",), ("up",)])
    time.sleep(0.3)
    run.check("a double click selects one word", copied(d) == "beta", d.js("return window.__kelpieCopied"))
    a, c = line_box(d, 2), line_box(d, 4)
    d.pointer([("move", a["left"] + 1, a["top"] + a["h"] / 2), ("down",), ("up",)])
    time.sleep(0.2)
    shift_click(d, c["right"] - 1, c["top"] + c["h"] / 2)
    time.sleep(0.3)
    run.check("a click then Shift+click selects from one to the other", copied(d) == text(2, 4), d.js("return window.__kelpieCopied"))

    # 2. Lines 2 to 4 are selected now: a drag from inside them selects anew.
    cancelled = drag_lines(d, 3, 4)
    got = copied(d)
    run.check("a drag that starts inside the selection selects anew (never drags the selected text away)", got == text(3, 4) and not cancelled, (repr(got[:100]), cancelled))
    run.shot("2-drag-over-selection")

    # 3. Everything selected: every press is inside the selection.
    d.keys("ctrl+a")
    time.sleep(0.2)
    cancelled = drag_lines(d, 2, 3)
    got = copied(d)
    run.check("after Ctrl+A a drag still selects (lines 2 and 3 only)", got == text(2, 3) and not cancelled, (repr(got[:100]), cancelled))

    # 4. The focus elsewhere in the window, the selection still drawn: a drag inside it gives the editor the focus and selects anew.
    away = d.js("const t = document.querySelector('.sidebar, aside, nav'); if (!t) return null; const r = t.getBoundingClientRect(); return [r.left + 20, r.bottom - 30];")
    if away:
        d.pointer([("move", away[0], away[1]), ("down",), ("up",)])
    time.sleep(0.4)
    run.check("the focus left the editor", not d.js("return !!document.querySelector('.cm-editor.cm-focused')"))
    cancelled = drag_lines(d, 2, 4)
    got = copied(d)
    run.check("…and a drag inside the old selection gives it the focus and selects anew", got == text(2, 4) and not cancelled, (repr(got[:100]), cancelled))
    run.check("the file was not changed by any of it (no text dragged and dropped)", open(os.path.join(proj, "sample.ts"), encoding="utf-8").read() == "\n".join(LINES) + "\n")


def main():
    d = run = None
    try:
        d, run = ss.start(SETUP)
        scenario(run)
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False)
    finally:
        ss.finish(SETUP, d, run)
    results = ss.results + (run.results if run else [])
    failed = [r for r in results if not r[1]]
    print({"passed": len(results) - len(failed), "failed": len(failed)})
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
