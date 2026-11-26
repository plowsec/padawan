//! App icon, drawn in code: no asset file, no image decoder.
//!
//! A dark tile, the margin rule in the accent, text lines right of it, one note left of it.
//! Shapes are rounded rectangles in tile units (0..1), filled through their signed distance.

const TILE: u32 = 0x1c1d1f;
const EDGE: u32 = 0x3a3d41;
const TEXT: u32 = 0xc9c7c1;
const RULE: u32 = 0xe0a850;

/// `(x0, y0, x1, y1, radius, colour)`, painted in order.
const SHAPES: [(f32, f32, f32, f32, f32, u32); 8] = [
    (0.0, 0.0, 1.0, 1.0, 0.22, EDGE),
    (0.012, 0.012, 0.988, 0.988, 0.208, TILE),
    (0.305, 0.15, 0.345, 0.85, 0.02, RULE),
    (0.115, 0.375, 0.235, 0.445, 0.035, RULE),
    (0.43, 0.215, 0.83, 0.285, 0.035, TEXT),
    (0.43, 0.375, 0.73, 0.445, 0.035, TEXT),
    (0.43, 0.535, 0.86, 0.605, 0.035, TEXT),
    (0.43, 0.695, 0.65, 0.765, 0.035, TEXT),
];

/// Largest side `rgba` draws.
pub const MAX_SIZE: u32 = 1024;

/// `size × size` RGBA8, straight alpha. `pad`: transparent border as a fraction of `size`
/// (macOS dock icons want about 0.1, Linux and Windows about 0.03). `size` is clamped to 1..=[`MAX_SIZE`].
pub fn rgba(size: u32, pad: f32) -> Vec<u8> {
    let size = size.clamp(1, MAX_SIZE);
    let n = size as f32;
    let pad = pad.clamp(0.0, 0.3) * n;
    let tile = n - 2.0 * pad;
    let mut out = vec![0u8; (size * size * 4) as usize];
    for (i, px) in out.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = (i as u32 % size) as f32 + 0.5;
        let y = (i as u32 / size) as f32 + 0.5;
        // Straight-alpha "over", in float.
        let (mut r, mut g, mut b, mut a) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for (x0, y0, x1, y1, rad, color) in SHAPES {
            let (cx, cy) = (pad + (x0 + x1) * 0.5 * tile, pad + (y0 + y1) * 0.5 * tile);
            // Bars stay at least one pixel thick at 16 px.
            let (hx, hy) = (((x1 - x0) * 0.5 * tile).max(0.5), ((y1 - y0) * 0.5 * tile).max(0.5));
            let rad = (rad * tile).min(hx).min(hy);
            let (qx, qy) = ((x - cx).abs() - (hx - rad), (y - cy).abs() - (hy - rad));
            let dist = qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - rad;
            let cover = (0.5 - dist).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            let c = [(color >> 16) as u8, (color >> 8) as u8, color as u8].map(|v| f32::from(v) / 255.0);
            let na = cover + a * (1.0 - cover);
            r = (c[0] * cover + r * a * (1.0 - cover)) / na;
            g = (c[1] * cover + g * a * (1.0 - cover)) / na;
            b = (c[2] * cover + b * a * (1.0 - cover)) / na;
            a = na;
        }
        *px = [r, g, b, a].map(|v| (v * 255.0).round() as u8);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(px: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    }

    #[test]
    fn sizes_and_corners() {
        for size in [16, 32, 48, 256] {
            let px = rgba(size, 0.0);
            assert_eq!(px.len(), (size * size * 4) as usize);
            assert_eq!(at(&px, size, 0, 0)[3], 0, "corner is transparent at {size}");
            assert_eq!(at(&px, size, size / 2, size - 1 - size / 16)[3], 255, "bottom edge is opaque at {size}");
        }
        assert_eq!(rgba(0, 0.0).len(), 4);
        assert_eq!(rgba(u32::MAX, 0.0).len(), (MAX_SIZE * MAX_SIZE * 4) as usize);
    }

    #[test]
    fn rule_is_accent_and_padding_is_empty() {
        let px = rgba(256, 0.0);
        let rule = at(&px, 256, 83, 128);
        assert_eq!(rule, [0xe0, 0xa8, 0x50, 255]);
        assert_eq!(at(&px, 256, 200, 128), [(TILE >> 16) as u8, (TILE >> 8) as u8, TILE as u8, 255]);
        let padded = rgba(256, 0.1);
        assert_eq!(at(&padded, 256, 128, 10)[3], 0);
        assert_eq!(at(&padded, 256, 128, 128)[3], 255);
    }
}
