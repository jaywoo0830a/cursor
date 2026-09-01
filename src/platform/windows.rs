//! Windows implementation of the platform layer: raw Win32 API via
//! `windows-sys`.
//!
//! The cursor itself is rendered by the Chromium webview (pure CSS), so this
//! layer only provides the native plumbing:
//! * global cursor position (`GetCursorPos`)
//! * target-window tracking (`EnumWindows`, `GetWindowRect`)
//! * click pass-through via `WS_EX_TRANSPARENT` (`SetWindowLongPtrW`)
//! * forwarding pointer input to the app below (`PostMessage`)

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;

/// A Windows window handle (`HWND`).
pub type HWND = *mut core::ffi::c_void;

/// Global mouse position in physical screen pixels (origin = top-left of
/// the primary monitor).
pub fn global_cursor_pos() -> Option<(f64, f64)> {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
    unsafe {
        let mut pt = std::mem::zeroed::<POINT>();
        if GetCursorPos(&mut pt) != 0 {
            Some((pt.x as f64, pt.y as f64))
        } else {
            None
        }
    }
}

// Cursor rendering moved to the Chromium frontend (pure CSS: the window owns
// the hit-testing inside the region, so the OS draws the CSS cursor for this
// window and apps below can never override it — no pen pop-out, works over
// DirectComposition). No ShowCursor / SetCursor / SetSystemCursor needed.
/// Find a visible, non-minimized top-level window whose title contains
/// `sub` (case-insensitive). Returns its `HWND`, or `None`.
pub fn find_window_by_title(sub: &str) -> Option<HWND> {
    use std::cell::RefCell;
    use windows_sys::Win32::UI::WindowsAndMessaging::EnumWindows;

    thread_local! {
        static SEARCH: RefCell<String> = const { RefCell::new(String::new()) };
        static FOUND: RefCell<HWND> = const { RefCell::new(std::ptr::null_mut()) };
    }

    unsafe extern "system" fn enum_proc(hwnd: HWND, _lparam: isize) -> i32 {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GetWindowTextW, IsIconic, IsWindowVisible,
        };
        let search = SEARCH.with(|s| s.borrow().clone());
        if search.is_empty() {
            return 1;
        }
        if IsWindowVisible(hwnd) == 0 || IsIconic(hwnd) != 0 {
            return 1;
        }
        let mut buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        let title = String::from_utf16_lossy(&buf[..len.max(0) as usize]);
        if title.to_lowercase().contains(&search) {
            FOUND.with(|f| *f.borrow_mut() = hwnd);
            return 0; // stop enumeration
        }
        1
    }

    SEARCH.with(|s| *s.borrow_mut() = sub.to_lowercase());
    FOUND.with(|f| *f.borrow_mut() = std::ptr::null_mut());
    unsafe {
        EnumWindows(Some(enum_proc), 0);
    }
    let hwnd = FOUND.with(|f| *f.borrow());
    (!hwnd.is_null()).then_some(hwnd)
}

/// Outer rectangle of a window, in physical screen pixels.
pub fn window_outer_rect(hwnd: HWND) -> Option<(i32, i32, i32, i32)> {
    use windows_sys::Win32::Foundation::RECT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect;
    unsafe {
        let mut r = std::mem::zeroed::<RECT>();
        if GetWindowRect(hwnd, &mut r) != 0 {
            Some((r.left, r.top, r.right, r.bottom))
        } else {
            None
        }
    }
}

/// Is the window minimized?
pub fn is_iconic(hwnd: HWND) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsIconic;
    unsafe { IsIconic(hwnd) != 0 }
}

/// Force click pass-through on/off by directly toggling the window's
/// `WS_EX_TRANSPARENT` style. This is a direct fallback on top of
/// winit's own mechanism (`set_cursor_hittest`), for setups where the
/// winit path is unreliable. `WS_EX_LAYERED` is kept for transparency.
pub fn apply_passthrough(hwnd: usize, passthrough: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_LAYERED, WS_EX_TRANSPARENT,
    };
    unsafe {
        let h = hwnd as *mut core::ffi::c_void;
        let style = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let new_style = if passthrough {
            style | (WS_EX_TRANSPARENT | WS_EX_LAYERED) as isize
        } else {
            style & !(WS_EX_TRANSPARENT as isize)
        };
        SetWindowLongPtrW(h, GWL_EXSTYLE, new_style);
    }
}

