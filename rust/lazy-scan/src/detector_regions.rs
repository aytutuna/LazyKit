use crate::background::percentile;
use crate::detector::{
    EDGE_THRESHOLD, Field, Line, MAX_HULL_VERTICES, MIN_REGION_CONTRAST, Options, POLARITY_REACH,
    at,
};
use crate::geometry::{Point, Quad};

pub(crate) use crate::refiner::fit_line;

// Outlines of the regions that stand out from the table, and an outline refitted to the edges around it.

// Andrew's monotone chain, clockwise on screen; fewer than three distinct points come back as they are
fn convex_hull(points: &[Point]) -> Vec<Point> {
    let mut pts = points.to_vec();
    pts.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    pts.dedup();
    if pts.len() < 3 {
        return pts;
    }
    let cross_z =
        |o: Point, a: Point, b: Point| (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x);
    let mut lower: Vec<Point> = Vec::new();
    for &p in &pts {
        while lower.len() >= 2 && cross_z(lower[lower.len() - 2], lower[lower.len() - 1], p) <= 0.0
        {
            lower.pop();
        }
        lower.push(p);
    }
    let mut upper: Vec<Point> = Vec::new();
    for &p in pts.iter().rev() {
        while upper.len() >= 2 && cross_z(upper[upper.len() - 2], upper[upper.len() - 1], p) <= 0.0
        {
            upper.pop();
        }
        upper.push(p);
    }
    lower.pop();
    upper.pop();
    lower.extend(upper);
    lower
}

// a square (2r+1) dilation: a pixel is set when any pixel within r (per axis) is
fn dilate(mask: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut tmp = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let any = (x.saturating_sub(r)..=(w - 1).min(x + r)).any(|xx| mask[y * w + xx] == 1);
            tmp[y * w + x] = any as u8;
        }
    }
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let any = (y.saturating_sub(r)..=(h - 1).min(y + r)).any(|yy| tmp[yy * w + x] == 1);
            out[y * w + x] = any as u8;
        }
    }
    out
}

// a square (2r+1) erosion: a pixel stays set when every pixel within r is; outside the image counts as clear
fn erode(mask: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut tmp = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let inside = x >= r && x + r < w;
            let all = inside && (x - r..=x + r).all(|xx| mask[y * w + xx] == 1);
            tmp[y * w + x] = all as u8;
        }
    }
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let inside = y >= r && y + r < h;
            let all = inside && (y - r..=y + r).all(|yy| tmp[yy * w + x] == 1);
            out[y * w + x] = all as u8;
        }
    }
    out
}

// one 4-connected part of a mask: its area and each row's horizontal extent
struct Part {
    area: usize,
    rows: Vec<(usize, usize, usize)>,
}

// the 4-connected parts of the set pixels, in the order their first pixel appears (row-major)
fn connected_components(mask: &[u8], width: usize, height: usize) -> Vec<Part> {
    let mut visited = vec![false; mask.len()];
    let mut parts: Vec<Part> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    for start in 0..mask.len() {
        if mask[start] != 1 || visited[start] {
            continue;
        }
        let mut extent: std::collections::BTreeMap<usize, (usize, usize)> = Default::default();
        let mut area = 0;
        visited[start] = true;
        stack.clear();
        stack.push(start);
        while let Some(idx) = stack.pop() {
            let (x, y) = (idx % width, idx / width);
            area += 1;
            extent
                .entry(y)
                .and_modify(|s| *s = (s.0.min(x), s.1.max(x)))
                .or_insert((x, x));
            let mut visit = |n: usize| {
                if mask[n] == 1 && !visited[n] {
                    visited[n] = true;
                    stack.push(n);
                }
            };
            if x > 0 {
                visit(idx - 1);
            }
            if x < width - 1 {
                visit(idx + 1);
            }
            if y > 0 {
                visit(idx - width);
            }
            if y < height - 1 {
                visit(idx + width);
            }
        }
        parts.push(Part {
            area,
            rows: extent.into_iter().map(|(y, (a, b))| (y, a, b)).collect(),
        });
    }
    parts
}

