#![allow(clippy::needless_range_loop)] // index loops kept as in the Swift and C# twins, so the three are easy to compare

use lazy_image::{Rgba, luma};

// Plain pixel loops on integer pixels and f64 (+, -, *, / and truncation only), so every platform gives the same bytes.

/// The edges are looked for in this share of the height from the top and from the bottom.
pub const EDGE_FRACTION: f64 = 0.25;
/// A row is paper when its brightness is at least this share of the paper's.
pub const PAPER_SHARE: f64 = 0.5;
/// An edge is the first of this many paper rows in a row.
pub const EDGE_RUN: usize = 3;
/// Columns around each one averaged when looking for the edge, on each side.
pub const EDGE_HALF_WIDTH: usize = 2;
/// The edges must be found in at least this share of the columns.
pub const MIN_FOUND_SHARE: f64 = 0.6;
/// A page whose edges are further apart than this share of its height is a page.
pub const MIN_PAGE_SHARE: f64 = 0.5;
/// The edges are smoothed across this share of the width, on each side (at least MIN_SMOOTH_COLUMNS).
pub const SMOOTH_SHARE: f64 = 0.01;
pub const MIN_SMOOTH_COLUMNS: usize = 2;
/// An edge that jumps by more than this share of the height between two neighbouring columns where it is found is no page
/// edge (a dark banner or a picture along the border), and the page is left alone.
pub const MAX_EDGE_STEP_SHARE: f64 = 0.02;
/// A page whose edges bend less than this share of its height is left alone.
pub const MIN_CURVE_SHARE: f64 = 0.01;
/// A page whose paper is darker than this gray level is left alone.
pub const MIN_PAPER: usize = 96;
/// The paper's brightness and colour are read on the middle rows between these shares of the height...
pub const PAPER_ROWS_FROM: f64 = 0.3;
pub const PAPER_ROWS_TO: f64 = 0.7;
/// ...as this share of the pixels there being darker.
pub const PAPER_PERCENTILE: f64 = 0.8;

/// Fingers are looked for this many inches inside each border...
pub const FINGER_BAND_INCHES: f64 = 1.0;
/// ...in patches of at least...
pub const MIN_FINGER_SQUARE_INCHES: f64 = 0.03;
/// ...and at most this many square inches.
pub const MAX_FINGER_SQUARE_INCHES: f64 = 4.0;
/// A patch is painted over this far beyond its pixels, in inches (the blurred skin at its rim).
pub const FINGER_GROW_INCHES: f64 = 0.04;
/// A pixel is skin when its red exceeds its green by this much more than the paper's does...
pub const SKIN_RED_OVER_GREEN: i64 = 14;
/// ...and exceeds its blue by this much more...
pub const SKIN_RED_OVER_BLUE: i64 = 25;
/// ...and its red is at least this.
pub const SKIN_MIN_RED: i64 = 80;
/// The paper's colour is taken from pixels at least this share as bright as the paper.
pub const INK_SHARE: f64 = 0.7;

