//! Pixel buffers and the few image operations the scanner needs. Decoding and encoding stay with the platform.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

/// Opaque or translucent RGBA, four bytes a pixel, rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgba {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Rgba {
    pub fn new(width: usize, height: usize, data: Vec<u8>) -> Self {
        assert_eq!(
            data.len(),
            width * height * 4,
            "pixel data does not match the size"
        );
        Rgba {
            width,
            height,
            data,
        }
    }

    pub fn filled(width: usize, height: usize, r: u8, g: u8, b: u8) -> Self {
        let mut data = Vec::with_capacity(width * height * 4);
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

/// Box-filter minification: each destination pixel is the mean, rounded down, of the source pixels it covers.
pub fn minified(image: &Rgba, width: usize, height: usize) -> Rgba {
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
