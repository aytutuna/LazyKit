use crate::background::{BackgroundMap, percentile};
use crate::color::is_effectively_gray;
use lazy_image::{Gray, Rgba, luma};

// How a page taken by a camera is cleaned up: how bright the paper is everywhere is estimated in large tiles, each tile's
// paper a high percentile of its luma; tiles much darker than their neighbours (a picture) are filled in from them unless
// the dark region reaches the page's edge (a shadow); each pixel is divided by the paper there. The levels are then
// stretched from a black point to a white point. The arithmetic and its order follow Lazy Scan's Swift and C# copies,
// which the checksums in testdata/scan/enhancer.json pin; Rust does not fuse multiply and add on its own, so the same
// order gives the same bytes on every target.
const TILES_ALONG_LONG_SIDE: usize = 16;
const MIN_TILE: usize = 8;
const BACKGROUND_PERCENTILE: f64 = 0.9;
const DARK_TILE_SHARE: f64 = 0.7;
const MIN_BACKGROUND_SHARE: f64 = 0.4;
const MIN_BACKGROUND: i32 = 32;
const PAPER_SHARE: f64 = 0.9;
const MIN_GAIN: f64 = 0.87;
const MAX_GAIN: f64 = 1.15;
const WHITE_POINT: f64 = 0.9;
const BLACK_PERCENTILE: f64 = 0.005;
const BILEVEL_SHARE: f64 = 0.78;
pub const DEFAULT_THRESHOLD: f64 = 0.5;
const MAX_BLACK_POINT: f64 = 0.5;
const LEVELS: i32 = 1024;
const SAMPLE_STEP: usize = 2;
const PHOTO_CLIP: f64 = 0.005;
const MIN_PHOTO_RANGE: usize = 64;
pub const MAX_PHOTO_GAIN: f64 = 1.3;
const GROUND_PERCENTILE: f64 = 0.5;
const LIGHT_CONTRAST: f64 = 1.5;
const MIN_LIGHT_SHARE: f64 = 0.005;
const WHITEBOARD_SATURATION: f64 = 1.5;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnhanceMode {
    /// As `Color`, then gray when the page has no color left.
    Auto,
    /// The lighting evened out, the paper white, a slight tint taken out, the print stretched to black.
    Color,
    Gray,
    /// The gray page thresholded: every pixel 0 or 255.
    BlackAndWhite,
    /// A photo: its levels stretched as a whole, nothing evened out.
    Photo,
    /// As `Color` with the print left as dark as it is and its colors made stronger.
    Whiteboard,
}

/// A page cleaned up and the mode applied (`Auto` resolved to `Color` or `Gray`).
#[derive(Clone, Debug)]
pub struct Enhanced {
    pub image: Rgba,
    pub mode: EnhanceMode,
}

pub fn tile_size(width: usize, height: usize) -> usize {
    MIN_TILE.max(width.max(height) / TILES_ALONG_LONG_SIDE)
}

pub fn paper_map(gray: &Gray) -> BackgroundMap {
    let raw = BackgroundMap::estimate(
        gray,
        tile_size(gray.width, gray.height),
        BACKGROUND_PERCENTILE,
    )
    .unwrap();
    let filled = raw.raised_dark_tiles(DARK_TILE_SHARE);
    let floor = (MIN_BACKGROUND as u32)
        .max((filled.median_tile() as f64 * MIN_BACKGROUND_SHARE).floor() as u32);
    filled.clamped_below(floor.min(255) as u8)
}

/// Light print on a dark ground (a chalkboard, a slide): no division by the paper can make its ground white.
pub fn is_light_on_dark(gray: &Gray) -> bool {
    let (w, h) = (gray.width, gray.height);
    let Some(ground) = BackgroundMap::estimate(gray, tile_size(w, h), GROUND_PERCENTILE) else {
        return false;
    };
    let highest = ground.max_smoothed();
    let lowest = ground.min_smoothed();
    let (mut light, mut dark, mut samples) = (0u64, 0u64, 0u64);
    for y in (0..h).step_by(SAMPLE_STEP) {
        for x in (0..w).step_by(SAMPLE_STEP) {
            let l = gray.data[y * w + x] as f64;
            let above = highest.value_at(x, y).max(MIN_BACKGROUND as f64);
            let below = lowest.value_at(x, y).max(MIN_BACKGROUND as f64);
            if l >= above * LIGHT_CONTRAST {
                light += 1;
            } else if l * LIGHT_CONTRAST <= below {
                dark += 1;
            }
            samples += 1;
        }
    }
    light > dark && light as f64 >= MIN_LIGHT_SHARE * samples as f64
}

