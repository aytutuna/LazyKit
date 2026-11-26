//! Pixel buffers and the few image operations the scanner needs. Decoding and encoding stay with the platform.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

/// Opaque or translucent RGBA, four bytes a pixel, rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// Bytes in a `width` x `height` RGBA buffer; panics where the product does not fit usize (wasm32 wraps at 4 GiB).
fn rgba_len(width: usize, height: usize) -> usize {
    width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(4))
        .expect("image size is too large")
}

impl Rgba {
    pub fn new(width: usize, height: usize, data: Vec<u8>) -> Self {
        assert_eq!(
            data.len(),
            rgba_len(width, height),
            "pixel data does not match the size"
        );
        Rgba {
            width,
            height,
            data,
        }
    }

    pub fn filled(width: usize, height: usize, r: u8, g: u8, b: u8) -> Self {
        let mut data = Vec::with_capacity(rgba_len(width, height));
        for _ in 0..width * height {
            data.extend_from_slice(&[r, g, b, 255]);
        }
        Rgba {
            width,
            height,
            data,
        }
    }
}

impl Rgba {
    /// Mirrored left to right.
    pub fn mirrored(&self) -> Rgba {
        let mut data = vec![0u8; self.data.len()];
        for (row, out) in self
            .data
            .chunks_exact((self.width * 4).max(4))
            .zip(data.chunks_exact_mut((self.width * 4).max(4)))
        {
            for x in 0..self.width {
                out[(self.width - 1 - x) * 4..(self.width - x) * 4]
                    .copy_from_slice(&row[x * 4..x * 4 + 4]);
            }
        }
        Rgba {
            width: self.width,
            height: self.height,
            data,
        }
    }

