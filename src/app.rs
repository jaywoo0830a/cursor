//! Core application state — **all logic lives in Rust, no webview.**
//!
//! * **Cursor**: a native OS `HCURSOR` built from a Rust-generated bitmap,
//!   applied with `SetCursor` while the overlay owns the hit-testing. The OS
//!   draws it, so it is smooth, pen-safe and visible over DirectComposition.
//! * **Rendering**: the region box + status badge are drawn natively with GDI
//!   into the layered window (`render::OverlaySurface`). No JS, no CSS.
//! * **Interaction**: global hotkeys (`Ctrl+Shift+C/R/O/0/Q`, `Esc`) and
//!   mouse-driven region editing, all in Rust.
//! * **Handwriting**: when the pen is active (raw HID in-range / down), the
//!   overlay goes click-through so the app below (e.g. OneNote) receives the
//!   real Windows Ink / WM_POINTER stroke with pressure.

use crate::cursor;
use crate::input::{self, Hotkey, InputEvent, InputSnapshot};
use crate::platform;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// A rectangle in logical (CSS) pixels, relative to the overlay window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

/// Minimum region size (logical px).
pub const MIN_REGION: f32 = 24.0;

/// Sticky pen-mode decay (ms): once a pen is seen, stay click-through for
/// this long after the last pen event, so strokes are never cut mid-way.
const PEN_ACTIVE_MS: u64 = 2000;

/// Edge grab zone (logical px) for the region resize handles (Rust editing).
const HANDLE: f32 = 14.0;

#[derive(Clone, Copy, PartialEq)]
enum EditMode {
    Move,
    ResizeN,
    ResizeS,
    ResizeE,
    ResizeW,
    ResizeNE,
    ResizeNW,
    ResizeSE,
    ResizeSW,
}

/// An in-progress region edit drag (Rust-driven, no JS).
struct EditDrag {
    mode: EditMode,
    sx: f32,
    sy: f32,
    region: RectF,
}

pub struct App {
    // --- settings (toggled by Rust hotkeys) ---
    pub enabled: bool,
    pub editing: bool,
    pub show_region: bool,
    pub region: RectF,
    pub target_window: Option<String>,
    quit: bool,

    // --- target-window tracking ---
    target_hwnd: usize,
    last_target_rect: Option<(i32, i32, i32, i32)>,
    window_follow_active: bool,
    region_initialized_for_target: bool,

    // --- window / coordinates ---
    native_hwnd: usize,
    /// Native `HCURSOR` (our circle) applied with `SetCursor` while owning.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    hcursor: usize,
    scale: f32,
    win_w: f32,
    win_h: f32,
    /// Overlay window top-left in physical screen pixels.
    win_origin: (f32, f32),

    // --- per-frame state ---
    last_out_region: Option<Instant>,
    last_passthrough: Option<bool>,
    last_pen_at: Option<Instant>,

    // --- Rust region editing (mouse) ---
    left_down: bool,
    prev_left_down: bool,
    edit_drag: Option<EditDrag>,

    // --- raw input ---
    raw: InputSnapshot,
    raw_rx: Option<Receiver<InputEvent>>,

    // --- native render state ---
    last_fp: String,
    /// Last rendered status text (read by the renderer).
    pub status_cache: String,
}

impl App {
    pub fn new(target_window: Option<String>) -> Self {
        Self {
            enabled: true,
            editing: false,
            show_region: false,
            region: RectF {
                x: 0.0,
                y: 0.0,
                w: 960.0,
                h: 540.0,
            },
            target_window,
            quit: false,
            target_hwnd: 0,
            last_target_rect: None,
            window_follow_active: false,
            region_initialized_for_target: false,
            native_hwnd: 0,
            #[cfg(target_os = "windows")]
            hcursor: {
                let (rgba, hot) = cursor::make_cursor_bitmap(32);
                let hc = platform::create_hcursor_from_rgba(&rgba, 32, 32, hot[0], hot[1]);
                log::info!("created native cursor handle: {hc}");
                hc
            },
            #[cfg(not(target_os = "windows"))]
            hcursor: 0,
            scale: 1.0,
            win_w: 0.0,
            win_h: 0.0,
            win_origin: (0.0, 0.0),
            last_out_region: None,
            last_passthrough: None,
            last_pen_at: None,
            left_down: false,
            prev_left_down: false,
            edit_drag: None,
            raw: InputSnapshot::default(),
            raw_rx: input::start(),
            last_fp: String::new(),
            status_cache: String::new(),
        }
    }

