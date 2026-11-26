// Lazy Scan's PhotoScenes (and the xorshift generator of its TestPages), drawn with the same integer arithmetic in the same
// order, so the photos are the apps' byte for byte: 360 x 480 phone photos of an A4-shaped sheet laid on a backdrop under a
// known perspective, each pixel the mean of 2 x 2 samples, then lit.
#![allow(dead_code)]
use lazy_image::Rgba;
use lazy_scan::{Point, Quad};

pub const WIDTH: i64 = 360;
pub const HEIGHT: i64 = 480;
pub const PAGE_WIDTH: i64 = 210;
pub const PAGE_HEIGHT: i64 = 297;

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

    pub fn below(&mut self, n: i64) -> i64 {
        (self.next() % n as u32) as i64
    }

    /// The sum of four uniform 0 to 6 values minus 12.
    pub fn noise(&mut self) -> i64 {
        let a = self.below(7);
        let b = self.below(7);
        let c = self.below(7);
        let d = self.below(7);
        a + b + c + d - 12
    }
}

fn clamp(v: i64) -> u8 {
    v.clamp(0, 255) as u8
}

/// The page-to-photo map x = (a u + b v + c) / (g u + h v + i), y = (d u + e v + f) / (g u + h v + i), in integers.
#[derive(Clone, Copy)]
pub struct IntHomography {
    a: i64,
    b: i64,
    c: i64,
    d: i64,
    e: i64,
    f: i64,
    g: i64,
    h: i64,
    i: i64,
}

const fn hom(
    a: i64,
    b: i64,
    c: i64,
    d: i64,
    e: i64,
    f: i64,
    g: i64,
    h: i64,
    i: i64,
) -> IntHomography {
    IntHomography {
        a,
        b,
        c,
        d,
        e,
        f,
        g,
        h,
        i,
    }
}

impl IntHomography {
    pub fn map(&self, u: f64, v: f64) -> Point {
        let w = self.g as f64 * u + self.h as f64 * v + self.i as f64;
        Point::new(
            (self.a as f64 * u + self.b as f64 * v + self.c as f64) / w,
            (self.d as f64 * u + self.e as f64 * v + self.f as f64) / w,
        )
    }

    /// The page's outline in the photo, named from the top left.
    pub fn corners(&self) -> Quad {
        let (w, h) = (PAGE_WIDTH as f64, PAGE_HEIGHT as f64);
        Quad::from_corners([
            self.map(0.0, 0.0),
            self.map(w, 0.0),
            self.map(w, h),
            self.map(0.0, h),
        ])
    }
}

const M: i64 = 1048576;
pub const WOOD_MAP: IntHomography = hom(
    1155332, -122914, 81788928, 177335, 930372, 75497472, 485, -598, M,
);
pub const DARK_MAP: IntHomography = hom(
    739319, -271102, 110100480, -28187, 541359, 94371840, -96, -1455, M,
);
pub const LIGHT_MAP: IntHomography = hom(
    1121991, 12060, 62914560, -46867, 1041969, 62914560, 272, -332, M,
);
pub const SIMILAR_MAP: IntHomography = hom(
    1095194, -44778, 73400320, -50732, 1055747, 83886080, -11, -267, M,
);
pub const CUT_OFF_MAP: IntHomography = hom(
    1228481, -108457, 157286400, 195764, 1313578, 41943040, -50, 298, M,
);
pub const UNDER_MAP: IntHomography = hom(
    806253, 18023, 31457280, -48774, 760787, 41943040, 39, -88, M,
);
pub const TOP_MAP: IntHomography = hom(
    1053725, -86609, 125829120, 102517, 953472, 157286400, 16, -160, M,
);
pub const LEFT_MAP: IntHomography = hom(
    741204, 21217, 20971520, -45071, 763036, 125829120, -46, 1, M,
);
pub const RIGHT_MAP: IntHomography =
    hom(748983, -21183, 199229440, 29959, 766131, 120586240, 0, 0, M);
