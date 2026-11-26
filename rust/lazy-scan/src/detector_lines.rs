use crate::detector::{
    CORRIDOR_BINS, CORRIDOR_DEPTH, CORRIDOR_MARGIN, CORRIDOR_OUTERMOST, CORRIDOR_OUTERMOST_SHARE,
    CORRIDOR_PEAKS, EDGE_THRESHOLD, Field, HOUGH_WINDOW, ISOLATION_FLOOR, ISOLATION_RADIUS, Line,
    MAX_AREA_SHARE, MAX_CORNER_COS, MIN_LINE_SHARE, OPPOSITE_MIN_COS, OUTSIDE_SHARE, Options,
    VOTE_CAP, angle_tables, at,
};
use crate::detector_regions::fit_line;
use crate::geometry::{Point, Quad};

// The straight edges of the analysis image and the outlines made of them.

/// The thinned edge pixels of the analysis image, with what the line searches need.
pub(crate) struct EdgeSet {
    pub count: usize,
    /// Pixel centres.
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    /// Unit gradients, turned to point down (or right when level).
    pub nx: Vec<f64>,
    pub ny: Vec<f64>,
    /// Hough votes: the gradient up to VOTE_CAP, less when crowded.
    pub weight: Vec<f64>,
    /// Direction bins (0 to 179), and the pixels by bin: those of bin k are order[start[k]] to order[start[k + 1] - 1].
    pub bin: Vec<usize>,
    pub start: Vec<usize>,
    pub order: Vec<usize>,
}

/// The edge pixels: gradients of at least EDGE_THRESHOLD thinned to their ridge (larger than the neighbour behind across the
/// gradient, at least the one ahead), each with its direction and its vote: its gradient up to VOTE_CAP, times 1.5 - 2.5 times
/// the share of edge pixels within ISOLATION_RADIUS px (between ISOLATION_FLOOR and 1).
pub(crate) fn find_edges(f: &Field) -> EdgeSet {
    let (w, h) = (f.w, f.h);
    let mut edges: Vec<usize> = Vec::new();
    let mut is_edge = vec![0i64; w * h];
    if h >= 3 && w >= 3 {
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let i = y * w + x;
                let m = f.mag[i];
                if m < EDGE_THRESHOLD {
                    continue;
                }
                let (ax, ay) = (f.gx[i].abs(), f.gy[i].abs());
                let (a, b) = if ay <= ax * 0.41421356 {
                    (i - 1, i + 1)
                } else if ax <= ay * 0.41421356 {
                    (i - w, i + w)
                } else if f.gx[i] * f.gy[i] > 0.0 {
                    (i - w - 1, i + w + 1)
                } else {
                    (i - w + 1, i + w - 1)
                };
                if m > f.mag[a] && m >= f.mag[b] {
                    edges.push(i);
                    is_edge[i] = 1;
                }
            }
        }
    }

    // the count of edge pixels in every rectangle, from a summed-area table
    let stride = w + 1;
    let mut integral = vec![0i64; stride * (h + 1)];
    for y in 0..h {
        let mut run = 0;
        for x in 0..w {
            run += is_edge[y * w + x];
            integral[(y + 1) * stride + x + 1] = integral[y * stride + x + 1] + run;
        }
    }

    let ne = edges.len();
    let mut es = EdgeSet {
        count: ne,
        x: vec![0.0; ne],
        y: vec![0.0; ne],
        nx: vec![0.0; ne],
        ny: vec![0.0; ne],
        weight: vec![0.0; ne],
        bin: vec![0; ne],
        start: Vec::new(),
        order: Vec::new(),
    };
    let radius = ISOLATION_RADIUS as usize;
    for (e, &i) in edges.iter().enumerate() {
        let (x, y) = (i % w, i / w);
        let m = f.mag[i];
        let (mut nx, mut ny) = (f.gx[i] / m, f.gy[i] / m);
        if ny < 0.0 || (ny == 0.0 && nx < 0.0) {
            nx = -nx;
            ny = -ny;
        }
        es.x[e] = x as f64 + 0.5;
        es.y[e] = y as f64 + 0.5;
        es.nx[e] = nx;
        es.ny[e] = ny;
        es.bin[e] = angle_bin(nx) % 180;
        let x0 = x.saturating_sub(radius);
        let x1 = w.min(x + radius + 1);
        let y0 = y.saturating_sub(radius);
        let y1 = h.min(y + radius + 1);
        let crowd =
            integral[y1 * stride + x1] - integral[y0 * stride + x1] - integral[y1 * stride + x0]
                + integral[y0 * stride + x0];
        let density = crowd as f64 / ((x1 - x0) * (y1 - y0)) as f64;
        es.weight[e] = m.min(VOTE_CAP) * (1.5 - 2.5 * density).clamp(ISOLATION_FLOOR, 1.0);
    }

    // the edge pixels by direction (in the order found within each bin), for refitting
    let mut start = vec![0usize; 181];
    for &bin in &es.bin {
        start[bin + 1] += 1;
    }
    for k in 0..180 {
        start[k + 1] += start[k];
    }
    let mut fill = vec![0usize; 180];
    let mut order = vec![0usize; ne];
    for (e, &k) in es.bin.iter().enumerate() {
        order[start[k] + fill[k]] = e;
        fill[k] += 1;
    }
    es.start = start;
    es.order = order;
    es
}