// the paper's white balance: over the sampled pixels at least PAPER_SHARE of the paper there, each channel's gain is the luma's sum over its own
fn white_balance(image: &Rgba, gray: &Gray, paper: &BackgroundMap) -> [f64; 3] {
    let (w, h) = (image.width, image.height);
    let (mut r, mut g, mut b, mut l) = (0i64, 0i64, 0i64, 0i64);
    for y in (0..h).step_by(SAMPLE_STEP) {
        for x in (0..w).step_by(SAMPLE_STEP) {
            let p = y * w + x;
            let lum = gray.data[p] as i64;
            if lum as f64 >= paper.value_at(x, y) * PAPER_SHARE {
                r += image.data[p * 4] as i64;
                g += image.data[p * 4 + 1] as i64;
                b += image.data[p * 4 + 2] as i64;
                l += lum;
            }
        }
    }
    if l == 0 || r == 0 || g == 0 || b == 0 {
        return [1.0; 3];
    }
    let gain = |channel: i64| (l as f64 / channel as f64).clamp(MIN_GAIN, MAX_GAIN);
    [gain(r), gain(g), gain(b)]
}

// a value against the paper's background, as a level 0..=LEVELS (at most the paper's own)
fn level(value: f64, background: f64) -> usize {
    ((value / background).min(1.0) * LEVELS as f64 + 0.5).floor() as usize
}

fn black_point(gray: &Gray, paper: &BackgroundMap) -> f64 {
    let mut histogram = vec![0i32; LEVELS as usize + 1];
    let mut samples = 0usize;
    for y in (0..gray.height).step_by(SAMPLE_STEP) {
        for x in (0..gray.width).step_by(SAMPLE_STEP) {
            histogram[level(gray.data[y * gray.width + x] as f64, paper.value_at(x, y))] += 1;
            samples += 1;
        }
    }
    (percentile(&histogram, samples, BLACK_PERCENTILE) as f64 / LEVELS as f64).min(MAX_BLACK_POINT)
}

// the share of the paper black and white makes black at a setting of 0..=1: BILEVEL_SHARE at the default, none at 0, all at 1
fn black_share(threshold: f64) -> f64 {
    let t = threshold.clamp(0.0, 1.0);
    if t <= DEFAULT_THRESHOLD {
        return BILEVEL_SHARE * t / DEFAULT_THRESHOLD;
    }
    BILEVEL_SHARE + (1.0 - BILEVEL_SHARE) * (t - DEFAULT_THRESHOLD) / (1.0 - DEFAULT_THRESHOLD)
}

// the levels 0..=LEVELS stretched linearly from lo (black) to hi (white)
fn stretch(lo: f64, hi: f64) -> Vec<u8> {
    (0..=LEVELS)
        .map(|k| {
            let v = ((k as f64 / LEVELS as f64 - lo) / (hi - lo)).clamp(0.0, 1.0);
            (v * 255.0 + 0.5).floor() as u8
        })
        .collect()
}

fn saturated(value: u8, around: i32) -> u8 {
    (around as f64 + ((value as i32 - around) as f64 * WHITEBOARD_SATURATION + 0.5).floor())
        .clamp(0.0, 255.0) as u8
}

// the levels that become 0 and 255 for a photo whose percentiles are lo and hi: those two, unless the gain would pass
// MAX_PHOTO_GAIN, then the range is widened, each end moving out in proportion to the room it has before 0 or 255
fn photo_range(lo: usize, hi: usize, capped: bool) -> (f64, f64) {
    let range = (hi - lo) as f64;
    let span = if capped {
        range.max(255.0 / MAX_PHOTO_GAIN)
    } else {
        range
    };
    let extra = span - range;
    if extra <= 0.0 {
        return (lo as f64, hi as f64);
    }
    let low = lo as f64 - extra * lo as f64 / (255.0 - range);
    (low, low + span)
}

/// A photo's levels stretched as a whole, the same for each channel; one whose 0.5 % and 99.5 % luma percentiles are less
/// than 64 apart is returned as it is. `capped` limits the gain to `MAX_PHOTO_GAIN`.
pub fn photo(image: &Rgba, capped: bool) -> Rgba {
    let gray = luma(image);
    let mut histogram = [0i32; 256];
    for &v in &gray.data {
        histogram[v as usize] += 1;
    }
    let count = image.width * image.height;
    let lo = percentile(&histogram, count, PHOTO_CLIP);
    let hi = percentile(&histogram, count, 1.0 - PHOTO_CLIP);
    if hi - lo < MIN_PHOTO_RANGE {
        return image.clone();
    }
    let (low, high) = photo_range(lo, hi, capped);
    let span = high - low;
    let lut: Vec<u8> = (0..256)
        .map(|v| (((v as f64 - low) * 255.0 / span + 0.5).floor()).clamp(0.0, 255.0) as u8)
        .collect();
    let mut data = image.data.clone();
    for p in data.as_chunks_mut::<4>().0.iter_mut() {
        p[0] = lut[p[0] as usize];
        p[1] = lut[p[1] as usize];
        p[2] = lut[p[2] as usize];
    }
    Rgba {
        width: image.width,
        height: image.height,
        data,
    }
}

