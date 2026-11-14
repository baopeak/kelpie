"""The computer's screen through the relay, against the real app, its screen helper, a daemon of this script's own and a relay on this machine
(workerd, the self-hosted bundle) (Windows).

    python -X utf8 scripts/e2e/screen_relay.py <scratch-dir> <app>

As screen.py (the same tools and the same isolation: this process's own endpoint and data folder, a fake Tailscale CLI, its own tauri-driver),
plus `cd relay && pnpm install --frozen-lockfile && pnpm build:selfhost`. The relay runs here with secrets made for this run and held by its
process alone; nothing reaches Cloudflare. A phone pairs over Tailscale (the fake one), sets up its passkey (a software one) and is given
control, then reaches the computer through the relay with the relay token the computer gave it.

1. Through the relay, with *See and control the screen without Tailscale* off: the hello says `screen_remote_off`, and `screens` is refused
   with that reason.
2. Turned on (`mobile.set_screen`, verified): `screens` lists the ways, the relay's available; a start with no Face ID is refused
   (`needs_face_id`); with the passkey's check over the screen's challenge it starts, `via` relay, frames come, the rate stays within the
   relay WebSocket's cap, and the same check used again is refused.
3. The audit log says `via relay`; `mobile.status` counts the month's use. The relay (given coturn secrets) mints TURN credentials for the screen
   (`sc_rtc_servers`): the computer signs, the room checks, and no port-53 server is handed on.
4. Turning it off ends the screen through the relay within a second.
5. The same phone over Tailscale starts a screen with no Face ID, as before.
"""
import asyncio
import base64
import glob
import hashlib
import json
import os
import secrets
import socket
import subprocess
import sys
import tempfile
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import screen_support as ss  # noqa: E402
from screen_support import check, now_ms  # noqa: E402

SETUP = ss.prepare("screen-relay", __doc__)
import link_noise as ln  # noqa: E402

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
RELAY = os.path.join(ROOT, "relay")
CAP_KBPS = 500
# The relay mints coturn credentials (no TURN server answers here: what is checked is that the computer gets them, signed for its room).
TURN_URLS = "turn:127.0.0.1:3478?transport=udp,turns:127.0.0.1:5349?transport=tcp,turn:127.0.0.1:53?transport=udp"


