//! What Windows says about the machine's CPU, memory, disks and GPUs (see `resources`).
//!
//! CPU and memory come from the same calls the load level uses (`GetSystemTimes`, `GlobalMemoryStatusEx`, the low-memory
//! notification). Disks and GPUs come from performance counters (PDH), read with one query opened once: a rate counter (a
//! disk's idle time, an engine's utilization) needs two collections, so the first reading after the start has none of them.
//! A counter this machine does not have is left out, and what depends on it reads as unknown.

use std::collections::HashMap;
use std::mem::size_of;

use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
use windows_sys::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetDriveTypeW};
use windows_sys::Win32::System::Memory::{CreateMemoryResourceNotification, LowMemoryResourceNotification, QueryMemoryResourceNotification};
use windows_sys::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PDH_CSTATUS_NEW_DATA, PDH_CSTATUS_VALID_DATA, PDH_FMT_COUNTERVALUE_ITEM_W,
    PDH_FMT_DOUBLE, PDH_HCOUNTER, PDH_HQUERY, PDH_MORE_DATA,
};
use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows_sys::Win32::System::Threading::GetSystemTimes;

use super::{DiskReading, GpuReading, MachineReading, VolumeReading};

/// Values over 100 % are kept (a queue length, several engines): PDH caps percentages unless told not to.
const PDH_FMT_NOCAP100: u32 = 0x8000;

fn ticks(t: FILETIME) -> u64 {
    (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// The counters asked for, by what they measure. Wildcards (`*`) give one value per disk, adapter or process.
const COUNTERS: [(&str, &str); 8] = [
    ("disk_idle", r"\PhysicalDisk(*)\% Idle Time"),
    ("disk_queue", r"\PhysicalDisk(*)\Avg. Disk Queue Length"),
    ("disk_bytes", r"\PhysicalDisk(*)\Disk Bytes/sec"),
    ("cpu_queue", r"\System\Processor Queue Length"),
    ("gpu_engine", r"\GPU Engine(*)\Utilization Percentage"),
    ("gpu_dedicated", r"\GPU Adapter Memory(*)\Dedicated Usage"),
    ("gpu_shared", r"\GPU Adapter Memory(*)\Shared Usage"),
    ("gpu_proc_mem", r"\GPU Process Memory(*)\Local Usage"),
];

struct Query {
    query: PDH_HQUERY,
    counters: Vec<(&'static str, PDH_HCOUNTER)>,
}

// The query is used from the one sampler thread at a time; PDH handles are not tied to a thread.
unsafe impl Send for Query {}

impl Query {
    fn open() -> Option<Query> {
        let mut query: PDH_HQUERY = std::ptr::null_mut();
        // SAFETY: a null data source (live counters) and a valid out-pointer.
        if unsafe { windows_sys::Win32::System::Performance::PdhOpenQueryW(std::ptr::null(), 0, &mut query) } != 0 {
            return None;
        }
        let mut counters = Vec::new();
        for (name, path) in COUNTERS {
            let mut counter: PDH_HCOUNTER = std::ptr::null_mut();
            let path = wide(path);
            // SAFETY: an open query, a NUL-terminated path and a valid out-pointer. A counter the machine lacks fails alone.
            if unsafe { PdhAddEnglishCounterW(query, path.as_ptr(), 0, &mut counter) } == 0 {
                counters.push((name, counter));
            } else {
                tracing::info!(counter = name, "performance counter not available");
            }
        }
        Some(Query { query, counters })
    }

    /// Collects once and reads every counter as (instance, value) pairs.
    fn read(&mut self) -> HashMap<&'static str, Vec<(String, f64)>> {
        let mut out = HashMap::new();
        // SAFETY: an open query.
        if unsafe { PdhCollectQueryData(self.query) } != 0 {
            return out;
        }
        for (name, counter) in &self.counters {
            if let Some(values) = array(*counter) {
                out.insert(*name, values);
            }
        }
        out
    }
}

impl Drop for Query {
    fn drop(&mut self) {
        // SAFETY: the query was opened by this value and is closed once.
        unsafe { PdhCloseQuery(self.query) };
    }
}

/// Every instance of a wildcard counter with valid data.
fn array(counter: PDH_HCOUNTER) -> Option<Vec<(String, f64)>> {
    let (mut size, mut count) = (0u32, 0u32);
    // SAFETY: asking for the size with a null buffer.
    let first = unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, &mut size, &mut count, std::ptr::null_mut()) };
    if first != PDH_MORE_DATA || size == 0 {
        return None;
    }
    // Whole words, so the items at the start of the buffer are aligned; their names are written after them.
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    let items = buf.as_mut_ptr().cast::<PDH_FMT_COUNTERVALUE_ITEM_W>();
    // SAFETY: a buffer of the size PDH asked for.
    if unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE | PDH_FMT_NOCAP100, &mut size, &mut count, items) } != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count as usize {
        // SAFETY: PDH filled `count` items at the start of the buffer; each name is a NUL-terminated string inside it.
        let item = unsafe { &*items.add(i) };
        if item.FmtValue.CStatus != PDH_CSTATUS_VALID_DATA && item.FmtValue.CStatus != PDH_CSTATUS_NEW_DATA {
            continue;
        }
        let name = unsafe {
            let mut len = 0;
            while *item.szName.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(item.szName, len))
        };
        // SAFETY: asked for as a double.
        out.push((name, unsafe { item.FmtValue.Anonymous.doubleValue }));
    }
    Some(out)
}

