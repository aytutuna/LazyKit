use crate::corners::SURE_CONFIDENCE;
use crate::geometry::{Point, Quad};
use lazy_image::{Rgba, minified};

// Moves each side of a page outline found roughly (the corner model's) onto the photo's edge, the same arithmetic in the
// same order as Lazy Scan's QuadRefiner. Within BAND of the diagonal either side of a side, every straight line is scored
// by the points along the side where it passes a ridge of the gradient across it; of the lines scoring at least
// OUTER_SHARE of the best, the outermost is the page's edge. That line is fitted to a fraction of a pixel where its edge is
// clear. A side whose edge is clear at fewer than MIN_SIDE_SUPPORT of its points keeps its place. The refinement is taken
// only when at least one side moved, the outline stays convex, no corner moved more than twice the band and its sides lie
// on stronger gradient, all told, than the outline given.
const ANALYSIS_LONG_SIDE: usize = 1000;
const EDGE_THRESHOLD: f64 = 3.0;
const VOTE_CAP: f64 = 20.0;
const BAND: f64 = 0.03;
const SAMPLES: usize = 48;
const MIN_SIDE_SUPPORT: f64 = 0.5;
const SUPPORT_REACH: f64 = 1.5;
const OUTER_SHARE: f64 = 0.7;
const OUTER_REACH: f64 = 1.0 / 3.0;

/// The page outline moved onto the photo's edges (or as given), whether it moved, and how well edges back its weakest side.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Refinement {
    pub quad: Quad,
    pub refined: bool,
    pub support: f64,
}

pub struct Field {
    w: usize,
    h: usize,
    pub gx: Vec<f64>,
    pub gy: Vec<f64>,
    diagonal: f64,
}

#[derive(Clone, Copy)]
struct Line {
    nx: f64,
    ny: f64,
    rho: f64,
}

// JS Math.round: halves go up
fn round(v: f64) -> f64 {
    let f = v.floor();
    if v - f >= 0.5 { f + 1.0 } else { f }
}

// channel `ch` smoothed by 1-4-6-4-1 across and down, edges repeated: 256 times the level
fn smooth(rgba: &[u8], w: usize, h: usize, ch: usize) -> Vec<i32> {
    let mut row = vec![0i32; w + 4];
    let mut across = vec![0i32; w * (h + 4)];
    for y in 0..h {
        for x in 0..w {
            row[x + 2] = rgba[(y * w + x) * 4 + ch] as i32;
        }
        row[0] = row[2];
        row[1] = row[2];
        row[w + 2] = row[w + 1];
        row[w + 3] = row[w + 1];
        let dst = (y + 2) * w;
        for x in 0..w {
            across[dst + x] =
                row[x] + 4 * row[x + 1] + 6 * row[x + 2] + 4 * row[x + 3] + row[x + 4];
        }
    }
    for x in 0..w {
        across[x] = across[2 * w + x];
        across[w + x] = across[2 * w + x];
        across[(h + 2) * w + x] = across[(h + 1) * w + x];
        across[(h + 3) * w + x] = across[(h + 1) * w + x];
    }
    let mut out = vec![0i32; w * h];
    for y in 0..h {
        let a = y * w;
        let (b, c, d, e) = (a + w, a + 2 * w, a + 3 * w, a + 4 * w);
        for x in 0..w {
            out[a + x] = across[a + x]
                + 4 * across[b + x]
                + 6 * across[c + x]
                + 4 * across[d + x]
                + across[e + x];
        }
    }
    out
}

