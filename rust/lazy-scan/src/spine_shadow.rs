#![allow(clippy::needless_range_loop)] // index loops kept as in the Swift and C# twins, so the three are easy to compare

use crate::background::percentile;
use crate::gray_image::GrayImage;
use crate::mask_ops;
use lazy_image::Rgba;

/// An edge of a page; the order is the clockwise order (`turned`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PageEdge {
    Left,
    Top,
    Right,
    Bottom,
}

impl PageEdge {
    fn index(self) -> i32 {
        match self {
            PageEdge::Left => 0,
            PageEdge::Top => 1,
            PageEdge::Right => 2,
            PageEdge::Bottom => 3,
        }
    }

    /// Where the edge lies once the page is turned `quarter_turns` clockwise.
    pub fn turned(self, quarter_turns: i32) -> PageEdge {
        match (self.index() + quarter_turns.rem_euclid(4)) % 4 {
            0 => PageEdge::Left,
            1 => PageEdge::Top,
            2 => PageEdge::Right,
            _ => PageEdge::Bottom,
        }
    }

    fn along_x(self) -> bool {
        matches!(self, PageEdge::Left | PageEdge::Right)
    }
}

/// What softening the spine's shadow did to a page.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpineShadowResult {
    /// The page's edge along the spine.
    pub edge: PageEdge,
    /// How much the shadow darkened the paper where it was deepest, 0 to 1 (0.3: to 70 %).
    pub depth: f64,
    /// How far from the edge the shadow reached.
    pub width_inches: f64,
    /// The most any pixel was brightened by (a factor, at most MAX_GAIN).
    pub max_gain: f64,
    /// The share of the band along the spine left as it was because it holds a picture.
    pub protected_fraction: f64,
}

/// The spine's shadow measured on a page's gray copy: the gain of every analysis pixel of the band along the edge, in the
/// band's own frame: row v runs along the edge (from the left or top end of the page), column u away from it (0 at the edge).
#[derive(Clone, Debug)]
pub struct SpineShadowModel {
    pub edge: PageEdge,
    /// Analysis pixels from the edge to the far side of the page (the page's width or height).
    pub extent: usize,
    /// Analysis pixels along the edge.
    pub length: usize,
    /// Analysis pixels from the edge the band reaches; every pixel beyond keeps its value.
    pub band: usize,
    /// The factor each band pixel is brightened by (1 or more): `length` rows of `band`.
    pub gains: Vec<f64>,
    /// 1 where the band holds a picture (left as it is), 0 elsewhere: `length` rows of `band`.
    pub pictures: Vec<u8>,
    pub result: SpineShadowResult,
}

/// A page with its spine's shadow softened, and what was done.
#[derive(Clone, Debug)]
pub struct SpineShadowCorrection {
    pub image: Rgba,
    pub result: SpineShadowResult,
}