/// `pid_1234_luid_0x0_0x1_phys_0_eng_0_engtype_3D` → (1234, "luid_0x0_0x1_phys_0", "3D").
fn parse_engine(name: &str) -> Option<(u32, String, String)> {
    let rest = name.strip_prefix("pid_")?;
    let (pid, rest) = rest.split_once('_')?;
    let adapter_end = rest.find("_eng_")?;
    let engtype = rest.rsplit_once("_engtype_").map(|(_, t)| t.to_string()).unwrap_or_default();
    Some((pid.parse().ok()?, rest[..adapter_end].to_string(), engtype))
}

/// `pid_1234_luid_0x0_0x1_phys_0` → (1234, "luid_0x0_0x1_phys_0").
fn parse_proc_adapter(name: &str) -> Option<(u32, String)> {
    let (pid, adapter) = name.strip_prefix("pid_")?.split_once('_')?;
    Some((pid.parse().ok()?, adapter.to_string()))
}

#[derive(Default)]
pub struct Probe {
    /// (idle, total) as of the last reading.
    cpu: Option<(u64, u64)>,
    low_memory: Option<isize>,
    query: Option<Option<Query>>,
}

impl Probe {
    pub fn read(&mut self, volumes: &[std::path::PathBuf]) -> MachineReading {
        let mut r = MachineReading { cores: std::thread::available_parallelism().map_or(1, |n| n.get()) as u32, ..MachineReading::default() };
        r.cpu_pct = self.cpu();
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
        // SAFETY: a structure with its length set.
        if unsafe { GlobalMemoryStatusEx(&mut status) } != 0 && status.ullTotalPhys > 0 {
            r.mem_total_mb = Some(status.ullTotalPhys >> 20);
            r.mem_avail_mb = Some(status.ullAvailPhys >> 20);
            r.commit_limit_mb = Some(status.ullTotalPageFile >> 20);
            r.commit_avail_mb = Some(status.ullAvailPageFile >> 20);
        }
        r.low_memory = self.low_memory();
        let counters = self.query.get_or_insert_with(Query::open).as_mut().map(Query::read).unwrap_or_default();
        r.cpu_queue = counters.get("cpu_queue").and_then(|v| v.first()).map(|(_, q)| *q);
        r.disks = disks(&counters);
        let (gpus, by_pid) = gpus(&counters);
        r.gpus = gpus;
        r.gpu_by_pid = by_pid;
        r.volumes = volumes.iter().filter_map(|v| volume(v)).collect();
        r
    }

