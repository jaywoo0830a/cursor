//! Core application state: region, target-window tracking, and the
//! cursor-owning / pass-through decision that drives the Chromium frontend.
//!
//! The cursor itself is rendered by the webview in pure CSS. What Rust owns
//! here is the *decision* about who gets the input / cursor:
//!
//! * inside the region the overlay window is **non-click-through** — it owns
//!   the hit-testing (and therefore `WM_SETCURSOR`), so the app below can
//!   never override the cursor (no pen pop-out, works over DirectComposition)
//!   and the CSS cursor is shown. All pointer input is then forwarded to the
//!   app below (`platform::forward_mouse`).
//! * outside the region the overlay is **click-through** (`WS_EX_TRANSPARENT`)
//!   — the app below keeps its own cursor and input.
//! * without a target window (or while it is missing) the overlay stays a
//!   fully click-through, invisible layer.
//!
//! The frontend is a *view*: it renders whatever [`App::tick`] reports and
//! sends editing commands back through [`App::handle_ipc`].

use crate::input::{self, InputEvent, InputSnapshot};
use crate::platform;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// A rectangle in logical (CSS) pixels, relative to the overlay window
/// (origin = the overlay's top-left, which equals the target window's
/// top-left when following one).
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

/// Minimum region size (logical px) enforced by the frontend editor.
pub const MIN_REGION: f32 = 24.0;

/// How long the overlay stays in “pen mode” (click-through) after the last
/// pen event, so the app below receives real Windows Ink / WM_POINTER
/// strokes without us stealing them.
const PEN_ACTIVE_MS: u64 = 1200;

pub struct App {
    // --- settings (mirrored to / edited from the frontend) ---
    pub enabled: bool,
    pub editing: bool,
    pub show_region: bool,
    pub settings_open: bool,
    pub region: RectF,
    pub target_window: Option<String>,
    quit: bool,
    force_push: bool,

    // --- target-window tracking ---
    target_hwnd: usize,
    last_target_rect: Option<(i32, i32, i32, i32)>,
    window_follow_active: bool,
    region_initialized_for_target: bool,

    // --- window / coordinates ---
    native_hwnd: usize,
    scale: f32,
    win_w: f32,
    win_h: f32,
    /// Overlay window top-left in physical screen pixels.
    win_origin: (f32, f32),

    // --- per-frame state ---
    /// When the pointer left the region (for the 60 ms out-of-region grace).
    last_out_region: Option<Instant>,
    last_passthrough: Option<bool>,
    last_sent: Option<String>,
    /// Frontend UI rectangles (logical px, window-local): status bar,
    /// settings panel. Forwarded clicks inside them are swallowed so our UI
    /// doesn't double-fire into the app below.
    ui_rects: Vec<(f32, f32, f32, f32)>,
    /// Last pen activity (raw HID `Pen` events).
    last_pen_at: Option<Instant>,
    /// Last pen activity reported by the frontend (PointerEvent type=pen).
    frontend_pen_at: Option<Instant>,

    // --- raw input ---
    raw: InputSnapshot,
    raw_rx: Option<Receiver<InputEvent>>,
}

