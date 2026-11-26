use crate::geometry::{Point, Quad};
use lazy_image::Rgba;

/// How much of a card or a glossy print a camera saw blown out by a reflection: the share of the measured cells whose every
/// channel is clipped, and the share the largest patch of them, joined side by side, covers.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GlareReading {
    pub clipped_share: f64,
    pub largest_blob_share: f64,
}

impl GlareReading {
    /// A patch of glare large enough to hide what is printed under it.
    pub fn has_glare(&self) -> bool {
        self.largest_blob_share >= MIN_BLOB_SHARE
    }
}

/// A cell is clipped when the mean of each of its channels is at least this (0 to 255).
pub const CLIP_LEVEL: u64 = 250;
/// Glare: the largest clipped patch covers at least this share of the measured cells.
pub const MIN_BLOB_SHARE: f64 = 0.01;
/// The band along the outline's edges left out, as a share of its shorter side.
pub const INSET_SHARE: f64 = 0.04;
/// The area is cut into at most this many cells along its longer side.
pub const ANALYSIS_LONG_SIDE: usize = 400;

/// The glare on `image` inside `region` (its pixels; `None`: the whole picture). The area is the outline less a band along its
/// edges, cut into square cells k pixels wide, k the whole number that brings the longer side to at most
/// ANALYSIS_LONG_SIDE cells; a cell counts when its centre lies in the inner outline and is clipped when the rounded mean of
/// each channel is at least CLIP_LEVEL. Measured on the photo as taken, before any clean-up stretches its whites.
pub fn measure(image: &Rgba, region: Option<&Quad>) -> GlareReading {
    let (width, height) = (image.width, image.height);
    if width == 0 || height == 0 {
        return GlareReading::default();
    }
    let whole = Quad::from_xy([
        [0.0, 0.0],
        [width as f64, 0.0],
        [width as f64, height as f64],
        [0.0, height as f64],
    ]);
    let outline = region.copied().unwrap_or(whole);
    if !outline.is_convex() {
        return GlareReading::default();
    }
    let c = outline.0;
    let min_x = c.iter().map(|p| p.x).fold(c[0].x, f64::min);
    let max_x = c.iter().map(|p| p.x).fold(c[0].x, f64::max);
    let min_y = c.iter().map(|p| p.y).fold(c[0].y, f64::min);
    let max_y = c.iter().map(|p| p.y).fold(c[0].y, f64::max);
    if !(min_x.is_finite() && max_x.is_finite() && min_y.is_finite() && max_y.is_finite()) {
        return GlareReading::default();
    }
    let x0 = min_x.floor().max(0.0) as usize;
    let y0 = min_y.floor().max(0.0) as usize;
    let x1 = (max_x.ceil().max(0.0) as usize).min(width);
    let y1 = (max_y.ceil().max(0.0) as usize).min(height);
    if x1 <= x0 || y1 <= y0 {
        return GlareReading::default();
    }
    let (area_w, area_h) = (x1 - x0, y1 - y0);
    let k = area_w.max(area_h).div_ceil(ANALYSIS_LONG_SIDE).max(1);
    let (columns, rows) = (area_w.div_ceil(k), area_h.div_ceil(k));
    let across = (outline.edge(0) + outline.edge(2)) / 2.0;
    let down = (outline.edge(1) + outline.edge(3)) / 2.0;
    let inset = INSET_SHARE * across.min(down);

    // 0: not measured, 1: measured, 2: measured and clipped
    let mut cells = vec![0u8; columns * rows];
    let (mut counted, mut clipped) = (0usize, 0usize);
    for row in 0..rows {
        let cy0 = y0 + row * k;
        let cy1 = (cy0 + k).min(y1);
        for column in 0..columns {
            let cx0 = x0 + column * k;
            let cx1 = (cx0 + k).min(x1);
            let centre = Point::new((cx0 + cx1) as f64 / 2.0, (cy0 + cy1) as f64 / 2.0);
            if !outline.contains(centre, -inset) {
                continue;
            }
            let mut sum = [0u64; 3];
            for y in cy0..cy1 {
                for x in cx0..cx1 {
                    let p = (y * width + x) * 4;
                    for (s, v) in sum.iter_mut().zip(&image.data[p..p + 3]) {
                        *s += *v as u64;
                    }
                }
            }
            let n = ((cx1 - cx0) * (cy1 - cy0)) as u64;
            counted += 1;
            if sum.iter().all(|s| (s + n / 2) / n >= CLIP_LEVEL) {
                cells[row * columns + column] = 2;
                clipped += 1;
            } else {
                cells[row * columns + column] = 1;
            }
        }
    }
    if counted == 0 {
        return GlareReading::default();
    }
    GlareReading {
        clipped_share: clipped as f64 / counted as f64,
        largest_blob_share: largest_patch(&cells, columns, rows) as f64 / counted as f64,
    }
}

/// Whether `image` shows glare inside `region`.
pub fn has_glare(image: &Rgba, region: Option<&Quad>) -> bool {
    measure(image, region).has_glare()
}

// the cells of the largest patch of clipped cells (value 2) joined side by side
fn largest_patch(cells: &[u8], columns: usize, rows: usize) -> usize {
    let mut seen = vec![false; cells.len()];
    let mut stack = Vec::new();
    let mut largest = 0;
    for start in 0..cells.len() {
        if cells[start] != 2 || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut size = 0;
        while let Some(index) = stack.pop() {
            size += 1;
            let (column, row) = (index % columns, index / columns);
            let around = [
                (column > 0).then(|| index - 1),
                (column < columns - 1).then(|| index + 1),
                (row > 0).then(|| index - columns),
                (row < rows - 1).then(|| index + columns),
            ];
            for next in around.into_iter().flatten() {
                if cells[next] == 2 && !seen[next] {
                    seen[next] = true;
                    stack.push(next);
                }
            }
        }
        largest = largest.max(size);
    }
    largest
}
