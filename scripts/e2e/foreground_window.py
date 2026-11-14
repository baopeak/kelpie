"""Another program's window in front of the app, for agent_browser.py: it covers a part of the app's main window, takes the foreground and the
keyboard (a text field of its own), and logs every mouse button, key and focus change it gets, so a check can tell whether an agent's input
reached it instead of the page under it. Windows only.

    python foreground_window.py <log file> <app exe> <left> <top> <width> <height>

The rectangle is in the client area of the app's main window (its largest visible window, found by the executable's path, never another
Kelpie), in physical pixels. The log gets JSON lines: {"ready": hwnd, "front": bool, "focus": bool} once it is up, then {"event": "button" |
"key" | "focus-in" | "focus-out", ...}, and {"event": "foreground", "app": bool, "pid": ...} each time another window takes the foreground
(looked at every 20 ms), saying whether it is the app's. When its standard input closes it gives the foreground back to the app's window and ends.
"""
import ctypes
import json
import os
import sys
import threading
from ctypes import wintypes

ctypes.windll.shcore.SetProcessDpiAwareness(2)  # physical pixels, as the rectangle is given
import tkinter as tk  # noqa: E402 - after the DPI awareness, which must come before any window

log_path, app_exe = sys.argv[1], os.path.normcase(sys.argv[2])
left, top, width, height = (int(v) for v in sys.argv[3:7])
u = ctypes.WinDLL("user32", use_last_error=True)
k32 = ctypes.WinDLL("kernel32", use_last_error=True)
u.GetForegroundWindow.restype = wintypes.HWND
u.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
u.ClientToScreen.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.POINT)]
u.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
k32.OpenProcess.restype = wintypes.HANDLE


def exe_of(pid):
    h = k32.OpenProcess(0x1000, False, pid)  # PROCESS_QUERY_LIMITED_INFORMATION
    if not h:
        return ""
    try:
        buf, size = ctypes.create_unicode_buffer(1024), wintypes.DWORD(1024)
        return buf.value if k32.QueryFullProcessImageNameW(h, 0, buf, ctypes.byref(size)) else ""
    finally:
        k32.CloseHandle(h)


def app_window():
    found = []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def each(hwnd, _):
        if u.IsWindowVisible(hwnd):
            pid = wintypes.DWORD()
            u.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
            if os.path.normcase(exe_of(pid.value)) == app_exe:
                r = wintypes.RECT()
                u.GetWindowRect(hwnd, ctypes.byref(r))
                found.append(((r.right - r.left) * (r.bottom - r.top), hwnd))
        return True

    u.EnumWindows(each, 0)
    return max(found)[1] if found else None


def to_front(hwnd):
    """Brings `hwnd` to the front, attached for a moment to the thread of the window in front (Windows lets only that one hand the foreground on)."""
    fg = u.GetForegroundWindow()
    them, me = u.GetWindowThreadProcessId(fg, None), k32.GetCurrentThreadId()
    attached = bool(them) and them != me and u.AttachThreadInput(me, them, True)
    u.BringWindowToTop(hwnd)
    u.SetForegroundWindow(hwnd)
    if attached:
        u.AttachThreadInput(me, them, False)


log = open(log_path, "a", encoding="utf-8")


def note(**what):
    log.write(json.dumps(what) + "\n")
    log.flush()


app = app_window()
if not app:
    note(error="no window of the app")
    sys.exit(1)
corner = wintypes.POINT(left, top)
u.ClientToScreen(app, ctypes.byref(corner))

root = tk.Tk()
root.title("Foreground probe")
root.geometry(f"{max(width, 200)}x{max(height, 120)}+{corner.x}+{corner.y}")
field = tk.Entry(root, width=40)
field.pack(padx=20, pady=20)
root.bind_all("<Button>", lambda e: note(event="button", num=e.num, x=e.x_root, y=e.y_root))
root.bind_all("<Key>", lambda e: note(event="key", keysym=e.keysym))
root.bind("<FocusIn>", lambda e: e.widget is root and note(event="focus-in"))
root.bind("<FocusOut>", lambda e: e.widget is root and note(event="focus-out"))
closing = threading.Event()
threading.Thread(target=lambda: (sys.stdin.read(), closing.set()), daemon=True).start()


def frame():
    return int(root.wm_frame(), 16)


def up():
    to_front(frame())
    field.focus_force()
    root.after(400, lambda: note(ready=frame(), front=u.GetForegroundWindow() == frame(), focus=root.focus_get() is field))


last_front = [None]


def watch():
    fg = u.GetForegroundWindow()
    if fg != last_front[0]:
        if last_front[0] is not None and fg != frame():
            pid = wintypes.DWORD()
            u.GetWindowThreadProcessId(fg, ctypes.byref(pid))
            note(event="foreground", app=os.path.normcase(exe_of(pid.value)) == app_exe, pid=pid.value)
        last_front[0] = fg
    if closing.is_set():
        if u.GetForegroundWindow() == frame():
            to_front(app)
        root.destroy()
        return
    root.after(20, watch)


root.after(200, up)
root.after(100, watch)
root.mainloop()