    // ---- setters called from main.rs --------------------------------

    pub fn set_native_hwnd(&mut self, hwnd: usize) {
        self.native_hwnd = hwnd;
    }

    pub fn set_scale(&mut self, scale: f64) {
        self.scale = scale.max(0.1) as f32;
    }

    pub fn scale(&self) -> f64 {
        self.scale as f64
    }

    /// Window size in logical (CSS) pixels.
    pub fn set_window_size(&mut self, w: f64, h: f64) {
        self.win_w = w as f32;
        self.win_h = h as f32;
        if !self.window_follow_active && self.region.w <= 0.0 {
            self.region = RectF {
                x: 0.0,
                y: 0.0,
                w: self.win_w,
                h: self.win_h,
            };
        }
    }

    pub fn should_quit(&self) -> bool {
        self.quit
    }

    /// Force a redraw on the next tick (e.g. after a window resize).
    pub fn invalidate(&mut self) {
        self.last_fp.clear();
    }

    // ---- per-frame driver -------------------------------------------

    /// Advance the state machine. Returns `true` when the overlay content
    /// (region / status / modes) changed and should be redrawn.
    pub fn tick(&mut self, window: &tao::window::Window) -> bool {
        // Global hotkeys (Rust).
        while let Some(hk) = input::take_hotkey() {
            self.apply_hotkey(hk);
        }

        // Drain raw input (mouse / pen / touch) into the snapshot.
        if let Some(rx) = &self.raw_rx {
            while let Ok(ev) = rx.try_recv() {
                if matches!(ev, InputEvent::Pen { .. }) {
                    self.last_pen_at = Some(Instant::now());
                }
                if let InputEvent::MouseButton { left: true, down, .. } = ev {
                    self.left_down = down;
                }
                self.raw.apply(&ev);
            }
        }

        self.update_target_window(window);

        // Global pointer (physical px) -> window-local logical position.
        let (plx, ply) = self.pointer_local();
        let in_region = self.window_follow_active && self.region.contains(plx, ply);

        // 60 ms out-of-region grace (pen/mouse jitter at the boundary).
        if in_region {
            self.last_out_region = None;
        } else {
            let _ = self.last_out_region.get_or_insert(Instant::now());
        }
        let in_region_eff = in_region
            || self
                .last_out_region
                .is_some_and(|t| t.elapsed() < Duration::from_millis(60));

        // Pen in use? While active we must NOT own the input — the app below
        // needs the real Windows Ink / WM_POINTER stroke. Sticky decay.
        let pen_active = self
            .last_pen_at
            .is_some_and(|t| t.elapsed().as_millis() < PEN_ACTIVE_MS as u128);

        // Rust-driven region editing (drag with the mouse).
        if self.editing {
            self.update_edit((plx, ply));
        }

        let overlay_on = self.enabled && !self.editing && self.window_follow_active;
        let owning = overlay_on && in_region_eff && !pen_active;

        let passthrough = if !self.window_follow_active {
            true
        } else if self.editing {
            false
        } else if pen_active {
            true
        } else {
            !owning
        };

        #[cfg(target_os = "windows")]
        if self.last_passthrough != Some(passthrough) {
            self.last_passthrough = Some(passthrough);
            if self.native_hwnd != 0 {
                platform::apply_passthrough(self.native_hwnd, passthrough);
            }
        }
        #[cfg(not(target_os = "windows"))]
        let _ = passthrough;

        // Forward pointer input to the app below while we own the hit-testing.
        #[cfg(target_os = "windows")]
        platform::set_forwarding(owning, self.native_hwnd);

        // Native cursor (Rust): our circle while owning, an arrow while
        // editing the region. Otherwise the app below owns the cursor.
        #[cfg(target_os = "windows")]
        {
            if owning {
                platform::set_cursor_handle(Some(self.hcursor));
            } else if self.editing {
                platform::set_cursor_handle(None);
            }
        }

        self.status_cache = self.status_text(owning, pen_active);

        let fp = format!(
            "{}|{}|{}|{}|{}|{}|{:?}|{}",
            self.enabled,
            self.editing,
            self.show_region,
            owning,
            passthrough,
            pen_active,
            self.region,
            self.status_cache
        );
        let dirty = fp != self.last_fp;
        self.last_fp = fp;
        dirty
    }

