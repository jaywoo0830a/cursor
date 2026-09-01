//! The overlay application: egui UI, region editing, cursor behavior,
//! settings panel and raw-input integration.

use eframe::egui::{
    self, pos2, vec2, Color32, ColorImage, CornerRadius, CursorIcon, FontId, Id, Rect, Stroke,
    StrokeKind, TextureHandle, TextureOptions,
};

use crate::config::{default_region, HANDLE_SIZE, MIN_REGION, CURSOR_DISPLAY_SIZE};
use crate::cursor::{self, CursorBitmap};
use crate::input::{self, InputEvent, InputSnapshot};
#[cfg(target_os = "windows")]
use crate::platform;

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

pub struct CursorOverlayApp {
    ctx: egui::Context,
    bitmap: CursorBitmap,
    texture: TextureHandle,

    /// The overlay region, in window (screen) points.
    region: Rect,

    /// Whether the settings panel is open.
    show_settings: bool,
    /// Whether the region can be edited (drag / resize). While editing the
    /// system cursor is shown normally.
    editing: bool,
    /// Master switch for the custom-cursor behavior.
    enabled: bool,
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
    /// A real Windows `HCURSOR` created from the cursor bitmap, used to force
    /// the custom cursor over any app.
    #[cfg_attr(not(target_os = "windows"), allow(dead_code))]
    hcursor: usize,

    /// Receiver for the raw low-level input thread (mouse / pen / touch).
    raw_rx: Option<std::sync::mpsc::Receiver<InputEvent>>,
    /// Latest decoded raw-input state (shown in the settings panel).
    raw: InputSnapshot,

    /// Consecutive frames the pointer was outside the region. Used for
    /// hysteresis so a brief glitch (e.g. a drawing-pad pen touch) doesn't
    /// disable the hidden/painted cursor.
    out_region_frames: u32,

    drag: Option<Handle>,
    drag_start_pointer: egui::Pos2,
    drag_start_region: Rect,
}