/// The strongest channel's gradient in levels per pixel (0 on the outermost pixels).
pub fn build_field(img: &Rgba) -> Field {
    let (w, h) = (img.width, img.height);
    let channels: Vec<Vec<i32>> = (0..3).map(|c| smooth(&img.data, w, h, c)).collect();
    let mut gx = vec![0.0; w * h];
    let mut gy = vec![0.0; w * h];
    if h >= 3 && w >= 3 {
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let i = y * w + x;
                let (mut best_x, mut best_y, mut best_m) = (0i64, 0i64, -1i64);
                for ch in &channels {
                    let right = ch[i - w + 1] + 2 * ch[i + 1] + ch[i + w + 1];
                    let left = ch[i - w - 1] + 2 * ch[i - 1] + ch[i + w - 1];
                    let below = ch[i + w - 1] + 2 * ch[i + w] + ch[i + w + 1];
                    let above = ch[i - w - 1] + 2 * ch[i - w] + ch[i - w + 1];
                    let (dx, dy) = ((right - left) as i64, (below - above) as i64);
                    let m = dx * dx + dy * dy;
                    if m > best_m {
                        best_m = m;
                        best_x = dx;
                        best_y = dy;
                    }
                }
                gx[i] = best_x as f64 / 2048.0;
                gy[i] = best_y as f64 / 2048.0;
            }
        }
    }
    Field {
        w,
        h,
        gx,
        gy,
        diagonal: ((w * w + h * h) as f64).sqrt(),
    }
}

// the gradient at a continuous point, bilinear between pixel centres
fn gradient_at(f: &Field, x: f64, y: f64) -> (f64, f64) {
    // a field under 3 pixels either way has no gradient anywhere
    if f.w < 3 || f.h < 3 {
        return (0.0, 0.0);
    }
    let cx = (x - 0.5).max(1.0).min(f.w as f64 - 2.000001);
    let cy = (y - 0.5).max(1.0).min(f.h as f64 - 2.000001);
    let (x0, y0) = (cx.floor(), cy.floor());
    let (tx, ty) = (cx - x0, cy - y0);
    let i = y0 as usize * f.w + x0 as usize;
    let w = f.w;
    let gx_top = f.gx[i] * (1.0 - tx) + f.gx[i + 1] * tx;
    let gx_bottom = f.gx[i + w] * (1.0 - tx) + f.gx[i + w + 1] * tx;
    let gy_top = f.gy[i] * (1.0 - tx) + f.gy[i + 1] * tx;
    let gy_bottom = f.gy[i + w] * (1.0 - tx) + f.gy[i + w + 1] * tx;
    (
        gx_top * (1.0 - ty) + gx_bottom * ty,
        gy_top * (1.0 - ty) + gy_bottom * ty,
    )
}

fn through(p: Point, q: Point) -> Line {
    let (dx, dy) = (q.x - p.x, q.y - p.y);
    let len = (dx * dx + dy * dy).sqrt();
    let (nx, ny) = (dy / len, -dx / len);
    Line {
        nx,
        ny,
        rho: nx * p.x + ny * p.y,
    }
}

fn intersect(a: Line, b: Line) -> Option<Point> {
    let det = a.nx * b.ny - a.ny * b.nx;
    if det.abs() < 1e-9 {
        return None;
    }
    Some(Point::new(
        (a.rho * b.ny - b.rho * a.ny) / det,
        (a.nx * b.rho - b.nx * a.rho) / det,
    ))
}

// total least squares line through the points
fn fit(points: &[Point]) -> Line {
    let n = points.len() as f64;
    let (mut mx, mut my) = (0.0, 0.0);
    for p in points {
        mx += p.x;
        my += p.y;
    }
    mx /= n;
    my /= n;
    let (mut sxx, mut sxy, mut syy) = (0.0, 0.0, 0.0);
    for p in points {
        let (dx, dy) = (p.x - mx, p.y - my);
        sxx += dx * dx;
        sxy += dx * dy;
        syy += dy * dy;
    }
    let half = (sxx - syy) / 2.0;
    let lambda = (sxx + syy) / 2.0 + (half * half + sxy * sxy).sqrt();
    let (ax, ay) = (lambda - syy, sxy);
    let (bx, by) = (sxy, lambda - sxx);
    let (mut dir_x, mut dir_y) = if ax.abs() + ay.abs() >= bx.abs() + by.abs() {
        (ax, ay)
    } else {
        (bx, by)
    };
    let mut len = (dir_x * dir_x + dir_y * dir_y).sqrt();
    if len == 0.0 {
        dir_x = 1.0;
        dir_y = 0.0;
        len = 1.0;
    }
    let (nx, ny) = (-dir_y / len, dir_x / len);
    Line {
        nx,
        ny,
        rho: nx * mx + ny * my,
    }
}

