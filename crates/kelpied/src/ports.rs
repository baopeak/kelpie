//! Which processes listen on which TCP ports. Asked of the operating system only when the app asks
//! (`process.*`): nothing here polls. Windows reads the extended TCP table (one call per address
//! family), Linux reads `/proc/net/tcp*` and matches socket inodes to the processes asked about, and
//! macOS asks `lsof`. The parsers take plain text or bytes so each can be tested on any machine.

use std::net::{Ipv4Addr, Ipv6Addr};

use serde::Serialize;

/// One listening TCP socket.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Listener {
    pub pid: u32,
    pub port: u16,
    /// The bound address (`0.0.0.0`, `127.0.0.1`, `::1`…).
    pub addr: String,
}

impl Listener {
    /// The host a browser on this machine reaches it by: `localhost` for a loopback or wildcard
    /// binding, the address itself when the server only listens on one network interface.
    pub fn host(&self) -> String {
        let local = self.addr.parse::<std::net::IpAddr>().map_or(true, |a| a.is_loopback() || a.is_unspecified());
        if local {
            "localhost".into()
        } else if self.addr.contains(':') {
            format!("[{}]", self.addr)
        } else {
            self.addr.clone()
        }
    }
}

/// Listening sockets owned by `pids` (`None`: by anyone, to tell who holds a port).
pub fn listeners(pids: Option<&[u32]>) -> Vec<Listener> {
    let mut all = os::listeners(pids);
    if let Some(pids) = pids {
        all.retain(|l| pids.contains(&l.pid));
    }
    all.sort_by(|a, b| (a.port, &a.addr, a.pid).cmp(&(b.port, &b.addr, b.pid)));
    all.dedup();
    all
}

/// Windows: rows of `MIB_TCPTABLE_OWNER_PID` (`ipv6 == false`) or `MIB_TCP6TABLE_OWNER_PID`.
/// A count of rows, then per row: state, local address, local port (network byte order), remote
/// address, remote port, owning pid — IPv6 rows carry 16-byte addresses and a scope id each.
#[cfg(any(windows, test))]
fn parse_windows_table(bytes: &[u8], ipv6: bool) -> Vec<Listener> {
    let word = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let rows = word(0).unwrap_or(0) as usize;
    let size = if ipv6 { 56 } else { 24 };
    let mut out = Vec::new();
    for i in 0..rows {
        let at = 4 + i * size;
        if bytes.len() < at + size {
            break;
        }
        let (addr, port, pid) = if ipv6 {
            let a: [u8; 16] = bytes[at..at + 16].try_into().unwrap();
            (Ipv6Addr::from(a).to_string(), word(at + 20), word(at + 52))
        } else {
            (Ipv4Addr::from(word(at + 4).unwrap_or(0).to_le_bytes()).to_string(), word(at + 8), word(at + 20))
        };
        if let (Some(port), Some(pid)) = (port, pid) {
            out.push(Listener { pid, port: u16::from_be(port as u16), addr });
        }
    }
    out
}

