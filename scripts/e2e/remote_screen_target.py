"""A window of the test's own for remote_screen_real.py: what a person does on a remote screen, each thing noted with the wall clock.

    python -X utf8 scripts/e2e/remote_screen_target.py <title> <events-file>

It has small and normal buttons, a block of text to select by dragging or double-clicking, a list with a scrollbar, a text field, a square to
drag and drop onto a target, a right-click menu, and two colour patches the script reads in the phone's picture: one that flips colour on each
click (click to photon), one that follows the cursor (left half red, right half blue: move to photon). Each event is a JSON line in the file.
It writes only that file and stops when its window closes.
"""
import ctypes
import json
import sys
import time
import tkinter as tk

ctypes.windll.shcore.SetProcessDpiAwareness(2)
TITLE, EVENTS = sys.argv[1], sys.argv[2]


def note(kind, **fields):
    with open(EVENTS, "a", encoding="utf-8") as f:
        f.write(json.dumps({"k": kind, "t": time.time() * 1000, **fields}, ensure_ascii=False) + "\n")


root = tk.Tk()
root.title(TITLE)
root.geometry("1000x700+60+60")
root.attributes("-topmost", True)
root.configure(bg="#f4f4f4")

# Row 1: buttons, small and normal, and the click patch.
top = tk.Frame(root, bg="#f4f4f4")
top.place(x=10, y=10, width=980, height=60)
for i, label in enumerate("ABCDE"):
    b = tk.Button(top, text=label, font=("Segoe UI", 7), padx=0, pady=0, command=lambda label=label: note("small", name=label))
    b.place(x=10 + i * 22, y=18, width=18, height=18)
tk.Button(top, text="Save", font=("Segoe UI", 11), command=lambda: note("button", name="Save")).place(x=140, y=10, width=90, height=36)
tk.Button(top, text="Cancel", font=("Segoe UI", 11), command=lambda: note("button", name="Cancel")).place(x=240, y=10, width=90, height=36)
patch = tk.Canvas(top, bg="#000000", highlightthickness=0)
patch.place(x=360, y=0, width=200, height=60)
flip = [0]


def on_patch(e):
    flip[0] ^= 1
    patch.configure(bg="#00ff00" if flip[0] else "#ff00ff")
    note("patch", n=flip[0], x=e.x_root, y=e.y_root)


patch.bind("<Button-1>", on_patch)

# The move patch: red while the cursor is on its left half, blue on its right.
mover = tk.Canvas(top, bg="#888888", highlightthickness=0)
mover.place(x=580, y=0, width=390, height=60)
side = [None]


def on_move(e):
    now = "left" if e.x < 195 else "right"
    if now != side[0]:
        side[0] = now
        mover.configure(bg="#ff0000" if now == "left" else "#0000ff")
        note("move_side", side=now, x=e.x_root, y=e.y_root)


mover.bind("<Motion>", on_move)

# Row 2: text to select, and a list with a scrollbar.
text = tk.Text(root, font=("Segoe UI", 12), wrap="word", undo=True)
text.place(x=10, y=80, width=560, height=260)
text.insert("1.0", "\n".join(f"Line {i:02d}: the quick brown fox jumps over the lazy dog, sentence {i}." for i in range(1, 40)))


def on_select(_e=None):
    try:
        note("selection", text=text.get("sel.first", "sel.last"))
    except tk.TclError:
        pass


text.bind("<<Selection>>", on_select)
text.bind("<Double-Button-1>", lambda e: root.after(10, lambda: note("double", where="text", x=e.x_root, y=e.y_root)))
text.bind("<<Copy>>", lambda e: note("copy"))
text.bind("<<Paste>>", lambda e: note("paste"))
text.bind("<KeyRelease>", lambda e: note("text_key", key=e.keysym, state=e.state))

frame = tk.Frame(root)
frame.place(x=590, y=80, width=400, height=260)
scroll = tk.Scrollbar(frame, width=18)
scroll.pack(side="right", fill="y")
lst = tk.Listbox(frame, font=("Segoe UI", 11), yscrollcommand=scroll.set, exportselection=False)
lst.pack(side="left", fill="both", expand=True)
for i in range(1, 201):
    lst.insert("end", f"Item {i:03d}")
scroll.config(command=lst.yview)


def on_list(_e=None):
    sel = lst.curselection()
    if sel:
        note("list_pick", item=lst.get(sel[0]))


lst.bind("<<ListboxSelect>>", on_list)
last_top = [None]


def watch_scroll():
    first = round(lst.yview()[0], 3)
    if first != last_top[0]:
        last_top[0] = first
        note("list_scroll", top=first)
    root.after(50, watch_scroll)


root.after(50, watch_scroll)

# Row 3: a text field, and a square to drag onto a target.
tk.Label(root, text="Type here:", font=("Segoe UI", 11), bg="#f4f4f4").place(x=10, y=355)
entry = tk.Entry(root, font=("Segoe UI", 13))
entry.place(x=100, y=350, width=470, height=34)
entry.bind("<KeyRelease>", lambda e: note("entry", value=entry.get(), key=e.keysym))
entry.bind("<Return>", lambda e: note("enter", value=entry.get()))