fn light_on_dark(image: &Rgba, mode: EnhanceMode, threshold: f64) -> Enhanced {
    let page = photo(image, false);
    if matches!(mode, EnhanceMode::Color | EnhanceMode::Whiteboard) {
        return Enhanced { image: page, mode };
    }
    if mode == EnhanceMode::Auto && !is_effectively_gray(&page.data, page.width, page.height) {
        return Enhanced {
            image: page,
            mode: EnhanceMode::Color,
        };
    }
    let gray = luma(&page);
    let cut = (threshold.clamp(0.0, 1.0) * 255.0 + 0.5).floor() as u8;
    let mut data = vec![255u8; page.width * page.height * 4];
    for (p, &l) in gray.data.iter().enumerate() {
        let v = if mode == EnhanceMode::BlackAndWhite {
            if l < cut { 0 } else { 255 }
        } else {
            l
        };
        data[p * 4..p * 4 + 3].fill(v);
    }
    let applied = if mode == EnhanceMode::Auto {
        EnhanceMode::Gray
    } else {
        mode
    };
    Enhanced {
        image: Rgba {
            width: page.width,
            height: page.height,
            data,
        },
        mode: applied,
    }
}

/// `image` cleaned up as `mode` says; black and white cuts at `threshold` (0 to 1, `DEFAULT_THRESHOLD` is the usual).
pub fn enhance(image: &Rgba, mode: EnhanceMode, threshold: f64) -> Enhanced {
    if mode == EnhanceMode::Photo {
        return Enhanced {
            image: photo(image, true),
            mode,
        };
    }
    let (w, h) = (image.width, image.height);
    let gray = luma(image);
    if is_light_on_dark(&gray) {
        return light_on_dark(image, mode, threshold);
    }
    let paper = paper_map(&gray);
    let lut = stretch(
        if mode == EnhanceMode::Whiteboard {
            0.0
        } else {
            black_point(&gray, &paper)
        },
        WHITE_POINT,
    );
    let color = matches!(
        mode,
        EnhanceMode::Auto | EnhanceMode::Color | EnhanceMode::Whiteboard
    );
    let balance = if color {
        white_balance(image, &gray, &paper)
    } else {
        [1.0; 3]
    };
    let black_below = (black_share(threshold) * LEVELS as f64 + 0.5).floor() as usize;
    let src = &image.data;
    let mut out = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let p = y * w + x;
            let i = p * 4;
            let background = paper.value_at(x, y);
            if color {
                out[i] = lut[level(src[i] as f64 * balance[0], background)];
                out[i + 1] = lut[level(src[i + 1] as f64 * balance[1], background)];
                out[i + 2] = lut[level(src[i + 2] as f64 * balance[2], background)];
                if mode == EnhanceMode::Whiteboard {
                    let l = (299 * out[i] as i32
                        + 587 * out[i + 1] as i32
                        + 114 * out[i + 2] as i32
                        + 500)
                        / 1000;
                    out[i] = saturated(out[i], l);
                    out[i + 1] = saturated(out[i + 1], l);
                    out[i + 2] = saturated(out[i + 2], l);
                }
            } else {
                let share = level(gray.data[p] as f64, background);
                let v = if mode == EnhanceMode::BlackAndWhite {
                    if share < black_below { 0 } else { 255 }
                } else {
                    lut[share]
                };
                out[i..i + 3].fill(v);
            }
            out[i + 3] = 255;
        }
    }
    if mode == EnhanceMode::Auto {
        if !is_effectively_gray(&out, w, h) {
            return Enhanced {
                image: Rgba {
                    width: w,
                    height: h,
                    data: out,
                },
                mode: EnhanceMode::Color,
            };
        }
        // no color left: the gray page, as the gray mode makes it
        for y in 0..h {
            for x in 0..w {
                let p = y * w + x;
                let v = lut[level(gray.data[p] as f64, paper.value_at(x, y))];
                out[p * 4..p * 4 + 3].fill(v);
            }
        }
        return Enhanced {
            image: Rgba {
                width: w,
                height: h,
                data: out,
            },
            mode: EnhanceMode::Gray,
        };
    }
    Enhanced {
        image: Rgba {
            width: w,
            height: h,
            data: out,
        },
        mode,
    }
}
