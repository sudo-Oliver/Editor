//! Dark mode for rendered pages. Paper becomes the theme background and ink
//! the theme text, while colored strokes keep their hue. Pixels of embedded
//! images stay as they are, so photos and plots do not turn into negatives.

/// The colors a page is drawn in, as `0xRRGGBB`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Recolor {
    pub paper: u32,
    pub ink: u32,
}

/// A rectangle in bitmap pixels, end exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

/// Recolors a BGRA bitmap in place, except inside `keep`.
pub fn recolor(bgra: &mut [u8], width: u32, colors: Recolor, keep: &[PixelRect]) {
    let width = width as usize;
    if width == 0 {
        return;
    }
    // One entry per luminance: white maps to paper, black to ink.
    let channel = |color: u32, shift: u32| ((color >> shift) & 0xff) as i32;
    let table: Vec<[i32; 3]> = (0..256)
        .map(|light| {
            let mix = |shift| channel(colors.ink, shift) + (channel(colors.paper, shift) - channel(colors.ink, shift)) * light / 255;
            [mix(0), mix(8), mix(16)]
        })
        .collect();
    let paper = [colors.paper as u8, (colors.paper >> 8) as u8, (colors.paper >> 16) as u8];
    let mut kept: Vec<(usize, usize)> = Vec::new();
    for (row, line) in bgra.chunks_exact_mut(width * 4).enumerate() {
        let pixels = line.as_chunks_mut::<4>().0;
        kept.clear();
        kept.extend(
            keep.iter()
                .filter(|rect| (rect.top as usize..rect.bottom as usize).contains(&row))
                .map(|rect| (rect.left as usize, (rect.right as usize).min(width))),
        );
        kept.sort_unstable();
        // The runs between the images of this row.
        let mut start = 0;
        for end in kept.iter().map(|(left, _)| *left).chain([width]) {
            for pixel in &mut pixels[start.min(end)..end] {
                // Most of a page is paper.
                if pixel[..3] == [255, 255, 255] {
                    pixel[..3].copy_from_slice(&paper);
                    continue;
                }
                let (blue, green, red) = (pixel[0] as i32, pixel[1] as i32, pixel[2] as i32);
                let light = (54 * red + 183 * green + 19 * blue) >> 8;
                let base = table[light as usize];
                // The distance to grey is the hue, which stays.
                pixel[0] = (base[0] + blue - light).clamp(0, 255) as u8;
                pixel[1] = (base[1] + green - light).clamp(0, 255) as u8;
                pixel[2] = (base[2] + red - light).clamp(0, 255) as u8;
            }
            if let Some(right) = kept.iter().filter(|(left, _)| *left == end).map(|(_, right)| *right).max() {
                start = start.max(right);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DARK: Recolor = Recolor { paper: 0x101010, ink: 0xcccccc };

    fn pixel(red: u8, green: u8, blue: u8) -> [u8; 4] {
        [blue, green, red, 255]
    }

    #[test]
    fn paper_and_ink_take_the_theme_colors() {
        let mut bgra = [pixel(255, 255, 255), pixel(0, 0, 0)].concat();
        recolor(&mut bgra, 2, DARK, &[]);
        assert_eq!(bgra, [pixel(0x10, 0x10, 0x10), pixel(0xcc, 0xcc, 0xcc)].concat());
    }

    #[test]
    fn colored_ink_keeps_its_hue() {
        let mut bgra = pixel(200, 30, 30).to_vec();
        recolor(&mut bgra, 1, DARK, &[]);
        let (blue, green, red) = (bgra[0], bgra[1], bgra[2]);
        assert!(red > green + 100 && green == blue, "{bgra:?}");
    }

    #[test]
    fn overlapping_images_stay_untouched() {
        let frog = pixel(40, 160, 60);
        let mut bgra = [frog; 5].concat();
        let rect = |left, right| PixelRect { left, top: 0, right, bottom: 1 };
        recolor(&mut bgra, 5, DARK, &[rect(1, 2), rect(1, 3), rect(2, 3)]);
        let kept: Vec<bool> = bgra.chunks(4).map(|pixel| pixel == frog).collect();
        assert_eq!(kept, [false, true, true, false, false]);
    }

    #[test]
    fn images_stay_untouched() {
        let frog = pixel(40, 160, 60);
        let mut bgra = [frog, frog, frog, frog].concat();
        recolor(&mut bgra, 2, DARK, &[PixelRect { left: 1, top: 0, right: 2, bottom: 1 }]);
        assert_ne!(bgra[0..4], frog);
        assert_eq!(bgra[4..8], frog);
        assert_ne!(bgra[8..12], frog);
        assert_ne!(bgra[12..16], frog);
    }
}