/// `image` with its page drawn flat between its curved top and bottom edges, for the curl of a book page photographed open:
/// the page's top and bottom edges are found column by column on a gray copy (the first and last rows where the paper is
/// bright, past the table beside it), cleaned up (a median, then a mean, across columns), and every column is drawn again
/// between them: output row y is the share (y - top reference) / (bottom reference - top reference) of the way down the page,
/// taken from the same share of the way between that column's own edges. Only the height is corrected. `None` when the edges
/// are not found or bend less than MIN_CURVE_SHARE of the height.
pub fn flatten(image: &Rgba) -> Option<Rgba> {
    let (w, h) = (image.width, image.height);
    if w < 16 || h < 16 {
        return None;
    }
    let gray = luma(image).data;
    let paper = paper_level(&gray, w, h);
    if paper < MIN_PAPER {
        return None;
    }
    let threshold = paper as f64 * PAPER_SHARE;
    let depth = (h as f64 * EDGE_FRACTION) as usize;
    let raw_top = edges(&gray, w, h, threshold, depth, true)?;
    let raw_bottom = edges(&gray, w, h, threshold, depth, false)?;
    let half = MIN_SMOOTH_COLUMNS.max((w as f64 * SMOOTH_SHARE) as usize);
    let top = smoothed(&raw_top, half);
    let bottom = smoothed(&raw_bottom, half);
    let (mut top_ref, mut bottom_ref) = (top[0], bottom[0]);
    for x in 0..w {
        top_ref = top_ref.min(top[x]);
        bottom_ref = bottom_ref.max(bottom[x]);
    }
    let hf = h as f64;
    if bottom_ref - top_ref < hf * MIN_PAGE_SHARE {
        return None;
    }
    let mut bend = 0f64;
    for x in 0..w {
        if bottom[x] - top[x] < hf * MIN_PAGE_SHARE {
            return None;
        }
        bend = bend.max((top[x] - top_ref) + (bottom_ref - bottom[x]));
    }
    if bend < hf * MIN_CURVE_SHARE {
        return None;
    }

    let mut output = vec![255u8; w * h * 4];
    let span = bottom_ref - top_ref;
    let last = (h - 1) as f64;
    for y in 0..h {
        let t = (y as f64 - top_ref) / span;
        for x in 0..w {
            let s = (top[x] + t * (bottom[x] - top[x])).clamp(0.0, last);
            let y0 = (s as usize).min(h - 2);
            let f = s - y0 as f64;
            let a = (y0 * w + x) * 4;
            let b = a + w * 4;
            let o = (y * w + x) * 4;
            for c in 0..3 {
                let v = image.data[a + c] as f64 * (1.0 - f) + image.data[b + c] as f64 * f;
                output[o + c] = (v + 0.5) as u8;
            }
        }
    }
    Some(Rgba::new(w, h, output))
}

/// `image`, taken at `dpi`, with the fingers at its margins painted over with paper: skin-coloured patches in a band along
/// each border that touch the border and end inside the band (a thumb holding the page, not a picture that runs on). `None`
/// when there are none.
pub fn fill_fingers(image: &Rgba, dpi: f64) -> Option<Rgba> {
    let (w, h) = (image.width, image.height);
    let band = (FINGER_BAND_INCHES * dpi) as usize;
    if w < 16 || h < 16 || band < 8 || band * 4 > w.min(h) {
        return None;
    }
    let gray = luma(image).data;
    let paper = paper_level(&gray, w, h);
    if paper < MIN_PAPER {
        return None;
    }
    let tone = paper_tone(image, &gray, paper)?;
    let red_over_green = tone.0 - tone.1 + SKIN_RED_OVER_GREEN;
    let red_over_blue = tone.0 - tone.2 + SKIN_RED_OVER_BLUE;

    let is_skin = |i: usize| {
        let (r, g, b) = (
            image.data[i] as i64,
            image.data[i + 1] as i64,
            image.data[i + 2] as i64,
        );
        r >= SKIN_MIN_RED && r - g >= red_over_green && r - b >= red_over_blue
    };
    let depth_at = |x: usize, y: usize| x.min(w - 1 - x).min(y.min(h - 1 - y));

    let mut skin = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            if depth_at(x, y) < band && is_skin((y * w + x) * 4) {
                skin[y * w + x] = 1;
            }
        }
    }

    let min_area = (MIN_FINGER_SQUARE_INCHES * dpi * dpi) as usize;
    let max_area = (MAX_FINGER_SQUARE_INCHES * dpi * dpi) as usize;
    let grow = 2.max((FINGER_GROW_INCHES * dpi) as usize);
    let brightest = paper as f64 * INK_SHARE;
    let mut output = image.clone();
    let mut painted = false;
    let mut seen = vec![0u8; w * h];
    for start in 0..w * h {
        if skin[start] != 1 || seen[start] != 0 {
            continue;
        }
        let mut cells = vec![start];
        let mut stack = vec![start];
        seen[start] = 1;
        let mut runs_on = false;
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (w, 0, h, 0);
        while let Some(at) = stack.pop() {
            let (x, y) = (at % w, at / w);
            if depth_at(x, y) + 1 >= band {
                runs_on = true;
            }
            min_x = min_x.min(x);
            max_x = max_x.max(x);
            min_y = min_y.min(y);
            max_y = max_y.max(y);
            let mut next = Vec::with_capacity(4);
            if x > 0 {
                next.push(at - 1);
            }
            if x < w - 1 {
                next.push(at + 1);
            }
            if y > 0 {
                next.push(at - w);
            }
            if y < h - 1 {
                next.push(at + w);
            }
            for n in next {
                if skin[n] == 1 && seen[n] == 0 {
                    seen[n] = 1;
                    cells.push(n);
                    stack.push(n);
                }
            }
        }
        if runs_on || cells.len() < min_area || cells.len() > max_area {
            continue;
        }
        if !cells.iter().any(|&at| depth_at(at % w, at / w) == 0) {
            continue;
        }

        // the paper beside the patch: bright pixels around its box that are not skin
        let (mut sum_r, mut sum_g, mut sum_b, mut count) = (0usize, 0usize, 0usize, 0usize);
        let x0 = min_x.saturating_sub(grow + 4);
        let x1 = (w - 1).min(max_x + grow + 4);
        let y0 = min_y.saturating_sub(grow + 4);
        let y1 = (h - 1).min(max_y + grow + 4);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let i = y * w + x;
                if skin[i] == 0 && gray[i] as f64 >= brightest {
                    sum_r += image.data[i * 4] as usize;
                    sum_g += image.data[i * 4 + 1] as usize;
                    sum_b += image.data[i * 4 + 2] as usize;
                    count += 1;
                }
            }
        }
        if count < 20 {
            continue;
        }
        let fill = [
            (sum_r / count) as u8,
            (sum_g / count) as u8,
            (sum_b / count) as u8,
        ];
        let gx0 = min_x.saturating_sub(grow);
        let gx1 = (w - 1).min(max_x + grow);
        let gy0 = min_y.saturating_sub(grow);
        let gy1 = (h - 1).min(max_y + grow);
        let (bw, bh) = (gx1 - gx0 + 1, gy1 - gy0 + 1);
        let mut cover = vec![0u8; bw * bh];
        for &at in &cells {
            cover[(at / w - gy0) * bw + (at % w - gx0)] = 1;
        }
        let wide = spread(&cover, bw, bh, grow, true);
        let grown = spread(&wide, bw, bh, grow, false);
        for y in gy0..=gy1 {
            for x in gx0..=gx1 {
                if grown[(y - gy0) * bw + (x - gx0)] == 1 {
                    let o = (y * w + x) * 4;
                    output.data[o..o + 3].copy_from_slice(&fill);
                }
            }
        }
        painted = true;
    }
    painted.then_some(output)
}

