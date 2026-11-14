//! Who may see and control this computer's screen, over which way, as things are now: pure, so that every case is tested.
//!
//! Over a **direct** link (Tailscale, this network, this computer) the rule is the one control has always had: a record with `control`, and for a
//! page in a browser its own HTTPS origin, a passkey, the lock on and a window its passkey opened. Through the **relay** (any page, the app the
//! relay hosts included) the screen is allowed only once the person turned on *See and control the screen without Tailscale* on the computer
//! (`ScreenOptions::remote`), with the lock on, a passkey and a window open, and each new screen needs a fresh passkey check with user
//! verification (Face ID), used once. Whoever runs the relay serves the hosted app's code: turning this on means trusting them, as Chrome
//! Remote Desktop means trusting Google (`docs/user-guide.md`). A pane's takeover keeps its own rule (`mobile::transport_cap`): never through
//! the relay.

use super::store::ScreenOptions;
use super::verbs::Permission;

/// What is known of the device and of its connection now.
#[derive(Clone, Copy, Debug)]
pub struct Facts {
    pub record: Permission,
    /// The native app on its own connection, which no page sits in front of: control as its record says, whatever the way.
    pub native: bool,
    /// Paired on this computer's HTTPS origin (or on the relay's hosted app, which is a secure origin of its own).
    pub secure_origin: bool,
    pub hosted: bool,
    pub lock_on: bool,
    pub passkey: bool,
    /// A window its passkey opened is open now.
    pub window: bool,
    /// This connection comes through the relay.
    pub relay: bool,
}

/// The way a screen goes, as asked (`auto` picks). `Tailscale` is the link when it is direct, `Relay` the link through the relay's WebSocket,
/// `P2p` WebRTC data channels straight between the phone and this computer (the link carries only their setup).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Auto,
    Tailscale,
    Relay,
    P2p,
}

impl Via {
    pub fn parse(s: Option<&str>) -> Option<Via> {
        match s.unwrap_or("auto") {
            "auto" => Some(Via::Auto),
            "tailscale" => Some(Via::Tailscale),
            "relay" => Some(Via::Relay),
            "p2p" => Some(Via::P2p),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Via::Auto => "auto",
            Via::Tailscale => "tailscale",
            Via::Relay => "relay",
            Via::P2p => "p2p",
        }
    }
}

/// Whether any way that is not the direct link is turned on: the relay's WebSocket, or peer to peer.
fn any_remote(opts: &ScreenOptions) -> bool {
    opts.methods.relay_ws || opts.methods.p2p
}

/// Whether the way `way` itself is turned on, where it has a switch: a direct link's screen is never ended by its switch (it is the way
/// control always went), the native app's way is its own.
fn way_on(f: &Facts, opts: &ScreenOptions, way: Via) -> bool {
    match way {
        Via::Relay => f.native || opts.methods.relay_ws,
        Via::P2p => opts.methods.p2p,
        Via::Auto | Via::Tailscale => true,
    }
}

/// The passkey check a new screen through the relay brings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Uv {
    /// Over this screen's challenge, with user verification, not expired and not used before.
    Fresh,
    Missing,
    /// Expired, over another challenge, without user verification, or failing.
    Stale,
    /// Used before.
    Replayed,
}

/// Why not, as the phone is told it (`code`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    NotGranted,
    PlainPage,
    MethodOff,
    NoDirectLink,
    NotOnRelay,
    ScreenRemoteOff,
    LockOff,
    NoPasskey,
    Locked,
    NeedsFaceId,
}

impl Reason {
    pub fn code(self) -> &'static str {
        match self {
            Reason::NotGranted => "not_granted",
            Reason::PlainPage => "plain_page",
            Reason::MethodOff => "method_off",
            Reason::NoDirectLink => "no_direct_link",
            Reason::NotOnRelay => "not_on_relay",
            Reason::ScreenRemoteOff => "screen_remote_off",
            Reason::LockOff => "lock_off",
            Reason::NoPasskey => "no_passkey",
            Reason::Locked => "locked",
            Reason::NeedsFaceId => "needs_face_id",
        }
    }

    /// What the phone says, and what would change it.
    pub fn message(self) -> &'static str {
        match self {
            Reason::NotGranted => "This device may not take control. Allow it in Kelpie → Settings → Mobile.",
            Reason::PlainPage => "Seeing the screen needs a page that came from your computer over HTTPS, or the relay with the screen allowed through it.",
            Reason::MethodOff => "That way is turned off on the computer (Kelpie → Settings → Mobile → Advanced).",
            Reason::NoDirectLink => "Tailscale is off on this phone or not reachable now.",
            Reason::NotOnRelay => "This phone is connected directly now: choose Automatic.",
            Reason::ScreenRemoteOff => "Turn on \"See and control the screen without Tailscale\" in Kelpie → Settings → Mobile.",
            Reason::LockOff => "Needs the app lock on: Kelpie → Settings → Mobile.",
            Reason::NoPasskey => "Needs a passkey for the app lock on this phone.",
            Reason::Locked => "Unlock the app on this phone first.",
            Reason::NeedsFaceId => "Confirm with Face ID (or your phone's passkey) to see the screen through the relay.",
        }
    }
}

