//! # Custom Cursor Overlay (egui 0.36 / eframe 0.36)
//!
//! A frameless, transparent, always-on-top, fullscreen overlay window.
//!
//! Behavior:
//! * Inside a **user-defined region** (a rectangle on the screen) the system
//!   cursor is hidden and replaced by a custom bitmap cursor that follows the
//!   mouse — *only* the custom cursor appears there.
//! * Outside that region the normal system cursor is shown again.
//!
//! Controls:
//! * `F1`  – toggle the settings panel
//! * `Esc` – quit
//! * While *Edit region* is on, drag the blue box to move it and the white
//!   handles to resize it.
//!
//! On Windows, *Click pass-through* (default on) lets clicks go through the
//! overlay to the apps below; the cursor position is then polled via
//! `GetCursorPos` and the system cursor is hidden/shown with `ShowCursor`.
//! While the settings panel or region editing is open, pass-through is
//! automatically disabled so the panel stays interactive.
//!
//! The custom cursor image is either `assets/cursor.png` (straight RGBA PNG)
//! or, if that file is missing, a classic arrow cursor generated in code.
//! You can also switch to an OS-level bitmap cursor (native `CustomCursor`,
//! applied to the whole window) via the settings panel.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::Path;
use std::sync::Arc;

use eframe::egui::{
    self, pos2, vec2, Color32, ColorImage, CornerRadius, CursorIcon, FontId, Id, Rect, Stroke,
    StrokeKind, TextureHandle, TextureOptions,
};
use eframe::egui::viewport::ViewportBuilder;

// ---------------------------------------------------------------------------
// Configurationvyw
// ---------------------------------------------------------------------------

/// Default overlay region (in window points, origin = top-left).
/// The window is fullscreen, so this is in screen coordinates on the
/// primary monitor. Edit it at runtime with the settings panel.
fn default_region() -> Rect {
    Rect::from_min_size(pos2(300.0, 200.0), vec2(1320.0, 680.0))
}

/// Logical display size (in points) of the *painted* custom cursor.
const CURSOR_DISPLAY_SIZE: f32 = 20.0;

/// Optional PNG used as the cursor bitmap (straight / non-premultiplied RGBA).
/// If missing, a default arrow cursor is generated at runtime.
const CURSOR_PNG: &str = "assets/cursor.png";

/// Hotspot (in bitmap pixels) used when loading `CURSOR_PNG`.
const PNG_HOTSPOT: [u16; 2] = [0, 0];

/// Size of the region resize handles.
const HANDLE_SIZE: f32 = 12.0;

/// Minimum region size (points).
const MIN_REGION: f32 = 24.0;

// ---------------------------------------------------------------------------
// Platform helpers
// ---------------------------------------------------------------------------

/// Global mouse position and system cursor visibility.
///
/// With click pass-through enabled the overlay window no longer receives
/// pointer events, and winit's per-window cursor API is ignored (the OS shows
/// the cursor of the window below the pointer). So on Windows we poll the
/// global cursor position with `GetCursorPos` and hide/show the cursor with
/// `ShowCursor`, which affects the whole desktop session.
#[cfg(target_os = "windows")]
mod platform {
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

    // ---- target window tracking (overlay a specific process's window) ----

    /// A Windows window handle (`HWND`).
    type HWND = *mut core::ffi::c_void;

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
            use windows_sys::Win32::UI::WindowsAndMessaging::{GetWindowTextW, IsIconic, IsWindowVisible};
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
}

#[cfg(not(target_os = "windows"))]
#[allow(dead_code)]
mod platform {
    pub fn global_cursor_pos() -> Option<(f64, f64)> {
        None
    }

    pub fn set_system_cursor_visible(_visible: bool) {}

    pub fn find_window_by_title(_sub: &str) -> Option<*mut core::ffi::c_void> {
        None
    }

    pub fn window_outer_rect(_hwnd: *mut core::ffi::c_void) -> Option<(i32, i32, i32, i32)> {
        None
    }

    pub fn is_iconic(_hwnd: *mut core::ffi::c_void) -> bool {
        false
    }

