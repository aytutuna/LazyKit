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

// ---- crop and straighten ----

/// A turn by a small angle as exact fixed-point sine and cosine (x 65536), so every platform draws the same pixels without
/// trigonometry.
#[derive(Clone, Copy)]
pub struct Turn {
    pub sin: i64,
    pub cos: i64,
}

pub const STRAIGHT: Turn = Turn { sin: 0, cos: 65536 };
pub const PLUS_2: Turn = Turn {
    sin: 2287,
    cos: 65496,
};
pub const PLUS_2_POINT_5: Turn = Turn {
    sin: 2859,
    cos: 65474,
};
pub const MINUS_2_POINT_5: Turn = Turn {
    sin: -2859,
    cos: 65474,
};
pub const PLUS_3: Turn = Turn {
    sin: 3430,
    cos: 65446,
};
pub const MINUS_3: Turn = Turn {
    sin: -3430,
    cos: 65446,
};

/// The sheet of `sheet_on_backdrop`: 6 x 8 in.
pub const SHEET_WIDTH: usize = 600;
pub const SHEET_HEIGHT: usize = 800;

/// Division rounding down (toward minus infinity), for negative offsets too.
pub fn floor_div(a: i64, b: i64) -> i64 {
    a.div_euclid(b)
}

/// A page of text: 34 lines of glyphs 6.5 in long, every 0.28 in, on noisy paper.
pub fn text_page() -> GrayImage {
    noisy(0x7E57_AA6E, 245, |p| {
        let mut rng = Rng::new(0x7E57_AA6F);
        for line in 0..34 {
            text_line(p, 100, 100 + line * 28, 650, 40, &mut rng);
        }
    })
}

/// `page` with its content turned clockwise by `turn` about the centre (nearest pixel); what comes in from outside the page is
/// `fill`.
pub fn turned(page: &GrayImage, turn: Turn, fill: u8) -> GrayImage {
    let (w, h) = (page.width() as i64, page.height() as i64);
    let mut p = vec![0u8; (w * h) as usize];
    let unit = 65536;
    for y in 0..h {
        let dy = 2 * y + 1 - h;
        for x in 0..w {
            let dx = 2 * x + 1 - w;
            let sx = floor_div(dx * turn.cos + dy * turn.sin + w * unit, 2 * unit);
            let sy = floor_div(-dx * turn.sin + dy * turn.cos + h * unit, 2 * unit);
            p[(y * w + x) as usize] = if sx >= 0 && sx < w && sy >= 0 && sy < h {
                page.gray.data[(sy * w + sx) as usize]
            } else {
                fill
            };
        }
    }
    GrayImage::new(
        Gray {
            width: w as usize,
            height: h as usize,
            data: p,
        },
        page.dpi,
    )
}

fn sheet_text_line(p: &mut [u8], x: usize, y: usize, max_width: usize, ink: u8, rng: &mut Rng) {
    const PICKS: [u32; 10] = [
        0b00111, 0b01011, 0b01101, 0b01110, 0b10011, 0b10101, 0b10110, 0b11001, 0b11010, 0b11100,
    ];
    let mut bar = |bx: usize, by: usize, bw: usize, bh: usize| {
        for py in by..by + bh {
            for px in bx..bx + bw {
                p[py * SHEET_WIDTH + px] = ink;
            }
        }
    };
    let mut cx = x;
    loop {
        let glyphs = 2 + rng.below(7);
        if cx + glyphs * 7 > x + max_width {
            break;
        }
        for _ in 0..glyphs {
            let bits = PICKS[rng.below(10)];
            if bits & 1 != 0 {
                bar(cx, y, 1, 9);
            }
            if bits & 2 != 0 {
                bar(cx + 4, y, 1, 9);
            }
            if bits & 4 != 0 {
                bar(cx, y, 5, 1);
            }
            if bits & 8 != 0 {
                bar(cx, y + 4, 5, 1);
            }
            if bits & 16 != 0 {
                bar(cx, y + 8, 5, 1);
            }
            cx += 7;
        }
        cx += 7;
    }
}

