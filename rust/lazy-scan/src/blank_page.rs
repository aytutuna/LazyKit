use crate::background::BackgroundMap;
use crate::gray_image::{ANALYSIS_DPI, GrayImage};
use crate::mask_ops;
use lazy_image::Rgba;

/// How much may be on a page for it to count as blank.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sensitivity {
    Low,
    #[default]
    Medium,
    High,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub sensitivity: Sensitivity,
    /// The band along every edge that is not looked at: edge shadows and staples live there. Punched holes reach past it and
    /// are dealt with by HOLE_BAND_INCHES. Pages whose short side is under SMALL_PAGE_INCHES use SMALL_PAGE_MARGIN_FRACTION of
    /// it instead.
    pub margin_inches: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            sensitivity: Sensitivity::Medium,
            margin_inches: MARGIN_INCHES,
        }
    }
}

/// What the blank-page check found on a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlankPageResult {
    /// Share of the area looked at covered by marks larger than dust (0 to 1).
    pub ink_coverage: f64,
    /// Marks larger than dust.
    pub ink_components: usize,
    /// A photo, a dark block or a dark band covers part of the page: such a page is never blank.
    pub has_large_dark_area: bool,
    /// The page counts as blank at the sensitivity it was checked with.
    pub is_blank: bool,
}

impl BlankPageResult {
    /// Whether the page counts as blank at `sensitivity`.
    pub fn is_blank_at(&self, sensitivity: Sensitivity) -> bool {
        !self.has_large_dark_area && self.ink_coverage < threshold(sensitivity)
    }

    fn not_blank() -> Self {
        BlankPageResult {
            ink_coverage: 0.0,
            ink_components: 0,
            has_large_dark_area: false,
            is_blank: false,
        }
    }
}

/// Edge band left out on normal pages, in inches.
pub const MARGIN_INCHES: f64 = 0.3;
/// Punched holes sit between MARGIN_INCHES and this distance from the edge. Inside it, dark marks too solid to be writing are
/// not ink.
pub const HOLE_BAND_INCHES: f64 = 0.7;
/// A mark counts as a punched hole when a square of this radius fits inside it, in inches.
pub const HOLE_RADIUS_INCHES: f64 = 0.04;
/// Pages with a shorter side than this (cards, receipts) use SMALL_PAGE_MARGIN_FRACTION.
pub const SMALL_PAGE_INCHES: f64 = 2.5;
/// Edge band of small pages, as a share of their short side.
pub const SMALL_PAGE_MARGIN_FRACTION: f64 = 0.15;
/// Side of a background tile, in inches (25 pixels at the analysis resolution).
pub const TILE_INCHES: f64 = 0.25;
/// A tile's background is this percentile of its pixels.
pub const BACKGROUND_PERCENTILE: f64 = 0.9;
/// Tiles darker than the page's median tile by more than this (of 255) are a dark area, not paper.
pub const DARK_AREA_DROP: i32 = 64;
/// A page with more than this share of dark-area tiles is never blank.
pub const DARK_TILE_FRACTION: f64 = 0.01;
/// A pixel is ink when darker than (1 - this) of the paper around it.
pub const INK_CONTRAST: f64 = 0.30;
/// A pixel is also ink when its chroma is more than this above the page's median chroma.
pub const INK_CHROMA: usize = 32;
/// Marks smaller than this many pixels at the analysis resolution are dust.
pub const MIN_MARK_PIXELS: f64 = 6.0;

/// The largest ink coverage a blank page may have: Low 0.008 %, Medium 0.024 %, High 0.08 %. A lone page number (about 0.01 %)
/// is blank at Medium; one line of text or a signature (about 0.12 % and more) is never blank.
pub fn threshold(sensitivity: Sensitivity) -> f64 {
    match sensitivity {
        Sensitivity::Low => 0.00008,
        Sensitivity::Medium => 0.00024,
        Sensitivity::High => 0.0008,
    }
}

fn rounded(v: f64) -> i64 {
    (v + 0.5).floor() as i64
}

/// Checks `page`, scanned at `dpi`: converted to gray at the analysis resolution first, with its chroma at the same resolution.
pub fn analyze_rgba(page: &Rgba, dpi: u32, options: &Options) -> BlankPageResult {
    analyze(
        &GrayImage::from_rgba(page, dpi),
        Some(&GrayImage::chroma(page, dpi)),
        options,
    )
}