pub const ROUND_TRIP_MAP: IntHomography = hom(
    1454408, -35269, 41943040, 82485, 1426885, 37748736, 50, 1, M,
);

/// An upright page 1.4 photo pixels per page pixel whose bottom edge lies `gap` px above the frame's bottom.
pub fn bottom_gap_map(gap: i64) -> IntHomography {
    hom(
        1468006,
        0,
        33 * M,
        0,
        1468006,
        (HEIGHT - gap) * M - 1468006 * PAGE_HEIGHT,
        0,
        0,
        M,
    )
}

/// The same page with its right edge `gap` px from the frame's right side.
pub fn right_gap_map(gap: i64) -> IntHomography {
    hom(
        1468006,
        0,
        (WIDTH - gap) * M - 1468006 * PAGE_WIDTH,
        0,
        1468006,
        30 * M,
        0,
        0,
        M,
    )
}

#[derive(Clone, Copy)]
pub enum Backdrop {
    Wood,
    Dark,
    Light,
    Similar,
    Plain,
}

/// A soft ellipse darkened by `depth` per mille, fading out to 2.5 times its size (in squared terms).
#[derive(Clone, Copy)]
pub struct Shadow {
    pub cx: i64,
    pub cy: i64,
    pub rx: i64,
    pub ry: i64,
    pub depth: i64,
}

/// A spot brightened toward white by up to `strength` per mille, fading quadratically to its radius.
#[derive(Clone, Copy)]
pub struct Glare {
    pub cx: i64,
    pub cy: i64,
    pub r: i64,
    pub strength: i64,
}

#[derive(Clone, Copy)]
pub struct Lighting {
    pub across_x: i64,
    pub down_y: i64,
    pub shadow: Option<Shadow>,
    pub glare: Option<Glare>,
}

const fn light(across_x: i64, down_y: i64) -> Lighting {
    Lighting {
        across_x,
        down_y,
        shadow: None,
        glare: None,
    }
}

pub struct Sheet {
    pub rgb: Vec<u8>,
    pub map: IntHomography,
}

fn page_sheet(seed: u32, map: IntHomography) -> Sheet {
    Sheet {
        rgb: page_raster(seed, 246, 244, 238),
        map,
    }
}

pub fn wood_scene() -> Rgba {
    scene(
        Backdrop::Wood,
        0x3D00_D001,
        light(180, 0),
        &[page_sheet(0x9A6E_0001, WOOD_MAP)],
    )
}

pub fn dark_scene() -> Rgba {
    let lit = Lighting {
        glare: Some(Glare {
            cx: 280,
            cy: 120,
            r: 70,
            strength: 900,
        }),
        ..light(0, 120)
    };
    scene(
        Backdrop::Dark,
        0x3D00_D002,
        lit,
        &[page_sheet(0x9A6E_0002, DARK_MAP)],
    )
}

pub fn light_scene() -> Rgba {
    let lit = Lighting {
        shadow: Some(Shadow {
            cx: 40,
            cy: 470,
            rx: 220,
            ry: 260,
            depth: 380,
        }),
        ..light(0, 0)
    };
    scene(
        Backdrop::Light,
        0x3D00_D003,
        lit,
        &[page_sheet(0x9A6E_0003, LIGHT_MAP)],
    )
}

pub fn similar_scene() -> Rgba {
    let lit = Lighting {
        glare: Some(Glare {
            cx: 180,
            cy: 250,
            r: 55,
            strength: 800,
        }),
        ..light(100, 60)
    };
    scene(
        Backdrop::Similar,
        0x3D00_D004,
        lit,
        &[page_sheet(0x9A6E_0004, SIMILAR_MAP)],
    )
}

pub fn no_page_scene() -> Rgba {
    let lit = Lighting {
        shadow: Some(Shadow {
            cx: 300,
            cy: 100,
            rx: 150,
            ry: 120,
            depth: 350,
        }),
        glare: Some(Glare {
            cx: 120,
            cy: 300,
            r: 60,
            strength: 900,
        }),
        ..light(150, 0)
    };
    scene(Backdrop::Wood, 0x3D00_D005, lit, &[])
}