    pub fn apply_passthrough(_hwnd: usize, _passthrough: bool) {}
}

// ---------------------------------------------------------------------------
// Cursor bitmap
// ---------------------------------------------------------------------------

struct CursorBitmap {
    /// Straight (non-premultiplied) RGBA pixels, `size[0] * size[1] * 4` bytes.
    rgba: Arc<[u8]>,
    size: [u16; 2],
    hotspot: [u16; 2],
}

/// Try to load `assets/cursor.png`; fall back to a generated circular cursor.
fn load_cursor_bitmap() -> CursorBitmap {
    let path = Path::new(CURSOR_PNG);
    if let Ok(img) = image::open(path) {
        let img = img.to_rgba8();
        let (w, h) = (img.width() as u16, img.height() as u16);
        if w > 0 && h > 0 && w <= 512 && h <= 512 {
            log::info!("Using custom cursor bitmap from {path:?} ({}x{})", w, h);
            return CursorBitmap {
                rgba: Arc::from(img.into_raw()),
                size: [w, h],
                hotspot: PNG_HOTSPOT,
            };
        }
        log::warn!("Could not load {path:?}, falling back to the generated cursor");
    }
    make_default_cursor()
}

/// Rasterize a modern, small precision-reticle cursor (32x32, anti-aliased):
/// a thin white ring with dark outlines and a small dark center dot, hotspot
/// at the center. Reads well on both light and dark backgrounds.
fn make_default_cursor() -> CursorBitmap {
    const S: usize = 32;
    const CX: f32 = (S as f32 - 1.0) * 0.5; // 15.5
    const CY: f32 = (S as f32 - 1.0) * 0.5;
    const AA: f32 = 1.0;

    // Radii (in px) of the reticle design:
    const R_DOT: f32 = 2.2; // dark center dot (aiming point)
    const R_RING_IN: f32 = 8.0; // white ring (inner)
    const R_RING_OUT: f32 = 9.8; // white ring (outer)
    const R_OUT_OUT: f32 = 10.8; // dark outline (outer side of the ring)

    // Coverage of the annulus [a, b] with 1px anti-aliased edges.
    let band = |d: f32, a: f32, b: f32| -> f32 {
        let inner = ((d - a) / AA + 0.5).clamp(0.0, 1.0);
        let outer = ((b - d) / AA + 0.5).clamp(0.0, 1.0);
        inner.min(outer)
    };

    let mut rgba = vec![0u8; S * S * 4];
    for y in 0..S {
        for x in 0..S {
            let d = ((x as f32 - CX).powi(2) + (y as f32 - CY).powi(2)).sqrt();
            // Center dot is a filled circle (full alpha in the middle).
            let dot = ((R_DOT - d) / AA + 0.5).clamp(0.0, 1.0);
            let ring = band(d, R_RING_IN, R_RING_OUT);
            let out = band(d, R_RING_OUT, R_OUT_OUT);

            let dark = dot.max(out);
            let white = ring;
            let a = dark.max(white);
            if a > 0.0 {
                let v = (20.0 * dark + 255.0 * white) / a; // dark dot/outline, white ring
                let i = (y * S + x) * 4;
                rgba[i..i + 4].copy_from_slice(&[
                    v.round() as u8,
                    v.round() as u8,
                    v.round() as u8,
                    (a * 255.0).round() as u8,
                ]);
            }
        }
    }

    CursorBitmap {
        rgba: Arc::from(rgba),
        size: [S as u16, S as u16],
        hotspot: [S as u16 / 2, S as u16 / 2],
    }
}