/// Outlines of the regions that stand out, found at half the analysis resolution (2 x 2 means): pixels whose colour differs
/// from the median colour of the frame's border (3 % of the short side) by more than an Otsu threshold (at least
/// MIN_REGION_CONTRAST), and pixels brighter than the Otsu threshold of the brightness; each mask opened by one pixel, its two
/// largest parts of at least the least page size taken, each part's convex hull simplified to MAX_HULL_VERTICES corners and
/// the four of them enclosing the largest area kept, in analysis pixels.
pub(crate) fn region_quads(f: &Field, o: &Options) -> Vec<Quad> {
    let fw = f.w;
    let (w, h) = (f.w / 2, f.h / 2);
    let count = w * h;
    let mut quads: Vec<Quad> = Vec::new();
    let (mut cr, mut cg, mut cb) = (
        vec![0usize; count],
        vec![0usize; count],
        vec![0usize; count],
    );
    for y in 0..h {
        for x in 0..w {
            let a = 2 * y * fw + 2 * x;
            let b = a + fw;
            let i = y * w + x;
            cr[i] = ((f.r[a] + f.r[a + 1] + f.r[b] + f.r[b + 1]) >> 10) as usize;
            cg[i] = ((f.g[a] + f.g[a + 1] + f.g[b] + f.g[b + 1]) >> 10) as usize;
            cb[i] = ((f.b[a] + f.b[a + 1] + f.b[b] + f.b[b + 1]) >> 10) as usize;
        }
    }
    let ring = 2usize.max((0.03 * w.min(h) as f64).round() as usize);
    let (mut hr, mut hg, mut hb) = (vec![0i32; 256], vec![0i32; 256], vec![0i32; 256]);
    let mut ring_count = 0;
    for y in 0..h {
        for x in 0..w {
            if x >= ring && x + ring < w && y >= ring && y + ring < h {
                continue;
            }
            let i = y * w + x;
            hr[cr[i]] += 1;
            hg[cg[i]] += 1;
            hb[cb[i]] += 1;
            ring_count += 1;
        }
    }
    let mr = percentile(&hr, ring_count, 0.5) as i64;
    let mg = percentile(&hg, ring_count, 0.5) as i64;
    let mb = percentile(&hb, ring_count, 0.5) as i64;

    let mut distance = vec![0i32; count];
    let mut luma = vec![0usize; count];
    let (mut hd, mut hl) = (vec![0i32; 256], vec![0i32; 256]);
    for i in 0..count {
        let (r, g, b) = (cr[i] as i64, cg[i] as i64, cb[i] as i64);
        let d = (r - mr).abs().max((g - mg).abs().max((b - mb).abs())) as usize;
        distance[i] = d as i32;
        hd[d] += 1;
        let l = ((299 * r + 587 * g + 114 * b + 500) / 1000) as usize;
        luma[i] = l;
        hl[l] += 1;
    }
    let td = MIN_REGION_CONTRAST.max(otsu(&hd, count));
    let tl = otsu(&hl, count) as usize;
    for pass in 0..2 {
        let mut mask = vec![0u8; count];
        let mut marked = 0usize;
        for i in 0..count {
            let on = if pass == 0 {
                distance[i] > td
            } else {
                luma[i] > tl
            };
            if on {
                mask[i] = 1;
                marked += 1;
            }
        }
        if (marked as f64) < o.min_area_share * count as f64 || marked as f64 > 0.97 * count as f64
        {
            continue;
        }
        mask = dilate(&erode(&mask, w, h, 1), w, h, 1);
        let parts = connected_components(&mask, w, h);
        let mut big: Vec<(usize, usize)> = parts
            .iter()
            .enumerate()
            .filter(|(_, p)| p.area as f64 >= o.min_area_share * count as f64)
            .map(|(i, p)| (p.area, i))
            .collect();
        big.sort_by(|a, b| {
            if a.0 != b.0 {
                b.0.cmp(&a.0)
            } else {
                a.1.cmp(&b.1)
            }
        });
        for &(_, index) in big.iter().take(2) {
            let mut points: Vec<Point> = Vec::new();
            for &(y, min_x, max_x) in &parts[index].rows {
                let (x0, x1) = (2.0 * min_x as f64, 2.0 * (max_x + 1) as f64);
                let (y0, y1) = (2.0 * y as f64, 2.0 * (y + 1) as f64);
                points.push(Point::new(x0, y0));
                points.push(Point::new(x1, y0));
                points.push(Point::new(x0, y1));
                points.push(Point::new(x1, y1));
            }
            let hull = simplify(convex_hull(&points), MAX_HULL_VERTICES);
            if let Some(q) = largest_quad(&hull) {
                quads.push(q);
            }
        }
    }
    quads
}