// refitted twice without the points farther from the line than 2.5 times the median distance (and 0.75 px)
fn fit_line(points: &[Point]) -> Line {
    let mut current = points.to_vec();
    let mut line = fit(&current);
    for _ in 0..2 {
        let distances: Vec<f64> = current
            .iter()
            .map(|p| (line.nx * p.x + line.ny * p.y - line.rho).abs())
            .collect();
        let mut sorted = distances.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let limit = 0.75f64.max(2.5 * sorted[sorted.len() / 2]);
        let kept: Vec<Point> = current
            .iter()
            .zip(&distances)
            .filter(|(_, d)| **d <= limit)
            .map(|(p, _)| *p)
            .collect();
        if kept.len() < 4 || kept.len() == current.len() {
            break;
        }
        current = kept;
        line = fit(&current);
    }
    line
}

fn in_field(f: &Field, x: f64, y: f64) -> bool {
    x >= 1.5 && y >= 1.5 && x <= f.w as f64 - 1.5 && y <= f.h as f64 - 1.5
}

fn fit_side(f: &Field, p: Point, q: Point, reach: f64, outward: f64) -> Option<Line> {
    let (dx, dy) = (q.x - p.x, q.y - p.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len.is_nan() || len < 8.0 || len > 1e6 {
        return None;
    }
    let (nx, ny) = (dy / len, -dx / len);
    let r = (reach.ceil() as usize).max(2);
    let span = 2 * r + 1;
    let n = SAMPLES;
    let mut values = vec![0.0f64; n * span];
    let mut xs = vec![0.0f64; n];
    let mut ys = vec![0.0f64; n];
    let mut inside = vec![false; n];
    let mut used = 0usize;
    for s in 0..n {
        let t = 0.1 + (0.8 * (s as f64 + 0.5)) / n as f64;
        let (x, y) = (p.x + dx * t, p.y + dy * t);
        xs[s] = x;
        ys[s] = y;
        if !in_field(f, x, y) {
            continue;
        }
        inside[s] = true;
        used += 1;
        for k in 0..span {
            let off = k as f64 - r as f64;
            let (ox, oy) = (x + nx * off, y + ny * off);
            if !in_field(f, ox, oy) {
                continue;
            }
            let g = gradient_at(f, ox, oy);
            values[s * span + k] = g.0 * nx + g.1 * ny;
        }
    }
    if used * 4 < n {
        return None;
    }

    // each ridge marked a pixel either way, so a line drawn through whole pixels still passes it
    let mut hits = vec![false; 2 * n * span];
    for side in 0..2 {
        let sign = if side == 0 { -1.0 } else { 1.0 };
        for s in 0..n {
            if !inside[s] {
                continue;
            }
            for k in 1..span - 1 {
                let v = sign * values[s * span + k];
                if v < EDGE_THRESHOLD {
                    continue;
                }
                if v < sign * values[s * span + k - 1] || v < sign * values[s * span + k + 1] {
                    continue;
                }
                for d in k - 1..=k + 1 {
                    hits[(side * n + s) * span + d] = true;
                }
            }
        }
    }
    let mut counts = vec![0i32; 2 * span * span];
    let mut strengths = vec![0.0f64; 2 * span * span];
    let mut best_count = 0i32;
    let mut best_index: Option<usize> = None;
    let mut best_near = f64::INFINITY;
    for side in 0..2 {
        let sign = if side == 0 { -1.0 } else { 1.0 };
        for a in 0..span {
            for b in 0..span {
                let mut count = 0;
                let mut strength = 0.0;
                for s in 0..n {
                    if !inside[s] {
                        continue;
                    }
                    let k = round(a as f64 + ((b as f64 - a as f64) * s as f64) / (n - 1) as f64)
                        as usize;
                    if !hits[(side * n + s) * span + k] {
                        continue;
                    }
                    count += 1;
                    strength += (sign * values[s * span + k]).min(VOTE_CAP);
                }
                let index = (side * span + a) * span + b;
                counts[index] = count;
                strengths[index] = strength;
                // the best line: the most points, the nearer to the side as given on a tie
                let near = (a as f64 - r as f64).abs() + (b as f64 - r as f64).abs();
                if count > best_count || (count == best_count && near < best_near) {
                    best_count = count;
                    best_index = Some(index);
                    best_near = near;
                }
            }
        }
    }
    if (best_count as f64) < 6f64.max(MIN_SIDE_SUPPORT * used as f64) {
        return None;
    }
    let mut best_sign = 1.0;
    let mut best_out = f64::NEG_INFINITY;
    let mut best_score = 0;
    let mut best_strength = 0.0;
    let (mut best_start, mut best_end) = (r, r);
    let out_reach = 2.0 * r as f64 * OUTER_REACH;
    for side in 0..2 {
        for a in 0..span {
            for b in 0..span {
                let index = (side * span + a) * span + b;
                let count = counts[index];
                if (count as f64) < OUTER_SHARE * best_count as f64 {
                    continue;
                }
                let outness = outward * (a as f64 + b as f64 - 2.0 * r as f64);
                if outness > out_reach && Some(index) != best_index {
                    continue;
                }
                if outness > best_out
                    || (outness == best_out
                        && (count > best_score
                            || (count == best_score && strengths[index] > best_strength)))
                {
                    best_out = outness;
                    best_score = count;
                    best_strength = strengths[index];
                    best_sign = if side == 0 { -1.0 } else { 1.0 };
                    best_start = a;
                    best_end = b;
                }
            }
        }
    }

    let mut points = Vec::new();
    for s in 0..n {
        if !inside[s] {
            continue;
        }
        let centre = round(
            best_start as f64 + ((best_end as f64 - best_start as f64) * s as f64) / (n - 1) as f64,
        ) as i64;
        let mut at: i64 = -1;
        let mut top = 0.0;
        for k in (centre - 2).max(0)..=(centre + 2).min(span as i64 - 1) {
            let v = best_sign * values[s * span + k as usize];
            if v >= EDGE_THRESHOLD && v > top {
                top = v;
                at = k;
            }
        }
        if at < 0 {
            continue;
        }
        let mut delta = 0.0;
        if at > 0 && at < span as i64 - 1 {
            let before = best_sign * values[s * span + at as usize - 1];
            let after = best_sign * values[s * span + at as usize + 1];
            let den = before - 2.0 * top + after;
            if den < 0.0 {
                delta = ((0.5 * (before - after)) / den).clamp(-0.5, 0.5);
            }
        }
        let off = at as f64 - r as f64 + delta;
        points.push(Point::new(xs[s] + nx * off, ys[s] + ny * off));
    }
    if (points.len() as f64) < 6f64.max(MIN_SIDE_SUPPORT * used as f64) {
        return None;
    }
    Some(fit_line(&points))
}

// how well edges back q: the share of each side backed by an edge, and the sum of the gradient right on the sides
fn backing(f: &Field, q: &Quad) -> ([f64; 4], f64) {
    let mut shares = [0.0; 4];
    let mut total = 0.0;
    for i in 0..4 {
        let (p, e) = (q.0[i], q.0[(i + 1) & 3]);
        let (dx, dy) = (e.x - p.x, e.y - p.y);
        let len = (dx * dx + dy * dy).sqrt();
        if len.is_nan() || len < 1.0 {
            continue;
        }
        let (nx, ny) = (dy / len, -dx / len);
        let mut pos = [0.0f64; SAMPLES];
        let mut neg = [0.0f64; SAMPLES];
        let mut on_side = [0.0f64; SAMPLES];
        let mut used = 0usize;
        let (mut sum_pos, mut sum_neg) = (0.0, 0.0);
        for s in 0..SAMPLES {
            let t = 0.1 + (0.8 * (s as f64 + 0.5)) / SAMPLES as f64;
            let (x, y) = (p.x + dx * t, p.y + dy * t);
            if !in_field(f, x, y) {
                pos[s] = -1.0;
                neg[s] = -1.0;
                continue;
            }
            used += 1;
            let mut k = -SUPPORT_REACH;
            while k <= SUPPORT_REACH {
                let g = gradient_at(f, x + nx * k, y + ny * k);
                let v = g.0 * nx + g.1 * ny;
                pos[s] = pos[s].max(v);
                neg[s] = neg[s].max(-v);
                if k == 0.0 {
                    on_side[s] = v;
                }
                k += 0.5;
            }
            sum_pos += pos[s];
            sum_neg += neg[s];
        }
        if used * 4 < SAMPLES {
            continue;
        }
        let sign = if sum_pos >= sum_neg { 1.0 } else { -1.0 };
        let side = if sign > 0.0 { &pos } else { &neg };
        let (mut backed, mut on) = (0usize, 0.0);
        for s in 0..SAMPLES {
            if side[s] >= EDGE_THRESHOLD {
                backed += 1;
            }
            if side[s] >= 0.0 {
                on += (sign * on_side[s]).clamp(0.0, VOTE_CAP);
            }
        }
        shares[i] = backed as f64 / used as f64;
        total += on / (VOTE_CAP * used as f64);
    }
    (shares, total)
}

fn weakest(shares: &[f64; 4]) -> f64 {
    shares.iter().cloned().fold(f64::INFINITY, f64::min)
}

/// `quad` (the photo's pixels) moved onto the edges of `image`, with its edge support.
pub fn refine(image: &Rgba, quad: &Quad) -> Refinement {
    let long = image.width.max(image.height);
    let small;
    let mut small_ref = image;
    if long > ANALYSIS_LONG_SIDE {
        let s = ANALYSIS_LONG_SIDE as f64 / long as f64;
        let w = (round(image.width as f64 * s) as usize).max(1);
        let h = (round(image.height as f64 * s) as usize).max(1);
        small = minified(image, w, h);
        small_ref = &small;
    }
    let unchanged = |support| Refinement {
        quad: *quad,
        refined: false,
        support,
    };
    if small_ref.width < 16 || small_ref.height < 16 {
        return unchanged(0.0);
    }
    let field = build_field(small_ref);
    let to_x = image.width as f64 / small_ref.width as f64;
    let to_y = image.height as f64 / small_ref.height as f64;
    let q = quad.scaled(1.0 / to_x, 1.0 / to_y);
    let reach = BAND * field.diagonal;
    let outward = if q.signed_area() >= 0.0 { 1.0 } else { -1.0 };
    let mut lines = Vec::with_capacity(4);
    let mut moved = 0;
    for i in 0..4 {
        let (a, b) = (q.0[i], q.0[(i + 1) & 3]);
        if let Some(line) = fit_side(&field, a, b, reach, outward) {
            lines.push(line);
            moved += 1;
        } else if a.distance_to(b) >= 1.0 {
            lines.push(through(a, b));
        } else {
            return unchanged(0.0);
        }
    }
    let (given_shares, given_fit) = backing(&field, &q);
    let mut corners = [Point::default(); 4];
    for i in 0..4 {
        match intersect(lines[(i + 3) & 3], lines[i]) {
            Some(c) => corners[i] = c,
            None => return unchanged(weakest(&given_shares)),
        }
    }
    let result = Quad(corners);
    let near = (0..4).all(|i| result.0[i].distance_to(q.0[i]) <= 2.0 * reach);
    // only an outline lying on its edges better than the one given: a page already on its edges is not moved onto another edge near it
    if moved > 0 && result.is_convex() && near {
        let (shares, fit_total) = backing(&field, &result);
        if fit_total > given_fit {
            return Refinement {
                quad: result.scaled(to_x, to_y),
                refined: true,
                support: weakest(&shares),
            };
        }
    }
    unchanged(weakest(&given_shares))
}

/// Where a page found by the model goes, as Lazy Scan's `PhotoImport.modelPlan` and Lazy Send's page finder do it: the
/// refined corners kept within the photo, placed straight away when the model and the edges are both sure and the page can
/// be corrected, else shown in the corner editor from the same corners.
pub fn placement(
    r: Option<&Refinement>,
    given: &Quad,
    confidence: f64,
    width: f64,
    height: f64,
) -> (Quad, bool) {
    let found = r.copied().unwrap_or(Refinement {
        quad: *given,
        refined: false,
        support: 0.0,
    });
    let clamped = found.quad.clamped_to_photo(width, height);
    let fine = clamped.usable();
    (
        if fine {
            clamped
        } else {
            Quad::start(width, height)
        },
        fine && confidence >= SURE_CONFIDENCE
            && found.support >= SURE_CONFIDENCE
            && clamped.correctable(width, height),
    )
}
