//! Windows implementation of the platform layer: raw Win32 API via
//! `windows-sys`.
//!
//! The cursor is rendered **natively in Rust**: we build a real `HCURSOR`
//! from a Rust-generated bitmap and `SetCursor` it whenever the overlay owns
//! the hit-testing (non-click-through inside the region). The OS draws it, so
//! it is smooth, pen-safe (the window owns `WM_SETCURSOR` → apps below can
//! never override) and visible over DirectComposition.
//!
//! Also provides the rest of the native plumbing:
//! * global cursor position (`GetCursorPos`)
//! * target-window tracking (`EnumWindows`, `GetWindowRect`)
//! * click pass-through via `WS_EX_TRANSPARENT` (`SetWindowLongPtrW`)
//! * forwarding pointer input to the app below (`PostMessage`)

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

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

/// Create a real `HCURSOR` from straight RGBA pixels (32bpp ARGB DIB +
/// empty mask). Returns the handle as `usize`, or 0 on failure.
pub fn create_hcursor_from_rgba(rgba: &[u8], w: u16, h: u16, hot_x: u16, hot_y: u16) -> usize {
    use std::mem::{size_of, zeroed};
    use windows_sys::Win32::Graphics::Gdi::{
        CreateBitmap, CreateDIBSection, DeleteObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, ICONINFO};

    unsafe {
        let (w, h) = (w as u32, h as u32);
        let mut bmi: BITMAPINFO = zeroed();
        bmi.bmiHeader.biSize = size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w as i32;
        bmi.bmiHeader.biHeight = -(h as i32); // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB;

        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let hbm_color = CreateDIBSection(
            std::ptr::null_mut(),
            &bmi,
            DIB_RGB_COLORS,
            &mut bits,
            std::ptr::null_mut(),
            0,
        );
        if hbm_color.is_null() || bits.is_null() {
            return 0;
        }

        // Copy as premultiplied BGRA (Windows' ARGB cursor format).
        let n = w as usize * h as usize;
        let dst = std::slice::from_raw_parts_mut(bits as *mut u8, n * 4);
        for (i, px) in rgba.chunks_exact(4).take(n).enumerate() {
            let (r, g, b, a) = (px[0] as u32, px[1] as u32, px[2] as u32, px[3] as u32);
            dst[i * 4 + 0] = (b * a / 255) as u8;
            dst[i * 4 + 1] = (g * a / 255) as u8;
            dst[i * 4 + 2] = (r * a / 255) as u8;
            dst[i * 4 + 3] = a as u8;
        }

        // 1bpp mask, all zeros (alpha comes from the color DIB).
        let mask_row_bytes = ((w + 31) / 32) * 4;
        let mask = vec![0u8; (mask_row_bytes as usize) * h as usize];
        let hbm_mask = CreateBitmap(w as i32, h as i32, 1, 1, mask.as_ptr().cast());
        if hbm_mask.is_null() {
            DeleteObject(hbm_color);
            return 0;
        }

        let info = ICONINFO {
            fIcon: 0, // cursor, not icon
            xHotspot: hot_x as u32,
            yHotspot: hot_y as u32,
            hbmMask: hbm_mask,
            hbmColor: hbm_color,
        };
        let hcursor = CreateIconIndirect(&info);
        DeleteObject(hbm_color);
        DeleteObject(hbm_mask);
        hcursor as usize
    }
}

/// Set the OS cursor to our custom handle (`Some(h)`) or the default arrow
/// (`None`). Called while the overlay owns the hit-testing so the OS draws
/// our circle for this window (re-asserted every frame/tick).
pub fn set_cursor_handle(hcursor: Option<usize>) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{LoadCursorW, SetCursor, IDC_ARROW};
    unsafe {
        let h = match hcursor {
            Some(h) if h != 0 => h as *mut core::ffi::c_void,
            _ => LoadCursorW(std::ptr::null_mut(), IDC_ARROW) as *mut core::ffi::c_void,
        };
        SetCursor(h);
    }
}

/// Destroy a cursor created with [`create_hcursor_from_rgba`].
pub fn destroy_cursor(hcursor: usize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::DestroyCursor;
    if hcursor != 0 {
        unsafe {
            DestroyCursor(hcursor as *mut core::ffi::c_void);
        }
    }
}
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
/// `WS_EX_TRANSPARENT` style. `WS_EX_LAYERED` is kept for transparency.
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

