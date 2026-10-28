"""Keystroke-to-screen latency in the real app, idle and with the CPU saturated by background
sessions, with "Prioritize visible terminals" off and on.

Each key goes in through xterm (`term.input`, as a keypress does) and counts as echoed once a
rendered frame shows it: the whole path app -> kelpied -> PTY -> program -> kelpied -> webview.

Prereqs: as smoke.py (daemon on KELPIE_ENDPOINT, `pnpm dev`, tauri-driver, debug app).

    python scripts/e2e/typing_latency.py <screenshot-dir> <kelpied.exe> [shell|claude ...]

KELPIE_E2E_TRUSTED_DIR: a folder Claude Code already trusts (default: this repository).
KELPIE_E2E_LOAD: busy background sessions (default 6; this machine's cores + 2 saturates it).
"""
import os
import statistics
import sys
import time

sys.path.insert(0, os.path.dirname(__file__))
from smoke_support import REPO, Run, daemon  # noqa: E402

shots, kelpied = sys.argv[1], sys.argv[2]
targets = sys.argv[3:] or ["shell", "claude"]
trusted = os.environ.get("KELPIE_E2E_TRUSTED_DIR", REPO)
load = int(os.environ.get("KELPIE_E2E_LOAD", "6"))
run = Run(shots)
d = run.d

# Types `keys` one at a time into the session's view; resolves with the per-key latency (ms, or
# null when a key never showed up within 5 s).
TYPE_JS = r"""
const [sid, marker, keys, gap] = arguments;
const done = arguments[arguments.length - 1];
const v = window.__kelpie.peekView(sid);
if (!v) return done({ error: 'no view' });
const term = v.term;
const screen = () => {
  const b = term.buffer.active;
  let s = '';
  for (let i = b.viewportY; i < b.viewportY + term.rows; i++) s += (b.getLine(i)?.translateToString(true) ?? '') + '\n';
  return s;
};
const typeOne = (ch, expected) => new Promise((resolve) => {
  const t0 = performance.now();
  let timer;
  const sub = term.onRender(() => {
    if (!screen().includes(expected)) return;
    sub.dispose();
    clearTimeout(timer);
    resolve(performance.now() - t0);
  });
  timer = setTimeout(() => { sub.dispose(); resolve(null); }, 5000);
  term.input(ch, true);
});
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
(async () => {
  let typed = '';
  for (const ch of marker) { typed += ch; await typeOne(ch, typed); }
  await sleep(300);
  const out = [];
  for (const ch of keys) {
    typed += ch;
    out.push(await typeOne(ch, typed));
    await sleep(gap);
  }
  done({ lat: out });
})();
"""

TEXT = "thequickbrownfoxjumpsoverthelazydog"


def pct(xs, p):
    xs = sorted(xs)
    return xs[round((len(xs) - 1) * p)] if xs else float("nan")


def measure(sid, marker):
    r = d.js_async(TYPE_JS, sid, marker, TEXT, 120)
    if "error" in r:
        raise RuntimeError(r["error"])
    lat = [x for x in r["lat"] if x is not None]
    return {"n": len(lat), "missed": len(r["lat"]) - len(lat), "p50": pct(lat, 0.5), "p95": pct(lat, 0.95), "max": max(lat or [float("nan")]), "mean": statistics.mean(lat) if lat else float("nan")}


def clear_line(handle, kind):
    # Escape clears the line in cmd and PSReadLine; Claude Code clears a non-empty input on a
    # double Escape.
    daemon(d, "session.input", {"handle": handle, "data": "\x1b" if kind == "shell" else "\x1b\x1b"})
    time.sleep(0.8)


def with_load(n):
    return [daemon(d, "session.create", {"kind": "custom", "cwd": REPO, "name": f"burn-{i}", "args": [kelpied, "bench-burn"]})["handle"] for i in range(n)]


def stop(handles):
    for h in handles:
        daemon(d, "session.close", {"handle": h})


results = {}
try:
    run.boot()
    # A loaded run types for well over WebDriver's default 30 s script limit.
    d._req("POST", d.s("/timeouts"), {"script": 600_000})
    d.js("const s = window.__kelpie.store; for (const p of [...s.state.projects]) s.removeProject(p.id);")
    d.js("window.__kelpie.store.addProject(arguments[0]);", trusted)
    pid = d.js("return window.__kelpie.store.state.activeProject")
    for i, kind in enumerate(targets):
        info = d.js_async("const done = arguments[arguments.length-1]; window.__kelpie.store.newSession(arguments[0], arguments[1]).then(done);", pid, kind)
        h, sid = info["handle"], info["id"]
        ready = "❯" if kind == "claude" else ">"
        end = time.time() + 90
        while ready not in daemon(d, "session.read", {"handle": h})["text"] and time.time() < end:
            time.sleep(1)
        d.wait("return !!window.__kelpie.peekView(arguments[0])?.attached", 30, 0.25, sid)
        time.sleep(3)
        scenarios = [("idle", 0, True), (f"load={load} priority=off", load, False), (f"load={load} priority=on", load, True)]
        for name, n, prio in scenarios:
            d.js("window.__kelpie.store.setSettings({ prioritizeVisible: arguments[0] })", prio)
            burns = with_load(n)
            time.sleep(2)
            try:
                r = measure(sid, f"z{i}{len(results)}q")
            finally:
                stop(burns)
            results[(kind, name)] = r
            print(f"{kind:7} {name:24} p50 {r['p50']:7.1f}  p95 {r['p95']:7.1f}  max {r['max']:7.1f}  mean {r['mean']:7.1f} ms  ({r['n']} echoed, {r['missed']} missed)", flush=True)
            clear_line(h, kind)
        run.shot(f"typing-{kind}")
        for name, _, _ in scenarios:
            run.check(f"{kind}: every key echoed ({name})", results[(kind, name)]["missed"] == 0, f"missed {results[(kind, name)]['missed']}")
        off, on = results[(kind, f"load={load} priority=off")], results[(kind, f"load={load} priority=on")]
        run.check(f"{kind}: priority cuts loaded p95", on["p95"] < off["p95"], f"{off['p95']:.0f} -> {on['p95']:.0f} ms")
        daemon(d, "session.close", {"handle": h})
except Exception as e:  # noqa: BLE001 — finish() exits; report the failure as a check
    import traceback
    traceback.print_exc()
    run.check("scenario ran to the end", False, repr(e))
finally:
    try:
        d.js("window.__kelpie.store.setSettings({ prioritizeVisible: true })")
    except Exception:  # noqa: BLE001 — the app may be gone; never hide the original error
        pass
    run.finish()