/// A 6 x 8 in sheet (`paper` with noise and 24 lines of glyphs) lying at the centre of the 850 x 1100 scan on a noisy
/// `backdrop`, turned clockwise by `turn`, with a `shadow` px edge shadow (200) all around it when above 0.
pub fn sheet_on_backdrop(
    turn: Turn,
    backdrop: i64,
    paper: i64,
    shadow: i64,
    seed: u32,
) -> GrayImage {
    let (sw, sh) = (SHEET_WIDTH, SHEET_HEIGHT);
    let mut sheet = vec![0u8; sw * sh];
    let mut rng = Rng::new(seed);
    for v in sheet.iter_mut() {
        *v = clamp(paper + rng.noise());
    }
    let mut text = Rng::new(seed ^ 0x5A5A_5A5A);
    for line in 0..24 {
        sheet_text_line(&mut sheet, 60, 60 + line * 28, 480, 40, &mut text);
    }

    let mut p = vec![0u8; WIDTH * HEIGHT];
    let mut noise = Rng::new(seed.wrapping_add(1));
    let unit = 65536i64;
    let (half_w, half_h) = (sw as i64 * unit, sh as i64 * unit);
    let (outer_w, outer_h) = (
        (sw as i64 + 2 * shadow) * unit,
        (sh as i64 + 2 * shadow) * unit,
    );
    for y in 0..HEIGHT as i64 {
        let dy = 2 * y + 1 - HEIGHT as i64;
        for x in 0..WIDTH as i64 {
            let dx = 2 * x + 1 - WIDTH as i64;
            let u = dx * turn.cos + dy * turn.sin;
            let v = -dx * turn.sin + dy * turn.cos;
            let value = if u.abs() < half_w && v.abs() < half_h {
                let sx = floor_div(u + half_w, 2 * unit);
                let sy = floor_div(v + half_h, 2 * unit);
                sheet[sy as usize * sw + sx as usize] as i64
            } else if shadow > 0 && u.abs() < outer_w && v.abs() < outer_h {
                200 + noise.noise()
            } else {
                backdrop + noise.noise()
            };
            p[y as usize * WIDTH + x as usize] = clamp(value);
        }
    }
    page(p)
}

// ---- patch codes ----

/// A separator sheet drawn at the standard widths at 100 dpi (narrow bars and gaps 8 px, wide bars 20 px, value 20) on noisy
/// paper: `code` bars `length` px long, across the page from (275, 200) down, or with `vertical` down the page from
/// (200, 400) across; five lines of text from y 850; the whole page turned clockwise by `turn`.
pub fn patch_page(
    code: lazy_scan::patch_code::PatchCode,
    vertical: bool,
    turn: Turn,
    length: i64,
) -> GrayImage {
    let page = noisy(0x9A7C_0DE5, 245, |p| {
        let mut pos = 200;
        for wide in code.bars() {
            let thickness = if wide { 20 } else { 8 };
            if vertical {
                fill(p, pos, 400, thickness, length, 20);
            } else {
                fill(p, 275, pos, length, thickness, 20);
            }
            pos += thickness + 8;
        }
        let mut rng = Rng::new(0x9A7C_0DE6);
        for line in 0..5 {
            text_line(p, 100, 850 + line * 28, 650, 40, &mut rng);
        }
    });
    turned(&page, turn, 245)
}

/// A ruled table on noisy paper, long lines that are no patch code: a dark header band 24 px tall with a 2 px rule under it,
/// 20 more rules every 28 px, four column rules, and under the table ten stripes 6 px tall, 8 px apart.
pub fn ruled_table() -> GrayImage {
    noisy(0x7AB1_E000, 245, |p| {
        fill(p, 100, 150, 650, 24, 40);
        for row in 0..=20 {
            fill(p, 100, 174 + row * 28, 650, 2, 40);
        }
        for x in [100, 300, 500, 748] {
            fill(p, x, 150, 2, 24 + 20 * 28 + 2, 40);
        }
        for k in 0..10 {
            fill(p, 100, 820 + k * 14, 650, 6, 40);
        }
    })
}

/// The page turned half a turn (each pixel to the opposite corner).
pub fn half_turned(page: &GrayImage) -> GrayImage {
    let mut data = page.gray.data.clone();
    data.reverse();
    GrayImage::new(
        Gray {
            width: page.width(),
            height: page.height(),
            data,
        },
        page.dpi,
    )
}

/// The page turned a quarter turn clockwise.
pub fn quarter_turned(page: &GrayImage) -> GrayImage {
    let (w, h) = (page.width(), page.height());
    let mut p = vec![0u8; w * h];
    // the new page is h wide and w tall; its pixel (x, y) is the old (y, h - 1 - x)
    for y in 0..w {
        for x in 0..h {
            p[y * h + x] = page.gray.data[(h - 1 - x) * w + y];
        }
    }
    GrayImage::new(
        Gray {
            width: h,
            height: w,
            data: p,
        },
        page.dpi,
    )
}