// `mask` with every set cell widened to the `by` cells on each side of it, along its rows or along its columns.
fn spread(mask: &[u8], bw: usize, bh: usize, by: usize, along_rows: bool) -> Vec<u8> {
    let mut result = vec![0u8; bw * bh];
    let (lines, length) = if along_rows { (bh, bw) } else { (bw, bh) };
    let (step_along, step_across) = if along_rows { (1, bw) } else { (bw, 1) };
    for line in 0..lines {
        let start = line * step_across;
        let mut since = by + 1;
        for k in 0..length {
            let i = start + k * step_along;
            since = if mask[i] == 1 { 0 } else { since + 1 };
            if since <= by {
                result[i] = 1;
            }
        }
        since = by + 1;
        for k in (0..length).rev() {
            let i = start + k * step_along;
            since = if mask[i] == 1 { 0 } else { since + 1 };
            if since <= by {
                result[i] = 1;
            }
        }
    }
    result
}

/// The paper's gray level: the PAPER_PERCENTILE of the middle rows.
pub fn paper_level(gray: &[u8], width: usize, height: usize) -> usize {
    let mut histogram = [0usize; 256];
    let from = (height as f64 * PAPER_ROWS_FROM) as usize;
    let to = (height as f64 * PAPER_ROWS_TO) as usize;
    for y in from..(from + 1).max(to) {
        for x in 0..width {
            histogram[gray[y * width + x] as usize] += 1;
        }
    }
    let wanted = (width * 1.max(to.saturating_sub(from))) as f64 * PAPER_PERCENTILE;
    let mut seen = 0.0;
    for (level, n) in histogram.iter().enumerate() {
        seen += *n as f64;
        if seen >= wanted {
            return level;
        }
    }
    255
}

