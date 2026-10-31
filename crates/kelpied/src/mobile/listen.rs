//! Where phone access listens: every address a phone could reach (private LAN ranges and Tailscale's), never 0.0.0.0, each
//! with a listener of its own on the same port. While no single address is configured, the set follows the machine's
//! interfaces: an address change (Windows `NotifyUnicastIpAddressChange`, and a 5 s look everywhere) is acted on after 2 s
//! of quiet, the listeners that are not wanted any more stop, new ones start, and every phone connected is told the new
//! list (`addrs`).

use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use parking_lot::Mutex;

use super::Generation;

/// How often the interfaces are looked at even without a change notification.
const POLL: Duration = Duration::from_secs(5);
/// Changes come in bursts (an adapter going down and up): act once they settle.
const SETTLE: Duration = Duration::from_secs(2);

/// Tailscale's IPv4 range, 100.64.0.0/10.
pub fn tailscale_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..128).contains(&o[1])
}

/// Tailscale's IPv6 range, fd7a:115c:a1e0::/48.
pub fn tailscale_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
}

pub fn tailscale(ip: IpAddr) -> bool {
    match ip.to_canonical() {
        IpAddr::V4(v4) => tailscale_v4(v4),
        IpAddr::V6(v6) => tailscale_v6(v6),
    }
}

/// Addresses a phone could reach: private IPv4 ranges and Tailscale's (both families).
pub fn usable(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => !v4.is_loopback() && (v4.is_private() || tailscale_v4(v4)),
        IpAddr::V6(v6) => tailscale_v6(v6),
    }
}

/// This machine's usable addresses with their interface names, in a stable order.
pub fn addresses() -> Vec<(IpAddr, String)> {
    let mut out: Vec<(IpAddr, String)> = if_addrs::get_if_addrs().unwrap_or_default().into_iter().filter(|i| usable(i.ip())).map(|i| (i.ip(), i.name.clone())).collect();
    out.sort();
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// What the listeners should be bound to: the configured address alone (a loopback one only for tests), or every usable one.
pub fn wanted(configured: Option<&str>) -> Result<Vec<IpAddr>> {
    if let Some(s) = configured {
        let ip: IpAddr = s.parse().context("invalid address")?;
        if !usable(ip) && !ip.is_loopback() {
            bail!("{ip} is not a private network address");
        }
        return Ok(vec![ip]);
    }
    let all: Vec<IpAddr> = addresses().into_iter().map(|(ip, _)| ip).collect();
    if all.is_empty() {
        bail!("no private network address found");
    }
    Ok(all)
}

/// `ip:port` as a URL host.
pub fn host(addr: &SocketAddr) -> String {
    match addr {
        SocketAddr::V4(a) => a.to_string(),
        SocketAddr::V6(a) => format!("[{}]:{}", a.ip(), a.port()),
    }
}

/// The computer's name for the phone (`<name>.local`, which most phones resolve on the LAN).
pub fn hostname() -> String {
    let name = std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok().filter(|n| !n.is_empty()).or_else(os_hostname).unwrap_or_else(|| "kelpie".into());
    format!("{}.local", name.to_lowercase())
}

#[cfg(unix)]
fn os_hostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most `len` bytes.
    let r = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    (r == 0).then(|| String::from_utf8_lossy(&buf[..buf.iter().position(|b| *b == 0).unwrap_or(buf.len())]).into_owned())
}

#[cfg(not(unix))]
fn os_hostname() -> Option<String> {
    None
}

/// The listeners of one server run: one per address, each with its own stop.
#[derive(Default)]
pub struct Listeners {
    bound: Mutex<BTreeMap<SocketAddr, Arc<Generation>>>,
}

impl Listeners {
    pub fn addrs(&self) -> Vec<SocketAddr> {
        self.bound.lock().keys().copied().collect()
    }

    /// Binds what is wanted and not bound, stops what is bound and not wanted. Returns the new listeners to serve, and
    /// whether anything changed. An address that cannot be bound is left out (and tried again on the next change).
    pub async fn rebind(&self, ips: &[IpAddr], port: u16) -> (Vec<(tokio::net::TcpListener, Arc<Generation>)>, bool, Option<anyhow::Error>) {
        let wanted: Vec<SocketAddr> = ips.iter().map(|ip| SocketAddr::new(*ip, port)).collect();
        let mut changed = false;
        {
            let mut bound = self.bound.lock();
            bound.retain(|addr, stop| {
                let keep = wanted.contains(addr);
                if !keep {
                    stop.cancel();
                    changed = true;
                }
                keep
            });
        }
        let missing: Vec<SocketAddr> = wanted.iter().filter(|a| !self.bound.lock().contains_key(a)).copied().collect();
        let mut fresh = Vec::new();
        let mut error = None;
        for addr in missing {
            match tokio::net::TcpListener::bind(addr).await {
                Ok(l) => {
                    let stop = Arc::new(Generation::new());
                    self.bound.lock().insert(addr, stop.clone());
                    fresh.push((l, stop));
                    changed = true;
                }
                Err(e) => {
                    tracing::warn!("phone access: cannot listen on {addr}: {e}");
                    error.get_or_insert_with(|| anyhow!(e).context(format!("listen on {}", host(&addr))));
                }
            }
        }
        (fresh, changed, error)
    }

