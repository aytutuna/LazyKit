use crate::geometry::Quad;
use lazy_image::Rgba;

/// What a capture check can say is wrong with a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Issue {
    Blur,
    Glare,
    Dark,
    Cut,
    Small,
}

/// The checks read the page at one size, so a threshold means the same on a 640 px live frame and a 3000 px photo.
pub const ANALYSIS: usize = 640;
/// Variance of the Laplacian of the sharpest tenth of the page.
pub const BLUR_BELOW: f64 = 60.0;
/// A patch of near-white cells this large (share of the page) is glare; a page that is all white is just bright paper.
pub const GLARE_FROM: f64 = 0.03;
pub const GLARE_UNTIL: f64 = 0.6;
const CLIPPED: f64 = 252.0;
pub const DARK_BELOW: f64 = 75.0;
/// The page covers less than this share of the frame: too far away.
pub const SMALL_BELOW: f64 = 0.3;
const EDGE_MARGIN: f64 = 0.012;
pub const CUT_CONFIDENCE: f64 = 0.6;

struct Reduced {
    w: usize,
    h: usize,
    y: Vec<f32>,
    clipped: Vec<u8>,
}

// the page averaged down (never up) to at most ANALYSIS pixels on its long side, as luminance
fn reduce(page: &Rgba) -> Reduced {
    let k = f64::max(1.0, page.width.max(page.height) as f64 / ANALYSIS as f64);
    let w = (page.width as f64 / k).floor() as usize;
    let h = (page.height as f64 / k).floor() as usize;
    let mut y = vec![0f32; w * h];
    let mut clipped = vec![0u8; w * h];
    for j in 0..h {
        let y0 = (j as f64 * k).floor() as usize;
        let y1 = (y0 + 1).max(page.height.min(((j + 1) as f64 * k).floor() as usize));
        for i in 0..w {
            let x0 = (i as f64 * k).floor() as usize;
            let x1 = (x0 + 1).max(page.width.min(((i + 1) as f64 * k).floor() as usize));
            let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let o = (yy * page.width + xx) * 4;
                    r += page.data[o] as u64;
                    g += page.data[o + 1] as u64;
                    b += page.data[o + 2] as u64;
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as f64;
            let (r, g, b) = (r as f64, g as f64, b as f64);
            y[j * w + i] = ((0.299 * r + 0.587 * g + 0.114 * b) / n) as f32;
            clipped[j * w + i] = (r.min(g).min(b) / n >= CLIPPED) as u8;
        }
    }
    Reduced { w, h, y, clipped }
}

/// The sharpest tenth of 8 x 8 tiles by variance of the Laplacian; `None` for a page with nothing on it.
pub fn sharpness(y: &[f32], w: usize, h: usize) -> Option<f64> {
    let mut lap = vec![0f32; w * h];
    for j in 1..h.saturating_sub(1) {
        for i in 1..w.saturating_sub(1) {
            let o = j * w + i;
            lap[o] = (y[o - 1] as f64 + y[o + 1] as f64 + y[o - w] as f64 + y[o + w] as f64
                - 4.0 * y[o] as f64) as f32;
        }
    }
    let tiles = 8;
    let mut scores = Vec::new();
    let (mut lo, mut hi) = (255f64, 0f64);
    for ty in 0..tiles {
        for tx in 0..tiles {
            let x0 = (tx * w / tiles).max(1);
            let x1 = ((tx + 1) * w / tiles).min(w.saturating_sub(1));
            let y0 = (ty * h / tiles).max(1);
            let y1 = ((ty + 1) * h / tiles).min(h.saturating_sub(1));
            let (mut sum, mut sq, mut n) = (0f64, 0f64, 0usize);
            for j in y0..y1 {
                for i in x0..x1 {
                    let v = lap[j * w + i] as f64;
                    sum += v;
                    sq += v * v;
                    n += 1;
                    lo = lo.min(y[j * w + i] as f64);
                    hi = hi.max(y[j * w + i] as f64);
                }
            }
            if n > 0 {
                let mean = sum / n as f64;
                scores.push(sq / n as f64 - mean * mean);
            }
        }
    }
    if hi - lo < 40.0 || scores.is_empty() {
        return None;
    }
    scores.sort_by(|a, b| a.total_cmp(b));
    Some(scores[(scores.len() - 1).min((scores.len() as f64 * 0.9).floor() as usize)])
}

/// The largest share of the page a 4-connected patch of clipped 8 x 8 cells covers.
pub fn glare_share(clipped: &[u8], w: usize, h: usize) -> f64 {
    let cell = 8;
    let (cw, ch) = (w / cell, h / cell);
    if cw == 0 || ch == 0 {
        return 0.0;
    }
    let mut on = vec![0u8; cw * ch];
    for j in 0..ch {
        for i in 0..cw {
            let mut n = 0u32;
            for dy in 0..cell {
                for dx in 0..cell {
                    n += clipped[(j * cell + dy) * w + i * cell + dx] as u32;
                }
            }
            on[j * cw + i] = (n as f64 >= (cell * cell) as f64 * 0.9) as u8;
        }
    }
    let mut best = 0usize;
    let mut stack = Vec::new();
    for s in 0..on.len() {
        if on[s] != 1 {
            continue;
        }
        on[s] = 2;
        stack.push(s);
        let mut count = 0;
        while let Some(at) = stack.pop() {
            count += 1;
            let (x, y) = (at % cw, at / cw);
            let around = [
                (x > 0).then(|| at - 1),
                (x < cw - 1).then(|| at + 1),
                (y > 0).then(|| at - cw),
                (y < ch - 1).then(|| at + cw),
            ];
            for next in around.into_iter().flatten() {
                if on[next] == 1 {
                    on[next] = 2;
                    stack.push(next);
                }
            }
        }
        best = best.max(count);
    }
    best as f64 / (cw * ch) as f64
}

pub fn mean_luminance(y: &[f32]) -> f64 {
    y.iter().map(|&v| v as f64).sum::<f64>() / y.len() as f64
}

/// Problems with the flattened page itself: blur, glare, too dark.
pub fn page_issues(page: &Rgba) -> Vec<Issue> {
    let Reduced { w, h, y, clipped } = reduce(page);
    let mut issues = Vec::new();
    if let Some(sharp) = sharpness(&y, w, h)
        && sharp < BLUR_BELOW
    {
        issues.push(Issue::Blur);
    }
    let glare = glare_share(&clipped, w, h);
    if (GLARE_FROM..=GLARE_UNTIL).contains(&glare) {
        issues.push(Issue::Glare);
    }
    if mean_luminance(&y) < DARK_BELOW {
        issues.push(Issue::Dark);
    }
    issues
}

/// Problems with where the page sits in a frame of width x height: a corner at or past the frame's edge (or, when the model
/// gave a confidence, a weak one) means a cut-off page; a small share of the frame means it is too far away.
pub fn frame_issues(quad: &Quad, confidence: Option<f64>, width: f64, height: f64) -> Vec<Issue> {
    let mut issues = Vec::new();
    let (mx, my) = (width * EDGE_MARGIN, height * EDGE_MARGIN);
    let touching = quad
        .0
        .iter()
        .any(|p| p.x <= mx || p.y <= my || p.x >= width - mx || p.y >= height - my);
    if touching || confidence.is_some_and(|c| c < CUT_CONFIDENCE) {
        issues.push(Issue::Cut);
    }
    if quad.signed_area() / (width * height) < SMALL_BELOW {
        issues.push(Issue::Small);
    }
    issues
}