// (system-cursor swapping removed — the Chromium frontend owns the cursor)
// ---------------------------------------------------------------------------
// Input forwarding — cursor-owning mode
// ---------------------------------------------------------------------------
// When the overlay window owns the hit-testing (non-click-through, so it also
// owns `WM_SETCURSOR` and the app below can never override the cursor), it
// also intercepts all mouse/pen input. We replay those events to the window
// below ours so the app keeps behaving normally (clicks, drawing, wheel,
// hover).

static FORWARD_ON: AtomicBool = AtomicBool::new(false);
static FORWARD_HWND: AtomicUsize = AtomicUsize::new(0);
/// Rectangles (physical screen px) where forwarded clicks are swallowed —
/// i.e. the frontend's own UI (status bar, settings panel). Points inside
/// are not replayed to the app below, so clicking our UI doesn't double-fire.
static FORWARD_BLOCK: Mutex<Vec<(i32, i32, i32, i32)>> = Mutex::new(Vec::new());

/// Enable/disable forwarding of pointer input to the window below our
/// overlay. `our_hwnd` is the overlay window (excluded from the target
/// search).
pub fn set_forwarding(enabled: bool, our_hwnd: usize) {
    FORWARD_ON.store(enabled, Ordering::Relaxed);
    FORWARD_HWND.store(our_hwnd, Ordering::Relaxed);
}

/// Set the frontend-UI rectangles (physical screen px) that must not be
/// replayed to the app below.
pub fn set_forward_block_rects(rects: &[(i32, i32, i32, i32)]) {
    *FORWARD_BLOCK.lock().unwrap() = rects.to_vec();
}

/// Topmost visible top-level window (excluding `exclude`) whose rect contains
/// `(x, y)`, walking the Z-order downward from the top.
fn window_below_at(x: i32, y: i32, exclude: usize) -> Option<HWND> {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetTopWindow, GetWindow, GetWindowRect, IsIconic, IsWindowVisible, GW_HWNDNEXT,
    };
    unsafe {
        let mut h = GetTopWindow(std::ptr::null_mut());
        while !h.is_null() {
            if h as usize != exclude && IsWindowVisible(h) != 0 && IsIconic(h) == 0 {
                let mut r = std::mem::zeroed::<windows_sys::Win32::Foundation::RECT>();
                if GetWindowRect(h, &mut r) != 0
                    && x >= r.left
                    && x < r.right
                    && y >= r.top
                    && y < r.bottom
                {
                    return Some(h);
                }
            }
            h = GetWindow(h, GW_HWNDNEXT);
        }
        None
    }
}

/// Replay one pointer message to the window below our overlay.
/// `wparam` is the message's wParam (modifier keys / wheel-delta high word).
pub fn forward_mouse(pt_x: i32, pt_y: i32, msg: u32, wparam: usize) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        PostMessageW, SetForegroundWindow, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_RBUTTONDOWN,
        WM_XBUTTONDOWN,
    };
    if !FORWARD_ON.load(Ordering::Relaxed) {
        return;
    }
    // Don't replay clicks that landed on our own frontend UI.
    {
        let block = FORWARD_BLOCK.lock().unwrap();
        if block
            .iter()
            .any(|&(x1, y1, x2, y2)| pt_x >= x1 && pt_x < x2 && pt_y >= y1 && pt_y < y2)
        {
            return;
        }
    }
    let exclude = FORWARD_HWND.load(Ordering::Relaxed);
    unsafe {
        let Some(hwnd) = window_below_at(pt_x, pt_y, exclude) else {
            return;
        };
        let mut pt = POINT { x: pt_x, y: pt_y };
        ScreenToClient(hwnd, &mut pt);
        let lparam = (((pt.y as u32) & 0xFFFF) as usize) << 16
            | ((pt.x as u32) & 0xFFFF) as usize;
        // Activate the target on press so clicks behave naturally.
        if matches!(
            msg,
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN
        ) {
            SetForegroundWindow(hwnd);
        }
        PostMessageW(hwnd, msg, wparam, lparam as isize);
    }
}