impl CursorOverlayApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = cc.egui_ctx.clone();

        let bitmap = cursor::load_cursor_bitmap();

        let texture = ctx.load_texture(
            "custom_cursor",
            ColorImage::from_rgba_unmultiplied(
                [bitmap.size[0] as usize, bitmap.size[1] as usize],
                &bitmap.rgba,
            ),
            TextureOptions::NEAREST,
        );

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

        // Build a real Windows HCURSOR so we can force the custom circle
        // over any app (e.g. a PDF viewer's canvas) that sets its own cursor.
        #[cfg(target_os = "windows")]
        let hcursor = platform::create_hcursor_from_rgba(
            &bitmap.rgba,
            bitmap.size[0],
            bitmap.size[1],
            bitmap.hotspot[0],
            bitmap.hotspot[1],
        );
        #[cfg(not(target_os = "windows"))]
        let hcursor = 0;

        // Start the low-level raw input thread (Windows). On other platforms
        // this returns None and the snapshot stays empty.
        let raw_rx = input::start();
        let raw = InputSnapshot::default();

        // Start the high-frequency cursor guard (Windows): continuously
        // re-hides the system cursor while the overlay is active.
        #[cfg(target_os = "windows")]
        platform::start_cursor_guard();

        Self {
            ctx,
            bitmap,
            texture,
            region: default_region(),
            // Start with the overlay active: no settings panel and no region
            // outline — just the transparent background and the cursor.
            show_settings: false,
            editing: false,
            enabled: true,
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
            hcursor,
            raw_rx,
            raw,
            out_region_frames: 0,
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
    /// poll the global cursor position instead (Windows only, with the
    /// low-level hook position as a fast fallback).
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))]
    fn pointer_pos(&self, ctx: &egui::Context, passthrough: bool) -> Option<egui::Pos2> {
        #[cfg(target_os = "windows")]
        {
            if passthrough {
                let pos = platform::global_cursor_pos().or_else(input::last_global_mouse_pos);
                if let Some((x, y)) = pos {
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

    /// Hide/show the OS cursor. Windows' `ShowCursor` uses a global display
    /// counter; `platform::set_system_cursor_visible(false)` pushes it deep
    /// below 0 so a handful of `ShowCursor(TRUE)` calls by apps cannot re-show
    /// the cursor between frames.
    #[cfg(target_os = "windows")]
    fn hide_cursor(&mut self, hide: bool) {
        if hide {
            platform::set_system_cursor_visible(false);
            self.cursor_hidden = true;
        } else if self.cursor_hidden {
            platform::set_system_cursor_visible(true);
            self.cursor_hidden = false;
        }
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
            .default_size(vec2(400.0, 0.0))
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
                ui.checkbox(&mut self.show_region_visual, "Show region outline");
                #[cfg(target_os = "windows")]
                ui.checkbox(&mut self.passthrough, "Click pass-through (mouse)")
                    .on_hover_text(
                        "Clicks pass through the overlay to the apps below.\n\
                         Inside the region the system cursor is fully hidden\n\
                         and the circle is painted instead — no other process\n\
                         or driver can show it. Uses global cursor tracking.",
                    );
                ui.separator();

                // ---- overlay a specific window (Windows) ----
                ui.label("Overlay a specific window (Windows):");
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.target_window_input)
                            .hint_text("window title substring, empty = whole screen")
                            .desired_width(230.0),
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

                // ---- raw low-level input (Windows) ----
                ui.label(
                    egui::RichText::new("Low-level raw input (Win32: WH_MOUSE_LL + WM_INPUT)")
                        .strong(),
                );
                if self.raw_rx.is_some() {
                    let pen = self.raw.pen;
                    let touch_on = self.raw.touch.is_some() as u8;
                    let pad = self.raw.touchpad.is_some();
                    ui.monospace(format!(
                        "mouse:  ({:.0}, {:.0})   rawΔ: ({}, {})  wheel: {}\n\
                         pen:    {}  barrel:{} eraser:{}  pressure {:.2}\n\
                         touch:  {}   touchpad: {}\n\
                         device: {}   HID reports: {}",
                        self.raw.mouse.0,
                        self.raw.mouse.1,
                        self.raw.raw_delta.0,
                        self.raw.raw_delta.1,
                        self.raw.wheel,
                        match pen {
                            Some(c) if c.down => "down  ".to_string(),
                            Some(_) => "up    ".to_string(),
                            None => "none  ".to_string(),
                        },
                        pen.map_or(0, |c| c.barrel as u8),
                        pen.map_or(0, |c| c.eraser as u8),
                        pen.map_or(0.0, |c| c.pressure),
                        touch_on,
                        if pad { "active" } else { "none" },
                        self.raw.last_device,
                        self.raw.hid_reports,
                    ));
                    ui.weak(
                        "Pen / touch / trackpad are captured raw via HID raw input\n\
                         (pen 0x0D/0x02, touch 0x0D/0x04, touch pad 0x0D/0x05);\n\
                         pressure/tilt are best-effort decodes of the HID report.",
                    );
                } else {
                    ui.weak("Raw input is only available on Windows.");
                }
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
        ctx.input(|i| i.viewport().outer_rect.or(i.viewport().inner_rect))
    }
}

impl eframe::App for CursorOverlayApp {
    /// Fully transparent window background.
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // ---- drain raw low-level input events (mouse / pen / touch) ----
        if let Some(rx) = &self.raw_rx {
            while let Ok(ev) = rx.try_recv() {
                self.raw.apply(&ev);
            }
        }

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
        // 원천 차단 방식: 오버레이가 활성이고 포인터가 영역 안이면 시스템 커서를
        // 완전히 숨기고(심층 카운터 + 고빈도 가드) 우리 원을 직접 그린다. 어떤
        // 프로세스/드라이버가 SetCursor/SetSystemCursor를 호출해도 시스템 커서는
        // 보이지 않으므로 "점령"이 무의미해진다. 영역 밖 또는 설정창이 열려 있으면
        // 정상 시스템 커서를 복원한다.
        let overlay_on = self.enabled && !self.editing && !self.show_settings;

        #[cfg(target_os = "windows")]
        let passthrough = self.passthrough && overlay_on;
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

        // Hysteresis: a drawing-pad pen can briefly glitch the reported
        // position the instant it touches, so don't disable the forced circle
        // on a single out-of-region frame — only after a few consecutive ones.
        if in_region {
            self.out_region_frames = 0;
        } else {
            self.out_region_frames = self.out_region_frames.saturating_add(1);
        }
        let in_region_eff = self.out_region_frames < 3;

        // High-frequency guard: continuously re-hide the system cursor while
        // the overlay is active inside the region, so no other process or
        // driver can ever show it in between frames.
        #[cfg(target_os = "windows")]
        platform::set_cursor_guard(overlay_on && in_region_eff);

        if overlay_on && in_region_eff {
            // 영역 안: 시스템 커서 숨김 + 우리 원 그림.
            #[cfg(target_os = "windows")]
            if self.hcursor != 0 {
                self.hide_cursor(true);
                // 백업: 만에 하나 커서가 잠깐 보여도 우리 원이 되도록 교체 유지.
                platform::set_system_cursor_active(true, self.hcursor);
            }
            if let Some(p) = pointer {
                self.paint_custom_cursor(&ctx, p);
            }
            ctx.set_cursor_icon(CursorIcon::None);
            ctx.set_cursor_image(None);
            #[cfg(target_os = "windows")]
            platform::set_cursor_handle(None);
        } else {
            // 영역 밖 또는 오버레이 일시정지: 시스템 커서 복원.
            #[cfg(target_os = "windows")]
            if self.hcursor != 0 {
                self.hide_cursor(false);
                platform::set_system_cursor_active(false, self.hcursor);
            }
            ctx.set_cursor_icon(CursorIcon::Default);
            ctx.set_cursor_image(None);
            #[cfg(target_os = "windows")]
            platform::set_cursor_handle(None);
        }

        // ---- live status line (lets you verify what the overlay is doing) ----
        if !self.show_settings {
            let device = if self.raw.last_device.is_empty() {
                String::new()
            } else {
                format!(" · in:{}", self.raw.last_device)
            };
            let status = format!(
                "F1 settings · Esc quit   |   pass:{} · region:{}{}",
                if passthrough { "ON" } else { "off" },
                if in_region { "IN" } else { "OUT" },
                device,
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
        #[cfg(target_os = "windows")]
        platform::set_system_cursor_visible(true);
        #[cfg(target_os = "windows")]
        if self.hcursor != 0 {
            // Restore the original system cursors, then free our HCURSOR.
            platform::restore_system_cursor_swap();
            platform::destroy_cursor(self.hcursor);
        }
        #[cfg(target_os = "windows")]
        platform::stop_cursor_guard();
        input::stop();
    }
}