/// What a page's control rests on over a direct link (`mobile::control_rule`, the reasons it gives).
fn direct_rule(f: &Facts) -> Result<(), Reason> {
    if f.native {
        return Ok(());
    }
    if f.hosted || !f.secure_origin {
        return Err(if f.hosted { Reason::NotOnRelay } else { Reason::PlainPage });
    }
    lock_rule(f)
}

fn lock_rule(f: &Facts) -> Result<(), Reason> {
    if !f.lock_on {
        Err(Reason::LockOff)
    } else if !f.passkey {
        Err(Reason::NoPasskey)
    } else if !f.window {
        Err(Reason::Locked)
    } else {
        Ok(())
    }
}

/// Whether the screen may be used on this connection now, before any one is started: what its verbs other than a start need, and what a hold rests
/// on as it goes (with `opts` as they are now). The way: `relay` for a connection through the relay, else direct.
pub fn allowed(f: &Facts, opts: &ScreenOptions) -> Result<(), Reason> {
    if f.record < Permission::Control {
        // A page on the relay's hosted app cannot be given control until `hosted_ceiling` allows it: say that step, since "may take control"
        // is not offered for it yet.
        let step = if f.hosted && !f.native { hosted_block(opts, f.lock_on, f.passkey) } else { None };
        return Err(step.unwrap_or(Reason::NotGranted));
    }
    if f.native {
        return Ok(());
    }
    if !f.relay {
        return direct_rule(f);
    }
    if !opts.remote {
        return Err(Reason::ScreenRemoteOff);
    }
    if !any_remote(opts) {
        return Err(Reason::MethodOff);
    }
    // Only a page paired on a secure origin comes through the relay at all (`link::reconnect`); said again here.
    if !f.secure_origin {
        return Err(Reason::PlainPage);
    }
    lock_rule(f)
}

/// Whether a new screen may start now, over `via` (`auto`: this connection's own way: through the relay its WebSocket when that is on, else
/// peer to peer), and the way it then goes. A screen through the relay's connection needs a fresh Face ID whichever way its picture takes.
pub fn may_start(f: &Facts, opts: &ScreenOptions, via: Via, uv: Uv) -> Result<Via, Reason> {
    let way = match via {
        Via::Auto if f.relay && !f.native && !opts.methods.relay_ws && opts.methods.p2p => Via::P2p,
        Via::Auto if f.relay => Via::Relay,
        Via::Auto => Via::Tailscale,
        Via::Tailscale if f.relay => return Err(Reason::NoDirectLink),
        Via::Relay if !f.relay => return Err(Reason::NotOnRelay),
        v => v,
    };
    if (way == Via::Tailscale && !opts.methods.tailscale && !f.native) || (way == Via::P2p && !opts.methods.p2p) {
        return Err(Reason::MethodOff);
    }
    allowed(f, opts)?;
    if !way_on(f, opts, way) {
        return Err(Reason::MethodOff);
    }
    if f.relay && !f.native && uv != Uv::Fresh {
        return Err(Reason::NeedsFaceId);
    }
    Ok(way)
}

/// What a screen going `way` rests on as it goes: the connection's rule (`allowed`), and that way still turned on.
pub fn held(f: &Facts, opts: &ScreenOptions, way: Via) -> Result<(), Reason> {
    allowed(f, opts)?;
    if way_on(f, opts, way) {
        Ok(())
    } else {
        Err(Reason::MethodOff)
    }
}

/// The record a page may be raised to over and above the lock's own rule: one paired on the relay's hosted app may be given control (the screen
/// only) once the screen is allowed through the relay, with the lock on and a passkey on record.
pub fn hosted_ceiling(opts: &ScreenOptions, lock_on: bool, passkey: bool) -> Permission {
    if hosted_block(opts, lock_on, passkey).is_none() {
        Permission::Control
    } else {
        Permission::Write
    }
}