board = tk.Canvas(root, bg="white", highlightthickness=1, highlightbackground="#999")
board.place(x=10, y=400, width=980, height=290)
board.create_rectangle(760, 60, 940, 240, outline="#2a2", width=3, dash=(6, 4), tags="target")
board.create_text(850, 150, text="Drop here", font=("Segoe UI", 12), fill="#2a2")
board.create_rectangle(40, 100, 120, 180, fill="#e8743b", outline="", tags="box")
held = {}


def press(e):
    if "box" in board.gettags("current"):
        held["at"] = (e.x, e.y)
        note("drag_start", x=e.x_root, y=e.y_root)


def drag(e):
    if "at" in held:
        dx, dy = e.x - held["at"][0], e.y - held["at"][1]
        board.move("box", dx, dy)
        held["at"] = (e.x, e.y)


def drop(e):
    if held.pop("at", None) is not None:
        x1, y1, x2, y2 = board.coords("box")
        cx, cy = (x1 + x2) / 2, (y1 + y2) / 2
        note("drop", inside=760 < cx < 940 and 60 < cy < 240, x=e.x_root, y=e.y_root)


board.bind("<ButtonPress-1>", press)
board.bind("<B1-Motion>", drag)
board.bind("<ButtonRelease-1>", drop)

menu = tk.Menu(root, tearoff=0)
for label in ("Copy", "Paste", "Delete"):
    menu.add_command(label=label, command=lambda label=label: note("menu", item=label))


def context(e):
    note("right", x=e.x_root, y=e.y_root)
    menu.tk_popup(e.x_root, e.y_root)


for w in (board, text, patch):
    w.bind("<Button-3>", context)
board.bind("<Double-Button-1>", lambda e: note("double", where="board", x=e.x_root, y=e.y_root))
board.bind("<MouseWheel>", lambda e: note("wheel", delta=e.delta))
root.bind_all("<Button-1>", lambda e: note("click_any", x=e.x_root, y=e.y_root), add="+")
root.bind_all("<Double-Button-1>", lambda e: note("double_any", x=e.x_root, y=e.y_root), add="+")
root.bind_all("<Control-v>", lambda e: note("ctrl_v"), add="+")
root.bind_all("<Control-c>", lambda e: note("ctrl_c"), add="+")


# Motion on demand: while `<events-file>.anim` exists, a square crosses the board's lower strip at 30 frames a second (the frame rate a moving
# picture gets is measured on it).
import os  # noqa: E402

runner = board.create_rectangle(0, 250, 30, 280, fill="#3366cc", outline="", state="hidden")
pos = [0]


def animate():
    if os.path.exists(EVENTS + ".anim"):
        pos[0] = (pos[0] + 12) % 940
        board.coords(runner, pos[0], 250, pos[0] + 30, 280)
        board.itemconfigure(runner, state="normal")
    root.after(33, animate)


root.after(33, animate)


# Someone else at this computer: the window minimized or restored, and mouse or keyboard input that was not injected (the remote screen's
# input is injected; a person's own is not). A measurement made meanwhile is not the phone's alone: the script marks it.
root.bind("<Unmap>", lambda e: e.widget is root and note("unmapped"))
root.bind("<Map>", lambda e: e.widget is root and note("mapped"))
import ctypes.wintypes as wt  # noqa: E402

HOOKPROC = ctypes.WINFUNCTYPE(ctypes.c_ssize_t, ctypes.c_int, wt.WPARAM, wt.LPARAM)
user32 = ctypes.windll.user32
user32.CallNextHookEx.argtypes = [ctypes.c_void_p, ctypes.c_int, wt.WPARAM, wt.LPARAM]
user32.CallNextHookEx.restype = ctypes.c_ssize_t
user32.SetWindowsHookExW.argtypes = [ctypes.c_int, HOOKPROC, ctypes.c_void_p, wt.DWORD]
user32.SetWindowsHookExW.restype = ctypes.c_void_p
last_human = [0.0]


class LL(ctypes.Structure):
    _fields_ = [("a", wt.DWORD), ("b", wt.DWORD), ("flags", wt.DWORD), ("time", wt.DWORD), ("extra", ctypes.c_size_t)]


class MSLL(ctypes.Structure):
    _fields_ = [("x", wt.LONG), ("y", wt.LONG), ("data", wt.DWORD), ("flags", wt.DWORD), ("time", wt.DWORD), ("extra", ctypes.c_size_t)]


def human(kind, injected):
    now = time.time()
    if not injected and now - last_human[0] > 0.5:
        last_human[0] = now
        note("human_input", what=kind)


@HOOKPROC
def mouse_hook(code, wparam, lparam):
    if code >= 0:
        human("mouse", ctypes.cast(lparam, ctypes.POINTER(MSLL)).contents.flags & 0x1)  # LLMHF_INJECTED
    return user32.CallNextHookEx(None, code, wparam, lparam)


@HOOKPROC
def key_hook(code, wparam, lparam):
    if code >= 0:
        human("key", ctypes.cast(lparam, ctypes.POINTER(LL)).contents.flags & 0x10)  # LLKHF_INJECTED
    return user32.CallNextHookEx(None, code, wparam, lparam)


hooks = [user32.SetWindowsHookExW(14, mouse_hook, None, 0), user32.SetWindowsHookExW(13, key_hook, None, 0)]  # WH_MOUSE_LL, WH_KEYBOARD_LL


def where():
    root.update_idletasks()
    note("ready", x=root.winfo_rootx(), y=root.winfo_rooty(), w=root.winfo_width(), h=root.winfo_height())


root.after(500, where)
root.mainloop()
