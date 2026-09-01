# Custom Cursor Overlay (egui 0.36 / eframe 0.36)

A transparent, frameless, always-on-top, fullscreen overlay written in Rust
with [egui 0.36.1](https://docs.rs/egui/latest/egui/) / eframe 0.36.1.

**Within a user-defined overlay region, only a custom cursor (a small circle)
appears**; the background is fully transparent and clicks pass through to the
apps below (Windows). Outside the region the normal system cursor is used.

## Features

- **Fully transparent background** by default: no settings panel and no
  region outline on startup — only the custom cursor is drawn.
- **Works over the whole screen by default**: the region is initialized to the
  full screen on the first frame, so the custom cursor replaces the system
  cursor anywhere you move the mouse. Use `F1` → *Edit region* to restrict it
  to a sub-area.
- **Modern precision-reticle cursor** (anti-aliased): a thin ring with a dark
  outline and a small center dot, hotspot at the center. Replace it with your
  own bitmap via `assets/cursor.png`.
- **Cursor-owning mode (default, root-cause fix)** — while the pointer is
  inside the region, the overlay window **stops being click-through and owns
  the hit-testing/cursor** (`WM_SETCURSOR`). Because it is the window on top,
  the apps below can *never* override the cursor — there is no "pop-out" at
  all, and it even works over DirectComposition / GPU canvases (the OS itself
  draws our circle for this window). Inside the region the window cursor is
  our circle (`SetCursorImage` + per-frame `SetCursor`); outside it the
  overlay is click-through again so the apps below keep their own cursor.
  All input — mouse moves, buttons, wheel, and pen strokes (Windows Ink /
  OTD synthesize mouse messages that the `WH_MOUSE_LL` hook sees) — is
  **forwarded** to the app below via `PostMessage` (with
  `ScreenToClient` + `SetForegroundWindow` on press), so clicking, drawing
  and scrolling all still work normally.
- **Fallback hide+paint mode** — if you turn *Own cursor* off, the earlier
  strategy is used instead: inside the region the OS cursor is **fully
  hidden** (`ShowCursor` pushed well below 0, re-enforced by a dedicated
  high-frequency guard thread) and our circle is **painted** on the topmost
  layer. Because the system cursor simply does not exist while the pointer is
  in the region, no other process or driver can make it "pop out" — whatever
  they call `SetCursor`/`SetSystemCursor` with is invisible. This is the
  legacy flicker-free fallback for pen/tablet writing (OTD, Windows Ink).
- **Click pass-through:** clicks pass through the overlay to the apps below.
  Enforced directly with `WS_EX_TRANSPARENT` (`SetWindowLongPtrW`). In
  cursor-owning mode pass-through is applied automatically *outside* the
  region (non-click-through inside, since we must own the hit-testing).
- **Low-level raw input (Windows):** a background thread runs its own Win32
  message loop and captures, via raw Win32 API (`windows-sys`):
  * **Mouse** — a global `WH_MOUSE_LL` hook (move / buttons / wheel with
    global position) plus high-frequency relative deltas from raw input
    (`WM_INPUT` → `GetRawInputData`).
  * **Pen (터치펜) / touch / trackpad** — HID raw input is registered for the
    digitizer usages (pen `0x0D/0x02`, touch `0x0D/0x04`, touch pad
    `0x0D/0x05`) and reports are decoded best-effort (contact, x/y,
    pressure, tilt).
  * **Debounced (~1 ms):** events are coalesced (latest state, summed
    deltas) and flushed by a dedicated thread, so high-rate HID devices
    don't flood the app. Live state is shown in the settings panel (F1).
- Region-limited behavior: inside the region the custom cursor is shown;
  outside it the normal system cursor is used.
- **Overlay a specific window (Windows):** give a window title substring and
  the overlay resizes to exactly cover that window and follows it as it
  moves/resizes. This is the standard, safe way to "attach" the overlay to a
  particular process — no DLL injection needed.
- Interactive region editor: drag the box to move it, drag the handles to
  resize it. Presets: Reset / Center / Fullscreen.

## Controls

| Key / action        | Effect                                    |
| ------------------- | ----------------------------------------- |
| `F1`                | Toggle the settings panel                 |
| `Esc`               | Quit                                      |
| Drag region box     | Move the overlay region (edit mode)       |
| Drag region handles | Resize the overlay region (edit mode)     |

While the settings panel is closed, a small status line is drawn at the top
left of the screen:

```
F1 settings · Esc quit   |   pass:ON · region:IN · in:pen
```

`pass` = click pass-through on, `region` = whether the mouse is currently
inside the overlay region, `in` = last raw input device seen (mouse / pen /
touch / touchpad). Inside the region the cursor is ours (owned window cursor
or hidden+painted, depending on mode); outside it the normal system cursor is
used.

## Build & run

```bash
# On Linux you may need the X11/Wayland dev packages first:
#   Debian/Ubuntu:
#     sudo apt install -y libxcb-render0-dev libxcb-shape0-dev \
#         libxcb-xfixes0-dev libxkbcommon-dev libgtk-3-dev
#
#   Fedora:
#     sudo dnf install -y libxcb-devel libxkbcommon-devel gtk3-devel

cargo run --release
```

## Overlaying a specific window (Windows)

Instead of covering the whole screen, you can make the overlay hug a specific
window (e.g. a game or app). The overlay exits fullscreen, resizes to exactly
cover the target window, and follows it when it moves or resizes.

```bash
cargo run --release -- --window "MyApp"      # title substring
```

Or set it in the settings panel (`F1` → *Overlay a specific window*).
The window title is matched case-insensitively as a substring; minimized
windows are ignored. When you clear the target, the overlay returns to
fullscreen. This uses only standard Windows APIs (`EnumWindows`,
`GetWindowTextW`, `GetWindowRect`) and `ViewportCommand::OuterPosition` /
`InnerSize` / `Fullscreen` — no injection into the target process.

## How it works

1. A fullscreen frameless transparent window is created via
   `egui::ViewportBuilder` (`with_transparent`, `with_always_on_top`,
   `with_fullscreen`, `with_decorations(false)`) and `App::clear_color`
   returns a fully transparent color.
2. Each frame the pointer position is read. With click pass-through enabled
   (Windows) the window stops receiving pointer events, so the position is
   polled with `GetCursorPos` and converted from physical pixels to points;
   otherwise `Context::pointer_interact_pos` is used.
3. If the pointer is inside the configured `region` (and the overlay is
   active): the system cursor is **hidden** (`ShowCursor` pushed well below
   0 and kept there by a high-frequency guard thread, so apps/drivers
   cannot re-show it) and the custom bitmap cursor is **painted** at the
   pointer position on a top layer (`Painter::image`), offset by its
   hotspot.
4. Outside the region (or while the settings panel is open) the system
   cursor is restored (`ShowCursor(TRUE)` / `CursorIcon::Default`).
5. Raw input: a background thread runs a Win32 message loop (`WH_MOUSE_LL`
   hook + `RegisterRawInputDevices`/`WM_INPUT`) and pushes mouse / pen /
   touch / trackpad events (debounced ~1 ms) to the app every frame.

Key egui 0.36 APIs used:

- `CursorIcon::None` – hide the cursor (in `egui::CursorIcon`).
- `Context::set_cursor_icon` / `Context::set_cursor_image`.
- `egui::CustomCursorImage { rgba: Arc<[u8]>, size: [u16; 2], hotspot: [u16; 2] }`.
- `eframe::App::ui` (the 0.36 App trait replaced `update`).

## Customizing the cursor image

Drop a straight-RGBA PNG at `assets/cursor.png` (default hotspot `[0, 0]`,
editable via `PNG_HOTSPOT` in `src/config.rs`). Keep it small (e.g. 32×32).
If the file is absent, a small precision-reticle cursor is generated
automatically (`make_default_cursor` in `src/cursor.rs`).

## Notes & limitations

- **Windows crash fix (0xc0000005 / STATUS_ACCESS_VIOLATION):** egui/eframe
  0.36 defaults to the **wgpu** renderer, which crashes at startup on some
  Windows GPUs/drivers (see [emilk/egui#3686](https://github.com/emilk/egui/issues/3686)).
  This project therefore defaults to the **glow (OpenGL)** backend via the
  `renderer` option and the `glow` cargo feature. You can switch the backend
  at runtime:

  ```bash
  cargo run --release                 # default: glow (OpenGL)
  cargo run --release -- --backend glow
  cargo run --release --features wgpu -- --backend wgpu   # wgpu backend
  ```

  To change the compile-time default, edit the `[features]` section in
  `Cargo.toml` (`default = ["glow"]` → `default = ["glow", "wgpu"]` or
  `default = ["wgpu"]`).
- **Click pass-through is implemented on Windows only.** With pass-through on,
  the overlay uses `GetCursorPos` to track the mouse (the window receives no
  pointer events) and `ShowCursor(FALSE/TRUE)` to hide/show the system cursor
  (winit's per-window cursor API is ignored for pass-through windows). The
  `ShowCursor` counter is pushed well below 0 so apps can't easily re-show
  the cursor, and it is always restored on exit.
- Transparency requires a compositor on Linux/X11 (e.g. picom, Mutter), and
  works with both the glow and wgpu backends on Windows.
- The painted circle is drawn inside our topmost transparent window, so it is
  invisible over rare GPU/DirectComposition hardware-overlay canvases (some
  PDF viewers) — the system cursor is still hidden there, so use mouse
  pass-through and the app's own cursor in that case.
- The HID pen/touch/trackpad report decode in `src/input.rs` is best-effort:
  Windows HID digitizer layouts vary by device, so use the raw report bytes
  (`InputEvent::HidRaw`) as the authoritative source if needed.
- The example was built with rustc 1.98.0; egui 0.36.1 needs rustc ≥ 1.95.

## Project structure

```
src/
├── main.rs            # entry point: backend selection, viewport, run_native
├── app.rs             # CursorOverlayApp: UI flow, region editing, settings
├── config.rs          # constants + default region
├── cursor.rs          # cursor bitmap: PNG loading + generated reticle
├── input.rs           # raw low-level input (WH_MOUSE_LL hook + WM_INPUT):
│                      #   mouse / pen (터치펜) / touch / trackpad, HID decode
└── platform/
    ├── mod.rs         # cfg dispatch (windows vs stub)
    ├── windows.rs     # raw Win32: GetCursorPos/ShowCursor, HCURSOR,
    │                  #   SetSystemCursor swap, WS_EX_TRANSPARENT,
    │                  #   EnumWindows target tracking
    └── stub.rs        # non-Windows no-ops
```