/// The band along the edge the shadow is looked for in, at most this many inches wide...
pub const BAND_INCHES: f64 = 1.5;
/// ...and at most half the page.
pub const MAX_BAND_FRACTION: f64 = 0.5;
/// A page whose band would be narrower than this many inches is left alone.
pub const MIN_BAND_INCHES: f64 = 0.25;
/// The paper's own brightness is read on this many inches just past the band.
pub const REFERENCE_INCHES: f64 = 0.4;
/// The rows along the edge are measured in blocks this many inches long...
pub const BLOCK_INCHES: f64 = 0.3;
/// ...and at least this many pixels.
pub const MIN_BLOCK_ROWS: usize = 4;
/// The paper's brightness in a cell (3 columns of a block): this share of its pixels are darker.
pub const PAPER_PERCENTILE: f64 = 0.9;
/// A page whose paper is darker than this gray level is left alone.
pub const MIN_PAPER: i32 = 96;
/// A block whose paper differs from the page's by more than this share takes the page's.
pub const PAPER_TOLERANCE: f64 = 0.1;
/// A block shows the spine's shadow when its paper is darker near the edge (by MIN_DEPTH, within MAX_DEEPEST_INCHES) and back
/// to within this share of the paper over the last quarter of the band...
pub const FADE_TOLERANCE: f64 = 0.05;
/// ...and at least this share of the blocks along the edge, and MIN_SHADOWED_BLOCKS, show it (a book can cover only part of
/// the glass; the rest is the lid).
pub const MIN_SHADOWED_SHARE: f64 = 0.3;
/// The fewest blocks that must show the shadow.
pub const MIN_SHADOWED_BLOCKS: usize = 2;
/// A cell is shadowed paper when it is within this share of the paper's brightness of the shadow those blocks agree on (their
/// median)...
pub const CONSENSUS_TOLERANCE: f64 = 0.05;
/// ...plus this share of the shadow's depth there (shadows are deeper at one end of the spine than the other).
pub const CONSENSUS_SPREAD: f64 = 0.4;
/// The shadow is softened when it darkens the paper by at least this share...
pub const MIN_DEPTH: f64 = 0.04;
/// ...it is deepest within this many inches of the edge, and it has faded before the end of the band.
pub const MAX_DEEPEST_INCHES: f64 = 0.25;
/// Paper darkened by less than this share is left alone (noise, the last trace of the shadow).
pub const DEAD_ZONE: f64 = 0.02;
/// A shadow fades gradually: from a quarter of the way back to the paper to within DEAD_ZONE of it takes at least this many
/// inches (a tint or a bar printed along the spine comes back in a step).
pub const MIN_FADE_INCHES: f64 = 0.04;
/// No pixel is brightened more than this many times (the fold itself can be nearly black).
pub const MAX_GAIN: f64 = 2.5;
/// The gain eases to none over the last this many inches of the band.
pub const TAPER_INCHES: f64 = 0.2;
/// A pixel darker than this share of the shadowed paper there may be part of a picture...
pub const PICTURE_LEVEL: f64 = 0.85;
/// ...once thin strokes (text, lines) are taken away: the mask is opened with this many inches...
pub const OPEN_INCHES: f64 = 0.02;
/// ...the parts of a picture split by its light areas are joined across this many inches...
pub const CLOSE_INCHES: f64 = 0.05;
/// ...and a picture covers at least a square this many inches on a side; its box is left alone.
pub const MIN_PICTURE_INCHES: f64 = 0.3;
/// Beyond a picture's box, the gain fades in over twice this many inches.
pub const FEATHER_INCHES: f64 = 0.03;

/// `image` (scanned at `dpi`) with the shadow along `edge` softened, and what was done; `None` when there is no shadow to
/// soften (or the page is too small or dark).
pub fn soften(image: &Rgba, dpi: u32, edge: PageEdge) -> Option<SpineShadowCorrection> {
    let gray = GrayImage::from_rgba(image, dpi);
    let model = estimate(&gray, edge)?;
    Some(SpineShadowCorrection {
        image: apply(image, &model),
        result: model.result,
    })
}

// Inches as whole pixels at `dpi`, rounded, at least 1.
fn pixels(inches: f64, dpi: f64) -> usize {
    1.max((inches * dpi + 0.5) as usize)
}

fn sorted(values: &[f64]) -> Vec<f64> {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    v
}

