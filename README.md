# Custom Cursor Overlay (egui 0.36 / eframe 0.36)

A transparent, frameless, always-on-top, fullscreen overlay written in Rust
with [egui 0.36.1](https://docs.rs/egui/latest/egui/) / eframe 0.36.1.

**Within a user-defined overlay region, only a custom cursor appears.** The
system cursor is hidden there and replaced by a custom bitmap cursor that
follows the mouse. Outside the region the normal system cursor is used.

## Features

- Region-limited custom cursor: inside the region the system cursor is
  hidden and replaced by a custom bitmap cursor that follows the mouse;
  outside it the normal system cursor is used.
- **Click pass-through (Windows):** clicks go through the overlay to the apps
  below — only the cursor changes. While the settings panel / region editor is
  open, pass-through is disabled automatically so the panel stays usable.
- Optional OS-level bitmap cursor (`Context::set_cursor_image` →
  `winit::window::CustomCursor`) applied to the whole window (no click-through).
- The cursor bitmap comes from `assets/cursor.png` if present, otherwise a
  classic arrow cursor is generated in code (no asset required).
- Interactive region editor: drag the box to move it, drag the handles to
  resize it. Presets: Reset / Center / Fullscreen.

## Controls

| Key / action        | Effect                                    |
| ------------------- | ----------------------------------------- |
| `F1`                | Toggle the settings panel                 |
| `Esc`               | Quit                                      |
| Drag region box     | Move the overlay region (edit mode)       |
| Drag region handles | Resize the overlay region (edit mode)     |

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
If the file is absent, a default arrow cursor is generated automatically.

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
