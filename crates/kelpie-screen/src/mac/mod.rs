//! The Mac side: reads the machine and carries out what the pure modules decide. Each module is one API:
//! - `ffi`: the C functions and constants of the frameworks used, and Core Foundation's owned objects.
//! - `system_mac`: AppKit's loop on the main thread, the user-interactive QoS, no App Nap while streaming.
//! - `capture_sck`: ScreenCaptureKit, the newest picture into a one-slot mailbox with what changed.
//! - `encode_vt`: VideoToolbox H.264, and the BGRA → NV12 scaling into its buffers.
//! - `targets_mac`, `states_mac`, `input_cg`: displays and windows, what the phone is told about them, posted Core Graphics events.
//! - `border_mac`: the sharing border and badge (left out of every capture), and the stop hotkey.
//! - `stream`: this side of `stream_core::Backend`, the input thread, and `Host`, which the stdin commands reach.

pub mod border_mac;
pub mod capture_sck;
pub mod encode_vt;
pub mod ffi;
pub mod input_cg;
pub mod states_mac;
pub mod stream;
pub mod system_mac;
pub mod targets_mac;
