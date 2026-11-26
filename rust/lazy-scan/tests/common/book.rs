// Lazy Scan's TestPages.Canvas and the book pages drawn on it: paper with noise, text blocks and the spine shadow, on a canvas of
// any size.
use super::pages::{Rng, clamp};
use lazy_image::Gray;
use lazy_scan::gray_image::GrayImage;

pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub p: Vec<u8>,
}

const PICKS: [u32; 10] = [
    0b00111, 0b01011, 0b01101, 0b01110, 0b10011, 0b10101, 0b10110, 0b11001, 0b11010, 0b11100,
];

impl Canvas {
    /// `paper` with scanner noise from `seed`.
    pub fn new(width: usize, height: usize, paper: i64, seed: u32) -> Self {
        let mut rng = Rng::new(seed);
        let p = (0..width * height)
            .map(|_| clamp(paper + rng.noise()))
            .collect();
        Canvas { width, height, p }
    }

    pub fn fill_rect(&mut self, x: i64, y: i64, w: i64, h: i64, value: u8) {
        let (y0, y1) = (y.max(0), (y + h).min(self.height as i64));
        let (x0, x1) = (x.max(0), (x + w).min(self.width as i64));
        if y0 >= y1 || x0 >= x1 {
            return;
        }
        for py in y0..y1 {
            for px in x0..x1 {
                self.p[py as usize * self.width + px as usize] = value;
            }
        }
    }

    /// `lines` lines of glyphs (words of 2 to 8 glyphs, 5 x 9 px in 7 px cells) from (x, y), each up to `max_width` wide, every
    /// `pitch` px.
    pub fn text_block(
        &mut self,
        x: i64,
        y: i64,
        max_width: i64,
        lines: i64,
        pitch: i64,
        ink: u8,
        rng: &mut Rng,
    ) {
        for line in 0..lines {
            let mut cx = x;
            let ly = y + line * pitch;
            loop {
                let glyphs = 2 + rng.below(7) as i64;
                if cx + glyphs * 7 > x + max_width {
                    break;
                }
                for _ in 0..glyphs {
                    let bits = PICKS[rng.below(10)];
                    if bits & 1 != 0 {
                        self.fill_rect(cx, ly, 1, 9, ink);
                    }
                    if bits & 2 != 0 {
                        self.fill_rect(cx + 4, ly, 1, 9, ink);
                    }
                    if bits & 4 != 0 {
                        self.fill_rect(cx, ly, 5, 1, ink);
                    }
                    if bits & 8 != 0 {
                        self.fill_rect(cx, ly + 4, 5, 1, ink);
                    }
                    if bits & 16 != 0 {
                        self.fill_rect(cx, ly + 8, 5, 1, ink);
                    }
                    cx += 7;
                }
                cx += 7;
            }
        }
    }

    /// The spine shadow on the columns around `x0`: each darkened by `depth_per_mille` x (r^2 - d^2) / r^2 at d columns from it
    /// (|d| < r).
    pub fn shadow_columns(&mut self, x0: i64, r: i64, depth_per_mille: i64) {
        let from = (x0 - r + 1).max(0);
        let to = (self.width as i64).min(x0 + r);
        if from >= to {
            return;
        }
        for x in from..to {
            let d = x - x0;
            let k = depth_per_mille * (r * r - d * d) / (r * r);
            for y in 0..self.height {
                let i = y * self.width + x as usize;
                self.p[i] = (self.p[i] as i64 * (1000 - k) / 1000) as u8;
            }
        }
    }

    /// A crease across the page: the rows around `y0` darkened as `shadow_columns` darkens columns.
    pub fn shadow_rows(&mut self, y0: i64, r: i64, depth_per_mille: i64) {
        let from = (y0 - r + 1).max(0);
        let to = (self.height as i64).min(y0 + r);
        if from >= to {
            return;
        }
        for y in from..to {
            let d = y - y0;
            let k = depth_per_mille * (r * r - d * d) / (r * r);
            for x in 0..self.width {
                let i = y as usize * self.width + x;
                self.p[i] = (self.p[i] as i64 * (1000 - k) / 1000) as u8;
            }
        }
    }