// ---------------------------------------------------------------------------
// Region editing handles
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Handle {
    Move,
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

// ---------------------------------------------------------------------------
// Application
// ---------------------------------------------------------------------------

struct CursorOverlayApp {
    ctx: egui::Context,
    bitmap: CursorBitmap,
    texture: TextureHandle,
    os_cursor: egui::CustomCursorImage,

    /// The overlay region, in window (screen) points.
    region: Rect,

    /// Whether the settings panel is open.
    show_settings: bool,
    /// Whether the region can be edited (drag / resize). While editing the
    /// system cursor is shown normally.
    editing: bool,
    /// Master switch for the custom-cursor behavior.
    enabled: bool,
    /// Use the native OS-level bitmap cursor (whole window) instead of the
    /// region-limited painted cursor. Only used without click pass-through.
    use_os_cursor: bool,
    /// Draw the faint blue region outline while the overlay is active.
    show_region_visual: bool,
    /// Click pass-through (Windows only): clicks go to the apps below and the
    /// cursor position is polled via `GetCursorPos`.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    passthrough: bool,
    /// Whether we currently hid the OS cursor via `ShowCursor(FALSE)`.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    cursor_hidden: bool,
    /// Last pass-through value sent to the viewport (to avoid re-sending).
    last_passthrough: Option<bool>,

    /// Target window title substring to overlay (Windows-only effect).
    /// `None` = cover the whole screen.
    target_window: Option<String>,
    /// Text field in the settings panel for the target title.
    target_window_input: String,
    /// Cached HWND of the target window (0 = not found yet).
    target_hwnd: usize,
    /// Last known target rect in physical px (to avoid re-sending commands).
    last_target_rect: Option<(i32, i32, i32, i32)>,
    /// Whether we are currently following a target window (fullscreen off).
    window_follow_active: bool,
    /// Whether the region has been initialized to cover the target window.
    region_initialized_for_target: bool,
    /// Whether the region was initialized to the full screen on the first frame.
    region_initialized: bool,
    /// Our own native window handle (HWND), used to force pass-through.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    native_hwnd: usize,

    drag: Option<Handle>,
    drag_start_pointer: egui::Pos2,
    drag_start_region: Rect,
}

impl CursorOverlayApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();

        let bitmap = load_cursor_bitmap();

        let texture = ctx.load_texture(
            "custom_cursor",
            ColorImage::from_rgba_unmultiplied(
                [bitmap.size[0] as usize, bitmap.size[1] as usize],
                &bitmap.rgba,
            ),
            TextureOptions::NEAREST,
        );

        let os_cursor = egui::CustomCursorImage {
            rgba: bitmap.rgba.clone(),
            size: bitmap.size,
            hotspot: bitmap.hotspot,
        };

        // Optional CLI: --window "<title substring>" overlays a specific
        // window instead of the whole screen.
        let target_window = std::env::args()
            .collect::<Vec<_>>()
            .windows(2)
            .find(|w| w[0] == "--window")
            .map(|w| w[1].clone());
        let target_window_input = target_window.clone().unwrap_or_default();

        // Grab our own native window handle (Windows) so we can force click
        // pass-through directly with SetWindowLong as a fallback.
        #[cfg(target_os = "windows")]
        let native_hwnd = {
            use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
            cc.winit_window()
                .and_then(|w| w.window_handle().ok())
                .and_then(|h| match h.as_raw() {
                    RawWindowHandle::Win32(wh) => Some(wh.hwnd.get() as usize),
                    _ => None,
                })
                .unwrap_or(0)
        };
        #[cfg(not(target_os = "windows"))]
        let native_hwnd = 0;

        Self {
            ctx,
            bitmap,
            texture,
            os_cursor,
            region: default_region(),
            // Start with the overlay active: no settings panel and no region
            // outline — just the transparent background and the cursor.
            show_settings: false,
            editing: false,
            enabled: true,
            // Painted cursor by default so click pass-through works. Toggle
            // "Use OS bitmap cursor image" for the ultra-reliable OS cursor
            // (which requires pass-through to be off).
            use_os_cursor: false,
            show_region_visual: false,
            // Click pass-through is only implemented on Windows (global
            // cursor polling via GetCursorPos).
            #[cfg(target_os = "windows")]
            passthrough: true,
            #[cfg(not(target_os = "windows"))]
            passthrough: false,
            cursor_hidden: false,
            last_passthrough: None,
            target_window,
            target_window_input,
            target_hwnd: 0,
            last_target_rect: None,
            window_follow_active: false,
            region_initialized_for_target: false,
            region_initialized: false,
            native_hwnd,
            drag: None,
            drag_start_pointer: egui::Pos2::ZERO,
            drag_start_region: default_region(),
        }
    }

    // ---- helpers --------------------------------------------------------

    /// Rectangles of the interactive handles around the region.
    fn handle_rects(&self) -> Vec<(Handle, Rect)> {
        let r = self.region;
        let h = HANDLE_SIZE;
        let c = |p: egui::Pos2| Rect::from_center_size(p, vec2(h, h));
        let (min, max) = (r.min, r.max);
        let mx = (min.x + max.x) * 0.5;
        let my = (min.y + max.y) * 0.5;

        vec![
            (
                Handle::Move,
                Rect::from_center_size(r.center(), vec2(r.width() - h, h)),
            ),
            (Handle::NW, c(min)),
            (Handle::N, c(pos2(mx, min.y))),
            (Handle::NE, c(pos2(max.x, min.y))),
            (Handle::W, c(pos2(min.x, my))),
            (Handle::E, c(pos2(max.x, my))),
            (Handle::SW, c(pos2(min.x, max.y))),
            (Handle::S, c(pos2(mx, max.y))),
            (Handle::SE, c(max)),
        ]
    }

    fn handle_region_drag(&mut self, ctx: &egui::Context, pointer: Option<egui::Pos2>) {
        let pressed = ctx.input(|i| i.pointer.primary_pressed());
        let released = ctx.input(|i| i.pointer.primary_released());

        if let Some(p) = pointer {
            if pressed && self.drag.is_none() {
                if let Some((handle, _)) = self
                    .handle_rects()
                    .iter()
                    .find(|(_, r)| r.contains(p))
                {
                    self.drag = Some(*handle);
                    self.drag_start_pointer = p;
                    self.drag_start_region = self.region;
                }
            }
        }

        if released {
            self.drag = None;
        }

        if let Some(handle) = self.drag {
            if let Some(p) = pointer {
                let d = p - self.drag_start_pointer;
                let mut r = self.drag_start_region;
                match handle {
                    Handle::Move => {
                        r = r.translate(d);
                    }
                    Handle::N => r.min.y = (r.min.y + d.y).min(r.max.y - MIN_REGION),
                    Handle::S => r.max.y = (r.max.y + d.y).max(r.min.y + MIN_REGION),
                    Handle::W => r.min.x = (r.min.x + d.x).min(r.max.x - MIN_REGION),
                    Handle::E => r.max.x = (r.max.x + d.x).max(r.min.x + MIN_REGION),
                    Handle::NW => {
                        r.min.x = (r.min.x + d.x).min(r.max.x - MIN_REGION);
                        r.min.y = (r.min.y + d.y).min(r.max.y - MIN_REGION);
                    }
                    Handle::NE => {
                        r.max.x = (r.max.x + d.x).max(r.min.x + MIN_REGION);
                        r.min.y = (r.min.y + d.y).min(r.max.y - MIN_REGION);
                    }
                    Handle::SW => {
                        r.min.x = (r.min.x + d.x).min(r.max.x - MIN_REGION);
                        r.max.y = (r.max.y + d.y).max(r.min.y + MIN_REGION);
                    }
                    Handle::SE => {
                        r.max.x = (r.max.x + d.x).max(r.min.x + MIN_REGION);
                        r.max.y = (r.max.y + d.y).max(r.min.y + MIN_REGION);
                    }
                }
                self.region = r;
            }
        }
    }

    /// Current pointer position in window points.
    ///
    /// In click-through mode winit stops delivering pointer events, so we
    /// poll the global cursor position instead (Windows only).
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    fn pointer_pos(&self, ctx: &egui::Context, passthrough: bool) -> Option<egui::Pos2> {
        #[cfg(target_os = "windows")]
        {
            if passthrough {
                if let Some((x, y)) = platform::global_cursor_pos() {
                    let ppp = ctx.pixels_per_point();
                    let origin = ctx
                        .input(|i| i.viewport().outer_rect.map(|r| r.min))
                        .unwrap_or(egui::Pos2::ZERO);
                    return Some(egui::pos2(x as f32 / ppp, y as f32 / ppp) - origin.to_vec2());
                }
            }
        }
        ctx.pointer_interact_pos()
    }

    /// Hide/show the OS cursor. With click-through enabled we must use the
    /// global `ShowCursor` API (winit's per-window cursor is ignored for
    /// transparent/pass-through windows). The calls are paired to keep
    /// Windows' display counter balanced.
    fn set_system_cursor_visible(&mut self, passthrough: bool, visible: bool) {
        #[cfg(target_os = "windows")]
        {
            if passthrough {
                if !visible && !self.cursor_hidden {
                    platform::set_system_cursor_visible(false);
                    self.cursor_hidden = true;
                } else if visible && self.cursor_hidden {
                    platform::set_system_cursor_visible(true);
                    self.cursor_hidden = false;
                }
            } else if self.cursor_hidden {
                // Left click-through mode; make sure the cursor is restored.
                platform::set_system_cursor_visible(true);
                self.cursor_hidden = false;
            }
        }
        #[cfg(not(target_os = "windows"))]
        let _ = (passthrough, visible);
    }

    /// Overlay a specific target window (Windows): resize the overlay to
    /// exactly cover it and follow it when it moves or resizes. This is the
    /// safe, standard way to "attach" the overlay to another process — no
    /// DLL injection needed.
    fn update_target_window(&mut self, ctx: &egui::Context) {
        #[cfg(target_os = "windows")]
        {
            let Some(target) = self.target_window.clone() else {
                // No target: go back to fullscreen if we had left it.
                if self.window_follow_active {
                    self.window_follow_active = false;
                    self.region_initialized_for_target = false;
                    self.target_hwnd = 0;
                    self.last_target_rect = None;
                    self.region = default_region();
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                }
                return;
            };

            // Find (or re-find) the target window by title.
            if self.target_hwnd == 0 {
                self.target_hwnd = platform::find_window_by_title(&target)
                    .map(|h| h as usize)
                    .unwrap_or(0);
            }
            let hwnd = self.target_hwnd;
            if hwnd == 0 {
                return;
            }

            if platform::is_iconic(hwnd as *mut core::ffi::c_void) {
                return; // minimized — leave the overlay where it is
            }

            match platform::window_outer_rect(hwnd as *mut core::ffi::c_void) {
                Some((l, t, r, b)) => {
                    let rect = (l, t, r, b);
                    if !self.window_follow_active {
                        self.window_follow_active = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                    }
                    if self.last_target_rect != Some(rect) {
                        self.last_target_rect = Some(rect);
                        let ppp = ctx.pixels_per_point();
                        let pos = egui::pos2(l as f32 / ppp, t as f32 / ppp);
                        let size = egui::vec2((r - l) as f32 / ppp, (b - t) as f32 / ppp);
                        ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(pos));
                        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                        if !self.region_initialized_for_target {
                            // Default: cover the whole target window.
                            self.region_initialized_for_target = true;
                            self.region = Rect::from_min_size(egui::Pos2::ZERO, size);
                        }
                    }
                }
                None => {
                    // Target closed / invalid — re-find it next frame.
                    self.target_hwnd = 0;
                    self.last_target_rect = None;
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = (
                &self.target_window,
                &self.target_hwnd,
                &self.last_target_rect,
                &self.window_follow_active,
                &self.region_initialized_for_target,
                ctx,
            );
        }
    }

    /// Draw the region rectangle (+ handles while editing).
    fn paint_region(&self, ui: &mut egui::Ui) {
        // In run mode the outline is optional (toggle in the settings).
        if !self.editing && !self.show_region_visual {
            return;
        }
        let painter = ui.painter();
        let r = self.region;

        painter.rect_filled(
            r,
            CornerRadius::ZERO,
            Color32::from_rgba_premultiplied(0, 200, 255, 8),
        );
        painter.rect_stroke(
            r,
            CornerRadius::ZERO,
            Stroke::new(1.5, Color32::from_rgba_premultiplied(0, 200, 255, 110)),
            StrokeKind::Inside,
        );

        if self.editing {
            for (_, hr) in self.handle_rects() {
                painter.rect_filled(
                    hr,
                    CornerRadius::ZERO,
                    Color32::from_rgba_premultiplied(0, 200, 255, 220),
                );
                painter.rect_stroke(
                    hr,
                    CornerRadius::ZERO,
                    Stroke::new(1.0, Color32::WHITE),
                    StrokeKind::Inside,
                );
            }
        }
    }

    /// Paint the custom cursor at `pointer` (hotspot-corrected) on the
    /// topmost debug layer, so nothing can cover it.
    fn paint_custom_cursor(&self, ctx: &egui::Context, pointer: egui::Pos2) {
        let (w, h) = (self.bitmap.size[0] as f32, self.bitmap.size[1] as f32);
        let scale = CURSOR_DISPLAY_SIZE / w;
        let size = vec2(CURSOR_DISPLAY_SIZE, CURSOR_DISPLAY_SIZE * h / w);
        let hotspot = vec2(self.bitmap.hotspot[0] as f32, self.bitmap.hotspot[1] as f32) * scale;
        let rect = Rect::from_min_size(pointer - hotspot, size);

        let painter = ctx.debug_painter(); // topmost layer, always rendered
        painter.image(
            self.texture.id(),
            rect,
            Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        egui::Window::new("Custom Cursor Overlay")
            .id(Id::new("settings"))
            .default_pos(pos2(16.0, 16.0))
            .default_size(vec2(360.0, 0.0))
            .collapsible(false)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label(
                    egui::RichText::new("F1: toggle panel  ·  Esc: quit")
                        .font(FontId::proportional(11.0))
                        .weak(),
                );
                ui.separator();

                ui.checkbox(&mut self.enabled, "Enable custom cursor");
                ui.checkbox(&mut self.editing, "Edit region (drag / resize)");
                ui.checkbox(&mut self.use_os_cursor, "Use OS bitmap cursor image")
                    .on_hover_text(
                        "Registers the cursor bitmap as a real OS cursor\n\
                         (winit CustomCursor). Very reliable — the OS draws it.\n\
                         Requires the window to receive cursor messages, so\n\
                         click pass-through is disabled in this mode.",
                    );
                ui.checkbox(&mut self.show_region_visual, "Show region outline");
                #[cfg(target_os = "windows")]
                ui.checkbox(&mut self.passthrough, "Click pass-through (mouse)")
                    .on_hover_text(
                        "Clicks pass through the overlay to the apps below.\n\
                         Only effective with the painted cursor\n\
                         (i.e. when 'Use OS bitmap cursor image' is off).\n\
                         Uses global cursor tracking (GetCursorPos).",
                    );
                ui.separator();

                // ---- overlay a specific window (Windows) ----
                ui.label("Overlay a specific window (Windows):");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.target_window_input)
                            .hint_text("window title substring, empty = whole screen")
                            .desired_width(210.0),
                    );
                    if ui.button("Apply").clicked() {
                        let s = self.target_window_input.trim();
                        self.target_window = (!s.is_empty()).then(|| s.to_owned());
                        self.target_hwnd = 0;
                        self.last_target_rect = None;
                        self.region_initialized_for_target = false;
                    }
                    if ui.button("Clear").clicked() {
                        self.target_window = None;
                        self.target_window_input.clear();
                        self.target_hwnd = 0;
                        self.last_target_rect = None;
                        self.region_initialized_for_target = false;
                        self.region = default_region();
                    }
                });
                ui.separator();

                let r = self.region;
                ui.monospace(format!(
                    "region: x={:.0}..{:.0}  y={:.0}..{:.0}\n\
                     size:   {:.0} x {:.0}",
                    r.min.x,
                    r.max.x,
                    r.min.y,
                    r.max.y,
                    r.width(),
                    r.height()
                ));

                ui.horizontal(|ui| {
                    if ui.button("Reset").clicked() {
                        if let Some(screen) = self.screen_rect() {
                            self.region = screen;
                        } else {
                            self.region = default_region();
                        }
                    }
                    if ui.button("Center").clicked() {
                        if let Some(screen) = self.screen_rect() {
                            let s = vec2(960.0, 540.0).min(screen.size() - vec2(80.0, 80.0));
                            self.region = Rect::from_center_size(screen.center(), s);
                        }
                    }
                    if ui.button("Fullscreen").clicked() {
                        if let Some(screen) = self.screen_rect() {
                            self.region = screen;
                        }
                    }
                });
                ui.separator();

                ui.horizontal(|ui| {
                    if ui.button("Close panel (F1)").clicked() {
                        self.show_settings = false;
                    }
                    if ui.button("Quit (Esc)").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            });
    }

    fn screen_rect(&self) -> Option<Rect> {
        let ctx = self.ctx.clone();
        ctx.input(|i| {
            i.viewport()
                .outer_rect
                .or(i.viewport().inner_rect)
        })
    }
}