    fn apply_hotkey(&mut self, hk: Hotkey) {
        match hk {
            Hotkey::ToggleEnabled => self.enabled = !self.enabled,
            Hotkey::ToggleEditing => {
                self.editing = !self.editing;
                self.edit_drag = None;
            }
            Hotkey::ToggleOutline => self.show_region = !self.show_region,
            Hotkey::RegionFull => {
                self.region = RectF {
                    x: 0.0,
                    y: 0.0,
                    w: self.win_w,
                    h: self.win_h,
                };
            }
            Hotkey::Quit => self.quit = true,
        }
    }

    // ---- Rust region editing (mouse drag) ---------------------------

    fn update_edit(&mut self, p: (f32, f32)) {
        let (px, py) = p;
        let down = self.left_down;
        let pressed = down && !self.prev_left_down;
        let released = !down && self.prev_left_down;
        self.prev_left_down = down;

        if released {
            self.edit_drag = None;
            return;
        }

        if self.edit_drag.is_none() && pressed {
            if let Some(mode) = self.grab_mode(px, py) {
                self.edit_drag = Some(EditDrag {
                    mode,
                    sx: px,
                    sy: py,
                    region: self.region,
                });
            }
        }

        let Some(d) = &self.edit_drag else {
            return;
        };
        let dx = px - d.sx;
        let dy = py - d.sy;
        let r = d.region;
        let mut nr = r;
        match d.mode {
            EditMode::Move => {
                nr.x = (r.x + dx).clamp(0.0, (self.win_w - r.w).max(0.0));
                nr.y = (r.y + dy).clamp(0.0, (self.win_h - r.h).max(0.0));
            }
            EditMode::ResizeE => nr.w = (r.w + dx).max(MIN_REGION),
            EditMode::ResizeS => nr.h = (r.h + dy).max(MIN_REGION),
            EditMode::ResizeW => {
                let w = (r.w - dx).max(MIN_REGION);
                nr.x = r.x + (r.w - w);
                nr.w = w;
            }
            EditMode::ResizeN => {
                let h = (r.h - dy).max(MIN_REGION);
                nr.y = r.y + (r.h - h);
                nr.h = h;
            }
            EditMode::ResizeNE => {
                nr.w = (r.w + dx).max(MIN_REGION);
                let h = (r.h - dy).max(MIN_REGION);
                nr.y = r.y + (r.h - h);
                nr.h = h;
            }
            EditMode::ResizeNW => {
                let w = (r.w - dx).max(MIN_REGION);
                nr.x = r.x + (r.w - w);
                nr.w = w;
                let h = (r.h - dy).max(MIN_REGION);
                nr.y = r.y + (r.h - h);
                nr.h = h;
            }
            EditMode::ResizeSE => {
                nr.w = (r.w + dx).max(MIN_REGION);
                nr.h = (r.h + dy).max(MIN_REGION);
            }
            EditMode::ResizeSW => {
                let w = (r.w - dx).max(MIN_REGION);
                nr.x = r.x + (r.w - w);
                nr.w = w;
                nr.h = (r.h + dy).max(MIN_REGION);
            }
        }
        nr.x = nr.x.clamp(0.0, (self.win_w - MIN_REGION).max(0.0));
        nr.y = nr.y.clamp(0.0, (self.win_h - MIN_REGION).max(0.0));
        if nr != r {
            self.region = nr;
        }
    }

    /// Which region handle (or move) the pointer is grabbing, if any.
    fn grab_mode(&self, px: f32, py: f32) -> Option<EditMode> {
        let r = self.region;
        let (x0, y0, x1, y1) = (r.x, r.y, r.x + r.w, r.y + r.h);
        let in_x = px >= x0 - HANDLE && px <= x1 + HANDLE;
        let in_y = py >= y0 - HANDLE && py <= y1 + HANDLE;
        let on_left = (px - x0).abs() <= HANDLE;
        let on_right = (px - x1).abs() <= HANDLE;
        let on_top = (py - y0).abs() <= HANDLE;
        let on_bottom = (py - y1).abs() <= HANDLE;
        let mode = if on_top && on_left {
            EditMode::ResizeNW
        } else if on_top && on_right {
            EditMode::ResizeNE
        } else if on_bottom && on_left {
            EditMode::ResizeSW
        } else if on_bottom && on_right {
            EditMode::ResizeSE
        } else if on_left && in_y {
            EditMode::ResizeW
        } else if on_right && in_y {
            EditMode::ResizeE
        } else if on_top && in_x {
            EditMode::ResizeN
        } else if on_bottom && in_x {
            EditMode::ResizeS
        } else if r.contains(px, py) {
            EditMode::Move
        } else {
            return None;
        };
        Some(mode)
    }

    // ---- helpers -----------------------------------------------------

