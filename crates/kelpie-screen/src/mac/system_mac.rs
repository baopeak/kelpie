//! How the helper runs on a Mac: an accessory app (no Dock icon, no menu bar) whose main thread runs AppKit's loop, which the sharing border
//! and Carbon's stop hotkey need; the stream and input threads at the user-interactive QoS; and no App Nap while a phone sees the screen,
//! which would otherwise slow a windowless process down.

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::msg_send;

use super::ffi::{class, nsstring, pthread_set_qos_class_self_np, QOS_CLASS_USER_INTERACTIVE};

/// The main thread from here on: AppKit's loop, with the hotkey handler installed first. The commands are read on another thread, which ends
/// the process when stdin closes.
pub fn run_main_loop() -> ! {
    unsafe {
        if let Some(cls) = class(c"NSApplication") {
            let app: Retained<AnyObject> = msg_send![cls, sharedApplication];
            // NSApplicationActivationPolicyAccessory.
            let _: bool = msg_send![&*app, setActivationPolicy: 1isize];
            super::border_mac::install_hotkey_handler();
            let _: () = msg_send![&*app, run];
        }
    }
    // Without AppKit there is nothing to show a border with: the commands still get their answers (a start is refused).
    loop {
        std::thread::park();
    }
}

/// The calling thread at the user-interactive QoS (what the stream and its input are).
pub fn interactive_thread() {
    unsafe {
        let _ = pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0);
    }
}

/// No App Nap and no timer coalescing while it lives (`NSProcessInfo beginActivityWithOptions:`); the machine may still sleep when idle.
pub struct Activity(Retained<AnyObject>);

/// `NSActivityUserInitiatedAllowingIdleSystemSleep | NSActivityLatencyCritical`.
const ACTIVITY: u64 = (0x00FF_FFFF & !(1u64 << 20)) | 0xFF_0000_0000;

impl Activity {
    pub fn begin() -> Option<Activity> {
        unsafe {
            let info: Retained<AnyObject> = msg_send![class(c"NSProcessInfo")?, processInfo];
            let reason = nsstring("A phone sees this screen");
            let token: Option<Retained<AnyObject>> = msg_send![&*info, beginActivityWithOptions: ACTIVITY, reason: &*reason];
            token.map(Activity)
        }
    }
}

impl Drop for Activity {
    fn drop(&mut self) {
        unsafe {
            if let Some(cls) = class(c"NSProcessInfo") {
                let info: Retained<AnyObject> = msg_send![cls, processInfo];
                let _: () = msg_send![&*info, endActivity: &*self.0];
            }
        }
    }
}
