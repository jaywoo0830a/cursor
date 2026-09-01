//! Platform layer: raw Win32 helpers on Windows, no-op stubs elsewhere.
//!
//! On Windows this uses the `windows-sys` crate to call the Win32 API
//! directly (the low-level half of "Rust for Windows"): global cursor
//! position (`GetCursorPos`), target-window tracking (`EnumWindows`), click
//! pass-through (`WS_EX_TRANSPARENT`) and input forwarding to the app below
//! (`PostMessage`). The cursor itself is rendered by the Chromium frontend.

#[cfg(target_os = "windows")]
mod windows;
#[cfg(target_os = "windows")]
pub use windows::*;

#[cfg(not(target_os = "windows"))]
mod stub;
#[cfg(not(target_os = "windows"))]
#[allow(unused_imports)]
pub use stub::*;
