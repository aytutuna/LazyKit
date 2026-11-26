// Lazy Scan's TestPages: synthetic US Letter pages at 100 dpi (850 x 1100 gray pixels), drawn with integer arithmetic and one
// xorshift32 generator consumed in the same order, so every platform draws the same pixels.
use lazy_image::{Gray, Rgba};
use lazy_scan::gray_image::GrayImage;

pub const WIDTH: usize = 850;
pub const HEIGHT: usize = 1100;

/// xorshift32 (13, 17, 5).
pub struct Rng(u32);

impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng(if seed == 0 { 1 } else { seed })
    }

    pub fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }

    /// 0 to n - 1.
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u32) as usize
    }

    /// Scanner noise: the sum of four uniform 0 to 6 values minus 12.
    pub fn noise(&mut self) -> i64 {
        let (a, b, c, d) = (self.below(7), self.below(7), self.below(7), self.below(7));
        (a + b + c + d) as i64 - 12
    }
}

pub fn clamp(v: i64) -> u8 {
    v.clamp(0, 255) as u8
}

pub fn page(pixels: Vec<u8>) -> GrayImage {
    GrayImage::new(
        Gray {
            width: WIDTH,
            height: HEIGHT,
            data: pixels,
        },
        100.0,
    )
}

pub fn fill(p: &mut [u8], x: i64, y: i64, w: i64, h: i64, value: u8) {
    let (y0, y1) = (y.max(0), (y + h).min(HEIGHT as i64));
    let (x0, x1) = (x.max(0), (x + w).min(WIDTH as i64));
    if y0 >= y1 || x0 >= x1 {
        return;
    }
    for py in y0..y1 {
        for px in x0..x1 {
            p[py as usize * WIDTH + px as usize] = value;
        }
    }
}

pub fn disc(p: &mut [u8], cx: i64, cy: i64, r: i64, value: u8) {
    for y in cy - r..=cy + r {
        for x in cx - r..=cx + r {
            if x < 0 || y < 0 || x >= WIDTH as i64 || y >= HEIGHT as i64 {
                continue;
            }
            if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r {
                p[y as usize * WIDTH + x as usize] = value;
            }
        }
    }
}

/// A straight pen stroke `thickness` px wide.
pub fn stroke(p: &mut [u8], x0: i64, y0: i64, x1: i64, y1: i64, thickness: i64, ink: u8) {
    let (dx, dy) = (x1 - x0, y1 - y0);
    let steps = dx.abs().max(dy.abs());
    for i in 0..=steps {
        let x = x0 + if steps == 0 { 0 } else { dx * i / steps };
        let y = y0 + if steps == 0 { 0 } else { dy * i / steps };
        fill(
            p,
            x - thickness / 2,
            y - thickness / 2,
            thickness,
            thickness,
            ink,
        );
    }
}

/// A line of text-like glyphs from (x, y), up to `max_width` wide: words of 2 to 8 glyphs, each glyph 5 x 9 px in a 7 px cell
/// made of three of five strokes (left, right, top, middle, bottom).
pub fn text_line(p: &mut [u8], x: i64, y: i64, max_width: i64, ink: u8, rng: &mut Rng) {
    const PICKS: [u32; 10] = [
        0b00111, 0b01011, 0b01101, 0b01110, 0b10011, 0b10101, 0b10110, 0b11001, 0b11010, 0b11100,
    ];
    let mut cx = x;
    loop {
        let glyphs = 2 + rng.below(7) as i64;
        if cx + glyphs * 7 > x + max_width {
            break;
        }
        for _ in 0..glyphs {
            let bits = PICKS[rng.below(10)];
            if bits & 1 != 0 {
                fill(p, cx, y, 1, 9, ink);
            }
            if bits & 2 != 0 {
                fill(p, cx + 4, y, 1, 9, ink);
            }
            if bits & 4 != 0 {
                fill(p, cx, y, 5, 1, ink);
            }
            if bits & 8 != 0 {
                fill(p, cx, y + 4, 5, 1, ink);
            }
            if bits & 16 != 0 {
                fill(p, cx, y + 8, 5, 1, ink);
            }
            cx += 7;
        }
        cx += 7;
    }
}

/// Paper of one value with noise, then `draw` on top.
pub fn noisy(seed: u32, paper: i64, draw: impl FnOnce(&mut Vec<u8>)) -> GrayImage {
    let mut rng = Rng::new(seed);
    let mut p: Vec<u8> = (0..WIDTH * HEIGHT)
        .map(|_| clamp(paper + rng.noise()))
        .collect();
    draw(&mut p);
    page(p)
}

pub fn clean_blank() -> GrayImage {
    GrayImage::filled(WIDTH, HEIGHT, 245)
}