    /// A printed picture: the rectangle (x, y, w, h) filled with a mid-tone texture, 60 + (7x + 13y) mod 176.
    pub fn picture(&mut self, x: i64, y: i64, w: i64, h: i64) {
        let (y0, y1) = (y.max(0), (y + h).min(self.height as i64));
        let (x0, x1) = (x.max(0), (x + w).min(self.width as i64));
        if y0 >= y1 || x0 >= x1 {
            return;
        }
        for py in y0..y1 {
            for px in x0..x1 {
                self.p[py as usize * self.width + px as usize] =
                    (60 + (px * 7 + py * 13) % 176) as u8;
            }
        }
    }

    /// The spine shadow along the right edge, `width` px wide: the column d px from the edge darkened by
    /// k x (width - d)^2 / width^2 per mille, where k runs from `depth_top` on the first row to `depth_bottom` on the last
    /// (a book pressed harder at one end).
    pub fn spine_shadow(&mut self, width: i64, depth_top: i64, depth_bottom: i64) {
        let columns = width.min(self.width as i64);
        if columns <= 0 {
            return;
        }
        for y in 0..self.height as i64 {
            let depth = depth_top + (depth_bottom - depth_top) * y / 1.max(self.height as i64 - 1);
            for d in 0..columns {
                let k = depth * (width - d) * (width - d) / (width * width);
                let i = y as usize * self.width + self.width - 1 - d as usize;
                self.p[i] = (self.p[i] as i64 * (1000 - k) / 1000) as u8;
            }
        }
    }

    pub fn gray(&self) -> GrayImage {
        GrayImage::new(
            Gray {
                width: self.width,
                height: self.height,
                data: self.p.clone(),
            },
            100.0,
        )
    }
}

/// One page of a book as cut from its spread, at 100 dpi (5.5 x 8.5 in, 550 x 850), the spine along its right edge: 36 lines of
/// text every 0.2 in from y = 60, from 0.6 in from the outer edge to 0.3 in from the spine; with `picture`, a picture 1.3 x
/// 2.2 in at (400, 300), 0.2 in from the spine; with `tint`, a flat gray band (150) 1.2 in long and 1 in tall reaching the
/// spine at (430, 640); then the spine shadow (`spine_shadow`, `shadow_width` px wide) when `depth_top` or `depth_bottom` is
/// above 0. The same arguments with no depth give the page as it would be without the shadow.
pub fn book_page(
    depth_top: i64,
    depth_bottom: i64,
    shadow_width: i64,
    seed: u32,
    picture: bool,
    tint: bool,
) -> GrayImage {
    let mut c = Canvas::new(550, 850, 245, seed);
    c.text_block(60, 60, 460, 36, 20, 40, &mut Rng::new(seed ^ 0x5A5A_5A5A));
    if picture {
        c.picture(400, 300, 130, 220);
    }
    if tint {
        c.fill_rect(430, 640, 120, 100, 150);
    }
    if depth_top > 0 || depth_bottom > 0 {
        c.spine_shadow(shadow_width, depth_top, depth_bottom);
    }
    c.gray()
}

/// A book page with a shadow 0.8 in wide, 45 % deep at the top of the spine and 30 % at the bottom.
pub fn shadowed_book_page() -> GrayImage {
    book_page(450, 300, 80, 0xB00C_0020, false, false)
}

/// `shadowed_book_page` as it would be without the shadow.
pub fn unshadowed_book_page() -> GrayImage {
    book_page(0, 0, 80, 0xB00C_0020, false, false)
}

/// A book page with a picture near the spine, in a shadow 0.8 in wide and 40 % deep.
pub fn picture_book_page() -> GrayImage {
    book_page(400, 400, 80, 0xB00C_0021, true, false)
}

/// `picture_book_page` as it would be without the shadow.
pub fn unshadowed_picture_book_page() -> GrayImage {
    book_page(0, 0, 80, 0xB00C_0021, true, false)
}

/// A book page with a flat gray band reaching the spine, in a shadow 0.8 in wide and 40 % deep.
pub fn tint_book_page() -> GrayImage {
    book_page(400, 400, 80, 0xB00C_0022, false, true)
}
