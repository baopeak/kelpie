//! What Linux and macOS say about the machine (see `resources`).
//!
//! CPU and memory come from `sysinfo` on both. Linux adds its pressure stall information (`/proc/pressure/*`: the share of
//! time tasks waited on the CPU, memory or I/O, the most direct sign of contention there is) and each disk's busy time and
//! queue from `/proc/diskstats`. macOS reads no disk or GPU figures yet, and Linux no GPU: those read as unknown, never as idle.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

#[cfg(target_os = "linux")]
use super::DiskReading;
use super::{MachineReading, VolumeReading};

pub struct Probe {
    sys: System,
    /// Per disk: (busy ms, sectors moved) at the last reading (Linux).
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    disks: HashMap<String, (u64, u64)>,
    at: Option<Instant>,
}

impl Default for Probe {
    fn default() -> Probe {
        let sys = System::new_with_specifics(RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing().with_cpu_usage()).with_memory(MemoryRefreshKind::everything()));
        Probe { sys, disks: HashMap::new(), at: None }
    }
}

impl Probe {
    pub fn read(&mut self, volumes: &[PathBuf]) -> MachineReading {
        self.sys.refresh_cpu_usage();
        self.sys.refresh_memory();
        let mut r = MachineReading { cores: std::thread::available_parallelism().map_or(1, |n| n.get()) as u32, ..MachineReading::default() };
        // The first reading has nothing to measure CPU use against.
        r.cpu_pct = self.at.is_some().then(|| f64::from(self.sys.global_cpu_usage()));
        let total = self.sys.total_memory();
        if total > 0 {
            r.mem_total_mb = Some(total >> 20);
            r.mem_avail_mb = Some(self.sys.available_memory() >> 20);
            if self.sys.total_swap() > 0 {
                r.commit_limit_mb = Some((total + self.sys.total_swap()) >> 20);
                r.commit_avail_mb = Some((self.sys.available_memory() + self.sys.free_swap()) >> 20);
            }
        }
        #[cfg(target_os = "linux")]
        {
            r.pressure = super::Pressure { cpu: psi("cpu", "some"), memory: psi("memory", "full"), io: psi("io", "full") };
            let now = Instant::now();
            let secs = self.at.map(|at| now.duration_since(at).as_secs_f64()).filter(|s| *s > 0.0);
            r.disks = self.diskstats(secs);
        }
        self.at = Some(Instant::now());
        r.volumes = volumes.iter().filter_map(|v| volume(v)).collect();
        r
    }

    /// Whole disks (`sda`, `nvme0n1`, `vda`; not their partitions): how busy each was since the last reading, its queue and throughput.
    #[cfg(target_os = "linux")]
    fn diskstats(&mut self, secs: Option<f64>) -> Vec<DiskReading> {
        let Ok(text) = std::fs::read_to_string("/proc/diskstats") else { return Vec::new() };
        let mut out = Vec::new();
        let mut seen = HashMap::new();
        for line in text.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            // major minor name reads merged sectors_read ms writes merged sectors_written ms in_flight io_ms weighted_ms …
            if f.len() < 14 || !std::path::Path::new(&format!("/sys/block/{}/device", f[2])).exists() {
                continue;
            }
            let num = |i: usize| f[i].parse::<u64>().unwrap_or(0);
            let (name, sectors, in_flight, io_ms) = (f[2].to_string(), num(5) + num(9), num(11), num(12));
            let before = self.disks.get(&name).copied();
            seen.insert(name.clone(), (io_ms, sectors));
            let (busy, mbps) = match (before, secs) {
                (Some((ms0, s0)), Some(secs)) => (Some((io_ms.saturating_sub(ms0) as f64 / (secs * 10.0)).min(100.0)), Some(sectors.saturating_sub(s0) as f64 * 512.0 / 1_048_576.0 / secs)),
                _ => (None, None),
            };
            out.push(DiskReading { name, busy_pct: busy, queue: Some(in_flight as f64), mbps });
        }
        self.disks = seen;
        out
    }
}

/// `avg10` of `/proc/pressure/<what>`'s `some` or `full` line: percent of the last ten seconds tasks stalled on it.
#[cfg(target_os = "linux")]
fn psi(what: &str, line: &str) -> Option<f64> {
    let text = std::fs::read_to_string(format!("/proc/pressure/{what}")).ok()?;
    let row = text.lines().find(|l| l.starts_with(line))?;
    row.split_whitespace().find_map(|kv| kv.strip_prefix("avg10=")).and_then(|v| v.parse().ok())
}

/// Free and total space of the volume holding `path`.
fn volume(path: &std::path::Path) -> Option<VolumeReading> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a NUL-terminated path and a valid out-pointer.
    if unsafe { libc::statvfs(c.as_ptr(), &mut st) } != 0 {
        return None;
    }
    let unit = st.f_frsize as u64;
    let (free, total) = (st.f_bavail as u64 * unit, st.f_blocks as u64 * unit);
    (total > 0).then(|| VolumeReading { path: path.to_string_lossy().into_owned(), free_mb: free >> 20, total_mb: total >> 20 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_of_this_machine_has_memory_and_its_volume() {
        let mut probe = Probe::default();
        let here = std::env::temp_dir();
        let _ = probe.read(std::slice::from_ref(&here));
        std::thread::sleep(std::time::Duration::from_millis(300));
        let r = probe.read(std::slice::from_ref(&here));
        assert!(r.mem_total_mb.unwrap() > 256);
        assert_eq!(r.volumes.len(), 1);
        assert!(r.cpu_pct.is_some());
    }
}
