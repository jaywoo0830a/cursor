//! Windows implementation of the platform layer: raw Win32 API via
//! `windows-sys`.
//!
//! Provides:
//! * global cursor position / visibility (`GetCursorPos`, `ShowCursor`)
//! * a real `HCURSOR` built from RGBA pixels (`CreateDIBSection` +
//!   `CreateIconIndirect`) and direct `SetCursor` forcing
//! * swapping the *system* cursor bitmaps (`SetSystemCursor`) so the custom
//!   circle shows over any app **and** click pass-through can stay on
//! * click pass-through via `WS_EX_TRANSPARENT` (`SetWindowLongPtrW`)
//! * target-window tracking (`EnumWindows`, `GetWindowRect`)

use std::sync::{Mutex, OnceLock};

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

/// Hide (`false`) / show (`true`) the system cursor. `ShowCursor` uses a
/// global display counter, so we force it into the desired state (other
/// windows may re-increment the counter).
pub fn set_system_cursor_visible(visible: bool) {
    use windows_sys::Win32::UI::WindowsAndMessaging::ShowCursor;
    unsafe {
        if visible {
            // Force the counter back up so the cursor is visible again.
            while ShowCursor(1) < 0 {}
        } else {
            // Force the counter down so the cursor is hidden.
            while ShowCursor(0) >= 0 {}
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

/// Set the system cursor to our custom handle (`Some`) or the default
/// arrow (`None`). Re-asserting this every frame overrides any cursor an
/// app (e.g. a PDF viewer's canvas) sets in between.
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

// ---------------------------------------------------------------------------
// System-cursor swapping — bitmap cursor + click pass-through together
// ---------------------------------------------------------------------------

/// The system-cursor IDs we replace while the pointer is inside the region,
/// so apps that switch cursors (I-beam, hand, size arrows, …) still show our
/// bitmap instead of their own.
fn ocr_ids() -> &'static [u32] {
    use windows_sys::Win32::UI::WindowsAndMessaging as w;
    &[
        w::OCR_NORMAL,
        w::OCR_IBEAM,
        w::OCR_WAIT,
        w::OCR_CROSS,
        w::OCR_UP,
        w::OCR_SIZEALL,
        w::OCR_SIZENWSE,
        w::OCR_SIZENESW,
        w::OCR_SIZEWE,
        w::OCR_SIZENS,
        w::OCR_HAND,
        w::OCR_APPSTARTING,
        w::OCR_NO,
    ]
}

struct SystemCursorState {
    /// (system cursor id, original cursor copy) — one per successfully saved id.
    saved: Vec<(u32, usize)>,
    /// Our custom cursor (owned by the app; we only ever pass copies of it).
    custom: usize,
    /// Whether the system cursors currently show our bitmap.
    active: bool,
}

static SYS_CURSOR: OnceLock<Mutex<Option<SystemCursorState>>> = OnceLock::new();

fn sys_cursor() -> &'static Mutex<Option<SystemCursorState>> {
    SYS_CURSOR.get_or_init(|| Mutex::new(None))
}

/// Save copies of every system cursor we may want to replace and remember our
/// custom cursor. Returns whether the swap is available.
pub fn init_system_cursor_swap(custom: usize) -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CopyIcon, LoadCursorW};
    if custom == 0 {
        return false;
    }
    let mut guard = sys_cursor().lock().unwrap();
    if guard.is_some() {
        return true;
    }
    let mut saved = Vec::new();
    unsafe {
        for &id in ocr_ids() {
            // OCR_* are numeric resource ids; pass them via MAKEINTRESOURCE.
            let orig = LoadCursorW(std::ptr::null_mut(), id as usize as *const u16);
            if orig.is_null() {
                continue;
            }
            let copy = CopyIcon(orig);
            if !copy.is_null() {
                saved.push((id, copy as usize));
            }
        }
    }
    if saved.is_empty() {
        return false;
    }
    *guard = Some(SystemCursorState {
        saved,
        custom,
        active: false,
    });
    true
}

/// Show our cursor bitmap (`active`) or restore the originals (`!active`) for
/// all system-cursor IDs. Only swaps on state transitions, and only when the
/// swap was initialized successfully.
pub fn set_system_cursor_active(active: bool, custom: usize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::{CopyIcon, SetSystemCursor};
    let mut guard = sys_cursor().lock().unwrap();
    if guard.is_none() {
        drop(guard);
        if !init_system_cursor_swap(custom) {
            return;
        }
        guard = sys_cursor().lock().unwrap();
    }
    let Some(state) = guard.as_mut() else {
        return;
    };
    if state.active == active {
        return;
    }
    unsafe {
        for (id, saved) in &state.saved {
            let src = if active { state.custom } else { *saved };
            if src == 0 {
                continue;
            }
            // SetSystemCursor destroys the cursor you pass, so hand it a copy.
            let copy = CopyIcon(src as *mut core::ffi::c_void);
            if !copy.is_null() {
                SetSystemCursor(copy, *id);
            }
        }
    }
    state.active = active;
}

/// Restore all system cursors (from the registry) and free our saved copies.
/// Called on shutdown.
pub fn restore_system_cursor_swap() {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CopyIcon, DestroyIcon, SetSystemCursor, SystemParametersInfoW, SPI_SETCURSORS,
        SPIF_SENDCHANGE,
    };
    let mut guard = sys_cursor().lock().unwrap();
    if let Some(state) = guard.take() {
        if state.active {
            unsafe {
                for (id, saved) in &state.saved {
                    let copy = CopyIcon(*saved as *mut core::ffi::c_void);
                    if !copy.is_null() {
                        SetSystemCursor(copy, *id);
                    }
                }
            }
        }
        unsafe {
            // Reload every system cursor from the registry.
            SystemParametersInfoW(SPI_SETCURSORS, 0, std::ptr::null_mut(), SPIF_SENDCHANGE);
            for (_, h) in &state.saved {
                if *h != 0 {
                    DestroyIcon(*h as *mut core::ffi::c_void);
                }
            }
        }
    }
}