/// Linux: the listening sockets of `/proc/net/tcp` (`ipv6 == false`) or `tcp6` as
/// `(address, port, socket inode)`. State `0A` is LISTEN.
#[cfg(any(target_os = "linux", test))]
fn parse_proc_net_tcp(text: &str, ipv6: bool) -> Vec<(String, u16, u64)> {
    let mut out = Vec::new();
    for line in text.lines().skip(1) {
        let f: Vec<&str> = line.split_whitespace().collect();
        // sl local rem st tx:rx tr:tm retrnsmt uid timeout inode
        if f.len() < 10 || f[3] != "0A" {
            continue;
        }
        let Some((host, port)) = f[1].split_once(':') else { continue };
        let (Ok(port), Ok(inode)) = (u16::from_str_radix(port, 16), f[9].parse::<u64>()) else { continue };
        let addr = if ipv6 {
            // Four 32-bit words, each in the machine's byte order (little-endian here, as on every
            // target Linux runs the daemon on that matters).
            if host.len() != 32 {
                continue;
            }
            let mut bytes = [0u8; 16];
            for (i, chunk) in host.as_bytes().chunks(8).enumerate() {
                let Ok(w) = u32::from_str_radix(std::str::from_utf8(chunk).unwrap_or(""), 16) else { return out };
                bytes[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
            }
            Ipv6Addr::from(bytes).to_string()
        } else {
            let Ok(w) = u32::from_str_radix(host, 16) else { continue };
            Ipv4Addr::from(w.to_le_bytes()).to_string()
        };
        out.push((addr, port, inode));
    }
    out
}

/// macOS: `lsof -nP -iTCP -sTCP:LISTEN -F pn`: a `p<pid>` line, then one `n<address>:<port>` line per socket.
#[cfg(any(target_os = "macos", test))]
fn parse_lsof(text: &str) -> Vec<Listener> {
    let mut out = Vec::new();
    let mut pid = None;
    for line in text.lines() {
        match line.split_at_checked(1) {
            Some(("p", rest)) => pid = rest.parse::<u32>().ok(),
            Some(("n", rest)) => {
                let (Some(pid), Some((host, port))) = (pid, rest.rsplit_once(':')) else { continue };
                let Ok(port) = port.parse::<u16>() else { continue };
                let host = host.trim_start_matches('[').trim_end_matches(']');
                out.push(Listener { pid, port, addr: if host == "*" { "0.0.0.0".into() } else { host.into() } });
            }
            _ => {}
        }
    }
    out
}

#[cfg(windows)]
mod os {
    use super::{parse_windows_table, Listener};
    use windows_sys::Win32::NetworkManagement::IpHelper::{GetExtendedTcpTable, TCP_TABLE_OWNER_PID_LISTENER};
    use windows_sys::Win32::Networking::WinSock::{AF_INET, AF_INET6};

    pub fn listeners(_pids: Option<&[u32]>) -> Vec<Listener> {
        let mut out = Vec::new();
        for (family, ipv6) in [(AF_INET, false), (AF_INET6, true)] {
            // The table can grow between the size query and the read: ask again with what it says.
            let mut buf = vec![0u8; 16 * 1024];
            for _ in 0..4 {
                let mut size = buf.len() as u32;
                let rc = unsafe { GetExtendedTcpTable(buf.as_mut_ptr().cast(), &mut size, 0, family as u32, TCP_TABLE_OWNER_PID_LISTENER, 0) };
                match rc {
                    0 => {
                        out.extend(parse_windows_table(&buf, ipv6));
                        break;
                    }
                    // ERROR_INSUFFICIENT_BUFFER
                    122 => buf.resize(size as usize + 1024, 0),
                    _ => break,
                }
            }
        }
        out
    }
}

#[cfg(target_os = "linux")]
mod os {
    use super::{parse_proc_net_tcp, Listener};
    use std::collections::HashMap;

    pub fn listeners(pids: Option<&[u32]>) -> Vec<Listener> {
        let mut sockets = Vec::new();
        for (file, ipv6) in [("/proc/net/tcp", false), ("/proc/net/tcp6", true)] {
            if let Ok(text) = std::fs::read_to_string(file) {
                sockets.extend(parse_proc_net_tcp(&text, ipv6));
            }
        }
        if sockets.is_empty() {
            return Vec::new();
        }
        // Which process holds each socket: read the descriptors of the processes asked about (or every process).
        let owners = |pid: u32| -> Vec<u64> {
            let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else { return Vec::new() };
            fds.filter_map(|fd| fd.ok()?.path().read_link().ok())
                .filter_map(|link| link.to_str()?.strip_prefix("socket:[")?.strip_suffix(']')?.parse().ok())
                .collect()
        };
        let all: Vec<u32>;
        let pids = match pids {
            Some(p) => p,
            None => {
                all = std::fs::read_dir("/proc").into_iter().flatten().filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok()).collect();
                &all
            }
        };
        let mut by_inode: HashMap<u64, u32> = HashMap::new();
        for &pid in pids {
            for inode in owners(pid) {
                by_inode.entry(inode).or_insert(pid);
            }
        }
        sockets.into_iter().filter_map(|(addr, port, inode)| Some(Listener { pid: *by_inode.get(&inode)?, port, addr })).collect()
    }
}

#[cfg(target_os = "macos")]
mod os {
    use super::{parse_lsof, Listener};