// the direction bin `d` bins from `k`, round the half turn
fn wrap(k: i32) -> usize {
    (k.rem_euclid(180)) as usize
}

/// The straight edges of the analysis image, strongest first: the edge pixels vote in a Hough transform (1 degree by 1 px,
/// within HOUGH_WINDOW degrees of their own direction, their weight); each peak is refitted to the edge pixels within 2.5 px of
/// it that face within 12 degrees of its way, and repeats are dropped. At most `max_lines`, from at most three times as many
/// peaks.
pub(crate) fn find_lines(f: &Field, es: &EdgeSet, max_lines: usize) -> Vec<Line> {
    let (w, h) = (f.w, f.h);
    let max_rho = f.diagonal().ceil() as i64 + 1;
    let nr = (2 * max_rho + 1) as usize;
    let mut acc = vec![0.0f64; 180 * nr];
    let (cos_t, sin_t) = angle_tables();
    for e in 0..es.count {
        let k = es.bin[e] as i32;
        let weight = es.weight[e];
        for d in -HOUGH_WINDOW..=HOUGH_WINDOW {
            let kk = wrap(k + d);
            let rho = es.x[e] * cos_t[kk] + es.y[e] * sin_t[kk];
            let ri = (rho + 0.5).floor() as i64 + max_rho;
            acc[kk * nr + ri as usize] += weight;
        }
    }
    let peaks = find_peaks(
        &acc,
        180,
        nr,
        MIN_LINE_SHARE * w.min(h) as f64 * EDGE_THRESHOLD,
    );
    let mut lines: Vec<Line> = Vec::new();
    for (tried, peak) in peaks.into_iter().enumerate() {
        if lines.len() >= max_lines || tried >= 3 * max_lines {
            break;
        }
        let (k, r) = (peak.1 / nr, peak.1 % nr);
        let mut line = Line {
            nx: cos_t[k],
            ny: sin_t[k],
            rho: (r as i64 - max_rho) as f64,
        };
        line = refit_line(es, k, &line).unwrap_or(line);
        if !is_repeat(&lines, &line) {
            lines.push(line);
        }
    }
    lines
}

