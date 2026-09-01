//! # Custom Cursor Overlay — pure native Rust (no webview)
//!
//! A transparent, frameless, always-on-top overlay that:
//! * shows a custom cursor (native OS `HCURSOR` built from a Rust bitmap)
//!   inside a user-defined region — the window owns the hit-testing there, so
//!   apps below can never override it (no pen pop-out, works over
//!   DirectComposition);
//! * forwards ALL input (mouse move / buttons / wheel / horizontal wheel /
//!   X-buttons / touchpad) to the app below via `PostMessage`, and lets the
//!   pen through by switching to click-through while the pen is active, so
//!   handwriting (OneNote etc.) receives the real Windows Ink stroke;
//! * draws the region box + a status badge natively with GDI
//!   (`render::OverlaySurface`, `UpdateLayeredWindow`).
//!
//! Interaction is 100% Rust: hotkeys `Ctrl+Shift+C/R/O/0/Q` and `Esc`, plus
//! mouse-driven region editing. There is no JavaScript and no webview.
//!
//! Usage: `custom-cursor-overlay --window "<window title substring>"`.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cursor;
mod input;
mod platform;
mod render;

use std::sync::{Arc, Mutex};

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::{Fullscreen, WindowBuilder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // `--window "<title>"` is mandatory.
    let args: Vec<String> = std::env::args().collect();
    let target = args
        .windows(2)
        .find(|w| w[0] == "--window")
        .map(|w| w[1].clone());
    let Some(target) = target else {
        eprintln!("custom-cursor-overlay: error: --window \"<window title substring>\" is required");
        eprintln!("usage: custom-cursor-overlay --window \"Window Title\"");
        std::process::exit(1);
    };
    log::info!("overlaying window matching: {target:?}");

    // Transparent, frameless, always-on-top, fullscreen overlay window.
    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Custom Cursor Overlay")
        .with_transparent(true)
        .with_decorations(false)
        .with_always_on_top(true)
        .with_fullscreen(Some(Fullscreen::Borderless(None)))
        .build(&event_loop)?;

    // Native overlay HWND (0 on non-Windows).
    #[cfg(target_os = "windows")]
    let overlay_hwnd: usize = {
        use tao::platform::windows::WindowExtWindows;
        window.hwnd() as usize
    };
    #[cfg(not(target_os = "windows"))]
    let overlay_hwnd: usize = 0;

    // Shared app state.
    let app_arc = Arc::new(Mutex::new(app::App::new(Some(target))));

    // Configure the app + window, then create the native render surface.
    {
        let mut app = app_arc.lock().unwrap();
        app.set_native_hwnd(overlay_hwnd);
        #[cfg(target_os = "windows")]
        {
            platform::polish_overlay_window(overlay_hwnd);
            platform::log_window_styles(overlay_hwnd);
        }
        let scale = window.scale_factor();
        app.set_scale(scale);
        let size = window.inner_size();
        app.set_window_size(size.width as f64 / scale, size.height as f64 / scale);
    }

    let mut surface = render::OverlaySurface::create(
        overlay_hwnd,
        window.inner_size().width as i32,
        window.inner_size().height as i32,
    );

    log::info!(
        "overlay ready — hotkeys: Ctrl+Shift C(on/off) R(영역편집) O(윤곽) 0(전체) Q(종료), Esc(종료)"
    );

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::ExitWithCode(0);
            }
            // Per-frame: run the state machine; redraw natively when the
            // overlay content changed.
            Event::MainEventsCleared => {
                let dirty = {
                    let mut app = app_arc.lock().unwrap();
                    app.tick(&window)
                };
                if dirty {
                    let (region, editing, show_region, status, scale) = {
                        let app = app_arc.lock().unwrap();
                        (
                            app.region,
                            app.editing,
                            app.show_region,
                            app.status_cache.clone(),
                            app.scale() as f32,
                        )
                    };
                    surface.draw(region, editing, show_region, &status, scale);
                }
                if app_arc.lock().unwrap().should_quit() {
                    *control_flow = ControlFlow::ExitWithCode(0);
                }
            }
            Event::WindowEvent {
                event: WindowEvent::ScaleFactorChanged { scale_factor, .. },
                ..
            } => {
                let mut app = app_arc.lock().unwrap();
                app.set_scale(scale_factor);
                app.invalidate();
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                let mut app = app_arc.lock().unwrap();
                let scale = app.scale();
                app.set_window_size(size.width as f64 / scale, size.height as f64 / scale);
                app.invalidate();
                surface.resize(size.width as i32, size.height as i32);
            }
            _ => {}
        }
    });
}