    pub fn listeners(pids: Option<&[u32]>) -> Vec<Listener> {
        let mut cmd = std::process::Command::new("/usr/sbin/lsof");
        cmd.args(["-nP", "-iTCP", "-sTCP:LISTEN", "-F", "pn"]);
        if let Some(pids) = pids {
            if pids.is_empty() {
                return Vec::new();
            }
            cmd.args(["-a", "-p", &pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",")]);
        }
        // lsof exits 1 when nothing matches: that is an empty answer, not an error.
        cmd.output().map(|o| parse_lsof(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod os {
    use super::Listener;

    pub fn listeners(_pids: Option<&[u32]>) -> Vec<Listener> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4_row(addr: [u8; 4], port: u16, pid: u32) -> Vec<u8> {
        let mut r = Vec::new();
        for w in [2u32, u32::from_le_bytes(addr), u32::from(port.to_be()), 0, 0, pid] {
            r.extend(w.to_le_bytes());
        }
        r
    }

    fn v6_row(addr: Ipv6Addr, port: u16, pid: u32) -> Vec<u8> {
        let mut r = addr.octets().to_vec();
        r.extend(0u32.to_le_bytes());
        r.extend(u32::from(port.to_be()).to_le_bytes());
        r.extend([0u8; 24]);
        r.extend(2u32.to_le_bytes());
        r.extend(pid.to_le_bytes());
        assert_eq!(r.len(), 56);
        r
    }

    #[test]
    fn windows_tables_parse_both_families() {
        let mut v4 = 2u32.to_le_bytes().to_vec();
        v4.extend(v4_row([127, 0, 0, 1], 5173, 4242));
        v4.extend(v4_row([0, 0, 0, 0], 8765, 77));
        assert_eq!(
            parse_windows_table(&v4, false),
            vec![Listener { pid: 4242, port: 5173, addr: "127.0.0.1".into() }, Listener { pid: 77, port: 8765, addr: "0.0.0.0".into() }]
        );
        let mut v6 = 1u32.to_le_bytes().to_vec();
        v6.extend(v6_row(Ipv6Addr::LOCALHOST, 3000, 9));
        assert_eq!(parse_windows_table(&v6, true), vec![Listener { pid: 9, port: 3000, addr: "::1".into() }]);
        // A table cut short, or one that claims more rows than it holds, gives the rows it has.
        assert_eq!(parse_windows_table(&v4[..v4.len() - 3], false).len(), 1);
        assert!(parse_windows_table(&[], false).is_empty());
        assert!(parse_windows_table(&[9, 0, 0, 0], true).is_empty());
    }

    #[test]
    fn proc_net_tcp_keeps_listening_sockets() {
        let tcp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 0100007F:1435 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 23456 1 0000000000000000 100 0 0 10 0\n\
   1: 00000000:1F40 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 23457 1 0000000000000000 100 0 0 10 0\n\
   2: 0100007F:D2A0 0100007F:1435 01 00000000:00000000 00:00000000 00000000  1000        0 99999 1 0000000000000000 20 4 30 10 -1\n";
        assert_eq!(parse_proc_net_tcp(tcp, false), vec![("127.0.0.1".into(), 5173, 23456), ("0.0.0.0".into(), 8000, 23457)]);
        let tcp6 = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
   0: 00000000000000000000000001000000:0BB8 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 31337 1 0000000000000000 100 0 0 10 0\n";
        assert_eq!(parse_proc_net_tcp(tcp6, true), vec![("::1".into(), 3000, 31337)]);
        assert!(parse_proc_net_tcp("header only\n", false).is_empty());
        assert!(parse_proc_net_tcp("h\n garbage line\n", true).is_empty());
    }

    #[test]
    fn lsof_output_is_read_per_process() {
        let out = "p501\nn*:5173\nn[::1]:5173\np777\nn127.0.0.1:8765\np12\nnbogus\n";
        assert_eq!(
            parse_lsof(out),
            vec![
                Listener { pid: 501, port: 5173, addr: "0.0.0.0".into() },
                Listener { pid: 501, port: 5173, addr: "::1".into() },
                Listener { pid: 777, port: 8765, addr: "127.0.0.1".into() },
            ]
        );
        assert!(parse_lsof("").is_empty());
    }

    #[test]
    fn a_server_bound_to_one_interface_is_reached_by_that_address() {
        let l = |addr: &str| Listener { pid: 1, port: 80, addr: addr.into() }.host();
        assert_eq!(l("127.0.0.1"), "localhost");
        assert_eq!(l("0.0.0.0"), "localhost");
        assert_eq!(l("::1"), "localhost");
        assert_eq!(l("::"), "localhost");
        assert_eq!(l("192.168.1.20"), "192.168.1.20");
        assert_eq!(l("fe80::1"), "[fe80::1]");
    }

    #[test]
    fn a_real_listener_is_found_by_its_owner() {
        let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = socket.local_addr().unwrap().port();
        let me = std::process::id();
        let mine = listeners(Some(&[me]));
        assert!(mine.iter().any(|l| l.port == port && l.pid == me), "{mine:?}");
        assert!(listeners(Some(&[me])).iter().all(|l| l.pid == me));
        assert!(listeners(None).iter().any(|l| l.port == port && l.pid == me));
        assert!(!listeners(Some(&[])).iter().any(|l| l.port == port));
        drop(socket);
    }
}
