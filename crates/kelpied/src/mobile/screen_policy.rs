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

/// The way a screen goes, as asked (`auto` picks).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Via {
    Auto,
    Tailscale,
    Relay,
}

impl Via {
    pub fn parse(s: Option<&str>) -> Option<Via> {
        match s.unwrap_or("auto") {
            "auto" => Some(Via::Auto),
            "tailscale" => Some(Via::Tailscale),
            "relay" => Some(Via::Relay),
            _ => None,
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            Via::Auto => "auto",
            Via::Tailscale => "tailscale",
            Via::Relay => "relay",
        }
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
        return Err(Reason::NotGranted);
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
    if !opts.methods.relay_ws {
        return Err(Reason::MethodOff);
    }
    // Only a page paired on a secure origin comes through the relay at all (`link::reconnect`); said again here.
    if !f.secure_origin {
        return Err(Reason::PlainPage);
    }
    lock_rule(f)
}

/// Whether a new screen may start now, over `via` (`auto`: this connection's own way), and the way it then goes.
pub fn may_start(f: &Facts, opts: &ScreenOptions, via: Via, uv: Uv) -> Result<Via, Reason> {
    let way = match via {
        Via::Auto if f.relay => Via::Relay,
        Via::Auto => Via::Tailscale,
        Via::Tailscale if f.relay => return Err(Reason::NoDirectLink),
        Via::Relay if !f.relay => return Err(Reason::NotOnRelay),
        v => v,
    };
    if way == Via::Tailscale && !opts.methods.tailscale && !f.native {
        return Err(Reason::MethodOff);
    }
    allowed(f, opts)?;
    if way == Via::Relay && !f.native && uv != Uv::Fresh {
        return Err(Reason::NeedsFaceId);
    }
    Ok(way)
}

/// The record a page may be raised to over and above the lock's own rule: one paired on the relay's hosted app may be given control (the screen
/// only) once the screen is allowed through the relay, with the lock on and a passkey on record.
pub fn hosted_ceiling(opts: &ScreenOptions, lock_on: bool, passkey: bool) -> Permission {
    if opts.remote && opts.methods.relay_ws && lock_on && passkey {
        Permission::Control
    } else {
        Permission::Write
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile::store::ScreenMethods;

    fn opts(remote: bool, relay_ws: bool, tailscale: bool) -> ScreenOptions {
        ScreenOptions { remote, methods: ScreenMethods { tailscale, p2p: true, relay_turn: true, relay_ws }, ..ScreenOptions::default() }
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
                                            for (relay_ws, tailscale) in [(true, true), (false, true), (true, false)] {
                                                for uv in [Uv::Fresh, Uv::Missing, Uv::Stale, Uv::Replayed] {
                                                    for via in [Via::Auto, Via::Tailscale, Via::Relay] {
                                                        if native && hosted {
                                                            continue;
                                                        }
                                                        rows += 1;
                                                        let f = Facts { record, native, secure_origin, hosted, lock_on, passkey, window, relay };
                                                        let o = opts(remote, relay_ws, tailscale);
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
                                                        let want = if via == Via::Tailscale && relay {
                                                            Err(Reason::NoDirectLink)
                                                        } else if via == Via::Relay && !relay {
                                                            Err(Reason::NotOnRelay)
                                                        } else if !relay && !tailscale && !native {
                                                            Err(Reason::MethodOff)
                                                        } else if record < Permission::Control {
                                                            Err(Reason::NotGranted)
                                                        } else if native {
                                                            Ok(if relay { Via::Relay } else { Via::Tailscale })
                                                        } else if !relay {
                                                            // Today's rule, unchanged: no new Face ID over a direct link.
                                                            if hosted {
                                                                Err(Reason::NotOnRelay)
                                                            } else if !secure_origin {
                                                                Err(Reason::PlainPage)
                                                            } else {
                                                                lock.map(|()| Via::Tailscale)
                                                            }
                                                        } else if !remote {
                                                            Err(Reason::ScreenRemoteOff)
                                                        } else if !relay_ws {
                                                            Err(Reason::MethodOff)
                                                        } else if !secure_origin {
                                                            Err(Reason::PlainPage)
                                                        } else if let Err(e) = lock {
                                                            Err(e)
                                                        } else if uv != Uv::Fresh {
                                                            Err(Reason::NeedsFaceId)
                                                        } else {
                                                            Ok(Via::Relay)
                                                        };
                                                        assert_eq!(got, want, "{f:?} remote {remote} relay_ws {relay_ws} tailscale {tailscale} {via:?} {uv:?}");
                                                        // What a hold rests on as it goes is the start's rule without the Face ID.
                                                        let held = allowed(&f, &o).is_ok();
                                                        let start_but_face = may_start(&f, &o, Via::Auto, Uv::Fresh).is_ok();
                                                        assert_eq!(held, start_but_face || (!relay && !tailscale && !native && allowed(&f, &o).is_ok()), "{f:?}");
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

    #[test]
    fn the_switch_off_is_the_old_rule_and_a_hosted_page_is_never_raised_past_typing_then() {
        let off = ScreenOptions::default();
        assert!(!off.remote, "off by default");
        assert_eq!(hosted_ceiling(&off, true, true), Permission::Write);
        let on = opts(true, true, true);
        assert_eq!(hosted_ceiling(&on, true, true), Permission::Control);
        assert_eq!(hosted_ceiling(&on, false, true), Permission::Write, "the lock off");
        assert_eq!(hosted_ceiling(&on, true, false), Permission::Write, "no passkey");
        assert_eq!(hosted_ceiling(&opts(true, false, true), true, true), Permission::Write, "the relay's way turned off");
        // A direct page with everything open and the switch off: today's answer.
        let f = Facts { record: Permission::Control, native: false, secure_origin: true, hosted: false, lock_on: true, passkey: true, window: true, relay: false };
        assert_eq!(may_start(&f, &off, Via::Auto, Uv::Missing), Ok(Via::Tailscale));
        assert_eq!(may_start(&Facts { relay: true, ..f }, &off, Via::Auto, Uv::Fresh), Err(Reason::ScreenRemoteOff));
    }

    #[test]
    fn every_reason_says_what_to_do() {
        for r in [Reason::NotGranted, Reason::PlainPage, Reason::MethodOff, Reason::NoDirectLink, Reason::NotOnRelay, Reason::ScreenRemoteOff, Reason::LockOff, Reason::NoPasskey, Reason::Locked, Reason::NeedsFaceId] {
            assert!(!r.code().is_empty() && r.message().ends_with('.'), "{r:?}");
        }
        assert!(Reason::ScreenRemoteOff.message().contains("See and control the screen without Tailscale"));
        assert_eq!(Via::parse(None), Some(Via::Auto));
        assert_eq!(Via::parse(Some("p2p")), None, "not a way this computer has yet");
    }
}