/// The Otsu threshold of a 256-level histogram: values above it are the brighter class.
pub(crate) fn otsu(histogram: &[i32], total: usize) -> i32 {
    let sum_all: f64 = histogram
        .iter()
        .take(256)
        .enumerate()
        .map(|(v, &n)| v as f64 * n as f64)
        .sum();
    let (mut sum_b, mut best) = (0.0, -1.0);
    let mut w_b = 0usize;
    let mut threshold = 0;
    for (t, &n) in histogram.iter().enumerate().take(256) {
        w_b += n as usize;
        if w_b == 0 {
            continue;
        }
        let w_f = total - w_b;
        if w_f == 0 {
            break;
        }
        sum_b += t as f64 * n as f64;
        let m_b = sum_b / w_b as f64;
        let m_f = (sum_all - sum_b) / w_f as f64;
        let between = w_b as f64 * w_f as f64 * (m_b - m_f) * (m_b - m_f);
        if between > best {
            best = between;
            threshold = t as i32;
        }
    }
    threshold
}

/// A convex polygon cut to at most `limit` corners by dropping, one at a time, the corner whose triangle with its neighbours is
/// smallest (the first on a tie).
pub(crate) fn simplify(polygon: Vec<Point>, limit: usize) -> Vec<Point> {
    let mut p = polygon;
    while p.len() > limit {
        let n = p.len();
        let (mut worst, mut smallest) = (0, f64::MAX);
        for i in 0..n {
            let (a, b, c) = (p[(i + n - 1) % n], p[i], p[(i + 1) % n]);
            let area = ((b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x)).abs();
            if area < smallest {
                smallest = area;
                worst = i;
            }
        }
        p.remove(worst);
    }
    p
}

fn cross(o: Point, a: Point, b: Point) -> f64 {
    (a.x - o.x) * (b.y - o.y) - (a.y - o.y) * (b.x - o.x)
}

/// The four corners of a convex polygon that enclose the largest area, as a quad; `None` for fewer than four corners.
pub(crate) fn largest_quad(hull: &[Point]) -> Option<Quad> {
    let n = hull.len();
    if n < 4 {
        return None;
    }
    let mut best = -1.0;
    let (mut ba, mut bb, mut bc, mut bd) = (0, 0, 0, 0);
    for a in 0..n {
        for c in a + 2..n {
            if n - (c - a) < 2 {
                continue;
            }
            let (mut best1, mut best2) = (-1.0, -1.0);
            let (mut b1, mut d1) = (None, None);
            for b in a + 1..c {
                let t = cross(hull[a], hull[b], hull[c]).abs();
                if t > best1 {
                    best1 = t;
                    b1 = Some(b);
                }
            }
            for d in c + 1..a + n {
                let t = cross(hull[c], hull[d % n], hull[a]).abs();
                if t > best2 {
                    best2 = t;
                    d1 = Some(d % n);
                }
            }
            let (Some(b1), Some(d1)) = (b1, d1) else {
                continue;
            };
            let area = best1 + best2;
            if area > best {
                best = area;
                ba = a;
                bb = b1;
                bc = c;
                bd = d1;
            }
        }
    }
    if best <= 0.0 {
        return None;
    }
    let q = Quad::from_corners([hull[ba], hull[bb], hull[bc], hull[bd]]);
    q.is_convex().then_some(q)
}