// The paper's mean colour over the middle rows' pixels at or above `level`; `None` when there are none.
fn paper_tone(image: &Rgba, gray: &[u8], level: usize) -> Option<(i64, i64, i64)> {
    let (w, h) = (image.width, image.height);
    let from = (h as f64 * PAPER_ROWS_FROM) as usize;
    let to = (h as f64 * PAPER_ROWS_TO) as usize;
    let (mut sum_r, mut sum_g, mut sum_b, mut count) = (0i64, 0i64, 0i64, 0i64);
    for y in from..(from + 1).max(to) {
        for x in 0..w {
            if gray[y * w + x] as usize >= level {
                let i = (y * w + x) * 4;
                sum_r += image.data[i] as i64;
                sum_g += image.data[i + 1] as i64;
                sum_b += image.data[i + 2] as i64;
                count += 1;
            }
        }
    }
    (count > 0).then(|| (sum_r / count, sum_g / count, sum_b / count))
}

// The row of the page's top edge in every column (or of its bottom edge, `from_top` false): the first of EDGE_RUN rows in a
// row that are paper, looked for within `depth` rows of the border, where a row is paper when the mean of the
// 2 x EDGE_HALF_WIDTH + 1 columns around it reaches `threshold`. Columns with no such row take the nearest column's that has
// one; `None` when fewer than MIN_FOUND_SHARE do, or when the edge jumps by more than MAX_EDGE_STEP_SHARE of the height
// between neighbouring columns that have one.
fn edges(
    gray: &[u8],
    w: usize,
    h: usize,
    threshold: f64,
    depth: usize,
    from_top: bool,
) -> Option<Vec<f64>> {
    let mut found = vec![-1i64; w];
    let mut found_count = 0usize;
    for x in 0..w {
        let c0 = x.saturating_sub(EDGE_HALF_WIDTH);
        let c1 = (w - 1).min(x + EDGE_HALF_WIDTH);
        let need = threshold * (c1 - c0 + 1) as f64;
        let mut run = 0;
        for step in 0..depth {
            let y = if from_top { step } else { h - 1 - step };
            let sum: usize = gray[y * w + c0..=y * w + c1]
                .iter()
                .map(|&v| v as usize)
                .sum();
            if sum as f64 >= need {
                run += 1;
                if run == EDGE_RUN {
                    let shift = (EDGE_RUN - 1) as i64;
                    found[x] = if from_top {
                        y as i64 - shift
                    } else {
                        y as i64 + shift
                    };
                    found_count += 1;
                    break;
                }
            } else {
                run = 0;
            }
        }
    }
    if (found_count as f64) < w as f64 * MIN_FOUND_SHARE {
        return None;
    }
    let max_step = h as f64 * MAX_EDGE_STEP_SHARE;
    let mut last: Option<usize> = None;
    for x in 0..w {
        if found[x] >= 0 {
            if let Some(l) = last
                && ((found[x] - found[l]) as f64).abs() > max_step
            {
                return None;
            }
            last = Some(x);
        }
    }
    let mut result = vec![0f64; w];
    let mut previous: Option<usize> = None;
    for x in 0..w {
        if found[x] < 0 {
            continue;
        }
        match previous {
            None => {
                for r in &mut result[..x] {
                    *r = found[x] as f64;
                }
            }
            Some(p) => {
                let gap = (x - p) as f64;
                for k in p + 1..x {
                    let share = (k - p) as f64 / gap;
                    result[k] = found[p] as f64 + share * (found[x] - found[p]) as f64;
                }
            }
        }
        result[x] = found[x] as f64;
        previous = Some(x);
    }
    if let Some(p) = previous {
        for r in &mut result[p + 1..] {
            *r = found[p] as f64;
        }
    }
    Some(result)
}

// `values` with a median over 2 x `half` + 1 columns (specks of text, a finger), then a mean over the same.
fn smoothed(values: &[f64], half: usize) -> Vec<f64> {
    let n = values.len();
    let mut middle = vec![0f64; n];
    for x in 0..n {
        let mut window = values[x.saturating_sub(half)..=(n - 1).min(x + half)].to_vec();
        window.sort_by(|a, b| a.total_cmp(b));
        middle[x] = window[window.len() / 2];
    }
    let mut result = vec![0f64; n];
    for x in 0..n {
        let (from, to) = (x.saturating_sub(half), (n - 1).min(x + half));
        let sum: f64 = middle[from..=to].iter().sum();
        result[x] = sum / (to - from + 1) as f64;
    }
    result
}
