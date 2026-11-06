//! Kelpie's screen helper: a phone with `control` sees a monitor or one window of this computer and drives it.
//!
//! The helper is spawned by the Kelpie app (`app/src-tauri/src/screen.rs`), which speaks to it over stdin (JSON lines) and stdout (frames,
//! `wire`). It captures with Windows Graphics Capture (a window) or DXGI Desktop Duplication (a monitor), encodes H.264 with Media Foundation
//! (hardware first, the inbox software encoder as the fallback), and injects the phone's input with `SendInput`. The app hands each frame to
//! the daemon, which sends it to the phone as a binary message on the phone's end-to-end encrypted link.
//!
//! What decides anything is in the modules below, which need no GPU and no Windows and are tested as such; the native code (`win`) only
//! reads the machine and carries the decisions out:
//! - `geom`: where a point of the picture is on the screen, in `SendInput`'s units, across monitors with their own DPI.
//! - `targets`: which windows are offered, and the shortcuts (IDEs, the Kelpie window).
//! - `states`: secure desktop, locked, elevated, minimized, closed, covered: what the phone is told and what input may do.
//! - `control`: the quality ladder, the encoder overuse detector, the bandwidth controller, the load caps and the keyframe limit.
//! - `pace`: when to take a picture (a constant-rate timer, one in flight, a boost after input), the top-off of a still one, the large-frame
//!   guard and when the encoder hears a new bitrate.
//! - `tiles`: which 64×64 cells changed, and which still ones get a crisp tile painted over the video.
//! - `sps`: the SPS rewrite that tells a decoder no frame is reordered, and the codec string.
//! - `wire`: the helper's stdout frames and the media messages the phone reads.
//! - `input`: the closed set of input a phone may send, the keys, the stop hotkey.

/// The window class of the sharing border, which is never offered as a target and never captured.
pub const BORDER_CLASS: &str = "KelpieScreenBorder";

pub mod control;
pub mod geom;
pub mod input;
pub mod pace;
pub mod sps;
pub mod states;
pub mod targets;
pub mod tiles;
pub mod wire;

#[cfg(feature = "helper")]
pub mod tile_image;

#[cfg(all(windows, feature = "helper"))]
pub mod win;