pub fn plain_scene() -> Rgba {
    scene(Backdrop::Plain, 0x3D00_D006, light(0, 0), &[])
}

pub fn cut_off_scene() -> Rgba {
    scene(
        Backdrop::Dark,
        0x3D00_D007,
        light(0, 0),
        &[page_sheet(0x9A6E_0007, CUT_OFF_MAP)],
    )
}

/// A smaller sheet of cream paper with a larger white one lying over its lower right part.
pub fn overlap_scene() -> Rgba {
    scene(
        Backdrop::Dark,
        0x3D00_D008,
        light(0, 0),
        &[
            Sheet {
                rgb: page_raster(0x9A6E_0008, 232, 226, 210),
                map: UNDER_MAP,
            },
            page_sheet(0x9A6E_0009, TOP_MAP),
        ],
    )
}

/// Two sheets of the same size side by side.
pub fn two_pages_scene() -> Rgba {
    scene(
        Backdrop::Light,
        0x3D00_D00A,
        light(0, 0),
        &[
            page_sheet(0x9A6E_000A, LEFT_MAP),
            page_sheet(0x9A6E_000B, RIGHT_MAP),
        ],
    )
}

/// The coloured blocks of `block_raster` filling most of the frame, for the round trip.
pub fn round_trip_scene() -> Rgba {
    scene(
        Backdrop::Dark,
        0x3D00_D00C,
        light(0, 0),
        &[Sheet {
            rgb: block_raster(0x9A6E_000C),
            map: ROUND_TRIP_MAP,
        }],
    )
}

pub fn bottom_gap_scene(gap: i64) -> Rgba {
    scene(
        Backdrop::Wood,
        0x3D00_D00D,
        light(120, 0),
        &[page_sheet(0x9A6E_000D, bottom_gap_map(gap))],
    )
}

pub fn right_gap_scene(gap: i64) -> Rgba {
    scene(
        Backdrop::Wood,
        0x3D00_D00E,
        light(120, 0),
        &[page_sheet(0x9A6E_000E, right_gap_map(gap))],
    )
}

fn fill_rgb(p: &mut [u8], x: i64, y: i64, w: i64, h: i64, rgb: [u8; 3]) {
    let (y0, y1) = (y.max(0), (y + h).min(PAGE_HEIGHT));
    let (x0, x1) = (x.max(0), (x + w).min(PAGE_WIDTH));
    if y0 >= y1 || x0 >= x1 {
        return;
    }
    for py in y0..y1 {
        for px in x0..x1 {
            let i = ((py * PAGE_WIDTH + px) * 3) as usize;
            p[i..i + 3].copy_from_slice(&rgb);
        }
    }
}