    fn cpu(&mut self) -> Option<f64> {
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut idle, mut kernel, mut user) = (zero, zero, zero);
        // SAFETY: three valid out-pointers.
        if unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } == 0 {
            return None;
        }
        // Kernel time includes idle time.
        let now = (ticks(idle), ticks(kernel) + ticks(user));
        let before = self.cpu.replace(now)?;
        let (idle, total) = (now.0.saturating_sub(before.0), now.1.saturating_sub(before.1));
        (total > 0).then(|| 100.0 * (1.0 - idle as f64 / total as f64))
    }

    fn low_memory(&mut self) -> bool {
        // SAFETY: creating a notification object takes no pointers.
        let handle = *self.low_memory.get_or_insert_with(|| unsafe { CreateMemoryResourceNotification(LowMemoryResourceNotification) } as isize);
        if handle == 0 {
            return false;
        }
        let mut low = 0;
        // SAFETY: a handle this probe created and a valid out-pointer.
        unsafe { QueryMemoryResourceNotification(handle as _, &mut low) != 0 && low != 0 }
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        if let Some(handle) = self.low_memory.filter(|h| *h != 0) {
            // SAFETY: the handle was created by this probe and is closed once.
            unsafe { CloseHandle(handle as _) };
        }
    }
}

/// Physical disks by their instance name (`0 C: D:`); `_Total` is left out.
fn disks(c: &HashMap<&'static str, Vec<(String, f64)>>) -> Vec<DiskReading> {
    let Some(idle) = c.get("disk_idle") else { return Vec::new() };
    let find = |key: &str, name: &str| c.get(key).and_then(|v| v.iter().find(|(n, _)| n == name)).map(|(_, x)| *x);
    idle.iter()
        .filter(|(name, _)| name != "_Total")
        .map(|(name, idle)| DiskReading {
            name: name.clone(),
            busy_pct: Some((100.0 - idle).clamp(0.0, 100.0)),
            queue: find("disk_queue", name),
            mbps: find("disk_bytes", name).map(|b| b / 1_048_576.0),
        })
        .collect()
}

/// Adapters with their busiest engine type (the way Task Manager shows a GPU's use) and memory, and GPU use by process.
fn gpus(c: &HashMap<&'static str, Vec<(String, f64)>>) -> (Vec<GpuReading>, HashMap<u32, (f64, u64)>) {
    // (adapter, engine type) → sum over processes; (pid, engine type) → sum over its engines.
    let mut by_engine: HashMap<(String, String), f64> = HashMap::new();
    let mut by_pid_engine: HashMap<(u32, String), f64> = HashMap::new();
    for (name, value) in c.get("gpu_engine").into_iter().flatten() {
        if let Some((pid, adapter, engtype)) = parse_engine(name) {
            *by_engine.entry((adapter, engtype.clone())).or_default() += value;
            *by_pid_engine.entry((pid, engtype)).or_default() += value;
        }
    }
    let mut adapters: HashMap<String, GpuReading> = HashMap::new();
    for ((adapter, _), value) in &by_engine {
        let g = adapters.entry(adapter.clone()).or_insert_with(|| GpuReading { id: adapter.clone(), ..GpuReading::default() });
        g.util_pct = Some(g.util_pct.unwrap_or(0.0).max(value.min(100.0)));
    }
    for (key, field) in [("gpu_dedicated", 0), ("gpu_shared", 1)] {
        for (name, bytes) in c.get(key).into_iter().flatten() {
            let g = adapters.entry(name.clone()).or_insert_with(|| GpuReading { id: name.clone(), ..GpuReading::default() });
            let mb = (*bytes as u64) >> 20;
            if field == 0 {
                g.dedicated_mb = Some(mb);
            } else {
                g.shared_mb = Some(mb);
            }
        }
    }
    let mut by_pid: HashMap<u32, (f64, u64)> = HashMap::new();
    for ((pid, _), value) in by_pid_engine {
        let e = by_pid.entry(pid).or_default();
        e.0 = e.0.max(value.min(100.0));
    }
    for (name, bytes) in c.get("gpu_proc_mem").into_iter().flatten() {
        if let Some((pid, _)) = parse_proc_adapter(name) {
            by_pid.entry(pid).or_default().1 += (*bytes as u64) >> 20;
        }
    }
    // An adapter nothing has ever used (a display-less render device) reports no engines: it is left out.
    let mut list: Vec<GpuReading> = adapters.into_values().filter(|g| g.util_pct.is_some()).collect();
    list.sort_by(|a, b| a.id.cmp(&b.id));
    (list, by_pid)
}

