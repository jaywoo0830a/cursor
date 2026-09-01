//! Shared configuration: constants and the default overlay region.

use eframe::egui::{pos2, vec2, Rect};

/// Default overlay region (in window points, origin = top-left).
pub fn default_region() -> Rect {
    Rect::from_min_size(pos2(300.0, 200.0), vec2(1320.0, 680.0))
}

/// Logical display size (in points) of the *painted* custom cursor.
pub const CURSOR_DISPLAY_SIZE: f32 = 20.0;

/// Optional PNG used as the cursor bitmap (straight / non-premultiplied RGBA).
pub const CURSOR_PNG: &str = "assets/cursor.png";

/// Hotspot (in bitmap pixels) used when loading `CURSOR_PNG`.
pub const PNG_HOTSPOT: [u16; 2] = [0, 0];

/// Size of the region resize handles.
pub const HANDLE_SIZE: f32 = 12.0;

/// Minimum region size (points).
pub const MIN_REGION: f32 = 24.0;