/// Checks a gray page, in steps: the edges are left out; the paper's brightness is mapped in tiles (90th percentile), each
/// tile raised to the brightest of its neighbours and to no less than the page's median minus DARK_AREA_DROP, so a photo or a
/// dark block is never taken for paper (a page with more than DARK_TILE_FRACTION such tiles is never blank); a pixel is ink
/// when darker than (1 - INK_CONTRAST) of the paper there, or, when `chroma` is known, more than INK_CHROMA above the page's
/// median chroma; solid dark marks within HOLE_BAND_INCHES of the edge are punched holes, not ink; marks (8-connected) below
/// MIN_MARK_PIXELS are dust. Pixel sizes follow the image's dpi. A page whose area inside the margins is smaller than one
/// tile is not blank: it is too small to tell.
pub fn analyze(gray: &GrayImage, chroma: Option<&GrayImage>, options: &Options) -> BlankPageResult {
    let dpi = if gray.dpi > 0.0 {
        gray.dpi
    } else {
        ANALYSIS_DPI as f64
    };
    let k = dpi / ANALYSIS_DPI as f64;
    let short_side = gray.width().min(gray.height());
    let margin_pixels = if (short_side as f64) / dpi < SMALL_PAGE_INCHES {
        SMALL_PAGE_MARGIN_FRACTION * short_side as f64
    } else {
        options.margin_inches.max(0.0) * dpi
    };
    let margin = (rounded(margin_pixels) as usize).min(short_side.saturating_sub(1) / 2);
    let width = gray.width() - 2 * margin;
    let height = gray.height() - 2 * margin;
    let tile_size = (rounded(TILE_INCHES * dpi) as usize).max(4);
    if width < tile_size || height < tile_size {
        return BlankPageResult::not_blank();
    }
    let Some(raw) = BackgroundMap::estimate_in(
        &gray.gray,
        margin,
        margin,
        width,
        height,
        tile_size,
        BACKGROUND_PERCENTILE,
    ) else {
        return BlankPageResult::not_blank();
    };

    let floor = (raw.median_tile() as i32 - DARK_AREA_DROP).max(0);
    let dark_tiles = raw.tiles.iter().filter(|&&t| (t as i32) < floor).count();
    let has_large_dark_area = dark_tiles as f64 > DARK_TILE_FRACTION * raw.tiles.len() as f64;
    let background = raw.max_smoothed().clamped_below(floor as u8);

    let mut ink = background.ink_mask(&gray.gray, INK_CONTRAST);
    if short_side as f64 / dpi >= SMALL_PAGE_INCHES && options.margin_inches > 0.0 {
        let band = rounded(HOLE_BAND_INCHES * dpi) - margin as i64;
        drop_holes(&mut ink, width, height, band, dpi);
    }
    if let Some(chroma) = chroma {
        add_color(&mut ink, chroma, margin, margin, width, height);
    }

    let min_mark = (rounded(MIN_MARK_PIXELS * k * k) as usize).max(1);
    let (mut ink_pixels, mut marks) = (0usize, 0usize);
    for area in mask_ops::component_areas(&ink, background.width(), background.height(), true) {
        if area >= min_mark {
            ink_pixels += area;
            marks += 1;
        }
    }
    let coverage = ink_pixels as f64 / (background.width() * background.height()) as f64;
    BlankPageResult {
        ink_coverage: coverage,
        ink_components: marks,
        has_large_dark_area,
        is_blank: !has_large_dark_area && coverage < threshold(options.sensitivity),
    }
}

// Clears the ink within `band` pixels of the edges of the looked-at area that belongs to a mark a square of
// HOLE_RADIUS_INCHES fits in: a punched hole shows the scanner backing, writing is thinner.
fn drop_holes(ink: &mut [u8], width: usize, height: usize, band: i64, dpi: f64) {
    let r = (rounded(HOLE_RADIUS_INCHES * dpi) as usize).max(1);
    let in_band = |x: usize, y: usize| {
        let (x, y, w, h) = (x as i64, y as i64, width as i64, height as i64);
        y < band || y >= h - band || x < band || x >= w - band
    };
    let mut count = 0usize;
    for y in 0..height {
        for x in 0..width {
            if ink[y * width + x] == 1 && in_band(x, y) {
                count += 1;
            }
        }
    }
    if count < (2 * r + 1) * (2 * r + 1) {
        return;
    }
    let eroded = mask_ops::erode(ink, width, height, r, true);
    let solid = mask_ops::dilate(&eroded, width, height, r);
    for y in 0..height {
        for x in 0..width {
            if solid[y * width + x] == 1 && in_band(x, y) {
                ink[y * width + x] = 0;
            }
        }
    }
}

// Sets the ink where the chroma of the area at (x, y) is more than INK_CHROMA above the area median chroma, so paper that is
// itself tinted stays paper.
fn add_color(ink: &mut [u8], chroma: &GrayImage, x: usize, y: usize, width: usize, height: usize) {
    let cw = chroma.width();
    let mut histogram = [0usize; 256];
    for ay in 0..height {
        let row = (y + ay) * cw + x;
        for &v in &chroma.gray.data[row..row + width] {
            histogram[v as usize] += 1;
        }
    }
    let mut median = 0;
    let mut seen = histogram[0];
    while seen * 2 < width * height {
        median += 1;
        seen += histogram[median];
    }
    let limit = median + INK_CHROMA;
    for ay in 0..height {
        let row = (y + ay) * cw + x;
        for ax in 0..width {
            if chroma.gray.data[row + ax] as usize > limit {
                ink[ay * width + ax] = 1;
            }
        }
    }
}