/// Blank paper as a cheap scanner delivers it: noise, a gradient from 92 % to 98 % white, 200 specks of dust, a 0.15 in dark
/// shadow along the left edge and three punched holes.
pub fn noisy_blank() -> GrayImage {
    let mut rng = Rng::new(0xB1A2_C3D4);
    let mut p = vec![0u8; WIDTH * HEIGHT];
    for y in 0..HEIGHT {
        let paper = 255 * (920 + 60 * y as i64 / (HEIGHT as i64 - 1)) / 1000;
        for x in 0..WIDTH {
            p[y * WIDTH + x] = clamp(paper + rng.noise());
        }
    }
    for _ in 0..200 {
        let size = 1 + rng.below(2) as i64;
        let x = rng.below(WIDTH - 2) as i64;
        let y = rng.below(HEIGHT - 2) as i64;
        fill(&mut p, x, y, size, size, 60);
    }
    for y in 0..HEIGHT {
        for x in 0..15 {
            let i = y * WIDTH + x;
            p[i] = (p[i] as usize).min(40 + x * 12) as u8;
        }
    }
    for cy in [200, 550, 900] {
        disc(&mut p, 40, cy, 12, 30);
    }
    page(p)
}

/// Blank paper with the other side's text showing through: a page of text mirrored left to right, darkening the paper by 12 %.
pub fn show_through() -> GrayImage {
    let mut front = vec![0u8; WIDTH * HEIGHT];
    let mut text_rng = Rng::new(0x5EED_0003);
    for line in 0..30 {
        text_line(&mut front, 75, 110 + line * 28, 700, 1, &mut text_rng);
    }
    let mut rng = Rng::new(0x0C0F_FEE3);
    let mut p = vec![0u8; WIDTH * HEIGHT];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let paper = if front[y * WIDTH + (WIDTH - 1 - x)] == 1 {
                245 * 88 / 100
            } else {
                245
            };
            p[y * WIDTH + x] = clamp(paper + rng.noise());
        }
    }
    page(p)
}

pub fn one_text_line() -> GrayImage {
    noisy(0xD0D0_D0D4, 245, |p| {
        text_line(p, 100, 400, 650, 50, &mut Rng::new(0x7E7E_0004))
    })
}

pub fn signature() -> GrayImage {
    noisy(0xE5E5_E5E5, 245, |p| {
        let xs = [
            450, 470, 490, 505, 520, 540, 560, 580, 610, 640, 670, 700, 730, 760,
        ];
        let ys = [
            880, 840, 900, 850, 905, 845, 895, 860, 890, 865, 885, 870, 880, 872,
        ];
        for k in 0..xs.len() - 1 {
            stroke(p, xs[k], ys[k], xs[k + 1], ys[k + 1], 3, 40);
        }
    })
}

pub fn page_number() -> GrayImage {
    noisy(0xF0F0_F0F6, 245, |p| {
        fill(p, 415, 1000, 2, 12, 50);
        fill(p, 413, 1011, 6, 1, 50);
        fill(p, 424, 1000, 7, 2, 50);
        fill(p, 429, 1000, 2, 6, 50);
        fill(p, 424, 1005, 7, 2, 50);
        fill(p, 424, 1005, 2, 6, 50);
        fill(p, 424, 1010, 7, 2, 50);
    })
}

/// A mid-gray 3 x 4 in photo with some texture, on noisy paper.
pub fn photo() -> GrayImage {
    let mut image = noisy(0x9A9A_9A97, 245, |_| {});
    let mut rng = Rng::new(0x0970_7007);
    for y in 300..700 {
        for x in 275..575 {
            image.gray.data[y * WIDTH + x] =
                clamp(128 + ((x / 20 + y / 30) % 5 * 6) as i64 + rng.noise() * 2);
        }
    }
    image
}

/// Recycled paper (85 % gray, flecked) with a short paragraph.
pub fn tinted_with_text() -> GrayImage {
    let mut image = noisy(0x8181_818A, 217, |_| {});
    let mut rng = Rng::new(0xF1EC_0008);
    for _ in 0..400 {
        let (x, y) = (rng.below(WIDTH) as i64, rng.below(HEIGHT) as i64);
        fill(&mut image.gray.data, x, y, 1, 1, 170);
    }
    let mut text = Rng::new(0x7E7E_0008);
    for line in 0..5 {
        text_line(
            &mut image.gray.data,
            100,
            300 + line * 25,
            640,
            60,
            &mut text,
        );
    }
    image
}

pub fn all() -> Vec<(&'static str, GrayImage)> {
    vec![
        ("a clean blank", clean_blank()),
        ("b noisy blank", noisy_blank()),
        ("c show-through", show_through()),
        ("d one text line", one_text_line()),
        ("e signature", signature()),
        ("f page number", page_number()),
        ("g photo", photo()),
        ("h tinted with text", tinted_with_text()),
    ]
}

/// A gray page as an RGBA scan at `factor` times the resolution (each pixel repeated).
pub fn enlarged(page: &GrayImage, factor: usize) -> Rgba {
    let (w, h) = (page.width() * factor, page.height() * factor);
    let mut data = vec![255u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let v = page.gray.data[(y / factor) * page.width() + x / factor];
            let i = (y * w + x) * 4;
            data[i..i + 3].fill(v);
        }
    }
    Rgba::new(w, h, data)
}

/// FNV-1a of the pixels: one number per page, the same on every platform.
pub fn checksum(page: &GrayImage) -> u32 {
    super::fnv(&page.gray.data)
}
