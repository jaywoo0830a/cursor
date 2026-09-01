//! # Custom Cursor Overlay (egui 0.36 / eframe 0.36)
//!
//! Entry point: pick the rendering backend and start the overlay app.
//!
//! Behavior:
//! * Inside a **user-defined region** (a rectangle on the screen) the system
//!   cursor is replaced by a custom bitmap cursor — *only* the custom cursor
//!   appears there.
//! * Outside that region the normal system cursor is shown again.
//!
//! Controls:
//! * `F1`  – toggle the settings panel
//! * `Esc` – quit
//! * While *Edit region* is on, drag the blue box to move it and the white
//!   handles to resize it.
//!
//! On Windows, *Click pass-through* (default on) lets clicks go through the
//! overlay to the apps below — and it now works together with the OS bitmap
//! cursor (the system cursor bitmaps are swapped with `SetSystemCursor`, so
//! the circle is drawn by Windows over any app, including GPU/DirectComposition
//! canvases like PDF viewers).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod config;
mod cursor;
mod input;
mod platform;
mod pointer_filter;

use eframe::egui::viewport::ViewportBuilder;

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
        Box::new(|cc| Ok(Box::new(app::CursorOverlayApp::new(cc)))),
    )
}
