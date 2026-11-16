//! The grain of paper mode: a still texture of fibers and specks laid over the
//! whole window, text included, so that type looks printed into the page.

use std::sync::{Arc, LazyLock};

use gpui::{Bounds, Corners, Pixels, RenderImage, Window, point, px, size};
use image::{Frame, RgbaImage};

/// The texture repeats every this many device pixels; large enough that the
/// eye finds no pattern.
const TILE: u32 = 256;
/// The strongest speck. Above a few percent the paper looks dirty.
const MAX_ALPHA: u8 = 14;
/// Warm grey fibers on light paper and pale ones on dark paper, as BGRA wants them.
const DARK_FIBER: [u8; 3] = [0x3c, 0x48, 0x56];
const LIGHT_FIBER: [u8; 3] = [0xd8, 0xe8, 0xf4];

static GRAIN: LazyLock<Arc<RenderImage>> = LazyLock::new(|| Arc::new(RenderImage::new([Frame::new(grain(DARK_FIBER))])));
static NIGHT_GRAIN: LazyLock<Arc<RenderImage>> =
    LazyLock::new(|| Arc::new(RenderImage::new([Frame::new(grain(LIGHT_FIBER))])));

/// The same noise on every start; a texture that changes looks like a fault.
fn hash(x: u32, y: u32) -> u32 {
    let mut value = x.wrapping_mul(0x8da6_b343) ^ y.wrapping_mul(0xd816_3841) ^ 0x2545_f491;
    value ^= value >> 15;
    value = value.wrapping_mul(0x2c1b_3c6d);
    value ^= value >> 12;
    value = value.wrapping_mul(0x297a_2d39);
    value ^ (value >> 15)
}

fn unit(x: u32, y: u32) -> f32 {
    (hash(x, y) & 0xffff) as f32 / 65535.
}

/// Specks on every pixel plus faint, mostly horizontal fibers, wrapping at the
/// edges so that the tiles meet without a seam.
pub fn grain(fiber: [u8; 3]) -> RgbaImage {
    let mut alpha = vec![0f32; (TILE * TILE) as usize];
    for y in 0..TILE {
        for x in 0..TILE {
            // Cubed, so most pixels stay clean and a few are dark.
            alpha[(y * TILE + x) as usize] = unit(x, y).powi(3) * 0.55;
        }
    }
    for fiber in 0..90 {
        let mut x = unit(fiber, 1) * TILE as f32;
        let mut y = unit(fiber, 2) * TILE as f32;
        let angle = (unit(fiber, 3) - 0.5) * 0.9;
        let length = 12 + (unit(fiber, 4) * 40.) as u32;
        let strength = 0.25 + unit(fiber, 5) * 0.35;
        for _ in 0..length {
            let index = ((y as u32 % TILE) * TILE + x as u32 % TILE) as usize;
            alpha[index] = alpha[index].max(strength);
            x = (x + angle.cos() + TILE as f32) % TILE as f32;
            y = (y + angle.sin() + TILE as f32) % TILE as f32;
        }
    }
    RgbaImage::from_fn(TILE, TILE, |x, y| {
        let [b, g, r] = fiber;
        image::Rgba([b, g, r, (alpha[(y * TILE + x) as usize] * MAX_ALPHA as f32).round() as u8])
    })
}

/// Lays the grain over `bounds`, one device pixel of texture per screen pixel.
pub fn paint(bounds: Bounds<Pixels>, window: &mut Window) {
    let tile = px(TILE as f32 / window.scale_factor());
    let grain = if crate::theme::is_light(crate::theme::palette().bg) { &GRAIN } else { &NIGHT_GRAIN };
    let columns = (bounds.size.width / tile).ceil() as usize;
    let rows = (bounds.size.height / tile).ceil() as usize;
    for row in 0..rows {
        for column in 0..columns {
            let origin = point(bounds.origin.x + tile * column as f32, bounds.origin.y + tile * row as f32);
            let tile_bounds = Bounds::new(origin, size(tile, tile));
            window.paint_image(bounds, tile_bounds, Corners::default(), (*grain).clone(), 0, false).ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grain_stays_faint_and_the_same() {
        let first = grain(DARK_FIBER);
        assert_eq!(first, grain(DARK_FIBER));
        let alphas: Vec<u8> = first.pixels().map(|pixel| pixel.0[3]).collect();
        assert!(alphas.iter().all(|alpha| *alpha <= MAX_ALPHA));
        let average = alphas.iter().map(|alpha| *alpha as f32).sum::<f32>() / alphas.len() as f32;
        assert!(average > 0.5 && average < 4., "average alpha {average}");
    }
}