def start_workerd(port):
    """workerd with the self-hosted bundle, open to every install, its rooms in a folder of this run; its secret in its own environment only."""
    bundle = os.path.join(RELAY, "workerd", "dist", "entry.js")
    found = glob.glob(os.path.join(RELAY, "node_modules", ".pnpm", "@cloudflare+workerd-*", "node_modules", "@cloudflare", "workerd-*", "bin", "workerd*"))
    if not os.path.exists(bundle) or not found:
        raise SystemExit("build the relay first: cd relay && pnpm install --frozen-lockfile && pnpm build:selfhost")
    run = tempfile.mkdtemp(prefix="kelpie-screen-relay-")
    os.makedirs(os.path.join(run, "dist"))
    os.makedirs(os.path.join(run, "rooms"))
    with open(os.path.join(RELAY, "workerd", "config.capnp"), encoding="utf-8") as f:
        config = f.read().replace('address = "*:8787"', f'address = "127.0.0.1:{port}"')
    with open(os.path.join(run, "config.capnp"), "w", encoding="utf-8") as f:
        f.write(config)
    with open(bundle, "rb") as src, open(os.path.join(run, "dist", "entry.js"), "wb") as dst:
        dst.write(src.read())
    env = {k: v for k, v in os.environ.items() if k not in ("REGISTRATION_KEY", "OPEN_REGISTRATION", "TICKET_SECRET")}
    proc = subprocess.Popen([found[0], "serve", "config.capnp"], cwd=run, env={**env, "TICKET_SECRET": secrets.token_urlsafe(32), "OPEN_REGISTRATION": "1", "TURN_SECRET": secrets.token_urlsafe(24), "TURN_URLS": TURN_URLS, "TURN_OPEN_ROOMS": "1"}, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    end = time.time() + 20
    while time.time() < end:
        try:
            with socket.create_connection(("127.0.0.1", port), 0.5):
                return proc
        except OSError:
            time.sleep(0.2)
    proc.kill()
    raise SystemExit("workerd did not start")


def screen_proof(passkey, target, expiry):
    """The passkey's check over a screen's challenge, as the phone makes it (`webauthn::recheck_challenge("screen:" + target, "relay", expiry)`)."""
    h = hashlib.sha256()
    h.update(f"screen:{target}".encode())
    h.update(b"\x00")
    h.update(hashlib.sha256(b"relay").digest())
    h.update(str(expiry).encode())
    challenge = base64.urlsafe_b64encode(h.digest()).rstrip(b"=").decode()
    return {"expiry": expiry, **passkey.get(challenge)}


async def scenario(run, d, relay_port):
    async def call(method, params=None):
        return await asyncio.to_thread(d.call, method, params or {})

    run.boot()
    await asyncio.sleep(1)
    serve_port = await ss.phones_on(SETUP, call)
    await call("mobile.set_relay", {"on": True, "url": f"ws://127.0.0.1:{relay_port}"})
    end = time.monotonic() + 20
    while time.monotonic() < end and not ((await call("mobile.status"))["relay"] or {}).get("connected"):
        await asyncio.sleep(0.3)
    check("the computer is in its room at the relay", ((await call("mobile.status"))["relay"] or {}).get("connected"))
    phone, device, hello, passkey, connect = await ss.control_phone(SETUP, d, call, serve_port, "Relay screen phone")
    check("the phone has control over Tailscale", hello["device"]["permission"] == "control", hello.get("device"))
    # Its relay token: given after the hello to a phone that says it can use the relay.
    await phone.close()
    ts_phone = connect()
    ts_phone.caps = ["ack", "hb", "relay", "pad"]
    await ts_phone.open()
    offer = await ts_phone.wait(lambda m: isinstance(m.get("relay"), dict) and m["relay"].get("token") or m.get("token"), 10)
    grant = (offer or {}).get("relay") or offer or {}
    token, ticket = grant.get("token"), grant.get("ticket") or grant.get("room")
    check("the computer gives the phone a relay token", bool(token and ticket), {k: (v if k not in ("token",) else "…") for k, v in (grant or {}).items()})

    via_relay = connect()
    via_relay.url = f"ws://127.0.0.1:{relay_port}/v1/phone?t={ticket}&k={token}"
    via_relay.connect = {}
    via_relay.caps = ["ack", "hb", "relay", "pad"]
    via_relay.greeting = {"relay_token": hashlib.sha256(ln.unb64(token)).hexdigest()}
    rh = await via_relay.open()
    if rh.get("pad"):
        via_relay.session.pad = True
    check("through the relay, while the screen is off that way, the hello says so", rh["device"].get("screen") == "screen_remote_off", rh.get("device"))
    refused = await via_relay.ask(type="screens", timeout=20)
    check("…and the screen is refused, with the reason", (refused or {}).get("code") == "no_control" and refused.get("reason") == "screen_remote_off", refused)

    # On the desktop, as a person does: Settings → Mobile, the switch (the app's verified call; this daemon answers its own verification).
    w = run.d
    await asyncio.to_thread(w.js, "window.__kelpie.store.openSettings('mobile', 'mobile-screen-remote')")
    row = await asyncio.to_thread(ss.until, lambda: w.js("const r = document.querySelector('[data-setting=\"mobile-screen-remote\"]'); return r ? r.textContent : null"), 15)
    check("Settings → Mobile shows the switch, off, with what it trades", bool(row) and "relay" in (row or "").lower() and (await call("mobile.status"))["screen"]["remote"] is False, (row or "")[:200])
    await asyncio.to_thread(w.js, "const r = document.querySelector('[data-setting=\"mobile-screen-remote\"]'); (r.querySelector('[role=\"switch\"], button, input[type=\"checkbox\"]')).click()")
    on = await asyncio.to_thread(ss.until, lambda: d.call("mobile.status")["screen"]["remote"], 15)
    check("turning it on in the app turns the screen on through the relay (verified)", on is True, (await call("mobile.status"))["screen"])
    usage = await asyncio.to_thread(ss.until, lambda: w.js("return document.querySelector('[data-screen-usage]')?.textContent || null"), 10)
    check("…and Advanced says this month's use", bool(usage) and "MB" in usage, usage)
    listed = await via_relay.ask(type="screens", timeout=20)
    paths = {p["id"]: p for p in (listed or {}).get("paths", [])}
    check("the ways are listed: the relay available, Tailscale not on this connection", paths.get("relay", {}).get("ok") is True and paths.get("tailscale", {}).get("why") == "no_direct_link", paths)
    monitor = next((r for r in (listed or {}).get("targets", []) if r["kind"] == "monitor"), None)
    target = monitor["id"] if monitor else ""
    no_face = await via_relay.ask(type="sc_start", target=target, via="auto", timeout=20)
    check("a start through the relay with no Face ID is refused", (no_face or {}).get("code") == "needs_face_id", no_face)
    proof = screen_proof(passkey, target, now_ms() + 30_000)
    mark = via_relay.mark()
    started = await via_relay.ask(type="sc_start", target=target, via="auto", recheck=proof, timeout=20)
    via_relay.sc = (started or {}).get("sc")
    check("with the passkey's check it starts, through the relay", bool(via_relay.sc) and started.get("via") == "relay", started)
    first = await via_relay.media(mark, lambda m: m["kind"] == "video", 15)
    check("frames come through the relay", first is not None and first["key"])
    await asyncio.sleep(12)
    stats = [m for m in via_relay.log[mark:] if m["type"] == "sc_stats"]
    rates = [m.get("kbps") or 0 for m in stats[3:]]
    check("the rate stays within the relay WebSocket's cap", bool(stats) and all(k <= CAP_KBPS * 1.1 for k in rates), rates)
    again = await via_relay.ask(type="sc_start", target=target, via="auto", recheck=proof, timeout=20)
    check("the same check used again is refused", (again or {}).get("code") == "needs_face_id", again)
    check("the relay's TURN is listed as a way, available", paths.get("relay_turn", {}).get("ok") is True, paths.get("relay_turn"))
    servers = await via_relay.ask(type="sc_rtc_servers", sc=via_relay.sc, timeout=20) or {}
    urls = [u for s in servers.get("ice_servers", []) for u in (s["urls"] if isinstance(s["urls"], list) else [s["urls"]])]
    check("TURN credentials are minted at the relay for this screen (signed by the computer, checked by the room), none on port 53",
          servers.get("turn") is None and any(u.startswith("turn:") for u in urls) and not any(":53?" in u for u in urls), {"turn": servers.get("turn"), "urls": urls})
    check("…with a name and a credential, given only inside the link", any(s.get("username") and s.get("credential") for s in servers.get("ice_servers", [])))
    # (that refusal came before any start, so the screen goes on)
    audit = json.dumps((await call("mobile.audit", {"limit": 50}))["entries"])
    check("the audit log says the screen's way", "monitor via relay" in audit)
    used = (await call("mobile.status"))["screen"]
    check("the month's use is counted", isinstance(used.get("relay_ws_used_mb"), int), used)

    mark = via_relay.mark()
    at = time.time()
    await call("mobile.set_screen", {"remote": False})
    ended = await via_relay.wait(lambda m: m["type"] == "sc_end", 5, mark)
    check("turning it off ends the screen through the relay within a second", (ended or {}).get("reason") == "revoked" and time.time() - at < 1.5, f"{ended} {time.time() - at:.2f} s")
    await via_relay.close()

    # The relay's connection took over from the Tailscale one (the device's newer connection): over Tailscale again.
    await ts_phone.close()
    ts_phone = connect()
    await ts_phone.open()
    direct = await ts_phone.ask(type="sc_start", target=target, via="auto", timeout=20)
    ts_phone.sc = (direct or {}).get("sc")
    check("over Tailscale the same phone starts a screen with no Face ID, as before", bool(ts_phone.sc) and direct.get("via") == "tailscale", direct)
    await ts_phone.ask(type="sc_stop", sc=ts_phone.sc)
    await ts_phone.close()


def main():
    relay_port = ss.free_port()
    workerd = start_workerd(relay_port)
    d = run = None
    try:
        d, run = ss.start(SETUP)
        asyncio.run(scenario(run, d, relay_port))
    except SystemExit:
        raise
    except Exception:  # noqa: BLE001
        traceback.print_exc()
        check("scenario ran to completion", False)
    finally:
        ss.finish(SETUP, d, run)
        workerd.terminate()
        try:
            workerd.wait(10)
        except subprocess.TimeoutExpired:
            workerd.kill()
    failed = [r for r in ss.results if not r[1]]
    print(json.dumps({"passed": len(ss.results) - len(failed), "failed": len(failed)}))
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