impl eframe::App for CursorOverlayApp {
    /// Fully transparent window background.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // ---- keyboard shortcuts ----
        if ctx.input(|i| i.key_pressed(egui::Key::F1)) {
            self.show_settings = !self.show_settings;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        // ---- first frame: default the region to the whole screen so the
        // custom cursor is visible everywhere until a sub-region is defined.
        if !self.region_initialized {
            self.region_initialized = true;
            let full = ctx.viewport_rect();
            if full.width() > 0.0 && full.height() > 0.0 {
                self.region = full;
            }
        }

        // ---- follow a target window, if configured (Windows) ----
        self.update_target_window(&ctx);

        // ---- overlay mode ----
        // Two ways to show the custom cursor:
        //  * OS bitmap cursor (default): a real OS cursor image registered
        //    with `Context::set_cursor_image` (winit `CustomCursor`). Windows
        //    draws it, so it is visible regardless of our rendering. It needs
        //    the window to receive cursor messages, so click pass-through is
        //    disabled in this mode.
        //  * Painted cursor: drawn by egui on the topmost layer. Supports
        //    click pass-through on Windows (the system cursor is hidden
        //    per-region via ShowCursor and the position is polled via
        //    GetCursorPos).
        // While the settings panel or region editor is open, the overlay is
        // paused and the normal system cursor is used.
        let overlay_on = self.enabled && !self.editing && !self.show_settings;
        let os_mode = self.use_os_cursor && overlay_on;

        #[cfg(target_os = "windows")]
        let passthrough = self.passthrough && overlay_on && !os_mode;
        #[cfg(not(target_os = "windows"))]
        let passthrough = false;

        if self.last_passthrough != Some(passthrough) {
            self.last_passthrough = Some(passthrough);
            ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(passthrough));
            #[cfg(target_os = "windows")]
            if self.native_hwnd != 0 {
                platform::apply_passthrough(self.native_hwnd, passthrough);
            }
        }