impl App {
    pub fn new(target_window: Option<String>) -> Self {
        Self {
            enabled: true,
            editing: false,
            show_region: false,
            settings_open: false,
            region: RectF {
                x: 0.0,
                y: 0.0,
                w: 960.0,
                h: 540.0,
            },
            target_window,
            quit: false,
            force_push: true,
            target_hwnd: 0,
            last_target_rect: None,
            window_follow_active: false,
            region_initialized_for_target: false,
            native_hwnd: 0,
            scale: 1.0,
            win_w: 0.0,
            win_h: 0.0,
            win_origin: (0.0, 0.0),
            last_out_region: None,
            last_passthrough: None,
            last_sent: None,
            ui_rects: Vec::new(),
            last_pen_at: None,
            frontend_pen_at: None,
            raw: InputSnapshot::default(),
            raw_rx: input::start(),
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
        // If we are not following a target yet, default the region to the
        // whole window so the frontend always has a sensible starting box.
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

    // ---- per-frame driver -------------------------------------------

    /// Advance the state machine and return a JSON snapshot for the frontend
    /// (or `None` if nothing changed since the last push).
    pub fn tick(&mut self, window: &tao::window::Window) -> Option<String> {
        // Drain raw input (mouse / pen / touch) into the snapshot.
        if let Some(rx) = &self.raw_rx {
            while let Ok(ev) = rx.try_recv() {
                if matches!(ev, InputEvent::Pen { .. }) {
                    self.last_pen_at = Some(Instant::now());
                }
                self.raw.apply(&ev);
            }
        }

        self.update_target_window(window);

        // Global pointer (physical px) -> window-local logical position.
        let (plx, ply) = self.pointer_local();
        let in_region = self.window_follow_active && self.region.contains(plx, ply);

        // Hysteresis: a pen can briefly glitch the reported position the
        // instant it touches, so don't drop the owned cursor on a momentary
        // out-of-region sample — keep it for 60 ms after leaving.
        if in_region {
            self.last_out_region = None;
        } else {
            let _ = self.last_out_region.get_or_insert(Instant::now());
        }
        let in_region_eff = in_region
            || self
                .last_out_region
                .is_some_and(|t| t.elapsed() < Duration::from_millis(60));

        // Pen in use? While the pen is active we must NOT own the input —
        // the app below needs the real Windows Ink / WM_POINTER stroke. We
        // go click-through instead (our forwarding only replays *mouse*
        // messages, which Ink pens don't always synthesize). Decay keeps it
        // stable across small gaps.
        let now = Instant::now();
        let pen_active = self
            .last_pen_at
            .is_some_and(|t| now.duration_since(t).as_millis() < PEN_ACTIVE_MS as u128)
            || self
                .frontend_pen_at
                .is_some_and(|t| now.duration_since(t).as_millis() < PEN_ACTIVE_MS as u128);

        let overlay_on =
            self.enabled && !self.editing && !self.settings_open && self.window_follow_active;
        let owning = overlay_on && in_region_eff && !pen_active;

        // Pass-through decision:
        //  * no target / disabled   -> click-through (invisible layer)
        //  * editing / settings     -> non-click-through (frontend needs input)
        //  * pen in use             -> click-through (app below gets real pen)
        //  * overlay on, in region  -> non-click-through (we own the cursor)
        //  * overlay on, out region -> click-through (app below gets input)
        let passthrough = if !self.window_follow_active {
            true
        } else if self.editing || self.settings_open {
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

        // Push the frontend UI rectangles (status bar / settings panel) so
        // clicks over our own UI are not replayed to the app below.
        #[cfg(target_os = "windows")]
        {
            let s = self.scale.max(0.1);
            let phys: Vec<(i32, i32, i32, i32)> = self
                .ui_rects
                .iter()
                .map(|&(x, y, w, h)| {
                    let (ox, oy) = self.win_origin;
                    (
                        (ox + x * s) as i32,
                        (oy + y * s) as i32,
                        (ox + (x + w) * s) as i32,
                        (oy + (y + h) * s) as i32,
                    )
                })
                .collect();
            platform::set_forward_block_rects(&phys);
        }

        if self.force_push {
            self.force_push = false;
            self.last_sent = None;
        }

        let state = self.state_json(owning, in_region_eff, passthrough, pen_active);
        if self.last_sent.as_deref() != Some(state.as_str()) {
            self.last_sent = Some(state.clone());
            return Some(state);
        }
        None
    }

    /// Handle a command message from the frontend (`window.ipc.postMessage`).
    pub fn handle_ipc(&mut self, json: &str) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else {
            return;
        };
        let Some(cmd) = v.get("cmd").and_then(|c| c.as_str()) else {
            return;
        };
        match cmd {
            "ready" => self.force_push = true,
            "set_enabled" => self.enabled = Self::bool_at(&v, "value", self.enabled),
            "set_editing" => self.editing = Self::bool_at(&v, "value", self.editing),
            "set_show_region" => self.show_region = Self::bool_at(&v, "value", self.show_region),
            "settings" => self.settings_open = Self::bool_at(&v, "value", self.settings_open),
            "set_region" => {
                let f = |k: &str, d: f32| -> f32 {
                    v.get(k).and_then(|x| x.as_f64()).unwrap_or(d as f64) as f32
                };
                self.region = RectF {
                    x: f("x", self.region.x),
                    y: f("y", self.region.y),
                    w: f("w", self.region.w).max(MIN_REGION),
                    h: f("h", self.region.h).max(MIN_REGION),
                };
            }
            "set_ui_rects" => {
                self.ui_rects = v
                    .get("rects")
                    .and_then(|r| r.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|r| {
                                let f = |k: &str| r.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                                Some((f("x"), f("y"), f("w"), f("h")))
                            })
                            .collect()
                    })
                    .unwrap_or_default();
            }
            "set_target" => {
                let title = v
                    .get("title")
                    .and_then(|t| t.as_str())
                    .unwrap_or("")
                    .trim()
                    .to_string();
                self.target_window = (!title.is_empty()).then_some(title);
                // Re-find next tick; update_target_window handles the
                // window-size transition (restore fullscreen if cleared).
                self.target_hwnd = 0;
                self.region_initialized_for_target = false;
                self.last_target_rect = None;
                self.force_push = true;
            }
            "preset" => {
                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("");
                match name {
                    "fullscreen" | "reset" => {
                        self.region = RectF {
                            x: 0.0,
                            y: 0.0,
                            w: self.win_w,
                            h: self.win_h,
                        };
                    }
                    "center" => {
                        let w = self.win_w.min(960.0);
                        let h = self.win_h.min(540.0);
                        self.region = RectF {
                            x: ((self.win_w - w) / 2.0).max(0.0),
                            y: ((self.win_h - h) / 2.0).max(0.0),
                            w,
                            h,
                        };
                    }
                    _ => {}
                }
                self.force_push = true;
            }
            "quit" => self.quit = true,
            "pen_active" => {
                let active = v
                    .get("value")
                    .and_then(|x| x.as_bool())
                    .unwrap_or(false);
                self.frontend_pen_at = if active {
                    Some(Instant::now())
                } else {
                    None
                };
            }
            _ => {}
        }
    }