    /// Turned clockwise on screen by `quarter_turns` quarter turns, any integer.
    pub fn rotated_quarter_turns(&self, quarter_turns: i32) -> Rgba {
        let turns = quarter_turns.rem_euclid(4);
        if turns == 0 {
            return self.clone();
        }
        let (w, h) = (self.width, self.height);
        let (nw, nh) = if turns % 2 == 1 { (h, w) } else { (w, h) };
        let mut data = vec![0u8; rgba_len(nw, nh)];
        for dy in 0..nh {
            for dx in 0..nw {
                // each destination pixel looks up its source pixel
                let (sx, sy) = match turns {
                    1 => (dy, h - 1 - dx),
                    2 => (w - 1 - dx, h - 1 - dy),
                    _ => (w - 1 - dy, dx),
                };
                let (si, di) = ((sy * w + sx) * 4, (dy * nw + dx) * 4);
                data[di..di + 4].copy_from_slice(&self.data[si..si + 4]);
            }
        }
        Rgba {
            width: nw,
            height: nh,
            data,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gray {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

/// Luma as integers, weights 0.299, 0.587 and 0.114 times 1000, rounded half up.
pub fn luma(image: &Rgba) -> Gray {
    let data = image
        .data
        .as_chunks::<4>()
        .0
        .iter()
        .map(|p| {
            ((299 * p[0] as u32 + 587 * p[1] as u32 + 114 * p[2] as u32 + 500) / 1000).min(255)
                as u8
        })
        .collect();
    Gray {
        width: image.width,
        height: image.height,
        data,
    }
}

/// Box-filter minification: each destination pixel is the mean, rounded down, of the source pixels it covers. An image
/// without pixels, or a size of nothing, gives an image of no pixels.
pub fn minified(image: &Rgba, width: usize, height: usize) -> Rgba {
    if image.width == 0 || image.height == 0 || width == 0 || height == 0 {
        return Rgba::new(0, 0, Vec::new());
    }
    let mut out = vec![0u8; width * height * 4];
    let x_ratio = image.width as f64 / width as f64;
    let y_ratio = image.height as f64 / height as f64;
    for dy in 0..height {
        let sy0 = (dy as f64 * y_ratio).floor() as usize;
        let sy1 = (sy0 + 1).max(
            image
                .height
                .min(((dy + 1) as f64 * y_ratio).ceil() as usize),
        );
        for dx in 0..width {
            let sx0 = (dx as f64 * x_ratio).floor() as usize;
            let sx1 = (sx0 + 1).max(image.width.min(((dx + 1) as f64 * x_ratio).ceil() as usize));
            let (mut r, mut g, mut b, mut count) = (0u64, 0u64, 0u64, 0u64);
            for sy in sy0..sy1 {
                for sx in sx0..sx1 {
                    let o = (sy * image.width + sx) * 4;
                    r += image.data[o] as u64;
                    g += image.data[o + 1] as u64;
                    b += image.data[o + 2] as u64;
                    count += 1;
                }
            }
            let o = (dy * width + dx) * 4;
            out[o] = (r / count) as u8;
            out[o + 1] = (g / count) as u8;
            out[o + 2] = (b / count) as u8;
            out[o + 3] = 255;
        }
    }
    Rgba {
        width,
        height,
        data: out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[should_panic(expected = "image size is too large")]
    fn a_size_that_overflows_is_refused() {
        Rgba::new(usize::MAX / 2, 4, Vec::new());
    }

    #[test]
    fn turning_moves_the_top_left_pixel_to_the_top_right() {
        let mut data = Vec::new();
        for v in 0..6u8 {
            data.extend_from_slice(&[v, 0, 0, 255]);
        }
        let image = Rgba::new(3, 2, data);
        let first = |i: &Rgba| i.data.iter().step_by(4).copied().collect::<Vec<_>>();
        let cw = image.rotated_quarter_turns(1);
        assert_eq!((cw.width, cw.height), (2, 3));
        assert_eq!(first(&cw), [3, 0, 4, 1, 5, 2]);
        assert_eq!(first(&image.rotated_quarter_turns(2)), [5, 4, 3, 2, 1, 0]);
        assert_eq!(first(&image.rotated_quarter_turns(-1)), [2, 5, 1, 4, 0, 3]);
        assert_eq!(image.rotated_quarter_turns(4), image);
    }

    #[test]
    fn mirroring_swaps_each_row_end_to_end() {
        let mut data = Vec::new();
        for v in 0..6u8 {
            data.extend_from_slice(&[v, 0, 0, 255]);
        }
        let image = Rgba::new(3, 2, data);
        let reds = |i: &Rgba| i.data.iter().step_by(4).copied().collect::<Vec<_>>();
        assert_eq!(reds(&image.mirrored()), [2, 1, 0, 5, 4, 3]);
        assert_eq!(image.mirrored().mirrored(), image);
        let empty = Rgba::new(0, 0, vec![]);
        assert_eq!(empty.mirrored(), empty);
    }

    #[test]
    fn luma_rounds_half_up() {
        let image = Rgba::new(2, 1, vec![255, 255, 255, 255, 0, 0, 1, 255]);
        assert_eq!(luma(&image).data, vec![255, 0]);
        // 0.299 * 2 = 0.598 rounds to 1, 0.114 * 4 = 0.456 to 0
        assert_eq!(luma(&Rgba::new(1, 1, vec![2, 0, 0, 255])).data, vec![1]);
        assert_eq!(luma(&Rgba::new(1, 1, vec![0, 0, 4, 255])).data, vec![0]);
    }

    #[test]
    fn minifying_takes_the_floor_of_the_mean() {
        let mut data = Vec::new();
        for v in [10u8, 11, 20, 21] {
            data.extend_from_slice(&[v, v, v, 255]);
        }
        let out = minified(&Rgba::new(4, 1, data), 2, 1);
        assert_eq!(out.data, vec![10, 10, 10, 255, 20, 20, 20, 255]);
    }

    #[test]
    fn minifying_nothing_or_to_nothing_gives_no_pixels() {
        let empty = Rgba::new(0, 0, vec![]);
        let one = Rgba::new(1, 1, vec![9, 8, 7, 255]);
        assert_eq!(minified(&empty, 4, 4), empty);
        assert_eq!(minified(&one, 0, 3), empty);
        assert_eq!(minified(&one, 3, 0), empty);
        assert_eq!(minified(&one, 1, 1), one);
    }

    #[test]
    fn minifying_by_a_fraction_covers_the_edge_pixels() {
        // 3 wide to 2: destination 0 covers source 0..2 (ceil of 1.5), destination 1 covers 1..3
        let mut data = Vec::new();
        for v in [0u8, 90, 180] {
            data.extend_from_slice(&[v, v, v, 255]);
        }
        let out = minified(&Rgba::new(3, 1, data), 2, 1);
        assert_eq!(out.data[0], 45);
        assert_eq!(out.data[4], 135);
    }
}
