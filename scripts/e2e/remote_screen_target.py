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
root.bind_all("<Control-v>", lambda e: note("ctrl_v"), add="+")
root.bind_all("<Control-c>", lambda e: note("ctrl_c"), add="+")


def where():
    root.update_idletasks()
    note("ready", x=root.winfo_rootx(), y=root.winfo_rooty(), w=root.winfo_width(), h=root.winfo_height())


root.after(500, where)
root.mainloop()