/// The shadow along `edge` of `gray` (a page's gray copy, at most the analysis resolution), as the gain of every pixel of the
/// band along it; `None` when the paper there is not darkened by at least MIN_DEPTH, or the page is too small or too dark.
pub fn estimate(gray: &GrayImage, edge: PageEdge) -> Option<SpineShadowModel> {
    let dpi = gray.dpi;
    let along_x = edge.along_x();
    let extent = if along_x { gray.width() } else { gray.height() };
    let length = if along_x { gray.height() } else { gray.width() };
    let band = pixels(BAND_INCHES, dpi).min((extent as f64 * MAX_BAND_FRACTION) as usize);
    let reference = pixels(REFERENCE_INCHES, dpi).min(extent - band);
    if band < pixels(MIN_BAND_INCHES, dpi) || reference < 1 {
        return None;
    }
    let span = band + reference;
    let strip = oriented(gray, edge, span);
    let block_rows = MIN_BLOCK_ROWS.max(pixels(BLOCK_INCHES, dpi));
    let blocks = 1.max(length / block_rows);

    // the paper's brightness in every cell: a block of rows, three columns
    let mut level = vec![0f64; blocks * span];
    let mut histogram = [0i32; 256];
    for j in 0..blocks {
        let (r0, r1) = (j * length / blocks, (j + 1) * length / blocks);
        for u in 0..span {
            histogram.fill(0);
            let (c0, c1) = (u.saturating_sub(1), (span - 1).min(u + 1));
            for r in r0..r1 {
                let row = r * span;
                for c in c0..=c1 {
                    histogram[strip[row + c] as usize] += 1;
                }
            }
            level[j * span + u] =
                percentile(&histogram, (r1 - r0) * (c1 - c0 + 1), PAPER_PERCENTILE) as f64;
        }
    }

    // the paper of each block, past the shadow: the upper quartile of the reference stretch; a block whose reference is no
    // paper (a picture, the lid) takes the page's
    let mut paper = vec![0f64; blocks];
    for j in 0..blocks {
        let stretch = sorted(&level[j * span + band..j * span + span]);
        paper[j] = stretch[reference * 3 / 4];
    }
    let page_paper = sorted(&paper)[blocks / 2];
    if page_paper < MIN_PAPER as f64 {
        return None;
    }
    for p in paper.iter_mut() {
        if *p < page_paper * (1.0 - PAPER_TOLERANCE) || *p > page_paper * (1.0 + PAPER_TOLERANCE) {
            *p = page_paper;
        }
    }
    let paper = running_median(&paper, 2);

    // how much of the paper's light is left in each cell
    let mut share = vec![0f64; blocks * band];
    for j in 0..blocks {
        for u in 0..band {
            share[j * band + u] = (level[j * span + u] / paper[j]).min(1.0);
        }
    }
    // the blocks that show the spine's shadow: darker near the edge, paper again at the far side of the band (not a picture or
    // a tint running across it, not the lid beside the book); the shadow they agree on is their median
    let near = band.min(pixels(MAX_DEEPEST_INCHES, dpi));
    let fade_from = band - band / 4;
    let mut shadowed = Vec::new();
    for j in 0..blocks {
        let mut darkest = 1.0f64;
        for u in 0..near {
            darkest = darkest.min(share[j * band + u]);
        }
        let mut faded = true;
        for u in fade_from..band {
            if share[j * band + u] < 1.0 - FADE_TOLERANCE {
                faded = false;
            }
        }
        if 1.0 - darkest >= MIN_DEPTH && faded {
            shadowed.push(j);
        }
    }
    let needed = MIN_SHADOWED_BLOCKS.max((blocks as f64 * MIN_SHADOWED_SHARE).ceil() as usize);
    if shadowed.len() < needed {
        return None;
    }
    let mut consensus = vec![0f64; band];
    for u in 0..band {
        let column: Vec<f64> = shadowed.iter().map(|&j| share[j * band + u]).collect();
        consensus[u] = sorted(&column)[shadowed.len() / 2];
    }
    let profile = smoothed(&consensus);
    let mut deepest = 1.0f64;
    let (mut deepest_at, mut reach) = (0usize, 0usize);
    for u in 0..band {
        if profile[u] < deepest {
            deepest = profile[u];
            deepest_at = u;
        }
        if profile[u] < 1.0 - DEAD_ZONE {
            reach = u + 1;
        }
    }
    let depth = 1.0 - deepest;
    if depth < MIN_DEPTH || deepest_at >= pixels(MAX_DEEPEST_INCHES, dpi) || reach >= band {
        return None;
    }
    // a shadow fades gradually; a tint or a bar printed all along the spine comes back to the paper in a step, and is no shadow
    let mut quarter_back = reach;
    for u in deepest_at..reach {
        if profile[u] >= deepest + depth / 4.0 {
            quarter_back = u;
            break;
        }
    }
    if reach - quarter_back < pixels(MIN_FADE_INCHES, dpi) {
        return None;
    }

    // cells that are shadowed paper: a cell darker than the shadow allows is no paper, and neither is anything between it and
    // the edge (a picture or a tint reaching the spine); those take the shadow of the blocks beside them
    let mut paper_cell = vec![false; blocks * band];
    for j in 0..blocks {
        let mut dark_further = false;
        for u in (0..band).rev() {
            let tolerance = CONSENSUS_TOLERANCE + CONSENSUS_SPREAD * (1.0 - consensus[u]);
            let difference = share[j * band + u] - consensus[u];
            if difference < -tolerance {
                dark_further = true;
            }
            paper_cell[j * band + u] = !dark_further;
        }
    }
    let mut shadow = vec![0f64; blocks * band];
    for u in 0..band {
        for j in 0..blocks {
            if paper_cell[j * band + u] {
                shadow[j * band + u] = share[j * band + u];
                continue;
            }
            let mut before = j as i64 - 1;
            let mut after = j + 1;
            while before >= 0 && !paper_cell[before as usize * band + u] {
                before -= 1;
            }
            while after < blocks && !paper_cell[after * band + u] {
                after += 1;
            }
            shadow[j * band + u] = if before >= 0 && after < blocks {
                let b = before as usize;
                let (x, y) = (share[b * band + u], share[after * band + u]);
                x + (y - x) * (j - b) as f64 / (after - b) as f64
            } else if before >= 0 {
                share[before as usize * band + u]
            } else if after < blocks {
                share[after * band + u]
            } else {
                consensus[u]
            };
        }
    }
    // smooth across the rows (twice), then along the edge's distance
    let shadow = smoothed_rows(&smoothed_rows(&shadow, blocks, band), blocks, band);
    let shadow = smoothed_columns(&shadow, blocks, band);

    // the gain of each cell: the paper back to white, capped, easing out at the far side of the band
    let taper = pixels(TAPER_INCHES, dpi);
    let mut cell_gain = vec![1f64; blocks * band];
    for j in 0..blocks {
        for u in 0..band {
            let light = shadow[j * band + u];
            let gain = if light <= 0.0 {
                MAX_GAIN
            } else {
                ((1.0 - DEAD_ZONE) / light).clamp(1.0, MAX_GAIN)
            };
            let ease = (1.0f64).min((band - u) as f64 / taper as f64);
            cell_gain[j * band + u] = 1.0 + (gain - 1.0) * ease;
        }
    }

    // every band pixel: the cells interpolated between block centres along the edge; pictures left alone
    let (first, second, weight) = interpolation(length, blocks);
    let mut dark = vec![0u8; length * band];
    for v in 0..length {
        let (j0, j1, f) = (first[v], second[v], weight[v]);
        let paper_here = paper[j0] + (paper[j1] - paper[j0]) * f;
        for u in 0..band {
            let (s0, s1) = (shadow[j0 * band + u], shadow[j1 * band + u]);
            let expected = paper_here * (s0 + (s1 - s0) * f);
            if (strip[v * span + u] as f64) < PICTURE_LEVEL * expected {
                dark[v * band + u] = 1;
            }
        }
    }
    let pictures = picture_areas(&dark, band, length, dpi);
    let protection = feathered(&pictures, band, length, pixels(FEATHER_INCHES, dpi));
    let mut gains = vec![1f64; length * band];
    let mut largest_gain = 1.0f64;
    let mut protected_pixels = 0usize;
    for v in 0..length {
        let (j0, j1, f) = (first[v], second[v], weight[v]);
        for u in 0..band {
            let (g0, g1) = (cell_gain[j0 * band + u], cell_gain[j1 * band + u]);
            let mut gain = g0 + (g1 - g0) * f;
            let i = v * band + u;
            gain = 1.0 + (gain - 1.0) * (1.0 - protection[i]);
            gains[i] = gain;
            if gain > largest_gain {
                largest_gain = gain;
            }
            if pictures[i] != 0 {
                protected_pixels += 1;
            }
        }
    }
    let result = SpineShadowResult {
        edge,
        depth,
        width_inches: reach as f64 / dpi,
        max_gain: largest_gain,
        protected_fraction: protected_pixels as f64 / (length * band) as f64,
    };
    Some(SpineShadowModel {
        edge,
        extent,
        length,
        band,
        gains,
        pictures,
        result,
    })
}