        // ---- pointer position ----
        let pointer = self.pointer_pos(&ctx, passthrough);

        // ---- region editing ----
        if self.editing {
            self.handle_region_drag(&ctx, pointer);
        }

        // ---- draw the region (only while editing / when toggled on) ----
        self.paint_region(ui);

        // ---- custom cursor behavior ----
        let in_region = pointer.is_some_and(|p| self.region.contains(p));

        if os_mode {
            // Real OS cursor image, region-limited by toggling it on/off.
            if in_region {
                ctx.set_cursor_image(Some(self.os_cursor.clone()));
            } else {
                ctx.set_cursor_image(None);
            }
            ctx.set_cursor_icon(CursorIcon::Default);
            self.set_system_cursor_visible(false, true);
        } else if overlay_on && passthrough {
            // Painted cursor with click pass-through (Windows): hide the
            // system cursor only inside the region and draw ours.
            if in_region {
                self.set_system_cursor_visible(true, false); // ShowCursor(FALSE)
                if let Some(p) = pointer {
                    self.paint_custom_cursor(&ctx, p);
                }
            } else {
                self.set_system_cursor_visible(true, true); // ShowCursor(TRUE)
            }
        } else if overlay_on && in_region {
            // Painted cursor without click pass-through (fallback).
            ctx.set_cursor_icon(CursorIcon::None);
            if let Some(p) = pointer {
                self.paint_custom_cursor(&ctx, p);
            }
            self.set_system_cursor_visible(false, true);
        } else {
            // Overlay paused or pointer outside the region: normal cursor.
            ctx.set_cursor_icon(CursorIcon::Default);
            ctx.set_cursor_image(None);
            self.set_system_cursor_visible(false, true); // restore OS cursor
        }