    /// Global pointer (physical px) -> window-local logical position.
    fn pointer_local(&self) -> (f32, f32) {
        let (gx, gy) = input::last_global_mouse_pos()
            .or_else(platform::global_cursor_pos)
            .unwrap_or((0.0, 0.0));
        let s = self.scale.max(0.1);
        (
            (gx as f32 - self.win_origin.0) / s,
            (gy as f32 - self.win_origin.1) / s,
        )
    }

    /// Find the target window by title and attach the overlay to it
    /// (Windows). Without a target the overlay stays a click-through layer.
    fn update_target_window(&mut self, window: &tao::window::Window) {
        #[cfg(target_os = "windows")]
        {
            use tao::dpi::{PhysicalPosition, PhysicalSize};
            use tao::window::Fullscreen;

            let Some(target) = self.target_window.clone() else {
                if self.window_follow_active {
                    window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                    self.polish_window();
                    self.window_follow_active = false;
                    self.region_initialized_for_target = false;
                    self.target_hwnd = 0;
                    self.last_target_rect = None;
                    self.win_origin = (0.0, 0.0);
                }
                return;
            };

            if self.target_hwnd == 0 {
                self.target_hwnd = platform::find_window_by_title(&target)
                    .map(|h| h as usize)
                    .unwrap_or(0);
            }
            let hwnd = self.target_hwnd;
            if hwnd == 0 {
                if self.window_follow_active {
                    window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                    self.polish_window();
                    self.window_follow_active = false;
                    self.region_initialized_for_target = false;
                    self.last_target_rect = None;
                    self.win_origin = (0.0, 0.0);
                }
                return;
            }

            if platform::is_iconic(hwnd as *mut core::ffi::c_void) {
                if self.window_follow_active {
                    window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                    self.polish_window();
                    self.window_follow_active = false;
                    self.region_initialized_for_target = false;
                    self.last_target_rect = None;
                    self.win_origin = (0.0, 0.0);
                }
                return;
            }

            match platform::window_outer_rect(hwnd as *mut core::ffi::c_void) {
                Some((l, t, r, b)) => {
                    let rect = (l, t, r, b);
                    if !self.window_follow_active {
                        self.window_follow_active = true;
                        window.set_fullscreen(None);
                        self.polish_window();
                        log::info!("target window found, following rect {rect:?}");
                    }
                    if self.last_target_rect != Some(rect) {
                        self.last_target_rect = Some(rect);
                        window.set_outer_position(PhysicalPosition::new(l, t));
                        window.set_inner_size(PhysicalSize::new((r - l) as u32, (b - t) as u32));
                        let scale = self.scale.max(0.1);
                        self.win_origin = (l as f32, t as f32);
                        self.win_w = (r - l) as f32 / scale;
                        self.win_h = (b - t) as f32 / scale;
                        if !self.region_initialized_for_target {
                            self.region_initialized_for_target = true;
                            self.region = RectF {
                                x: 0.0,
                                y: 0.0,
                                w: self.win_w,
                                h: self.win_h,
                            };
                        }
                    }
                }
                None => {
                    self.target_hwnd = 0;
                    self.last_target_rect = None;
                    if self.window_follow_active {
                        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
                        self.polish_window();
                        self.window_follow_active = false;
                        self.region_initialized_for_target = false;
                        self.win_origin = (0.0, 0.0);
                    }
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let _ = window;
        }
    }

    /// Re-apply the frameless/layered styles after a fullscreen transition.
    fn polish_window(&self) {
        #[cfg(target_os = "windows")]
        if self.native_hwnd != 0 {
            platform::polish_overlay_window(self.native_hwnd);
        }
    }

    fn status_text(&self, owning: bool, pen_active: bool) -> String {
        let t = self.target_window.clone().unwrap_or_default();
        let mode = if self.editing {
            "영역편집"
        } else if pen_active {
            "펜"
        } else if owning {
            "커서"
        } else {
            "대기"
        };
        let on = if self.enabled { "ON" } else { "OFF" };
        format!(
            "[{t}] {mode} {on}  |  Ctrl+Shift C:on/off R:영역 O:윤곽 0:전체 Q:종료 Esc:종료"
        )
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Stop the raw-input threads / unhook the low-level hooks.
        input::stop();
        // Disable input forwarding so no replay happens after teardown.
        #[cfg(target_os = "windows")]
        platform::set_forwarding(false, 0);
        // Free our native cursor.
        #[cfg(target_os = "windows")]
        if self.hcursor != 0 {
            platform::destroy_cursor(self.hcursor);
        }
    }
}