/// Make the overlay window a bulletproof transparent tool window by forcing
/// the styles directly (tao's `with_transparent` uses `DwmEnableBlurBehindWindow`
/// which conflicts with our `UpdateLayeredWindow`, so we do it all here):
/// * GWL_STYLE  -> `WS_POPUP`, remove caption / thick frame / sys menu / min-max
/// * GWL_EXSTYLE-> `WS_EX_LAYERED` (per-pixel alpha) + `WS_EX_TOOLWINDOW`
///                 (no taskbar / Alt-Tab), remove `WS_EX_APPWINDOW`
pub fn polish_overlay_window(hwnd: usize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, GWL_EXSTYLE, GWL_STYLE, HWND_TOP,
        SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, WS_CAPTION, WS_EX_APPWINDOW,
        WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_POPUP, WS_SYSMENU,
        WS_THICKFRAME,
    };
    unsafe {
        let h = hwnd as *mut core::ffi::c_void;
        let style = GetWindowLongPtrW(h, GWL_STYLE);
        let style = (style | WS_POPUP as isize)
            & !(WS_CAPTION | WS_THICKFRAME | WS_MINIMIZEBOX | WS_MAXIMIZEBOX | WS_SYSMENU)
                as isize;
        SetWindowLongPtrW(h, GWL_STYLE, style);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        let ex = (ex | (WS_EX_LAYERED | WS_EX_TOOLWINDOW) as isize) & !(WS_EX_APPWINDOW as isize);
        SetWindowLongPtrW(h, GWL_EXSTYLE, ex);
        // Apply the frame change so the OS drops the title bar / border.
        SetWindowPos(
            h,
            HWND_TOP,
            0,
            0,
            0,
            0,
            SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        );
    }
}

/// Log the window's style / extended style (diagnostic: confirms
/// transparency + pass-through are actually applied).
pub fn log_window_styles(hwnd: usize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, GWL_EXSTYLE, GWL_STYLE, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
        WS_EX_TRANSPARENT, WS_POPUP, WS_THICKFRAME, WS_VISIBLE,
    };
    unsafe {
        let h = hwnd as *mut core::ffi::c_void;
        let s = GetWindowLongPtrW(h, GWL_STYLE);
        let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
        log::info!(
            "overlay window: style=0x{:x} exstyle=0x{:x} | popup={} thickframe={} visible={} layered={} transparent={} toolwindow={}",
            s,
            ex,
            (s & WS_POPUP as isize) != 0,
            (s & WS_THICKFRAME as isize) != 0,
            (s & WS_VISIBLE as isize) != 0,
            (ex & WS_EX_LAYERED as isize) != 0,
            (ex & WS_EX_TRANSPARENT as isize) != 0,
            (ex & WS_EX_TOOLWINDOW as isize) != 0,
        );
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

/// Enable/disable forwarding of pointer input to the window below our
/// overlay. `our_hwnd` is the overlay window (excluded from the target
/// search).
pub fn set_forwarding(enabled: bool, our_hwnd: usize) {
    FORWARD_ON.store(enabled, Ordering::Relaxed);
    FORWARD_HWND.store(our_hwnd, Ordering::Relaxed);
}

/// Topmost visible top-level window (excluding `exclude`) whose rect contains
/// `(x, y)`, walking the Z-order downward from the top — then digs to the
/// **deepest child** at that point (`RealChildWindowFromPoint`) so forwarded
/// mouse messages reach canvases / panes and hover effects work in them.
fn window_below_at(x: i32, y: i32, exclude: usize) -> Option<HWND> {
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetTopWindow, GetWindow, GetWindowRect, IsIconic, IsWindowVisible,
        RealChildWindowFromPoint, GW_HWNDNEXT,
    };
    unsafe {
        let mut top: HWND = std::ptr::null_mut();
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
                    top = h;
                    break;
                }
            }
            h = GetWindow(h, GW_HWNDNEXT);
        }
        if top.is_null() {
            return None;
        }
        let mut pt = windows_sys::Win32::Foundation::POINT { x, y };
        ScreenToClient(top, &mut pt);
        let child = RealChildWindowFromPoint(top, pt);
        if !child.is_null() {
            Some(child)
        } else {
            Some(top)
        }
    }
}

/// Replay one pointer message to the window below our overlay.
/// `wparam` is the message's wParam (mouse-key/modifier flags, wheel delta,
/// X-button id). Wheel messages (`WM_MOUSEWHEEL`/`WM_MOUSEHWHEEL`) carry
/// **screen** coordinates in lParam; all other mouse messages carry **client**
/// coordinates, so we convert with `ScreenToClient`.
pub fn forward_mouse(pt_x: i32, pt_y: i32, msg: u32, wparam: usize) {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        PostMessageW, SetForegroundWindow, WM_LBUTTONDOWN, WM_MBUTTONDOWN, WM_MOUSEHWHEEL,
        WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_XBUTTONDOWN,
    };
    if !FORWARD_ON.load(Ordering::Relaxed) {
        return;
    }
    let exclude = FORWARD_HWND.load(Ordering::Relaxed);
    unsafe {
        let Some(hwnd) = window_below_at(pt_x, pt_y, exclude) else {
            return;
        };
        // Wheel messages use screen coordinates; everything else uses client.
        let (lx, ly) = if matches!(msg, WM_MOUSEWHEEL | WM_MOUSEHWHEEL) {
            (pt_x, pt_y)
        } else {
            let mut pt = POINT { x: pt_x, y: pt_y };
            ScreenToClient(hwnd, &mut pt);
            (pt.x, pt.y)
        };
        let lparam =
            (((ly as u32) & 0xFFFF) as usize) << 16 | ((lx as u32) & 0xFFFF) as usize;
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