    pub fn stop_all(&self) {
        for (_, stop) in std::mem::take(&mut *self.bound.lock()) {
            stop.cancel();
        }
    }
}

/// Waits until the machine's addresses may have changed: a notification from the OS, or the next poll.
pub async fn changed() {
    tokio::select! {
        _ = tokio::time::sleep(POLL) => {}
        _ = os_change() => {}
    }
}

/// Waits for changes to settle.
pub async fn settle() {
    tokio::time::sleep(SETTLE).await;
}

#[cfg(windows)]
async fn os_change() {
    os::notified().await;
}

#[cfg(not(windows))]
async fn os_change() {
    std::future::pending::<()>().await;
}

#[cfg(windows)]
mod os {
    use std::sync::OnceLock;

    use tokio::sync::Notify;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::NetworkManagement::IpHelper::{NotifyUnicastIpAddressChange, MIB_UNICASTIPADDRESS_ROW};
    use windows_sys::Win32::Networking::WinSock::AF_UNSPEC;

    static CHANGED: Notify = Notify::const_new();
    static REGISTERED: OnceLock<bool> = OnceLock::new();

    unsafe extern "system" fn on_change(_ctx: *const core::ffi::c_void, _row: *const MIB_UNICASTIPADDRESS_ROW, _kind: i32) {
        CHANGED.notify_waiters();
    }

    /// Registered once for the daemon's life (it costs nothing while nothing changes).
    pub async fn notified() {
        let ok = *REGISTERED.get_or_init(|| {
            let mut handle: HANDLE = std::ptr::null_mut();
            // SAFETY: the callback only touches a static; the handle is kept for the process's life.
            unsafe { NotifyUnicastIpAddressChange(AF_UNSPEC, Some(on_change), std::ptr::null(), false, &mut handle) == 0 }
        });
        if ok {
            CHANGED.notified().await;
        } else {
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_a_phone_can_use() {
        let ok = |s: &str| usable(s.parse().unwrap());
        assert!(ok("192.168.1.20") && ok("10.0.0.5") && ok("172.20.1.1"));
        assert!(ok("100.101.102.103"), "tailscale");
        assert!(ok("fd7a:115c:a1e0::1234"), "tailscale v6");
        for bad in ["8.8.8.8", "100.20.0.1", "127.0.0.1", "0.0.0.0", "::1", "fe80::1", "fd00::1", "2001:db8::1"] {
            assert!(!ok(bad), "{bad}");
        }
        assert!(tailscale("::ffff:100.64.0.1".parse().unwrap()), "a mapped v4 address");
        assert!(!tailscale("192.168.1.2".parse().unwrap()));
        assert_eq!(host(&"[fd7a:115c:a1e0::1]:4817".parse().unwrap()), "[fd7a:115c:a1e0::1]:4817");
        assert!(wanted(Some("8.8.8.8")).is_err());
        assert!(wanted(Some("0.0.0.0")).is_err(), "never every address at once");
        assert_eq!(wanted(Some("127.0.0.1")).unwrap(), vec!["127.0.0.1".parse::<IpAddr>().unwrap()], "loopback for tests");
        assert!(hostname().ends_with(".local"));
    }

    #[tokio::test]
    async fn listeners_follow_the_wanted_set() {
        let l = Listeners::default();
        let lo: IpAddr = "127.0.0.1".parse().unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let (fresh, changed, err) = l.rebind(&[lo], port).await;
        assert!(changed && err.is_none() && fresh.len() == 1);
        let stop = fresh[0].1.clone();
        let (again, changed, _) = l.rebind(&[lo], port).await;
        assert!(again.is_empty() && !changed, "nothing to do");
        let (_, changed, _) = l.rebind(&[], port).await;
        assert!(changed && stop.is_cancelled(), "an address that went away stops its listener");
        assert!(l.addrs().is_empty());
    }
}