/// The outline with each edge refitted to the edge points within `reach` px across it (`refit_side`; an edge with too few keeps
/// its line, at most one), the corners where the new edges cross; `None` when fewer than three edges refit, the result is not
/// convex, or a corner moved more than twice the reach and 2 px.
pub(crate) fn refine_quad(f: &Field, q: &Quad, reach: f64) -> Option<Quad> {
    let mut lines: Vec<Line> = Vec::new();
    let mut refit = 0;
    for i in 0..4 {
        let (p0, p1) = (at(q, i), at(q, i + 1));
        if p0.distance_to(p1) < 4.0 {
            return None;
        }
        match refit_side(f, p0, p1, reach) {
            Some(line) => {
                lines.push(line);
                refit += 1;
            }
            None => lines.push(Line::through(p0, p1)),
        }
    }
    if refit < 3 {
        return None;
    }
    let mut corners = [Point::default(); 4];
    for i in 0..4 {
        corners[i] = Line::intersect(&lines[(i + 3) & 3], &lines[i])?;
    }
    let result = Quad(corners);
    if !result.is_convex() {
        return None;
    }
    if (0..4).any(|i| at(&result, i).distance_to(at(q, i)) > 2.0 * reach + 2.0) {
        return None;
    }
    Some(result)
}

/// At points every 2 px along the edge (8 % to 92 %), the gradient across it within `reach` px of the sign that prevails within
/// POLARITY_REACH px of the edge as it is, the strongest (nearer ones favoured: one at the reach counts half), placed to a
/// fraction of a pixel by a parabola through its neighbours; those of at least EDGE_THRESHOLD fitted by `fit_line`. `None` when
/// fewer than a third of the points (and 6) are found.
fn refit_side(f: &Field, p: Point, q: Point, reach: f64) -> Option<Line> {
    let (dx, dy) = (q.x - p.x, q.y - p.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len.is_nan() || len < 1.0 || len > 1e6 {
        return None;
    }
    let (ux, uy) = (dx / len, dy / len);
    let (nx, ny) = (uy, -ux);
    let r = reach.ceil() as i32;
    let span = (2 * r + 1) as usize;
    let samples = ((len / 2.0) as usize).clamp(8, 300);
    let mut values = vec![0.0f64; samples * span];
    let mut usable = vec![false; samples];
    let mut xs = vec![0.0f64; samples];
    let mut ys = vec![0.0f64; samples];
    let (mut sum_pos, mut sum_neg) = (0.0, 0.0);
    let (lo, hi_x, hi_y) = (1.5, f.w as f64 - 1.5, f.h as f64 - 1.5);
    for s in 0..samples {
        let t = 0.08 + 0.84 * (s as f64 + 0.5) / samples as f64;
        let (x, y) = (p.x + dx * t, p.y + dy * t);
        xs[s] = x;
        ys[s] = y;
        let rr = r as f64;
        let (ex0, ey0, ex1, ey1) = (x - nx * rr, y - ny * rr, x + nx * rr, y + ny * rr);
        if ex0 < lo
            || ey0 < lo
            || ex1 < lo
            || ey1 < lo
            || ex0 > hi_x
            || ex1 > hi_x
            || ey0 > hi_y
            || ey1 > hi_y
        {
            continue;
        }
        usable[s] = true;
        let (mut best_pos, mut best_neg) = (0.0, 0.0);
        for k in -r..=r {
            let (gx, gy) = f.gradient_at(x + nx * k as f64, y + ny * k as f64);
            let g = gx * nx + gy * ny;
            values[s * span + (k + r) as usize] = g;
            if !(-POLARITY_REACH..=POLARITY_REACH).contains(&k) {
                continue;
            }
            if g > best_pos {
                best_pos = g;
            }
            if -g > best_neg {
                best_neg = -g;
            }
        }
        sum_pos += best_pos;
        sum_neg += best_neg;
    }
    let sign = if sum_pos >= sum_neg { 1.0 } else { -1.0 };
    let mut points: Vec<Point> = Vec::new();
    for s in 0..samples {
        if !usable[s] {
            continue;
        }
        // the strongest response of that sign, nearer ones favoured (a full-strength edge at the reach counts half), so a
        // stronger parallel edge further off does not pull the line
        let mut best: Option<usize> = None;
        let mut best_weighted = 0.0;
        for k in 0..span {
            let v = sign * values[s * span + k];
            if v < EDGE_THRESHOLD {
                continue;
            }
            let weighted = v * (1.0 - 0.5 * (k as i32 - r).abs() as f64 / (r as f64 + 1.0));
            if weighted > best_weighted {
                best_weighted = weighted;
                best = Some(k);
            }
        }
        let Some(best) = best else { continue };
        let best_value = sign * values[s * span + best];
        let mut delta = 0.0;
        if best > 0 && best < span - 1 {
            let a = sign * values[s * span + best - 1];
            let c = sign * values[s * span + best + 1];
            let den = a - 2.0 * best_value + c;
            if den < 0.0 {
                delta = (0.5 * (a - c) / den).clamp(-0.5, 0.5);
            }
        }
        let off = (best as i32 - r) as f64 + delta;
        points.push(Point::new(xs[s] + nx * off, ys[s] + ny * off));
    }
    if points.len() < 6.max(samples / 3) {
        return None;
    }
    Some(fit_line(&points))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn otsu_splits_two_levels() {
        let mut histogram = vec![0i32; 256];
        histogram[40] = 300;
        histogram[45] = 200;
        histogram[200] = 400;
        histogram[210] = 100;
        let t = otsu(&histogram, 1000);
        assert!((45..=199).contains(&t), "{t}");
    }

    #[test]
    fn a_line_is_fitted_past_an_outlier() {
        let mut points: Vec<Point> = (0..20)
            .map(|i| {
                let (x, y) = (10.0 + 5.0 * i as f64, 20.0 + 2.5 * i as f64);
                Point::new(x, y + if i % 2 == 0 { 0.3 } else { -0.3 })
            })
            .collect();
        points.push(Point::new(50.0, 90.0));
        let line = fit_line(&points);
        // y = 20 + 0.5 (x - 10): every inlier within half a pixel of the line
        for p in &points[..20] {
            assert!(line.distance(p.x, p.y).abs() < 0.5);
        }
        assert!(line.distance(50.0, 90.0).abs() > 40.0);
    }

    #[test]
    fn the_largest_quad_of_an_outline_is_its_rectangle() {
        // a rectangle with its corners slightly cut, as a mask's hull comes out
        let hull: Vec<Point> = [
            (10.0, 12.0),
            (12.0, 10.0),
            (98.0, 10.0),
            (100.0, 12.0),
            (100.0, 58.0),
            (98.0, 60.0),
            (12.0, 60.0),
            (10.0, 58.0),
        ]
        .map(|(x, y)| Point::new(x, y))
        .to_vec();
        let quad = largest_quad(&simplify(hull.clone(), 40)).unwrap();
        assert!(quad.is_convex());
        assert!((4300.0..=4500.0).contains(&quad.area()), "{}", quad.area());
        assert_eq!(simplify(hull, 3).len(), 3);
    }

    #[test]
    fn the_hull_of_a_square_with_inner_points_is_its_corners() {
        let mut points: Vec<Point> = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)]
            .map(|(x, y)| Point::new(x, y))
            .to_vec();
        points.push(Point::new(5.0, 5.0));
        points.push(Point::new(0.0, 0.0));
        assert_eq!(convex_hull(&points).len(), 4);
        assert_eq!(convex_hull(&points[..2]).len(), 2);
    }
}
