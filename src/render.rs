//! Native overlay rendering (Windows): draws the region box + a small status
//! badge into a layered window with GDI via `UpdateLayeredWindow`
//! (per-pixel alpha). No webview, no JS — everything is Rust + Win32.
//!
//! The surface is a 32bpp top-down DIB backed by a memory DC. Every frame we
//! clear it to transparent, draw the status badge + region box (+ resize
//! handles while editing) with plain pixel writes, rasterize the status text
//! with GDI, premultiply alpha, and present with `UpdateLayeredWindow`.

use crate::app::RectF;

pub struct OverlaySurface {
    #[cfg(target_os = "windows")]
    inner: Option<Inner>,
}

#[cfg(target_os = "windows")]
struct Inner {
    hwnd: usize,
    w: i32,
    h: i32,
    hdc_mem: windows_sys::Win32::Graphics::Gdi::HDC,
    hbmp: windows_sys::Win32::Graphics::Gdi::HBITMAP,
    bits: *mut u8,
}

// GDI constants (numeric to avoid guessing feature paths). `DIB_RGB_COLORS` /
// `BI_RGB` are provided by the `Gdi::*` glob import, so we don't redeclare
// them here.
#[cfg(target_os = "windows")]
const BK_TRANSPARENT: i32 = 1;
#[cfg(target_os = "windows")]
const FONT_GUI: i32 = 17;
#[cfg(target_os = "windows")]
const BLEND_OVER: u8 = 0;
#[cfg(target_os = "windows")]
const BLEND_SRC_ALPHA: u8 = 1;
#[cfg(target_os = "windows")]
const ULW_ALPHA_FLAG: u32 = 2;
#[cfg(target_os = "windows")]
const WHITE: u32 = 0x00FF_FFFF;