/// `image` with every pixel in the model's band brightened by its gain (each of red, green and blue multiplied, rounded, at
/// most 255; alpha kept): the model's band pixels interpolated (bilinear) at the image's resolution, easing to no gain past the
/// band. A pixel whose nearest model pixel is on a picture's box keeps its value; on an image finer than the model, so does
/// one whose nearest model pixel is beside a box (the picture's edge may lie in it).
pub fn apply(image: &Rgba, model: &SpineShadowModel) -> Rgba {
    let along_x = model.edge.along_x();
    let (w, h) = (image.width, image.height);
    let extent = if along_x { w } else { h };
    let length = if along_x { h } else { w };
    // analysis pixels per image pixel, away from the edge and along it
    let scale_u = model.extent as f64 / extent as f64;
    let scale_v = model.length as f64 / length as f64;
    let band = model.band;
    let reach = extent.min(((band + 1) as f64 / scale_u) as usize + 1);
    let mut u0s = vec![0usize; reach];
    let mut u1s = vec![0usize; reach];
    let mut fus = vec![0f64; reach];
    let mut nearest_us = vec![0usize; reach];
    for uf in 0..reach {
        let ua = (uf as f64 + 0.5) * scale_u - 0.5;
        if ua > 0.0 {
            let k = ua as usize;
            u0s[uf] = k;
            u1s[uf] = k + 1; // column `band` and beyond: no gain
            fus[uf] = ua - k as f64;
            nearest_us[uf] = (ua + 0.5) as usize;
        }
    }
    let mut output = image.data.clone();
    let gains = &model.gains;
    // on a finer image, a picture's edge can fall inside an analysis pixel that is lighter than the picture and so outside its
    // box: the boxes are grown by one analysis pixel, so that edge keeps its pixels too
    let grown;
    let pictures = if scale_u < 1.0 || scale_v < 1.0 {
        grown = mask_ops::dilate(&model.pictures, band, model.length, 1);
        &grown
    } else {
        &model.pictures
    };
    for vf in 0..length {
        let va = (vf as f64 + 0.5) * scale_v - 0.5;
        let (v0, v1, nearest_v, fv);
        if va <= 0.0 {
            (v0, v1, fv, nearest_v) = (0, 0, 0.0, 0);
        } else if va >= (model.length - 1) as f64 {
            (v0, v1, fv, nearest_v) = (model.length - 1, model.length - 1, 0.0, model.length - 1);
        } else {
            v0 = va as usize;
            v1 = v0 + 1;
            fv = va - v0 as f64;
            nearest_v = (va + 0.5) as usize;
        }
        for uf in 0..reach {
            let (a, b) = (u0s[uf], u1s[uf]);
            if a >= band {
                break;
            }
            // a pixel of a picture's box (grown on a finer image) keeps its value, whatever the gain beside it
            let nearest_u = nearest_us[uf];
            if nearest_u < band && pictures[nearest_v * band + nearest_u] != 0 {
                continue;
            }
            let (g00, g10) = (gains[v0 * band + a], gains[v1 * band + a]);
            let g01 = if b < band { gains[v0 * band + b] } else { 1.0 };
            let g11 = if b < band { gains[v1 * band + b] } else { 1.0 };
            let top = g00 + (g01 - g00) * fus[uf];
            let bottom = g10 + (g11 - g10) * fus[uf];
            let gain = top + (bottom - top) * fv;
            if gain <= 1.0 {
                continue;
            }
            let (x, y) = match model.edge {
                PageEdge::Left => (uf, vf),
                PageEdge::Right => (w - 1 - uf, vf),
                PageEdge::Top => (vf, uf),
                PageEdge::Bottom => (vf, h - 1 - uf),
            };
            let i = (y * w + x) * 4;
            for c in 0..3 {
                let value = (output[i + c] as f64 * gain + 0.5) as i64;
                output[i + c] = value.min(255) as u8;
            }
        }
    }
    Rgba::new(w, h, output)
}

