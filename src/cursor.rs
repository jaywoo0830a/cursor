//! Pure-Rust cursor bitmap generation (no image dependencies).
//!
//! The custom cursor is rendered natively: we generate a reticle (ring +
//! center dot) as straight RGBA, build a real Windows `HCURSOR` from it and
//! `SetCursor` it whenever the overlay owns the hit-testing. The OS then
//! draws the cursor — no webview/JS involved, so it is smooth, pen-safe
//! (the window owns `WM_SETCURSOR`) and visible over DirectComposition.

/// Generate a `size x size` straight-RGBA reticle cursor.
///
/// Returns `(rgba, [hot_x, hot_y])` with the hotspot at the center.
pub fn make_cursor_bitmap(size: u32) -> (Vec<u8>, [u16; 2]) {
    let s = size as f64;
    let c = (s - 1.0) / 2.0;
    let r_out = s * 0.38; // outer ring radius
    let r_in = s * 0.24; // inner ring radius
    let dot = s * 0.08; // center dot radius
    let edge = 1.2; // anti-alias edge width (px)

    let mut px = vec![0u8; (size as usize) * (size as usize) * 4];
    for y in 0..size as usize {
        for x in 0..size as usize {
            let dx = x as f64 - c;
            let dy = y as f64 - c;
            let d = (dx * dx + dy * dy).sqrt();
            let i = (y * size as usize + x) * 4;

            // Ring band r_in..r_out with soft (anti-aliased) edges.
            let outer = 1.0 - smoothstep(r_out - edge, r_out, d);
            let inner = smoothstep(r_in - edge, r_in, d);
            let ring = outer * inner;

            // Dark outline just inside the outer edge for contrast.
            let outline = (1.0 - smoothstep(r_out - edge * 1.7, r_out - edge * 0.5, d)) * ring;

            // Center dot.
            let dot_a = 1.0 - smoothstep(dot - edge * 0.5, dot + edge * 0.5, d);

            let ring_a = (ring * 0.98).clamp(0.0, 1.0);
            let a = ring_a.max(dot_a);
            if a <= 0.0 {
                continue;
            }

            // Color: white ring, dark outline + dark center dot.
            let dark = outline.clamp(0.0, 1.0).max(dot_a.clamp(0.0, 1.0));
            let r = 255.0 + (20.0 - 255.0) * dark;
            let g = 255.0 + (24.0 - 255.0) * dark;
            let b = 255.0 + (32.0 - 255.0) * dark;

            px[i] = r.round() as u8;
            px[i + 1] = g.round() as u8;
            px[i + 2] = b.round() as u8;
            px[i + 3] = (a * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
    (px, [(size / 2) as u16, (size / 2) as u16])
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