/// Lines that could be edge `side` of `q`: a Hough transform of the edge pixels between the lines of its two neighbouring
/// edges (at most CORRIDOR_MARGIN px outside either), no deeper inside the outline than CORRIDOR_DEPTH of its depth there,
/// facing within 20 degrees of the edge's normal, for directions within CORRIDOR_BINS degrees of the edge's; the
/// CORRIDOR_PEAKS strongest peaks and the CORRIDOR_OUTERMOST outermost others, refitted, without repeats.
pub(crate) fn corridor_lines(es: &EdgeSet, q: &Quad, side: usize) -> Vec<Line> {
    let mut result: Vec<Line> = Vec::new();
    let (p0, p1) = (at(q, side), at(q, side + 1));
    if p0.distance_to(p1) < 4.0 {
        return result;
    }
    let edge = Line::through(p0, p1);
    let before = Line::through(at(q, side + 3), at(q, side));
    let after = Line::through(at(q, side + 1), at(q, side + 2));
    let mx = (at(q, side + 2).x + at(q, side + 3).x) / 2.0;
    let my = (at(q, side + 2).y + at(q, side + 3).y) / 2.0;
    let depth = -edge.distance(mx, my);
    let mut nx = edge.nx;
    if edge.ny < 0.0 || (edge.ny == 0.0 && nx < 0.0) {
        nx = -nx;
    }
    let k0 = (angle_bin(nx) % 180) as i32;
    let (cos_t, sin_t) = angle_tables();
    let mut members: Vec<usize> = Vec::new();
    for e in 0..es.count {
        let (x, y) = (es.x[e], es.y[e]);
        if before.distance(x, y) > CORRIDOR_MARGIN || after.distance(x, y) > CORRIDOR_MARGIN {
            continue;
        }
        if edge.distance(x, y) < -CORRIDOR_DEPTH * depth {
            continue;
        }
        let c = es.nx[e] * edge.nx + es.ny[e] * edge.ny;
        if c < 0.94 && c > -0.94 {
            continue;
        }
        members.push(e);
    }
    if members.len() < 10 {
        return result;
    }
    // each member votes for the directions within HOUGH_WINDOW of its own (d, relative to the edge's bin k0, within
    // CORRIDOR_BINS); first the distances they reach, so the accumulator spans only those, with room for the peak search
    let span = (2 * CORRIDOR_BINS + 1) as usize;
    let mut first = vec![0i32; members.len()];
    let mut last = vec![0i32; members.len()];
    let (mut lowest, mut highest) = (i64::MAX, i64::MIN);
    for (m, &e) in members.iter().enumerate() {
        let mut rel = es.bin[e] as i32 - k0;
        if rel > 90 {
            rel -= 180;
        }
        if rel < -90 {
            rel += 180;
        }
        first[m] = (-CORRIDOR_BINS).max(rel - HOUGH_WINDOW);
        last[m] = CORRIDOR_BINS.min(rel + HOUGH_WINDOW);
        if first[m] > last[m] {
            continue;
        }
        for d in first[m]..=last[m] {
            let kk = wrap(k0 + d);
            let ri = (es.x[e] * cos_t[kk] + es.y[e] * sin_t[kk] + 0.5).floor() as i64;
            lowest = lowest.min(ri);
            highest = highest.max(ri);
        }
    }
    if lowest > highest {
        return result;
    }
    let max_rho = -lowest + 5;
    let nr = (highest - lowest + 11) as usize;
    let mut acc = vec![0.0f64; span * nr];
    for (m, &e) in members.iter().enumerate() {
        let (x, y) = (es.x[e], es.y[e]);
        if first[m] > last[m] {
            continue;
        }
        for d in first[m]..=last[m] {
            let kk = wrap(k0 + d);
            let rho = x * cos_t[kk] + y * sin_t[kk];
            let ri = (rho + 0.5).floor() as i64 + max_rho;
            acc[(d + CORRIDOR_BINS) as usize * nr + ri as usize] += es.weight[e];
        }
    }
    let peaks = find_peaks(&acc, span, nr, 10.0 * EDGE_THRESHOLD);
    // the strongest peaks, and the outermost ones (a sheet's edge lies outside the print on it): how far out each crosses the
    // normal through the edge's midpoint
    let (cx, cy) = ((p0.x + p1.x) / 2.0, (p0.y + p1.y) / 2.0);
    let mut outward: Vec<(f64, usize)> = Vec::new();
    for (i, peak) in peaks.iter().enumerate() {
        let d = (peak.1 / nr) as i32 - CORRIDOR_BINS;
        let r = (peak.1 % nr) as i64;
        let kk = wrap(k0 + d);
        let den = cos_t[kk] * edge.nx + sin_t[kk] * edge.ny;
        if den.abs() < 1e-6 {
            continue;
        }
        let along = cos_t[kk] * cx + sin_t[kk] * cy;
        outward.push((((r - max_rho) as f64 - along) / den, i));
    }
    outward.sort_by(|a, b| {
        if a.0 != b.0 {
            b.0.total_cmp(&a.0)
        } else {
            a.1.cmp(&b.1)
        }
    });
    let mut chosen: Vec<usize> = (0..peaks.len().min(CORRIDOR_PEAKS)).collect();
    let strongest = peaks.first().map_or(0.0, |p| p.0);
    let mut added = 0;
    for (_, rank) in &outward {
        if added >= CORRIDOR_OUTERMOST {
            break;
        }
        if chosen.contains(rank) || peaks[*rank].0 < CORRIDOR_OUTERMOST_SHARE * strongest {
            continue;
        }
        chosen.push(*rank);
        added += 1;
    }
    for index in chosen {
        let peak = peaks[index];
        let d = (peak.1 / nr) as i32 - CORRIDOR_BINS;
        let r = (peak.1 % nr) as i64;
        let kk = wrap(k0 + d);
        let mut line = Line {
            nx: cos_t[kk],
            ny: sin_t[kk],
            rho: (r - max_rho) as f64,
        };
        let mut points: Vec<Point> = Vec::new();
        for &e in &members {
            let dist = line.nx * es.x[e] + line.ny * es.y[e] - line.rho;
            if !(-2.5..=2.5).contains(&dist) {
                continue;
            }
            let c = es.nx[e] * line.nx + es.ny[e] * line.ny;
            if c < 0.978 && c > -0.978 {
                continue;
            }
            points.push(Point::new(es.x[e], es.y[e]));
        }
        if points.len() >= 10 {
            line = fit_line(&points);
        }
        if !is_repeat(&result, &line) {
            result.push(line);
        }
    }
    result
}

