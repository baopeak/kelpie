//! What macOS says about the machine's load (see `load`). Only `std` and the system's C
//! interfaces, declared here: no Objective-C crate for a handful of calls.

use std::ffi::{c_char, c_int, c_void, CString};

use super::Reading;

type Id = *mut c_void;

#[link(name = "objc")]
extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Id;
    fn objc_msgSend();
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
}

#[link(name = "Foundation", kind = "framework")]
extern "C" {}

extern "C" {
    fn mach_host_self() -> u32;
    fn host_statistics(host: u32, flavor: c_int, info: *mut c_int, count: *mut u32) -> c_int;
    fn sysctlbyname(name: *const c_char, old: *mut c_void, old_len: *mut usize, new: *mut c_void, new_len: usize) -> c_int;
}

const HOST_CPU_LOAD_INFO: c_int = 3;
/// `NSActivityUserInitiatedAllowingIdleSystemSleep` (`NSActivityUserInitiated` without
/// `NSActivityIdleSystemSleepDisabled`): exempt from App Nap, and the Mac may still go to sleep.
/// Keeping the computer awake stays with the existing keep-awake setting.
const ACTIVITY_USER_INITIATED: u64 = 0x00EF_FFFF;

fn selector(name: &str) -> Id {
    let name = CString::new(name).expect("a selector has no NUL");
    // SAFETY: a valid C string.
    unsafe { sel_registerName(name.as_ptr()) }
}

/// `[object selector]` returning an object.
unsafe fn send(object: Id, name: &str) -> Id {
    let call: unsafe extern "C" fn(Id, Id) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    call(object, selector(name))
}

/// `[object selector]` returning a flag.
unsafe fn send_flag(object: Id, name: &str) -> bool {
    let call: unsafe extern "C" fn(Id, Id) -> u8 = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    call(object, selector(name)) != 0
}

/// `[object selector]` returning an integer.
unsafe fn send_int(object: Id, name: &str) -> isize {
    let call: unsafe extern "C" fn(Id, Id) -> isize = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
    call(object, selector(name))
}

/// `[NSProcessInfo processInfo]`.
unsafe fn process_info() -> Id {
    let class = objc_getClass(c"NSProcessInfo".as_ptr());
    send(class, "processInfo")
}

#[derive(Default)]
pub struct Probe {
    /// Ticks (user, system, idle, nice) as of the last reading.
    cpu: Option<[u32; 4]>,
}

impl Probe {
    pub fn read(&mut self) -> Reading {
        // SAFETY: the pool is popped before returning; the calls are read-only queries of the process object.
        let (battery_saver, thermal) = unsafe {
            let pool = objc_autoreleasePoolPush();
            let info = process_info();
            // Low Power Mode is macOS's battery saver; "serious" or "critical" heat is thermal pressure.
            let result = (send_flag(info, "isLowPowerModeEnabled"), send_int(info, "thermalState") >= 2);
            objc_autoreleasePoolPop(pool);
            result
        };
        Reading { cpu: self.cpu(), memory_free: memory_free(), low_memory: false, battery_saver, thermal }
    }

    fn cpu(&mut self) -> Option<f64> {
        let mut ticks = [0u32; 4];
        let mut count = ticks.len() as u32;
        // SAFETY: `ticks` holds the four counters this flavor returns, and `count` says so.
        let status = unsafe { host_statistics(mach_host_self(), HOST_CPU_LOAD_INFO, ticks.as_mut_ptr() as *mut c_int, &mut count) };
        if status != 0 {
            return None;
        }
        let before = self.cpu.replace(ticks)?;
        let delta = |i: usize| u64::from(ticks[i].wrapping_sub(before[i]));
        let total: u64 = (0..4).map(delta).sum();
        (total > 0).then(|| 100.0 * (1.0 - delta(2) as f64 / total as f64))
    }
}

/// Percent of memory the system counts as available (`kern.memorystatus_level`).
fn memory_free() -> Option<f64> {
    let mut level: c_int = 0;
    let mut size = std::mem::size_of::<c_int>();
    // SAFETY: an int-sized out buffer, as the sysctl returns.
    let status = unsafe { sysctlbyname(c"kern.memorystatus_level".as_ptr(), &mut level as *mut c_int as *mut c_void, &mut size, std::ptr::null_mut(), 0) };
    (status == 0).then(|| f64::from(level))
}

/// Not needed here: the process class is left alone and macOS has no EcoQoS.
pub fn opt_out_of_throttling() {}

/// An activity that keeps macOS from napping the daemon (App Nap) while a phone is connected; it does
/// not keep the Mac awake (idle system sleep is allowed).
pub struct Activity(Id);

// SAFETY: a retained Objective-C token, used only to end the activity from whichever thread drops it.
unsafe impl Send for Activity {}

impl Activity {
    pub fn begin() -> Activity {
        // SAFETY: strings and the activity token are created and retained inside a pool; the token is released in `drop`.
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let string_class = objc_getClass(c"NSString".as_ptr());
            let make: unsafe extern "C" fn(Id, Id, *const c_char) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let reason = make(string_class, selector("stringWithUTF8String:"), c"Kelpie phone connected".as_ptr());
            let begin: unsafe extern "C" fn(Id, Id, u64, Id) -> Id = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let token = begin(process_info(), selector("beginActivityWithOptions:reason:"), ACTIVITY_USER_INITIATED, reason);
            let token = if token.is_null() { token } else { send(token, "retain") };
            objc_autoreleasePoolPop(pool);
            Activity(token)
        }
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        if self.0.is_null() {
            return;
        }
        // SAFETY: `self.0` is the retained token `begin` made; it is ended and released once.
        unsafe {
            let pool = objc_autoreleasePoolPush();
            let end: unsafe extern "C" fn(Id, Id, Id) = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            end(process_info(), selector("endActivity:"), self.0);
            send(self.0, "release");
            objc_autoreleasePoolPop(pool);
        }
    }
}

/// No thread listing here: the runtime's own counters in the log say what is queued.
pub fn thread_report() -> String {
    String::new()
}
