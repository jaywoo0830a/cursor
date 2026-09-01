//! Custom cursor bitmap: loading from `assets/cursor.png` or generating the
//! default precision reticle.

use std::path::Path;
use std::sync::Arc;

use crate::config::{CURSOR_PNG, PNG_HOTSPOT};

/// Straight (non-premultiplied) RGBA pixels, `size[0] * size[1] * 4` bytes.
pub struct CursorBitmap {
    pub rgba: Arc<[u8]>,
    pub size: [u16; 2],
    pub hotspot: [u16; 2],
}

/// Try to load `assets/cursor.png`; fall back to a generated circular cursor.
pub fn load_cursor_bitmap() -> CursorBitmap {
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

/// Rasterize a modern, small precision-reticle cursor (32x32, anti-aliased):
/// a thin white ring with dark outlines and a small dark center dot, hotspot
/// at the center. Reads well on both light and dark backgrounds.
pub fn make_default_cursor() -> CursorBitmap {
    const S: usize = 32;
    const CX: f32 = (S as f32 - 1.0) * 0.5; // 15.5
    const CY: f32 = (S as f32 - 1.0) * 0.5;
    const AA: f32 = 1.0;

    // Radii (in px) of the reticle design:
    const R_DOT: f32 = 2.2; // dark center dot (aiming point)
    const R_RING_IN: f32 = 8.0; // white ring (inner)
    const R_RING_OUT: f32 = 9.8; // white ring (outer)
    const R_OUT_OUT: f32 = 10.8; // dark outline (outer side of the ring)

    // Coverage of the annulus [a, b] with 1px anti-aliased edges.
    let band = |d: f32, a: f32, b: f32| -> f32 {
        let inner = ((d - a) / AA + 0.5).clamp(0.0, 1.0);
        let outer = ((b - d) / AA + 0.5).clamp(0.0, 1.0);
        inner.min(outer)
    };

    let mut rgba = vec![0u8; S * S * 4];
    for y in 0..S {
        for x in 0..S {
            let d = ((x as f32 - CX).powi(2) + (y as f32 - CY).powi(2)).sqrt();
            // Center dot is a filled circle (full alpha in the middle).
            let dot = ((R_DOT - d) / AA + 0.5).clamp(0.0, 1.0);
            let ring = band(d, R_RING_IN, R_RING_OUT);
            let out = band(d, R_RING_OUT, R_OUT_OUT);

            let dark = dot.max(out);
            let white = ring;
            let a = dark.max(white);
            if a > 0.0 {
                let v = (20.0 * dark + 255.0 * white) / a; // dark dot/outline, white ring
                let i = (y * S + x) * 4;
                rgba[i..i + 4].copy_from_slice(&[
                    v.round() as u8,
                    v.round() as u8,
                    v.round() as u8,
                    (a * 255.0).round() as u8,
                ]);
            }
        }
    }

    CursorBitmap {
        rgba: Arc::from(rgba),
        size: [S as u16, S as u16],
        hotspot: [S as u16 / 2, S as u16 / 2],
    }
}