// The first `span` columns of `gray` away from `edge`, as rows along it: pixel (u, v) at v * span + u is u pixels from the
// edge, v along it from its left or top end.
fn oriented(gray: &GrayImage, edge: PageEdge, span: usize) -> Vec<u8> {
    let (w, h) = (gray.width(), gray.height());
    let length = if edge.along_x() { h } else { w };
    let mut output = vec![0u8; length * span];
    for v in 0..length {
        for u in 0..span {
            let index = match edge {
                PageEdge::Left => v * w + u,
                PageEdge::Right => v * w + (w - 1 - u),
                PageEdge::Top => u * w + v,
                PageEdge::Bottom => (h - 1 - u) * w + v,
            };
            output[v * span + u] = gray.gray.data[index];
        }
    }
    output
}

// Each value replaced by the median of itself and `radius` values either side (fewer at the ends; the upper middle of an even
// count).
fn running_median(values: &[f64], radius: usize) -> Vec<f64> {
    let n = values.len();
    (0..n)
        .map(|i| {
            let window = sorted(&values[i.saturating_sub(radius)..=(n - 1).min(i + radius)]);
            window[window.len() / 2]
        })
        .collect()
}

// The 1-2-1 filter, the end values repeated.
fn smoothed(values: &[f64]) -> Vec<f64> {
    let n = values.len();
    (0..n)
        .map(|i| {
            let (before, after) = (values[i.saturating_sub(1)], values[(n - 1).min(i + 1)]);
            (before + 2.0 * values[i] + after) / 4.0
        })
        .collect()
}

