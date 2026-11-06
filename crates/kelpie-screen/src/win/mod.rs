//! The Windows side: reads the machine and carries out what the pure modules decide. Nothing here decides anything worth a test of its
//! own; each module is one API:
//! - `system`: per-monitor DPI awareness, priority, EcoQoS, MMCSS.
//! - `d3d`: the D3D11 device on the right adapter, monitors, textures, readback.
//! - `capture_dxgi` (a monitor) and `capture_wgc` (a window): the newest picture into the texture pool, with what changed if known.
//! - `convert`: BGRA → NV12 at the encoder's size (D3D11 video processor).
//! - `encode_mf`: Media Foundation H.264, hardware first, the inbox software encoder as the fallback.
//! - `targets_win`, `states_win`, `input_win`: windows and monitors, what the phone is told about them, `SendInput`.
//! - `border`: the sharing border and badge (left out of every capture), and the stop hotkey.
//! - `stream`: one stream's loop, and `Host`, which the stdin commands reach.

pub mod border;
pub mod capture_dxgi;
pub mod capture_wgc;
pub mod convert;
pub mod d3d;
pub mod encode_mf;
pub mod input_win;
pub mod states_win;
pub mod stream;
pub mod system;
pub mod targets_win;

use std::time::{SystemTime, UNIX_EPOCH};

/// Wall-clock milliseconds, the clock the daemon stamps input with.
pub fn wall_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// A wide string with its terminating zero.
pub fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// A wide buffer up to its first zero.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}