    fn bool_at(v: &serde_json::Value, key: &str, def: bool) -> bool {
        v.get(key).and_then(|x| x.as_bool()).unwrap_or(def)
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
                // Not found (yet) — stay inactive, restore fullscreen if we
                // were following something before.
                if self.window_follow_active {
                    window.set_fullscreen(Some(Fullscreen::Borderless(None)));
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
                            // Default: cover the whole target window.
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
                    // Target closed / invalid — re-find it next frame.
                    self.target_hwnd = 0;
                    self.last_target_rect = None;
                    if self.window_follow_active {
                        window.set_fullscreen(Some(Fullscreen::Borderless(None)));
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

    fn state_json(&self, owning: bool, in_region: bool, passthrough: bool, pen_active: bool) -> String {
        serde_json::json!({
            "enabled": self.enabled,
            "editing": self.editing,
            "show_region": self.show_region,
            "settings": self.settings_open,
            "owning": owning,
            "passthrough": passthrough,
            "in_region": in_region,
            "pen_active": pen_active,
            "found": self.window_follow_active,
            "target": self.target_window.clone().unwrap_or_default(),
            "device": self.raw.last_device,
            "region": {
                "x": self.region.x,
                "y": self.region.y,
                "w": self.region.w,
                "h": self.region.h,
            },
            "win": { "w": self.win_w, "h": self.win_h },
        })
        .to_string()
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Stop the raw-input threads / unhook the low-level hook.
        input::stop();
        // Disable input forwarding so no replay happens after teardown.
        #[cfg(target_os = "windows")]
        platform::set_forwarding(false, 0);
    }
}