/// A page: paper with noise, a dark blue-gray header bar, 15 lines of glyphs every 14 px and a picture block.
pub fn page_raster(seed: u32, paper_r: i64, paper_g: i64, paper_b: i64) -> Vec<u8> {
    let mut p = vec![0u8; (PAGE_WIDTH * PAGE_HEIGHT * 3) as usize];
    let mut rng = Rng::new(seed);
    for i in 0..(PAGE_WIDTH * PAGE_HEIGHT) as usize {
        let n = rng.noise();
        p[i * 3] = clamp(paper_r + n);
        p[i * 3 + 1] = clamp(paper_g + n);
        p[i * 3 + 2] = clamp(paper_b + n);
    }
    fill_rgb(&mut p, 20, 18, 170, 14, [52, 64, 92]);
    let mut text = Rng::new(seed ^ 0x5A5A_5A5A);
    let picks = [
        0b00111, 0b01011, 0b01101, 0b01110, 0b10011, 0b10101, 0b10110, 0b11001, 0b11010, 0b11100,
    ];
    let ink = [40, 40, 46];
    for line in 0..15 {
        let y = 46 + line * 14;
        let max_width = if line % 5 == 4 { 90 } else { 170 };
        let mut cx = 20;
        loop {
            let glyphs = 2 + text.below(7);
            if cx + glyphs * 7 > 20 + max_width {
                break;
            }
            for _ in 0..glyphs {
                let bits = picks[text.below(10) as usize];
                if bits & 1 != 0 {
                    fill_rgb(&mut p, cx, y, 1, 9, ink);
                }
                if bits & 2 != 0 {
                    fill_rgb(&mut p, cx + 4, y, 1, 9, ink);
                }
                if bits & 4 != 0 {
                    fill_rgb(&mut p, cx, y, 5, 1, ink);
                }
                if bits & 8 != 0 {
                    fill_rgb(&mut p, cx, y + 4, 5, 1, ink);
                }
                if bits & 16 != 0 {
                    fill_rgb(&mut p, cx, y + 8, 5, 1, ink);
                }
                cx += 7;
            }
            cx += 7;
        }
    }
    for y in 262..286 {
        for x in 120..190 {
            let i = ((y * PAGE_WIDTH + x) * 3) as usize;
            let v = 70 + (x * 7 + y * 13) % 120;
            p[i] = v as u8;
            p[i + 1] = (v * 9 / 10) as u8;
            p[i + 2] = (v * 7 / 10) as u8;
        }
    }
    p
}

/// A sheet of 10 x 10 px blocks in random colours (each channel 40 to 239), for the round trip.
pub fn block_raster(seed: u32) -> Vec<u8> {
    let mut p = vec![0u8; (PAGE_WIDTH * PAGE_HEIGHT * 3) as usize];
    let mut rng = Rng::new(seed);
    let mut by = 0;
    while by < PAGE_HEIGHT {
        let mut bx = 0;
        while bx < PAGE_WIDTH {
            let r = 40 + rng.below(200);
            let g = 40 + rng.below(200);
            let b = 40 + rng.below(200);
            fill_rgb(&mut p, bx, by, 10, 10, [r as u8, g as u8, b as u8]);
            bx += 10;
        }
        by += 10;
    }
    p
}

/// The sheet's pixels as an RGBA image, to compare a corrected page with.
pub fn sheet_image(rgb: &[u8]) -> Rgba {
    let mut data = vec![255u8; (PAGE_WIDTH * PAGE_HEIGHT * 4) as usize];
    for i in 0..(PAGE_WIDTH * PAGE_HEIGHT) as usize {
        data[i * 4..i * 4 + 3].copy_from_slice(&rgb[i * 3..i * 3 + 3]);
    }
    Rgba::new(PAGE_WIDTH as usize, PAGE_HEIGHT as usize, data)
}

fn adjugate(m: &IntHomography) -> [i64; 9] {
    [
        m.e * m.i - m.f * m.h,
        m.c * m.h - m.b * m.i,
        m.b * m.f - m.c * m.e,
        m.f * m.g - m.d * m.i,
        m.a * m.i - m.c * m.g,
        m.c * m.d - m.a * m.f,
        m.d * m.h - m.e * m.g,
        m.b * m.g - m.a * m.h,
        m.a * m.e - m.b * m.d,
    ]
}

