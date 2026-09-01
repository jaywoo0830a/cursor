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
    self, pos2, vec2, Color32, ColorImage, CornerRadius, CursorIcon, FontId, Id, LayerId, Order,
    Rect, Stroke, StrokeKind, TextureHandle, TextureOptions,
};
use eframe::egui::viewport::ViewportBuilder;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// Default overlay region (in window points, origin = top-left).
/// The window is fullscreen, so this is in screen coordinates on the
/// primary monitor. Edit it at runtime with the settings panel.
fn default_region() -> Rect {
    Rect::from_min_size(pos2(300.0, 200.0), vec2(1320.0, 680.0))
}

/// Logical display size (in points) of the *painted* custom cursor.
const CURSOR_DISPLAY_SIZE: f32 = 24.0;

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
    /// global display counter, so every `false` must be paired with a `true`.
    pub fn set_system_cursor_visible(visible: bool) {
        use windows_sys::Win32::UI::WindowsAndMessaging::ShowCursor;
        unsafe {
            let _ = ShowCursor(visible as i32);
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

/// Try to load `assets/cursor.png`; fall back to a generated arrow cursor.
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

/// Rasterize a classic arrow pointer (32x32, white outline + black fill).
fn make_default_cursor() -> CursorBitmap {
    const S: usize = 32;
    // Simple, non-self-intersecting arrow polygon (tip at top-left).
    const P: &[(f32, f32)] = &[
        (1.0, 1.0),  // tip
        (1.0, 25.0), // bottom of the shaft
        (7.0, 25.0),
        (7.0, 13.0),
        (17.0, 21.0), // tail (bottom)
        (21.0, 17.0), // tail (end)
        (11.0, 9.0),  // tail (top)
        (19.0, 9.0),  // right edge
        (8.0, 1.0),   // top right
    ];

    let mut grid = [[false; S]; S];
    for y in 0..S {
        for x in 0..S {
            grid[y][x] = point_in_polygon(x as f32 + 0.5, y as f32 + 0.5, P);
        }
    }

    let mut rgba = vec![0u8; S * S * 4];
    for y in 0..S {
        for x in 0..S {
            let i = (y * S + x) * 4;
            if grid[y][x] {
                rgba[i..i + 4].copy_from_slice(&[0, 0, 0, 255]); // black fill
            } else {
                // 1px white outline around the shape.
                let border = (x > 0 && grid[y][x - 1])
                    || (x + 1 < S && grid[y][x + 1])
                    || (y > 0 && grid[y - 1][x])
                    || (y + 1 < S && grid[y + 1][x]);
                if border {
                    rgba[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
                }
            }
        }
    }

    CursorBitmap {
        rgba: Arc::from(rgba),
        size: [S as u16, S as u16],
        hotspot: [2, 2],
    }
}

/// Ray-casting point-in-polygon test.
fn point_in_polygon(px: f32, py: f32, poly: &[(f32, f32)]) -> bool {
    let mut inside = false;
    let n = poly.len();
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if ((yi > py) != (yj > py)) && (px < (xj - xi) * (py - yi) / (yj - yi) + xi) {
            inside = !inside;
        }
        j = i;
    }
    inside
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

        Self {
            ctx,
            bitmap,
            texture,
            os_cursor,
            region: default_region(),
            show_settings: true,
            editing: false,
            enabled: true,
            use_os_cursor: false,
            show_region_visual: true,
            // Click pass-through is only implemented on Windows (global
            // cursor polling via GetCursorPos).
            #[cfg(target_os = "windows")]
            passthrough: true,
            #[cfg(not(target_os = "windows"))]
            passthrough: false,
            cursor_hidden: false,
            last_passthrough: None,
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
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    fn set_system_cursor_visible(&mut self, passthrough: bool, visible: bool) {
        if !passthrough {
            return;
        }
        #[cfg(target_os = "windows")]
        {
            if !visible && !self.cursor_hidden {
                platform::set_system_cursor_visible(false);
                self.cursor_hidden = true;
            } else if visible && self.cursor_hidden {
                platform::set_system_cursor_visible(true);
                self.cursor_hidden = false;
            }
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

    /// Paint the custom cursor at `pointer` (hotspot-corrected), on a top layer.
    fn paint_custom_cursor(&self, ctx: &egui::Context, pointer: egui::Pos2) {
        let (w, h) = (self.bitmap.size[0] as f32, self.bitmap.size[1] as f32);
        let scale = CURSOR_DISPLAY_SIZE / w;
        let size = vec2(CURSOR_DISPLAY_SIZE, CURSOR_DISPLAY_SIZE * h / w);
        let hotspot = vec2(self.bitmap.hotspot[0] as f32, self.bitmap.hotspot[1] as f32) * scale;
        let rect = Rect::from_min_size(pointer - hotspot, size);

        let layer = LayerId::new(Order::Tooltip, Id::new("custom_cursor"));
        let painter = ctx.layer_painter(layer);
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
                ui.checkbox(
                    &mut self.use_os_cursor,
                    "Use OS bitmap cursor (whole window, native)",
                )
                .on_hover_text(
                    "Registers the bitmap as a real OS cursor via winit.\n\
                     Applies to the whole window, not just the region.\n\
                     Disabled while click pass-through is active.",
                );
                ui.checkbox(&mut self.show_region_visual, "Show region outline");
                #[cfg(target_os = "windows")]
                ui.checkbox(&mut self.passthrough, "Click pass-through (mouse)")
                    .on_hover_text(
                        "Clicks pass through the overlay to the apps below.\n\
                         Uses global cursor tracking (GetCursorPos).",
                    );
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
                        self.region = default_region();
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

        // ---- click pass-through ----
        // Enabled only while the window is not needed for interaction
        // (settings panel and region editing closed) and only on Windows,
        // where we can poll the global cursor position.
        #[cfg(target_os = "windows")]
        let passthrough = self.passthrough && !self.show_settings && !self.editing;
        #[cfg(not(target_os = "windows"))]
        let passthrough = false;

        if self.last_passthrough != Some(passthrough) {
            self.last_passthrough = Some(passthrough);
            ctx.send_viewport_cmd(egui::ViewportCommand::MousePassthrough(passthrough));
        }

        // ---- pointer position ----
        let pointer = self.pointer_pos(&ctx, passthrough);

        // ---- region editing ----
        if self.editing {
            self.handle_region_drag(&ctx, pointer);
        }

        // ---- draw the region (visual guide, always drawn faintly) ----
        self.paint_region(ui);

        // ---- custom cursor behavior ----
        let active = self.enabled && !self.editing;
        let in_region = pointer.is_some_and(|p| self.region.contains(p));

        if active && self.use_os_cursor && !passthrough {
            // Native OS-level bitmap cursor: not region-limited, but a real,
            // un-clipped cursor provided by the OS/winit (no click-through).
            ctx.set_cursor_image(Some(self.os_cursor.clone()));
            ctx.set_cursor_icon(CursorIcon::Default);
            self.set_system_cursor_visible(passthrough, true);
        } else if active && in_region {
            // Hide the system cursor and paint only the custom cursor.
            if passthrough {
                self.set_system_cursor_visible(passthrough, false); // ShowCursor(FALSE)
            } else {
                ctx.set_cursor_icon(CursorIcon::None);
            }
            if let Some(p) = pointer {
                self.paint_custom_cursor(&ctx, p);
            }
        } else {
            // Outside the region (or overlay disabled): normal system cursor.
            ctx.set_cursor_icon(CursorIcon::Default);
            ctx.set_cursor_image(None);
            self.set_system_cursor_visible(passthrough, true); // restore
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

    let native_options = eframe::NativeOptions {
        renderer,
        viewport: ViewportBuilder::default()
            .with_app_id("custom_cursor_overlay")
            .with_title("Custom Cursor Overlay")
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_fullscreen(true),
        ..Default::default()
    };

    eframe::run_native(
        "custom_cursor_overlay",
        native_options,
        Box::new(|cc| Ok(Box::new(CursorOverlayApp::new(cc)))),
    )
}