        // ---- live status line (lets you verify what the overlay is doing) ----
        if !self.show_settings {
            let status = format!(
                "F1 settings · Esc quit   |   cursor:{} · pass:{} · region:{}",
                if os_mode { "IMG" } else { "PAINT" },
                if passthrough { "ON" } else { "off" },
                if in_region { "IN" } else { "OUT" },
            );
            let painter = ctx.debug_painter();
            painter.text(
                egui::pos2(8.0, 6.0),
                egui::Align2::LEFT_TOP,
                status,
                FontId::proportional(11.0),
                Color32::from_gray(220).gamma_multiply(0.6),
            );
        }

        // ---- settings panel ----
        if self.show_settings {
            self.settings_window(&ctx);
        }

        // Keep tracking the mouse smoothly even when it is idle.
        ctx.request_repaint();
    }
}

impl Drop for CursorOverlayApp {
    fn drop(&mut self) {
        // Never leave the system cursor hidden.
        self.set_system_cursor_visible(true, true);
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Pick the rendering backend from `--backend glow|wgpu` (default: glow).
///
/// wgpu is eframe's default backend but is known to crash with
/// `STATUS_ACCESS_VIOLATION` (`0xc0000005`) at startup on some Windows
/// machines (https://github.com/emilk/egui/issues/3686), so we prefer the
/// glow (OpenGL) backend. To use wgpu, enable the `wgpu` cargo feature and
/// pass `--backend wgpu`.
fn select_renderer() -> eframe::Renderer {
    let backend = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[0] == "--backend")
        .map(|w| w[1].to_ascii_lowercase());

    match backend.as_deref() {
        Some("glow") => {
            #[cfg(feature = "glow")]
            {
                return eframe::Renderer::Glow;
            }
            #[cfg(not(feature = "glow"))]
            eprintln!("warning: the glow backend is not compiled; enable the `glow` feature");
        }
        Some("wgpu") => {
            #[cfg(feature = "wgpu")]
            {
                return eframe::Renderer::Wgpu;
            }
            #[cfg(not(feature = "wgpu"))]
            eprintln!(
                "warning: the wgpu backend is not compiled; enable the `wgpu` feature \
                 (cargo run --features wgpu)"
            );
        }
        Some(other) => {
            eprintln!("warning: unknown backend {other:?} (expected \"glow\" or \"wgpu\")");
        }
        None => {}
    }

    // Default: glow when available, otherwise wgpu.
    #[cfg(feature = "glow")]
    {
        eframe::Renderer::Glow
    }
    #[cfg(not(feature = "glow"))]
    {
        eframe::Renderer::Wgpu
    }
}

fn main() -> eframe::Result {
    env_logger::init();

    let renderer = select_renderer();
    log::info!("Using the {renderer:?} renderer");
    #[cfg(target_os = "windows")]
    eprintln!("custom-cursor-overlay: renderer={renderer:?}, click pass-through supported");
    #[cfg(not(target_os = "windows"))]
    eprintln!("custom-cursor-overlay: renderer={renderer:?} (click pass-through is Windows-only)");
    eprintln!("custom-cursor-overlay: F1 = settings panel, Esc = quit");

    let viewport = ViewportBuilder::default()
        .with_app_id("custom_cursor_overlay")
        .with_title("Custom Cursor Overlay")
        .with_decorations(false)
        .with_transparent(true)
        .with_always_on_top()
        .with_fullscreen(true);

    // The overlay starts with the settings panel closed, so click
    // pass-through is enabled from the very first frame on Windows.
    #[cfg(target_os = "windows")]
    let viewport = viewport.with_mouse_passthrough(true);
    #[cfg(not(target_os = "windows"))]
    let viewport = viewport.with_mouse_passthrough(false);

    let native_options = eframe::NativeOptions {
        renderer,
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "custom_cursor_overlay",
        native_options,
        Box::new(|cc| Ok(Box::new(CursorOverlayApp::new(cc)))),
    )
}