// The 1-2-1 filter down each column of a rows x columns grid (across the blocks), the end rows repeated.
fn smoothed_rows(grid: &[f64], rows: usize, columns: usize) -> Vec<f64> {
    let mut output = vec![0f64; grid.len()];
    for j in 0..rows {
        let above = j.saturating_sub(1) * columns;
        let below = (rows - 1).min(j + 1) * columns;
        let here = j * columns;
        for u in 0..columns {
            output[here + u] = (grid[above + u] + 2.0 * grid[here + u] + grid[below + u]) / 4.0;
        }
    }
    output
}

// The 1-2-1 filter along each row of a rows x columns grid (away from the edge), the end columns repeated.
fn smoothed_columns(grid: &[f64], rows: usize, columns: usize) -> Vec<f64> {
    let mut output = vec![0f64; grid.len()];
    for j in 0..rows {
        let row = j * columns;
        for u in 0..columns {
            let before = grid[row + u.saturating_sub(1)];
            let after = grid[row + (columns - 1).min(u + 1)];
            output[row + u] = (before + 2.0 * grid[row + u] + after) / 4.0;
        }
    }
    output
}

// For each of `length` rows split into `blocks` blocks (as the cells were): the blocks whose centres surround the row's centre
// and the second one's weight; the nearest block beyond the outer centres.
fn interpolation(length: usize, blocks: usize) -> (Vec<usize>, Vec<usize>, Vec<f64>) {
    let mut first = vec![0usize; length];
    let mut second = vec![0usize; length];
    let mut weight = vec![0f64; length];
    let centers: Vec<f64> = (0..blocks)
        .map(|t| (t * length / blocks + (t + 1) * length / blocks) as f64 / 2.0)
        .collect();
    let mut k = 0;
    for p in 0..length {
        let c = p as f64 + 0.5;
        while k < blocks - 1 && centers[k + 1] <= c {
            k += 1;
        }
        if c <= centers[0] {
            (first[p], second[p], weight[p]) = (0, 0, 0.0);
        } else if k >= blocks - 1 {
            (first[p], second[p], weight[p]) = (blocks - 1, blocks - 1, 0.0);
        } else {
            first[p] = k;
            second[p] = k + 1;
            weight[p] = (c - centers[k]) / (centers[k + 1] - centers[k]);
        }
    }
    (first, second, weight)
}