/// What keeps a page on the relay's hosted app from being given control, the first that applies; None once it may be.
fn hosted_block(opts: &ScreenOptions, lock_on: bool, passkey: bool) -> Option<Reason> {
    if !opts.remote {
        Some(Reason::ScreenRemoteOff)
    } else if !any_remote(opts) {
        Some(Reason::MethodOff)
    } else if !lock_on {
        Some(Reason::LockOff)
    } else if !passkey {
        Some(Reason::NoPasskey)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile::store::ScreenMethods;

    fn opts(remote: bool, relay_ws: bool, tailscale: bool) -> ScreenOptions {
        opts_p2p(remote, relay_ws, tailscale, true)
    }

    fn opts_p2p(remote: bool, relay_ws: bool, tailscale: bool, p2p: bool) -> ScreenOptions {
        ScreenOptions { remote, methods: ScreenMethods { tailscale, p2p, relay_turn: true, relay_ws }, ..ScreenOptions::default() }
    }

    /// Every combination of page kind, way, switch, lock, passkey, window, Face ID and allowed ways gives the rule's answer: computed here the
    /// long way, one condition at a time, and compared.
    #[test]
    fn the_grid() {
        let mut rows = 0;
        for native in [false, true] {
            for hosted in [false, true] {
                for secure_origin in [false, true] {
                    for relay in [false, true] {
                        for remote in [false, true] {
                            for lock_on in [false, true] {
                                for passkey in [false, true] {
                                    for window in [false, true] {
                                        for record in [Permission::Write, Permission::Control] {
                                            for (relay_ws, tailscale, p2p) in [(true, true, true), (false, true, true), (true, false, true), (true, true, false), (false, true, false)] {
                                                for uv in [Uv::Fresh, Uv::Missing, Uv::Stale, Uv::Replayed] {
                                                    for via in [Via::Auto, Via::Tailscale, Via::Relay, Via::P2p] {
                                                        if native && hosted {
                                                            continue;
                                                        }
                                                        rows += 1;
                                                        let f = Facts { record, native, secure_origin, hosted, lock_on, passkey, window, relay };
                                                        let o = opts_p2p(remote, relay_ws, tailscale, p2p);
                                                        let got = may_start(&f, &o, via, uv);
                                                        let lock = if !lock_on {
                                                            Err(Reason::LockOff)
                                                        } else if !passkey {
                                                            Err(Reason::NoPasskey)
                                                        } else if !window {
                                                            Err(Reason::Locked)
                                                        } else {
                                                            Ok(())
                                                        };
                                                        // Automatic through the relay: its WebSocket, or peer to peer when that alone is on.
                                                        let way = match via {
                                                            Via::Auto if relay && !native && !relay_ws && p2p => Via::P2p,
                                                            Via::Auto if relay => Via::Relay,
                                                            Via::Auto => Via::Tailscale,
                                                            v => v,
                                                        };
                                                        let want = if via == Via::Tailscale && relay {
                                                            Err(Reason::NoDirectLink)
                                                        } else if via == Via::Relay && !relay {
                                                            Err(Reason::NotOnRelay)
                                                        } else if (way == Via::Tailscale && !tailscale && !native) || (way == Via::P2p && !p2p) {
                                                            Err(Reason::MethodOff)
                                                        } else if record < Permission::Control {
                                                            // A hosted page is told the step before control can be given it.
                                                            if hosted && !remote {
                                                                Err(Reason::ScreenRemoteOff)
                                                            } else if hosted && !relay_ws && !p2p {
                                                                Err(Reason::MethodOff)
                                                            } else if hosted && !lock_on {
                                                                Err(Reason::LockOff)
                                                            } else if hosted && !passkey {
                                                                Err(Reason::NoPasskey)
                                                            } else {
                                                                Err(Reason::NotGranted)
                                                            }
                                                        } else if native {
                                                            Ok(way)
                                                        } else if !relay {
                                                            // Today's rule, unchanged: no new Face ID over a direct link, peer to peer or not.
                                                            if hosted {
                                                                Err(Reason::NotOnRelay)
                                                            } else if !secure_origin {
                                                                Err(Reason::PlainPage)
                                                            } else {
                                                                lock.map(|()| way)
                                                            }
                                                        } else if !remote {
                                                            Err(Reason::ScreenRemoteOff)
                                                        } else if !relay_ws && !p2p {
                                                            Err(Reason::MethodOff)
                                                        } else if !secure_origin {
                                                            Err(Reason::PlainPage)
                                                        } else if let Err(e) = lock {
                                                            Err(e)
                                                        } else if way == Via::Relay && !relay_ws {
                                                            Err(Reason::MethodOff)
                                                        } else if uv != Uv::Fresh {
                                                            Err(Reason::NeedsFaceId)
                                                        } else {
                                                            Ok(way)
                                                        };
                                                        assert_eq!(got, want, "{f:?} remote {remote} relay_ws {relay_ws} tailscale {tailscale} p2p {p2p} {via:?} {uv:?}");
                                                        // What a hold rests on as it goes is the start's rule without the Face ID, for the way it went.
                                                        if let Ok(way) = may_start(&f, &o, via, Uv::Fresh) {
                                                            assert_eq!(held(&f, &o, way), Ok(()), "{f:?} {way:?}");
                                                        }
                                                        assert_eq!(allowed(&f, &o).is_ok(), held(&f, &o, Via::Tailscale).is_ok(), "{f:?}");
                                                    }
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        assert!(rows > 10_000, "{rows}");
    }

    /// A screen going one way ends when that way is turned off; the switch for the other way leaves it alone.
    #[test]
    fn a_way_turned_off_ends_its_screens_only() {
        let f = Facts { record: Permission::Control, native: false, secure_origin: true, hosted: true, lock_on: true, passkey: true, window: true, relay: true };
        assert_eq!(held(&f, &opts_p2p(true, false, true, true), Via::P2p), Ok(()), "the relay's WebSocket off: peer to peer goes on");
        assert_eq!(held(&f, &opts_p2p(true, false, true, true), Via::Relay), Err(Reason::MethodOff));
        assert_eq!(held(&f, &opts_p2p(true, true, true, false), Via::P2p), Err(Reason::MethodOff));
        assert_eq!(held(&f, &opts_p2p(false, true, true, true), Via::P2p), Err(Reason::ScreenRemoteOff), "the switch ends every way through the relay");
        let direct = Facts { hosted: false, relay: false, ..f };
        assert_eq!(held(&direct, &opts_p2p(false, false, false, true), Via::P2p), Ok(()), "over a direct link the switch is not asked");
        assert_eq!(held(&direct, &opts_p2p(false, false, false, false), Via::P2p), Err(Reason::MethodOff));
        // Automatic through the relay with its WebSocket off starts peer to peer, with Face ID.
        assert_eq!(may_start(&f, &opts_p2p(true, false, true, true), Via::Auto, Uv::Fresh), Ok(Via::P2p));
        assert_eq!(may_start(&f, &opts_p2p(true, false, true, true), Via::Auto, Uv::Missing), Err(Reason::NeedsFaceId));
        assert_eq!(hosted_ceiling(&opts_p2p(true, false, true, true), true, true), Permission::Control, "peer to peer alone is a way through");
    }
    #[test]
    fn the_switch_off_is_the_old_rule_and_a_hosted_page_is_never_raised_past_typing_then() {
        let off = ScreenOptions::default();
        assert!(!off.remote, "off by default");
        assert_eq!(hosted_ceiling(&off, true, true), Permission::Write);
        let on = opts(true, true, true);
        assert_eq!(hosted_ceiling(&on, true, true), Permission::Control);
        assert_eq!(hosted_ceiling(&on, false, true), Permission::Write, "the lock off");
        assert_eq!(hosted_ceiling(&on, true, false), Permission::Write, "no passkey");
        assert_eq!(hosted_ceiling(&opts_p2p(true, false, true, false), true, true), Permission::Write, "every way through the relay turned off");
        // A direct page with everything open and the switch off: today's answer.
        let f = Facts { record: Permission::Control, native: false, secure_origin: true, hosted: false, lock_on: true, passkey: true, window: true, relay: false };
        assert_eq!(may_start(&f, &off, Via::Auto, Uv::Missing), Ok(Via::Tailscale));
        assert_eq!(may_start(&Facts { relay: true, ..f }, &off, Via::Auto, Uv::Fresh), Err(Reason::ScreenRemoteOff));
        // A hosted page that may type is told the switch, which control waits on, not a "may take control" the computer does not offer yet;
        // once control may be given, that is what it is told.
        let hosted = Facts { record: Permission::Write, hosted: true, relay: true, ..f };
        assert_eq!(allowed(&hosted, &off), Err(Reason::ScreenRemoteOff));
        assert_eq!(allowed(&Facts { passkey: false, ..hosted }, &on), Err(Reason::NoPasskey));
        assert_eq!(allowed(&hosted, &on), Err(Reason::NotGranted));
        assert_eq!(allowed(&Facts { hosted: false, ..hosted }, &off), Err(Reason::NotGranted), "a page from this computer can be given control");
    }

    #[test]
    fn every_reason_says_what_to_do() {
        for r in [Reason::NotGranted, Reason::PlainPage, Reason::MethodOff, Reason::NoDirectLink, Reason::NotOnRelay, Reason::ScreenRemoteOff, Reason::LockOff, Reason::NoPasskey, Reason::Locked, Reason::NeedsFaceId] {
            assert!(!r.code().is_empty() && r.message().ends_with('.'), "{r:?}");
        }
        assert!(Reason::ScreenRemoteOff.message().contains("See and control the screen without Tailscale"));
        assert_eq!(Via::parse(None), Some(Via::Auto));
        assert_eq!(Via::parse(Some("p2p")), Some(Via::P2p));
        assert_eq!(Via::parse(Some("turn")), None, "not a way this computer has yet");
    }
}