/// The photo: for every pixel the backdrop (one noise draw each), the topmost sheet at each of its 2 x 2 sample points or the
/// backdrop, averaged (halves up), then dimmed by the light and brightened by the glare.
pub fn scene(backdrop: Backdrop, seed: u32, light: Lighting, sheets: &[Sheet]) -> Rgba {
    let mut rng = Rng::new(seed);
    let adj: Vec<[i64; 9]> = sheets.iter().map(|s| adjugate(&s.map)).collect();
    let mut pixels = vec![0u8; (WIDTH * HEIGHT * 4) as usize];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let back = backdrop_color(backdrop, x, y, &mut rng);
            let (mut sr, mut sg, mut sb) = (0, 0, 0);
            for sample in 0..4 {
                let qx = 4 * x + if sample & 1 == 0 { 1 } else { 3 };
                let qy = 4 * y + if sample >> 1 == 0 { 1 } else { 3 };
                let [mut cr, mut cg, mut cb] = back;
                for (s, sheet) in sheets.iter().enumerate() {
                    let k = &adj[s];
                    let mut u = k[0] * qx + k[1] * qy + k[2] * 4;
                    let mut v = k[3] * qx + k[4] * qy + k[5] * 4;
                    let mut w = k[6] * qx + k[7] * qy + k[8] * 4;
                    if w < 0 {
                        u = -u;
                        v = -v;
                        w = -w;
                    }
                    if w == 0 || u < 0 || v < 0 || u >= PAGE_WIDTH * w || v >= PAGE_HEIGHT * w {
                        continue;
                    }
                    let i = (((v / w) * PAGE_WIDTH + u / w) * 3) as usize;
                    cr = sheet.rgb[i] as i64;
                    cg = sheet.rgb[i + 1] as i64;
                    cb = sheet.rgb[i + 2] as i64;
                }
                sr += cr;
                sg += cg;
                sb += cb;
            }
            let (mut r, mut g, mut b) = ((sr + 2) / 4, (sg + 2) / 4, (sb + 2) / 4);
            let mut dim = 1000 - light.across_x * x / WIDTH - light.down_y * y / HEIGHT;
            if let Some(sh) = light.shadow {
                let (dx, dy) = (x - sh.cx, y - sh.cy);
                let (rx2, ry2) = (sh.rx * sh.rx, sh.ry * sh.ry);
                let t = (dx * dx * ry2 + dy * dy * rx2) * 1000 / (rx2 * ry2);
                if t <= 1000 {
                    dim -= sh.depth;
                } else if t < 2500 {
                    dim -= sh.depth * (2500 - t) / 1500;
                }
            }
            r = r * dim / 1000;
            g = g * dim / 1000;
            b = b * dim / 1000;
            if let Some(gl) = light.glare {
                let (dx, dy) = (x - gl.cx, y - gl.cy);
                let t = (dx * dx + dy * dy) * 1000 / (gl.r * gl.r);
                if t < 1000 {
                    let k = gl.strength * (1000 - t) / 1000;
                    r += (255 - r) * k / 1000;
                    g += (255 - g) * k / 1000;
                    b += (255 - b) * k / 1000;
                }
            }
            let o = ((y * WIDTH + x) * 4) as usize;
            pixels[o] = clamp(r);
            pixels[o + 1] = clamp(g);
            pixels[o + 2] = clamp(b);
            pixels[o + 3] = 255;
        }
    }
    Rgba::new(WIDTH as usize, HEIGHT as usize, pixels)
}

fn backdrop_color(backdrop: Backdrop, x: i64, y: i64, rng: &mut Rng) -> [i64; 3] {
    let n = rng.noise();
    let (r, g, b) = match backdrop {
        Backdrop::Wood => {
            // grain bands warped across the frame, and the seams between planks every 150 px
            let k = (x + 3 * y / 7) % 240;
            let tri = if k < 120 { 120 - k } else { k - 120 };
            let phase = 2 * y + tri / 2;
            let band = phase / 9 % 7;
            let mut shade = match band {
                0 => -26,
                1 => -12,
                3 => -6,
                _ => 0,
            };
            if (x + 75) % 150 < 2 {
                shade -= 40;
            }
            (
                150 + shade + 2 * n,
                104 + shade * 3 / 4 + 2 * n,
                64 + shade / 2 + 2 * n,
            )
        }
        Backdrop::Dark => (38 + 2 * n, 40 + 2 * n, 46 + 2 * n),
        Backdrop::Light => (196 + n, 198 + n, 200 + n),
        Backdrop::Similar => (228 + n, 222 + n, 210 + n),
        Backdrop::Plain => (128 + n, 128 + n, 128 + n),
    };
    [clamp(r) as i64, clamp(g) as i64, clamp(b) as i64]
}

/// FNV-1a of the RGBA bytes.
pub fn checksum(image: &Rgba) -> u32 {
    super::fnv(&image.data)
}