// The pictures among the dark pixels of the band: the mask opened (thin strokes of text and lines gone) and closed (a picture's
// parts joined across its light areas, the band's sides counting as set), then the box around each part at least
// MIN_PICTURE_INCHES square in area (printed pictures are rectangles; a light corner of one is still inside its box).
fn picture_areas(dark: &[u8], width: usize, height: usize, dpi: f64) -> Vec<u8> {
    let opening = pixels(OPEN_INCHES, dpi);
    let closing = pixels(CLOSE_INCHES, dpi);
    let eroded = mask_ops::erode(dark, width, height, opening, false);
    let opened = mask_ops::dilate(&eroded, width, height, opening);
    let dilated = mask_ops::dilate(&opened, width, height, closing);
    let closed = mask_ops::erode(&dilated, width, height, closing, true);
    let side = pixels(MIN_PICTURE_INCHES, dpi);
    let min_area = side * side;
    let mut output = vec![0u8; width * height];
    for component in mask_ops::connected_components(&closed, width, height) {
        if component.area < min_area {
            continue;
        }
        let (mut top, mut bottom, mut left, mut right) =
            (height as i64, -1i64, width as i64, -1i64);
        for (&row, &(min_x, max_x)) in &component.rows {
            top = top.min(row as i64);
            bottom = bottom.max(row as i64);
            left = left.min(min_x as i64);
            right = right.max(max_x as i64);
        }
        if top > bottom || left > right {
            continue;
        }
        for y in top..=bottom {
            for x in left..=right {
                output[y as usize * width + x as usize] = 1;
            }
        }
    }
    output
}

// How much each pixel is left alone, 0 to 1: 1 on a picture's box, fading to 0 over the 2 x `radius` pixels beyond it (the mask
// grown by `radius`, then averaged over a square of 2 x radius + 1, clipped to the band). `apply` keeps the box itself hard at
// the image's resolution.
fn feathered(mask: &[u8], width: usize, height: usize, radius: usize) -> Vec<f64> {
    let grown = mask_ops::dilate(mask, width, height, radius);
    // summed-area table of the grown mask
    let stride = width + 1;
    let mut sums = vec![0i64; stride * (height + 1)];
    for y in 0..height {
        let mut row_sum = 0i64;
        for x in 0..width {
            row_sum += grown[y * width + x] as i64;
            sums[(y + 1) * stride + x + 1] = sums[y * stride + x + 1] + row_sum;
        }
    }
    let mut output = vec![0f64; width * height];
    for y in 0..height {
        let (y0, y1) = (y.saturating_sub(radius), height.min(y + radius + 1));
        for x in 0..width {
            let (x0, x1) = (x.saturating_sub(radius), width.min(x + radius + 1));
            let total = sums[y1 * stride + x1] - sums[y0 * stride + x1] - sums[y1 * stride + x0]
                + sums[y0 * stride + x0];
            output[y * width + x] = total as f64 / ((y1 - y0) * (x1 - x0)) as f64;
        }
    }
    output
}