/// The local maxima of a Hough accumulator (`bins` directions by `nr` distances) of at least `min_votes`: no neighbour within
/// two directions and four distances is larger (or equal and earlier); strongest first, the earlier on a tie.
fn find_peaks(acc: &[f64], bins: usize, nr: usize, min_votes: f64) -> Vec<(f64, usize)> {
    let mut peaks: Vec<(f64, usize)> = Vec::new();
    for k in 0..bins {
        for r in 0..nr {
            let idx = k * nr + r;
            let v = acc[idx];
            if v < min_votes {
                continue;
            }
            let mut is_max = true;
            let mut dk = -2i32;
            while dk <= 2 && is_max {
                let kk = k as i32 + dk;
                if kk >= 0 && (kk as usize) < bins {
                    for dr in -4i32..=4 {
                        let rr = r as i32 + dr;
                        if rr < 0 || rr as usize >= nr || (dk == 0 && dr == 0) {
                            continue;
                        }
                        let n = kk as usize * nr + rr as usize;
                        let nv = acc[n];
                        if nv > v || (nv == v && n < idx) {
                            is_max = false;
                            break;
                        }
                    }
                }
                dk += 1;
            }
            if is_max {
                peaks.push((v, idx));
            }
        }
    }
    peaks.sort_by(|a, b| {
        if a.0 != b.0 {
            b.0.total_cmp(&a.0)
        } else {
            a.1.cmp(&b.1)
        }
    });
    peaks
}

/// A line within 3 degrees and 6 px of one in `lines`.
fn is_repeat(lines: &[Line], line: &Line) -> bool {
    lines.iter().any(|l| {
        let dot = l.nx * line.nx + l.ny * line.ny;
        let rho_diff = if dot >= 0.0 {
            (l.rho - line.rho).abs()
        } else {
            (l.rho + line.rho).abs()
        };
        dot.abs() >= 0.9986 && rho_diff <= 6.0
    })
}