/// `GetDriveTypeW`: a network drive.
const DRIVE_REMOTE: u32 = 4;

/// Free and total space of the volume whose root is `path` (a mapped network drive is left out: asking it can stall).
fn volume(path: &std::path::Path) -> Option<VolumeReading> {
    let p = wide(&path.to_string_lossy());
    // SAFETY: a NUL-terminated root path.
    if unsafe { GetDriveTypeW(p.as_ptr()) } == DRIVE_REMOTE {
        return None;
    }
    let (mut free, mut total) = (0u64, 0u64);
    // SAFETY: a NUL-terminated path and valid out-pointers (the third, total free bytes, is not asked for).
    if unsafe { GetDiskFreeSpaceExW(p.as_ptr(), &mut free, &mut total, std::ptr::null_mut()) } == 0 || total == 0 {
        return None;
    }
    Some(VolumeReading { path: path.to_string_lossy().into_owned(), free_mb: free >> 20, total_mb: total >> 20 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_and_process_instances_are_read_apart() {
        assert_eq!(parse_engine("pid_10144_luid_0x00000000_0x0000f6e9_phys_0_eng_0_engtype_3D"), Some((10144, "luid_0x00000000_0x0000f6e9_phys_0".into(), "3D".into())));
        assert_eq!(parse_engine("luid_0x0_phys_0"), None);
        assert_eq!(parse_proc_adapter("pid_7_luid_0x00000000_0x0000f6e9_phys_0"), Some((7, "luid_0x00000000_0x0000f6e9_phys_0".into())));
    }

    #[test]
    fn an_adapter_is_as_busy_as_its_busiest_engine_type() {
        let mut c: HashMap<&'static str, Vec<(String, f64)>> = HashMap::new();
        c.insert(
            "gpu_engine",
            vec![
                ("pid_1_luid_A_phys_0_eng_0_engtype_3D".into(), 30.0),
                ("pid_2_luid_A_phys_0_eng_0_engtype_3D".into(), 25.0),
                ("pid_2_luid_A_phys_0_eng_3_engtype_VideoDecode".into(), 40.0),
            ],
        );
        c.insert("gpu_proc_mem", vec![("pid_2_luid_A_phys_0".into(), 64.0 * 1_048_576.0)]);
        c.insert("gpu_dedicated", vec![("luid_A_phys_0".into(), 0.0)]);
        let (gpus, by_pid) = gpus(&c);
        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].util_pct, Some(55.0), "3D: 30 + 25, above video decode's 40");
        assert_eq!(by_pid[&2], (40.0, 64));
        assert_eq!(by_pid[&1].0, 30.0);
    }

    /// The real counters on this machine: nothing here may fail on a computer without a GPU or disk counters.
    #[test]
    fn a_reading_of_this_machine_has_memory_and_its_volume() {
        let mut probe = Probe::default();
        let here = std::env::temp_dir();
        let _ = probe.read(std::slice::from_ref(&here));
        std::thread::sleep(std::time::Duration::from_millis(300));
        let r = probe.read(std::slice::from_ref(&here));
        assert!(r.mem_total_mb.unwrap() > 512);
        assert!(r.mem_avail_mb.unwrap() <= r.mem_total_mb.unwrap());
        assert!(r.commit_limit_mb.unwrap() >= r.mem_total_mb.unwrap() / 2);
        assert_eq!(r.volumes.len(), 1);
        assert!(r.volumes[0].free_mb <= r.volumes[0].total_mb);
        assert!(r.cpu_pct.is_some());
    }
}