impl OverlaySurface {
    /// Create a surface for the overlay window. Always returns a valid (maybe
    /// empty on failure / non-Windows) surface.
    pub fn create(hwnd: usize, w: i32, h: i32) -> Self {
        #[cfg(target_os = "windows")]
        {
            Self {
                inner: Inner::new(hwnd, w, h),
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (hwnd, w, h);
            Self {}
        }
    }

    /// Recreate the surface for a new window size.
    pub fn resize(&mut self, w: i32, h: i32) {
        #[cfg(target_os = "windows")]
        {
            if let Some(inner) = &mut self.inner {
                if inner.w == w && inner.h == h {
                    return;
                }
                let hwnd = inner.hwnd;
                *self = Self::create(hwnd, w, h);
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (w, h);
        }
    }

    /// Redraw and present. `region` is in logical px (window-local).
    pub fn draw(&mut self, region: RectF, editing: bool, show_region: bool, status: &str, scale: f32) {
        #[cfg(target_os = "windows")]
        {
            if let Some(inner) = &mut self.inner {
                inner.draw(region, editing, show_region, status, scale);
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (region, editing, show_region, status, scale);
        }
    }
}

#[cfg(target_os = "windows")]
impl Inner {
    fn new(hwnd: usize, w: i32, h: i32) -> Option<Self> {
        use windows_sys::Win32::Graphics::Gdi::*;
        unsafe {
            let hdc = CreateCompatibleDC(std::ptr::null_mut());
            if hdc.is_null() {
                return None;
            }
            let (w, h) = (w.max(1), h.max(1));
            let mut bmi: BITMAPINFO = std::mem::zeroed();
            bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bmi.bmiHeader.biWidth = w;
            bmi.bmiHeader.biHeight = -h; // top-down
            bmi.bmiHeader.biPlanes = 1;
            bmi.bmiHeader.biBitCount = 32;
            bmi.bmiHeader.biCompression = BI_RGB;
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let hbmp = CreateDIBSection(
                std::ptr::null_mut(),
                &bmi,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            if hbmp.is_null() || bits.is_null() {
                DeleteDC(hdc);
                return None;
            }
            SelectObject(hdc, hbmp as HGDIOBJ);
            log::info!("overlay surface created: {w}x{h}");
            Some(Self {
                hwnd,
                w,
                h,
                hdc_mem: hdc,
                hbmp,
                bits: bits as *mut u8,
            })
        }
    }

    fn draw(&mut self, region: RectF, editing: bool, show_region: bool, status: &str, scale: f32) {
        use windows_sys::Win32::Foundation::{POINT, RECT, SIZE};
        use windows_sys::Win32::Graphics::Gdi::*;
        use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowRect, UpdateLayeredWindow};
        unsafe {
            let w = self.w;
            let h = self.h;
            let buf = std::slice::from_raw_parts_mut(self.bits, (w as usize) * (h as usize) * 4);
            buf.fill(0);

            let s = scale.max(0.1) as f32;

            // ---- region box (physical px) ----
            if show_region || editing {
                let bw = (2.0 * s).round().max(1.0) as i32;
                let rx = (region.x * s) as i32;
                let ry = (region.y * s) as i32;
                let rw = (region.w * s) as i32;
                let rh = (region.h * s) as i32;
                // border
                fill_rect(buf, w, h, rx, ry, rw, bw, 0, 150, 255, 235);
                fill_rect(buf, w, h, rx, ry + rh - bw, rw, bw, 0, 150, 255, 235);
                fill_rect(buf, w, h, rx, ry, bw, rh, 0, 150, 255, 235);
                fill_rect(buf, w, h, rx + rw - bw, ry, bw, rh, 0, 150, 255, 235);
                // resize handles
                if editing {
                    let hs = (12.0 * s).round().max(6.0) as i32;
                    let hx = |cx: i32| cx - hs / 2;
                    let hy = |cy: i32| cy - hs / 2;
                    let pts = [
                        (rx, ry),
                        (rx + rw / 2, ry),
                        (rx + rw, ry),
                        (rx + rw, ry + rh / 2),
                        (rx + rw, ry + rh),
                        (rx + rw / 2, ry + rh),
                        (rx, ry + rh),
                        (rx, ry + rh / 2),
                    ];
                    for (cx, cy) in pts {
                        fill_rect(buf, w, h, hx(cx), hy(cy), hs, hs, 255, 255, 255, 255);
                    }
                }
            }

            // ---- status badge + GDI text ----
            let hfont = GetStockObject(FONT_GUI);
            let old_font = SelectObject(self.hdc_mem, hfont);
            SetBkMode(self.hdc_mem, BK_TRANSPARENT);
            SetTextColor(self.hdc_mem, WHITE);
            let text: Vec<u16> = status.encode_utf16().collect();
            let mut ext = SIZE { cx: 0, cy: 0 };
            GetTextExtentPoint32W(self.hdc_mem, text.as_ptr(), text.len() as i32, &mut ext);
            let pad = (8.0 * s).round().max(4.0) as i32;
            let tx = pad;
            let ty = pad;
            let bw = ext.cx + pad * 2;
            let bh = ext.cy + pad;
            fill_rect(buf, w, h, tx, ty, bw, bh, 20, 20, 30, 165);
            TextOutW(self.hdc_mem, tx + pad, ty + pad / 2, text.as_ptr(), text.len() as i32);
            SelectObject(self.hdc_mem, old_font);

            // ---- premultiply alpha (non-premultiplied -> premultiplied BGRA) ----
            for px in buf.chunks_exact_mut(4) {
                let a = px[3] as u32;
                if a == 0 {
                    px[0] = 0;
                    px[1] = 0;
                    px[2] = 0;
                    continue;
                }
                px[0] = (px[0] as u32 * a / 255) as u8;
                px[1] = (px[1] as u32 * a / 255) as u8;
                px[2] = (px[2] as u32 * a / 255) as u8;
            }

            // ---- present ----
            let hdc_dst = GetDC(std::ptr::null_mut());
            let mut rect = std::mem::zeroed::<RECT>();
            GetWindowRect(self.hwnd as *mut core::ffi::c_void, &mut rect);
            let pos = POINT {
                x: rect.left,
                y: rect.top,
            };
            let size = SIZE { cx: w, cy: h };
            let src = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: BLEND_OVER,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: BLEND_SRC_ALPHA,
            };
            UpdateLayeredWindow(
                self.hwnd as *mut core::ffi::c_void,
                hdc_dst,
                &pos,
                &size,
                self.hdc_mem,
                &src,
                0,
                &blend,
                ULW_ALPHA_FLAG,
            );
            ReleaseDC(std::ptr::null_mut(), hdc_dst);
        }
    }
}

impl Drop for OverlaySurface {
    fn drop(&mut self) {
        #[cfg(target_os = "windows")]
        if let Some(inner) = &self.inner {
            unsafe {
                windows_sys::Win32::Graphics::Gdi::DeleteObject(inner.hbmp as _);
                windows_sys::Win32::Graphics::Gdi::DeleteDC(inner.hdc_mem);
            }
        }
    }
}

/// Fill a rectangle with a non-premultiplied RGBA color (clamped to bounds).
#[cfg(target_os = "windows")]
fn fill_rect(
    buf: &mut [u8],
    w: i32,
    h: i32,
    x: i32,
    y: i32,
    ww: i32,
    hh: i32,
    r: u8,
    g: u8,
    b: u8,
    a: u8,
) {
    let x0 = x.max(0);
    let y0 = y.max(0);
    let x1 = (x + ww).min(w);
    let y1 = (y + hh).min(h);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    for py in y0..y1 {
        let row = (py as usize) * (w as usize);
        for px in x0..x1 {
            let i = (row + px as usize) * 4;
            buf[i] = b;
            buf[i + 1] = g;
            buf[i + 2] = r;
            buf[i + 3] = a;
        }
    }
}
