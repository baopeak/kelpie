//! How the helper runs: per-monitor DPI aware (v2), so every rectangle is physical pixels; above normal priority while it streams, never
//! high or real-time; opted out of EcoQoS, which would slow a windowless process down; capture and encode threads registered with MMCSS as
//! "Capture".

use windows::core::w;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Threading::{
    AvRevertMmThreadCharacteristics, AvSetMmThreadCharacteristicsW, GetCurrentProcess, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation, ABOVE_NORMAL_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS,
    PROCESS_POWER_THROTTLING_CURRENT_VERSION, PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};

/// Once, first thing: coordinates in physical pixels everywhere.
pub fn init_process() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// While a stream runs: above normal priority, and no EcoQoS throttling.
pub fn streaming(on: bool) {
    unsafe {
        let process = GetCurrentProcess();
        let _ = SetPriorityClass(process, if on { ABOVE_NORMAL_PRIORITY_CLASS } else { NORMAL_PRIORITY_CLASS });
        // ControlMask set, StateMask clear: execution speed is never throttled for this process.
        let state = PROCESS_POWER_THROTTLING_STATE { Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION, ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED, StateMask: 0 };
        let _ = SetProcessInformation(process, ProcessPowerThrottling, &state as *const _ as *const _, std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32);
    }
}

/// The calling thread as MMCSS "Capture" for as long as the guard lives.
pub struct Mmcss(HANDLE);

impl Mmcss {
    pub fn capture() -> Option<Mmcss> {
        let mut index = 0u32;
        unsafe { AvSetMmThreadCharacteristicsW(w!("Capture"), &mut index).ok().map(Mmcss) }
    }
}

impl Drop for Mmcss {
    fn drop(&mut self) {
        unsafe {
            let _ = AvRevertMmThreadCharacteristics(self.0);
        }
    }
}
