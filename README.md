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
- **OS bitmap cursor by default** — Windows draws the cursor itself, so it is
  visible over *any* app, including GPU/DirectComposition canvases such as
  PDF viewers where the system cursor would otherwise be covered or
  overridden. The circle is registered as a real OS cursor
  (`Context::set_cursor_image` → winit `CustomCursor`) **and** re-applied with
  a direct `SetCursor` every frame, so apps that set their own cursor cannot
  hide ours. This mode disables click pass-through (the window must receive
  cursor messages).
- **Click pass-through (painted mode, optional):** turn *Off* *Use OS bitmap
  cursor image* and the custom cursor is drawn by egui on the topmost layer,
  the system cursor is hidden with `ShowCursor` (re-applied every frame), the
  pointer is polled with `GetCursorPos`, and clicks pass through to the apps
  below (enforced with `WS_EX_TRANSPARENT`).
- Region-limited behavior: inside the region the system cursor is replaced by
  the custom cursor; outside it the normal system cursor is used.
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
F1 settings · Esc quit   |   cursor:IMG · pass:off · region:IN
```

`cursor:IMG` = OS bitmap cursor (default, reliable, no pass-through),
`cursor:PAINT` = egui-painted cursor (supports pass-through), `pass` = click
pass-through on, `region` = whether the mouse is currently inside the overlay
region. Use it to verify the logic: if `region:IN` is shown but no circle
appears, it is a rendering issue; if it always says `OUT`, the pointer/region
coordinates are misaligned.

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
   active):
   - the system cursor is hidden — via `ShowCursor(FALSE)` in click-through
     mode, or `ctx.set_cursor_icon(CursorIcon::None)` otherwise — and
   - the custom bitmap cursor is painted at the pointer position on a top
     layer (`Painter::image`), offset by its hotspot.
4. Outside the region the system cursor is restored (`ShowCursor(TRUE)` /
   `CursorIcon::Default`).
5. Optional native mode: `ctx.set_cursor_image(Some(CustomCursorImage { .. }))`
   registers the RGBA bitmap as a real OS cursor (`egui::CustomCursorImage`),
   which is not clipped by the window — but applies to the whole window and is
   disabled while click pass-through is active.

Key egui 0.36 APIs used:

- `CursorIcon::None` – hide the cursor (in `egui::CursorIcon`).
- `Context::set_cursor_icon` / `Context::set_cursor_image`.
- `egui::CustomCursorImage { rgba: Arc<[u8]>, size: [u16; 2], hotspot: [u16; 2] }`.
- `eframe::App::ui` (the 0.36 App trait replaced `update`).

## Customizing the cursor image

Drop a straight-RGBA PNG at `assets/cursor.png` (default hotspot `[0, 0]`,
editable via `PNG_HOTSPOT` in `src/main.rs`). Keep it small (e.g. 32×32).
If the file is absent, a small circular cursor is generated automatically.

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
  `ShowCursor` calls are paired so Windows' global display counter stays
  balanced, and the cursor is always restored on exit.
- Transparency requires a compositor on Linux/X11 (e.g. picom, Mutter), and
  works with both the glow and wgpu backends on Windows.
- The OS-level bitmap cursor mode (`use_os_cursor`) applies to the entire
  window, not only the region — that is a limitation of the OS cursor API.
- The example was built with rustc 1.98.0; egui 0.36.1 needs rustc ≥ 1.95.