/// The bin (0 to 180) of the angle whose cosine is nearest `nx`.
fn angle_bin(nx: f64) -> usize {
    let (cos_t, _) = angle_tables();
    let (mut lo, mut hi) = (0usize, 180usize);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if cos_t[mid] >= nx {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    if cos_t[lo] - nx <= nx - cos_t[hi] {
        lo
    } else {
        hi
    }
}

/// The line fitted to the edge pixels within 2.5 px of `line` (of direction bin `bin`) whose gradient is within 12 degrees of
/// its normal, looked for in the bins within 13 degrees of it; `None` when fewer than 10.
fn refit_line(es: &EdgeSet, bin: usize, line: &Line) -> Option<Line> {
    let mut points: Vec<Point> = Vec::new();
    for d in -13i32..=13 {
        let k = wrap(bin as i32 + d);
        for j in es.start[k]..es.start[k + 1] {
            let e = es.order[j];
            let dist = line.nx * es.x[e] + line.ny * es.y[e] - line.rho;
            if !(-2.5..=2.5).contains(&dist) {
                continue;
            }
            let c = es.nx[e] * line.nx + es.ny[e] * line.ny;
            if c < 0.978 && c > -0.978 {
                continue;
            }
            points.push(Point::new(es.x[e], es.y[e]));
        }
    }
    if points.len() < 10 {
        return None;
    }
    Some(fit_line(&points))
}

/// Outlines made of the straight edges: two pairs of near-parallel edges (within 40 degrees, at least 15 % of the short side
/// apart), the pairs crossing at 40 degrees or more; the four crossings, clockwise, with every corner at most OUTSIDE_SHARE of
/// the frame outside it.
pub(crate) fn line_quads(f: &Field, lines: &[Line], o: &Options) -> Vec<Quad> {
    let mut quads: Vec<Quad> = Vec::new();
    let n = lines.len();
    let min_gap = 0.15 * f.w.min(f.h) as f64;
    let mut pairs: Vec<(usize, usize)> = Vec::new();
    for i in 0..n {
        for j in i + 1..n {
            let dot = lines[i].nx * lines[j].nx + lines[i].ny * lines[j].ny;
            if dot.abs() < OPPOSITE_MIN_COS {
                continue;
            }
            // the gap between the two lines at the frame's centre
            let (cx, cy) = (f.w as f64 / 2.0, f.h as f64 / 2.0);
            let pa = project(&lines[i], cx, cy);
            let gap = lines[j].distance(pa.x, pa.y).abs();
            if gap < min_gap {
                continue;
            }
            pairs.push((i, j));
        }
    }
    let (min_x, max_x) = (
        -OUTSIDE_SHARE * f.w as f64,
        (1.0 + OUTSIDE_SHARE) * f.w as f64,
    );
    let (min_y, max_y) = (
        -OUTSIDE_SHARE * f.h as f64,
        (1.0 + OUTSIDE_SHARE) * f.h as f64,
    );
    let frame = f.w as f64 * f.h as f64;
    for p in 0..pairs.len() {
        for r in p + 1..pairs.len() {
            let (a, b) = pairs[p];
            let (c, d) = pairs[r];
            if a == c || a == d || b == c || b == d {
                continue;
            }
            let crossing = (lines[a].nx * lines[c].nx + lines[a].ny * lines[c].ny).abs();
            if crossing > MAX_CORNER_COS {
                continue;
            }
            let (Some(p0), Some(p1), Some(p2), Some(p3)) = (
                Line::intersect(&lines[a], &lines[c]),
                Line::intersect(&lines[c], &lines[b]),
                Line::intersect(&lines[b], &lines[d]),
                Line::intersect(&lines[d], &lines[a]),
            ) else {
                continue;
            };
            let corners = [p0, p1, p2, p3];
            let inside = corners
                .iter()
                .all(|pt| !(pt.x < min_x || pt.x > max_x || pt.y < min_y || pt.y > max_y));
            if !inside {
                continue;
            }
            let q = Quad::from_corners(corners);
            if !q.is_convex() {
                continue;
            }
            let share = q.area() / frame;
            if share < o.min_area_share || share > MAX_AREA_SHARE {
                continue;
            }
            quads.push(q);
        }
    }
    quads
}

fn project(l: &Line, x: f64, y: f64) -> Point {
    let d = l.distance(x, y);
    Point::new(x - l.nx * d, y - l.ny * d)
}
