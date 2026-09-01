//! # Custom Cursor Overlay (Chromium / WebView2)
//!
//! Ultra-lightweight custom-cursor overlay: a transparent, frameless,
//! always-on-top window that renders the cursor **in pure CSS** via an
//! embedded Chromium webview (WebView2 on Windows). The window owns the
//! hit-testing inside the region, so the app below can never override the
//! cursor — no pen pop-out, and it works even over DirectComposition/GPU
//! canvases. All pointer input is forwarded to the app below.
//!
//! Stack:
//! * **Rust core** (`tao` window + `wry` webview): raw input capture
//!   (WH_MOUSE_LL + WM_INPUT), target-window finding/following, region-based
//!   click pass-through (`WS_EX_TRANSPARENT`) and input forwarding
//!   (`PostMessage`).
//! * **Chromium frontend** (`index.html`, pure CSS/JS, fully offline): the
//!   custom cursor, the region editor and the settings panel.
//!
//! Usage: `custom-cursor-overlay --window "<window title substring>"`.
//! A target window is mandatory — the overlay attaches to it and follows it.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod cursor;
mod input;
mod platform;

use std::sync::{Arc, Mutex};

use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoop};
use tao::window::{Fullscreen, WindowBuilder};
use wry::http::{Response, StatusCode};
use wry::{PageLoadEvent, WebViewBuilder};

/// The whole frontend (cursor CSS, region editor, settings UI) is embedded
/// here so the app runs fully offline with no external files or CDNs.
const INDEX_HTML: &str = include_str!("../index.html");

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::init();

    // `--window "<title>"` is mandatory: there is no whole-screen mode.
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

    // Shared app state (main loop + IPC handler).
    let app_arc = Arc::new(Mutex::new(app::App::new(Some(target))));

    // Grab the native HWND so we can toggle WS_EX_TRANSPARENT directly and
    // polish the window (transparent + hidden from the taskbar / Alt-Tab).
    {
        let mut app = app_arc.lock().unwrap();
        #[cfg(target_os = "windows")]
        {
            use tao::platform::windows::WindowExtWindows;
            let hwnd = window.hwnd() as usize;
            app.set_native_hwnd(hwnd);
            platform::polish_overlay_window(hwnd);
        }
        let scale = window.scale_factor();
        app.set_scale(scale);
        let size = window.inner_size();
        app.set_window_size(size.width as f64 / scale, size.height as f64 / scale);
    }

    // Embed the Chromium webview (WebView2 on Windows). It is a pure-CSS,
    // non-interactive view (no JS API): Rust pushes presentation state via
    // evaluate_script and handles all input/hotkeys/editing itself.
    let webview = WebViewBuilder::new()
        .with_transparent(true)
        .with_focused(false)
        .with_hotkeys_zoom(false)
        .with_url("local://localhost/index.html")
        .with_custom_protocol("local".into(), |_webview_id, _request| {
            Response::builder()
                .status(StatusCode::OK)
                .header("Content-Type", "text/html; charset=utf-8")
                .header("Cache-Control", "no-cache")
                .body(INDEX_HTML.as_bytes().into())
                .unwrap()
        })
        .with_on_page_load_handler({
            let app = app_arc.clone();
            move |event, _url| {
                if matches!(event, PageLoadEvent::Finished) {
                    app.lock().unwrap().request_push();
                }
            }
        })
        .build(&window)?;

    log::info!("overlay ready: F1/gear toggles settings, Esc quits (via the frontend)");

    event_loop.run(move |event, _target, control_flow| {
        *control_flow = ControlFlow::Wait;
        match event {
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::ExitWithCode(0);
            }
            // Per-frame: drain raw input, run the state machine and push any
            // state change to the frontend.
            Event::MainEventsCleared => {
                let json = {
                    let mut app = app_arc.lock().unwrap();
                    app.tick(&window)
                };
                if let Some(json) = json {
                    let _ = webview.evaluate_script(&format!("window.__setState({json})"));
                }
                if app_arc.lock().unwrap().should_quit() {
                    *control_flow = ControlFlow::ExitWithCode(0);
                }
            }
            Event::WindowEvent {
                event: WindowEvent::ScaleFactorChanged { scale_factor, .. },
                ..
            } => {
                app_arc.lock().unwrap().set_scale(scale_factor);
            }
            Event::WindowEvent {
                event: WindowEvent::Resized(size),
                ..
            } => {
                let mut app = app_arc.lock().unwrap();
                let scale = app.scale();
                app.set_window_size(size.width as f64 / scale, size.height as f64 / scale);
            }
            _ => {}
        }
    });
}
