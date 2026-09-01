//! Non-Windows no-op implementation of the platform layer.

#![allow(dead_code)]

use core::ffi::c_void;

pub type HWND = *mut c_void;

pub fn global_cursor_pos() -> Option<(f64, f64)> {
    None
}

pub fn create_hcursor_from_rgba(_rgba: &[u8], _w: u16, _h: u16, _x: u16, _y: u16) -> usize {
    0
}

pub fn set_cursor_handle(_hcursor: Option<usize>) {}

pub fn destroy_cursor(_hcursor: usize) {}

pub fn find_window_by_title(_sub: &str) -> Option<HWND> {
    None
}

pub fn window_outer_rect(_hwnd: HWND) -> Option<(i32, i32, i32, i32)> {
    None
}

pub fn is_iconic(_hwnd: HWND) -> bool {
    false
}

pub fn apply_passthrough(_hwnd: usize, _passthrough: bool) {}

pub fn set_forwarding(_enabled: bool, _our_hwnd: usize) {}

pub fn set_forward_block_rects(_rects: &[(i32, i32, i32, i32)]) {}

pub fn forward_mouse(_x: i32, _y: i32, _msg: u32, _wparam: usize) {}

