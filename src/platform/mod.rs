//! Platform layer: raw Win32 helpers on Windows, no-op stubs elsewhere.
//!
//! On Windows this uses the `windows-sys` crate to call the Win32 API
//! directly (the low-level half of "Rust for Windows"): global cursor
//! position / visibility, a real `HCURSOR` built from RGBA pixels, direct
//! `SetCursor` forcing, system-cursor swapping (`SetSystemCursor`), click
//! pass-through (`WS_EX_TRANSPARENT`) and target-window tracking.

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(not(target_os = "windows"))]
mod stub;
#[cfg(not(target_os = "windows"))]
#[allow(unused_imports)]
pub use stub::*;
