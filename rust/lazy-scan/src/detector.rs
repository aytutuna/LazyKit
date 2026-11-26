use crate::detector_lines::{EdgeSet, corridor_lines, find_edges, find_lines, line_quads};
use crate::detector_regions::{refine_quad, region_quads};
use crate::geometry::{Point, Quad};
use crate::refiner::smooth;
use crate::warp::{MATCHED, PAPER_ASPECT_TOLERANCE, estimate_aspect, match_paper};
use lazy_image::{Rgba, minified};

// Finds the four corners of a sheet of paper in a camera photo, as Lazy Scan's PhotoDocumentDetector does: a page seen at an
// angle (any convex quadrilateral) on a table that may be textured, dark, light or close to the paper's own colour, with the
// phone's shadow, uneven light and glare. The same steps, constants and arithmetic in the same order (integers for the blur
// and the gradient, no trigonometry: the angle table is built from two constants), and every sort with a tie-break, so the
// apps and this find the same corners.

pub const ANALYSIS_LONG_SIDE: usize = 1000;
pub const MIN_AREA_SHARE: f64 = 0.05;
pub const MAX_AREA_SHARE: f64 = 0.995;
pub const ACCEPT_SCORE: f64 = 0.55;
pub const MIN_SIDE_SCORE: f64 = 0.35;
pub const CANDIDATE_SCORE: f64 = 0.25;
pub const EDGE_THRESHOLD: f64 = 3.0;
pub const VOTE_CAP: f64 = 20.0;
pub const HOUGH_WINDOW: i32 = 4;
pub const ISOLATION_RADIUS: i32 = 4;
pub const ISOLATION_FLOOR: f64 = 0.25;
pub const MAX_LINES: usize = 28;
pub const SWAP_LINES: usize = 60;
pub const SWAP_KEEP: usize = 4;
pub const SWAP_MIN_COS: f64 = 0.866;
pub const SWAP_SIDE_MARGIN: f64 = 0.2;
pub const CORRIDOR_MARGIN: f64 = 3.0;
pub const CORRIDOR_BELOW: f64 = 0.95;
pub const CORRIDOR_BINS: i32 = 15;
pub const CORRIDOR_DEPTH: f64 = 0.3;
pub const CORRIDOR_PEAKS: usize = 8;
pub const CORRIDOR_OUTERMOST: usize = 2;
pub const CORRIDOR_OUTERMOST_SHARE: f64 = 0.15;
pub const MIN_LINE_SHARE: f64 = 0.08;
pub const OPPOSITE_MIN_COS: f64 = 0.766;
pub const MAX_CORNER_COS: f64 = 0.766;
pub const FREE_CORNER_COS: f64 = 0.5;
pub const OUTSIDE_SHARE: f64 = 0.25;
pub const CUT_OFF_TOLERANCE: f64 = 1.0;
pub const BORDER_BAND: f64 = 2.0;
pub const SCORE_SAMPLES: usize = 48;
pub const SCREEN_SAMPLES: usize = 12;
pub const SCREEN_KEEP: usize = 30;
pub const REFINE_KEEP: usize = 8;
pub const EXTENSION_PENALTY: f64 = 0.6;
pub const EXTENSION_SHARE: f64 = 0.2;
pub const EXTENSION_MIN: f64 = 8.0;
pub const TEXTURE_RATIO: f64 = 2.0;
pub const TEXTURE_NEAR: i32 = 4;
pub const TEXTURE_FAR: i32 = 7;
pub const POLARITY_REACH: i32 = 2;
pub const SUPPORT_REACH: i32 = 2;
pub const CONTRAST_FULL: f64 = 16.0;
pub const CONTRAST_FLOOR: f64 = 0.25;
pub const PLAIN_LONG_OVER_SHORT: f64 = 4.0;
pub const MAX_LONG_OVER_SHORT: f64 = 8.0;
pub const MIXED_POLARITY_FACTOR: f64 = 0.75;
pub const PARTIAL_FACTOR: f64 = 0.9;
pub const PAPER_BONUS: f64 = 0.04;
pub const HINT_BONUS: f64 = 0.05;
pub const HINT_SHARE: f64 = 0.05;
pub const OUTER_MARGIN: f64 = 0.1;
pub const DISTINCT_SHARE: f64 = 0.08;
pub const SCREEN_DUPLICATE_SHARE: f64 = 0.005;
pub const DUPLICATE_SHARE: f64 = 0.02;
pub const AMBIGUITY_MARGIN: f64 = 0.1;
pub const LARGER_PAGE_RATIO: f64 = 1.4;
pub const MAX_HULL_VERTICES: usize = 40;
pub const MIN_REGION_CONTRAST: i32 = 14;

/// Why `detect` returned no page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    /// A page was found.
    None,
    /// Nothing in the photo looks like the outline of a sheet.
    NothingFound,
    /// The page runs off the photo: a corner lies outside the frame, or an edge along its border.
    CutOff,
    /// Two different sheets of about the same size are equally likely.
    Ambiguous,
    /// Something sheet-like was seen, but not clearly enough to trust.
    LowConfidence,
}

/// Options of `detect`; the defaults are its constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub analysis_long_side: usize,
    pub min_area_share: f64,
    pub accept_score: f64,
    pub min_side_score: f64,
    pub paper_aspect_tolerance: f64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            analysis_long_side: ANALYSIS_LONG_SIDE,
            min_area_share: MIN_AREA_SHARE,
            accept_score: ACCEPT_SCORE,
            min_side_score: MIN_SIDE_SCORE,
            paper_aspect_tolerance: PAPER_ASPECT_TOLERANCE,
        }
    }
}

/// What `detect` found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    /// The page's corners in the photo's pixels; `None` when none was taken.
    pub quad: Option<Quad>,
    /// The page's score, 0 to 1 (0 when nothing was found).
    pub confidence: f64,
    pub rejection: Rejection,
    /// The best outline seen when none was taken (cut off, ambiguous, unclear), in the photo's pixels.
    pub candidate: Option<Quad>,
}

impl Detection {
    pub const NOTHING: Detection = Detection {
        quad: None,
        confidence: 0.0,
        rejection: Rejection::NothingFound,
        candidate: None,
    };
}

// cos and sin of 0, 1, ..., 180 degrees, built by turning (1, 0) by one degree at a time
// written out as in the apps, so the table is the same bits
#[allow(clippy::excessive_precision)]
const COS1: f64 = 0.99984769515639127;
#[allow(clippy::excessive_precision)]
const SIN1: f64 = 0.017452406437283512;

pub(crate) fn angle_tables() -> ([f64; 181], [f64; 181]) {
    let mut c = [0.0; 181];
    let mut s = [0.0; 181];
    c[0] = 1.0;
    for k in 1..=180 {
        c[k] = c[k - 1] * COS1 - s[k - 1] * SIN1;
        s[k] = s[k - 1] * COS1 + c[k - 1] * SIN1;
    }
    (c, s)
}

// the corner `i` of a quad, round the outline
pub(crate) fn at(q: &Quad, i: usize) -> Point {
    q.0[i & 3]
}

/// The smoothed colours and the gradient of the analysis image.
pub(crate) struct Field {
    pub w: usize,
    pub h: usize,
    /// Smoothed channels, 256 times the level.
    pub r: Vec<i32>,
    pub g: Vec<i32>,
    pub b: Vec<i32>,
    /// The strongest channel's gradient, in levels per pixel (0 on the outermost pixels).
    pub gx: Vec<f64>,
    pub gy: Vec<f64>,
    pub mag: Vec<f64>,
}

impl Field {
    pub fn build(image: &Rgba) -> Field {
        let (w, h) = (image.width, image.height);
        let r = smooth(&image.data, w, h, 0);
        let g = smooth(&image.data, w, h, 1);
        let b = smooth(&image.data, w, h, 2);
        let mut gx = vec![0.0; w * h];
        let mut gy = vec![0.0; w * h];
        let mut mag = vec![0.0; w * h];
        if h >= 3 && w >= 3 {
            for y in 1..h - 1 {
                for x in 1..w - 1 {
                    let i = y * w + x;
                    let (mut best_x, mut best_y, mut best_m) = (0i64, 0i64, -1i64);
                    for ch in [&r, &g, &b] {
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
                    let (dx, dy) = (best_x as f64 / 2048.0, best_y as f64 / 2048.0);
                    gx[i] = dx;
                    gy[i] = dy;
                    mag[i] = (dx * dx + dy * dy).sqrt();
                }
            }
        }
        Field {
            w,
            h,
            r,
            g,
            b,
            gx,
            gy,
            mag,
        }
    }

    pub fn diagonal(&self) -> f64 {
        ((self.w * self.w + self.h * self.h) as f64).sqrt()
    }

    /// The gradient at a continuous point, bilinear between pixel centres (clamped inside the gradient's area).
    pub fn gradient_at(&self, x: f64, y: f64) -> (f64, f64) {
        let (fx, fy) = (x - 0.5, y - 0.5);
        let cx = fx.max(1.0).min(self.w as f64 - 2.000001);
        let cy = fy.max(1.0).min(self.h as f64 - 2.000001);
        let (x0f, y0f) = (cx.floor(), cy.floor());
        let (x0, y0) = (x0f as usize, y0f as usize);
        let (tx, ty) = (cx - x0f, cy - y0f);
        let i = y0 * self.w + x0;
        let w = self.w;
        let gx_top = self.gx[i] * (1.0 - tx) + self.gx[i + 1] * tx;
        let gx_bottom = self.gx[i + w] * (1.0 - tx) + self.gx[i + w + 1] * tx;
        let gy_top = self.gy[i] * (1.0 - tx) + self.gy[i + 1] * tx;
        let gy_bottom = self.gy[i + w] * (1.0 - tx) + self.gy[i + w + 1] * tx;
        (
            gx_top * (1.0 - ty) + gx_bottom * ty,
            gy_top * (1.0 - ty) + gy_bottom * ty,
        )
    }
}

/// A straight line n . p = rho with a unit normal.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Line {
    pub nx: f64,
    pub ny: f64,
    pub rho: f64,
}

impl Line {
    pub fn distance(&self, x: f64, y: f64) -> f64 {
        self.nx * x + self.ny * y - self.rho
    }

    pub fn through(p: Point, q: Point) -> Line {
        let (dx, dy) = (q.x - p.x, q.y - p.y);
        let len = (dx * dx + dy * dy).sqrt();
        let (nx, ny) = (dy / len, -dx / len);
        Line {
            nx,
            ny,
            rho: nx * p.x + ny * p.y,
        }
    }

    pub fn intersect(a: &Line, b: &Line) -> Option<Point> {
        let det = a.nx * b.ny - a.ny * b.nx;
        if det.abs() < 1e-9 {
            return None;
        }
        Some(Point::new(
            (a.rho * b.ny - b.rho * a.ny) / det,
            (a.nx * b.rho - b.nx * a.rho) / det,
        ))
    }
}

/// An outline's score; `partial` when one edge lies outside the frame or along its border, unscored.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Score {
    pub total: f64,
    pub edge: f64,
    pub min_side: f64,
    pub valid: bool,
    pub partial: bool,
}

const INVALID: Score = Score {
    total: 0.0,
    edge: 0.0,
    min_side: 0.0,
    valid: false,
    partial: false,
};

#[derive(Clone, Copy, Debug)]
pub(crate) struct Candidate {
    pub quad: Quad,
    pub score: Score,
    pub order: usize,
}

impl Candidate {
    fn new(quad: Quad, order: usize) -> Candidate {
        Candidate {
            quad,
            score: INVALID,
            order,
        }
    }
}

struct Found {
    quad: Option<Quad>,
    confidence: f64,
    rejection: Rejection,
    candidate: Option<Quad>,
}

/// The page in `image`, or none with the reason. `hint`, a page found in a previous video frame (in this photo's pixels), is
/// tried as an outline of its own and favours outlines near it.
pub fn detect(image: &Rgba, hint: Option<&Quad>, options: &Options) -> Detection {
    let o = options;
    let long_side = image.width.max(image.height);
    let small;
    let mut small_ref = image;
    if long_side > o.analysis_long_side && o.analysis_long_side > 0 {
        let s = o.analysis_long_side as f64 / long_side as f64;
        let aw = ((image.width as f64 * s).round() as usize).max(1);
        let ah = ((image.height as f64 * s).round() as usize).max(1);
        small = minified(image, aw, ah);
        small_ref = &small;
    }
    if small_ref.width < 16 || small_ref.height < 16 {
        return Detection::NOTHING;
    }
    let to_x = image.width as f64 / small_ref.width as f64;
    let to_y = image.height as f64 / small_ref.height as f64;
    let small_hint = hint.map(|h| h.scaled(1.0 / to_x, 1.0 / to_y));

    let field = Field::build(small_ref);
    let found = decide(&field, &pool(&field, small_hint.as_ref(), o), o);
    Detection {
        quad: found.quad.map(|q| q.scaled(to_x, to_y)),
        confidence: found.confidence,
        rejection: found.rejection,
        candidate: found.candidate.map(|q| q.scaled(to_x, to_y)),
    }
}

/// The scored outlines of the analysis image, best first (for tests and tuning).
pub fn explain(
    analysis: &Rgba,
    hint: Option<&Quad>,
    options: &Options,
) -> Vec<(Quad, f64, f64, f64)> {
    let f = Field::build(analysis);
    pool(&f, hint, options)
        .into_iter()
        .map(|c| (c.quad, c.score.total, c.score.edge, c.score.min_side))
        .collect()
}

fn rescored(f: &Field, quad: Quad, order: usize, hint: Option<&Quad>, o: &Options) -> Candidate {
    let mut c = Candidate::new(quad, order);
    c.score = score_quad(f, &quad, SCORE_SAMPLES, hint, o);
    c
}

// a rough outline refined to its edges first, then again closely
fn refined_twice(f: &Field, q: &Quad, reach: f64) -> Quad {
    match refine_quad(f, q, reach) {
        Some(r) => refine_quad(f, &r, 2.0).unwrap_or(r),
        None => *q,
    }
}

/// Every candidate outline, scored, refitted, best first, without repeats.
fn pool(f: &Field, hint: Option<&Quad>, o: &Options) -> Vec<Candidate> {
    let diag = f.diagonal();
    let mut pool: Vec<Candidate> = Vec::new();
    let mut order = 0;

    // straight edges combined into outlines, screened with a few samples per edge
    let edges = find_edges(f);
    let all_lines = find_lines(f, &edges, SWAP_LINES);
    let lines = &all_lines[..all_lines.len().min(MAX_LINES)];
    let mut screened: Vec<Candidate> = Vec::new();
    for q in line_quads(f, lines, o) {
        let mut c = Candidate::new(q, order);
        order += 1;
        c.score = score_quad(f, &q, SCREEN_SAMPLES, hint, o);
        if c.score.valid {
            screened.push(c);
        }
    }
    sort_by_score(&mut screened);
    let mut kept: Vec<Candidate> = Vec::new();
    for c in screened {
        if kept.len() >= SCREEN_KEEP {
            break;
        }
        let repeat = kept
            .iter()
            .any(|k| corner_distance(&c.quad, &k.quad) < SCREEN_DUPLICATE_SHARE * diag);
        if !repeat {
            kept.push(c);
        }
    }
    for mut c in kept {
        c.score = score_quad(f, &c.quad, SCORE_SAMPLES, hint, o);
        if c.score.valid {
            pool.push(c);
        }
    }

    // regions standing out from the table, refined to their edges first (their outline is rough)
    let region_reach = 4.0f64.max((0.02 * diag).round_ties_even());
    for q in region_quads(f, o) {
        let c = rescored(f, refined_twice(f, &q, region_reach), order, hint, o);
        order += 1;
        if c.score.valid {
            pool.push(c);
        }
    }

    // the previous frame's page
    if let Some(hq) = hint
        && hq.is_convex()
    {
        let c = rescored(f, refined_twice(f, hq, region_reach), order, hint, o);
        if c.score.valid {
            pool.push(c);
        }
    }

    // the best outlines refitted to their edges
    sort_by_score(&mut pool);
    let distinct = distinct_outlines(&pool, DUPLICATE_SHARE * diag);
    let mut ranked: Vec<Candidate> = Vec::new();
    for (i, c) in distinct.into_iter().enumerate() {
        ranked.push(c);
        if i >= REFINE_KEEP {
            continue;
        }
        if let Some(refined) = refine_quad(f, &c.quad, 3.0) {
            let r = rescored(f, refined, c.order, hint, o);
            if r.score.valid && r.score.total >= c.score.total {
                *ranked.last_mut().unwrap() = r;
            }
        }
    }
    sort_by_score(&mut ranked);
    ranked = distinct_outlines(&ranked, DUPLICATE_SHARE * diag);

    // the best outlines with an edge swapped for another straight edge where that scores better (an outline that took a line
    // printed on the page, or missed a weak edge)
    let count = ranked.len();
    for i in 0..count.min(SWAP_KEEP) {
        let base = ranked[i];
        if let Some(mut swapped) = swap_sides(f, &base, &all_lines, &edges, hint, o) {
            if let Some(refined) = refine_quad(f, &swapped.quad, 3.0) {
                let r = rescored(f, refined, swapped.order, hint, o);
                if r.score.valid && r.score.total >= swapped.score.total {
                    swapped = r;
                }
            }
            ranked.push(swapped);
        }
    }
    sort_by_score(&mut ranked);
    ranked = distinct_outlines(&ranked, DUPLICATE_SHARE * diag);
    sort_by_score(&mut ranked);
    ranked
}

/// `c` with one edge at a time replaced by another straight edge running within 30 degrees of it, while that scores better
/// (two rounds at most): one of the image's straight edges or one found near that edge (`corridor_lines`, a weak edge the
/// print elsewhere outvoted). `None` when no swap helps.
fn swap_sides(
    f: &Field,
    c: &Candidate,
    global_lines: &[Line],
    edges: &EdgeSet,
    hint: Option<&Quad>,
    o: &Options,
) -> Option<Candidate> {
    let mut best = *c;
    let mut improved_any = false;
    let frame = f.w as f64 * f.h as f64;
    let (min_x, max_x) = (
        -OUTSIDE_SHARE * f.w as f64,
        (1.0 + OUTSIDE_SHARE) * f.w as f64,
    );
    let (min_y, max_y) = (
        -OUTSIDE_SHARE * f.h as f64,
        (1.0 + OUTSIDE_SHARE) * f.h as f64,
    );
    for _ in 0..2 {
        let q = best.quad;
        let sides: Vec<Line> = (0..4)
            .map(|i| Line::through(at(&q, i), at(&q, i + 1)))
            .collect();
        let mut round_best: Option<Candidate> = None;
        let mut bar = best.score.total;
        for side in 0..4 {
            let held = side_score(f, &q, side, SCREEN_SAMPLES).map_or(0.0, |s| s.0);
            let mut lines: Vec<Line> = global_lines.to_vec();
            if held < CORRIDOR_BELOW {
                lines.extend(corridor_lines(edges, &q, side));
            }
            for line in &lines {
                if (line.nx * sides[side].nx + line.ny * sides[side].ny).abs() < SWAP_MIN_COS {
                    continue;
                }
                let (Some(a), Some(b)) = (
                    Line::intersect(&sides[(side + 3) & 3], line),
                    Line::intersect(line, &sides[(side + 1) & 3]),
                ) else {
                    continue;
                };
                if a.x < min_x
                    || a.x > max_x
                    || a.y < min_y
                    || a.y > max_y
                    || b.x < min_x
                    || b.x > max_x
                    || b.y < min_y
                    || b.y > max_y
                {
                    continue;
                }
                let mut corners = q.0;
                corners[side] = a;
                corners[(side + 1) & 3] = b;
                let candidate = Quad(corners);
                if !candidate.is_convex() {
                    continue;
                }
                let share = candidate.area() / frame;
                if share < o.min_area_share || share > MAX_AREA_SHARE {
                    continue;
                }
                // the new edge itself first: much weaker than the one it replaces, it cannot help
                if side_score(f, &candidate, side, SCREEN_SAMPLES).map_or(0.0, |s| s.0)
                    < held - SWAP_SIDE_MARGIN
                {
                    continue;
                }
                let screen = score_quad(f, &candidate, SCREEN_SAMPLES, hint, o);
                if !screen.valid || screen.total <= bar - 0.1 {
                    continue;
                }
                let score = score_quad(f, &candidate, SCORE_SAMPLES, hint, o);
                if !score.valid || score.total <= bar + 1e-9 {
                    continue;
                }
                bar = score.total;
                round_best = Some(Candidate {
                    quad: Quad::from_corners(corners),
                    score,
                    order: c.order,
                });
            }
        }
        let Some(improved) = round_best else { break };
        best = improved;
        improved_any = true;
    }
    improved_any.then_some(best)
}

/// The choice among the scored outlines (best first).
fn decide(f: &Field, pool: &[Candidate], o: &Options) -> Found {
    let nothing = || Found {
        quad: None,
        confidence: 0.0,
        rejection: Rejection::NothingFound,
        candidate: None,
    };
    if pool.is_empty() {
        return nothing();
    }
    let diag = f.diagonal();
    let (w, h) = (f.w as f64, f.h as f64);
    let accepted: Vec<&Candidate> = pool
        .iter()
        .filter(|c| c.score.total >= o.accept_score && c.score.min_side >= o.min_side_score)
        .collect();
    if accepted.is_empty() {
        let best = &pool[0];
        if best.score.total < CANDIDATE_SCORE {
            return nothing();
        }
        return Found {
            quad: None,
            confidence: 0.0,
            rejection: Rejection::LowConfidence,
            candidate: Some(best.quad),
        };
    }
    let top = accepted[0];
    let cut = |c: &Candidate| !c.quad.is_inside(w, h, -CUT_OFF_TOLERANCE) || c.score.partial;
    if cut(top) {
        return Found {
            quad: None,
            confidence: 0.0,
            rejection: Rejection::CutOff,
            candidate: Some(top.quad),
        };
    }
    let mut chosen = top;
    for other in accepted.iter().skip(1) {
        if other.score.total < top.score.total - AMBIGUITY_MARGIN {
            break;
        }
        if cut(other) {
            continue;
        }
        let tolerance = DUPLICATE_SHARE * diag;
        if corner_distance(&other.quad, &chosen.quad) < DISTINCT_SHARE * diag {
            // the same sheet: its outer outline (a card's edge rather than the picture on it) when about as good
            if other.score.total >= chosen.score.total - OUTER_MARGIN
                && contains_all(&other.quad, &chosen.quad, 1.0)
            {
                chosen = other;
            }
            continue;
        }
        // one inside the other or overlapping it (a sheet on a larger one, a frame printed on the page, a sheet lying across
        // another): the better one, fully seen, stays
        if contains_all(&chosen.quad, &other.quad, tolerance)
            || contains_all(&other.quad, &chosen.quad, tolerance)
        {
            continue;
        }
        if !separated(&chosen.quad, &other.quad, 0.0) {
            continue;
        }
        let (a, b) = (chosen.quad.area(), other.quad.area());
        if a.max(b) >= LARGER_PAGE_RATIO * a.min(b) {
            if b > a {
                chosen = other;
            }
            continue;
        }
        return Found {
            quad: None,
            confidence: 0.0,
            rejection: Rejection::Ambiguous,
            candidate: Some(top.quad),
        };
    }
    Found {
        quad: Some(chosen.quad),
        confidence: chosen.score.total.clamp(0.0, 1.0),
        rejection: Rejection::None,
        candidate: None,
    }
}

/// A straight line (one of their edges' lines) runs between the two convex outlines, at least `gap` from both.
pub(crate) fn separated(a: &Quad, b: &Quad, gap: f64) -> bool {
    for pass in 0..2 {
        let (p, o) = if pass == 0 { (a, b) } else { (b, a) };
        for i in 0..4 {
            let (s, e) = (at(p, i), at(p, i + 1));
            let len = s.distance_to(e);
            if len <= 0.0 {
                continue;
            }
            let all = (0..4).all(|j| {
                let c = at(o, j);
                let outside = ((e.x - s.x) * (s.y - c.y) - (e.y - s.y) * (s.x - c.x)) / len;
                outside.is_nan() || outside >= gap
            });
            if all {
                return true;
            }
        }
    }
    false
}

fn contains_all(outer: &Quad, inner: &Quad, tolerance: f64) -> bool {
    (0..4).all(|i| outer.contains(at(inner, i), tolerance))
}

/// The mean distance between the corners of two outlines, whichever corner each starts from.
pub(crate) fn corner_distance(a: &Quad, b: &Quad) -> f64 {
    let mut best = f64::MAX;
    for shift in 0..4 {
        let mut sum = 0.0;
        for i in 0..4 {
            sum += at(a, i).distance_to(at(b, i + shift));
        }
        best = best.min(sum / 4.0);
    }
    best
}

fn sort_by_score(list: &mut [Candidate]) {
    list.sort_by(|a, b| {
        if a.score.total != b.score.total {
            b.score.total.total_cmp(&a.score.total)
        } else {
            a.order.cmp(&b.order)
        }
    });
}

/// The outlines (best first) without those within `radius` of a better one; of such near-identical outlines the one kept is
/// the outer one when it scores within OUTER_MARGIN of the best (a page's edge rather than a bar printed just inside it).
fn distinct_outlines(sorted: &[Candidate], radius: f64) -> Vec<Candidate> {
    let mut kept: Vec<Candidate> = Vec::new();
    let mut best: Vec<f64> = Vec::new();
    for c in sorted {
        let Some(duplicate) = kept
            .iter()
            .position(|k| corner_distance(&c.quad, &k.quad) < radius)
        else {
            kept.push(*c);
            best.push(c.score.total);
            continue;
        };
        let held = kept[duplicate];
        if c.score.total >= best[duplicate] - OUTER_MARGIN
            && c.quad.area() > held.quad.area()
            && contains_all(&c.quad, &held.quad, 1.0)
        {
            kept[duplicate] = *c;
        }
    }
    kept
}

/// How much an outline looks like a sheet's: its edges (`side_score`), half on average and half on the weakest, times its
/// corner angles, its shape and its size, plus the bonuses for a known paper size and for lying near the hint. An edge outside
/// the frame or along its border is left out and the outline is partial, its score times PARTIAL_FACTOR; edges of mixed
/// polarity cost MIXED_POLARITY_FACTOR. Not valid (and 0) for an outline that is not convex, has a corner outside 40 to 140
/// degrees, a shape beyond MAX_LONG_OVER_SHORT, a size outside the limits, or two edges outside the frame.
pub(crate) fn score_quad(
    f: &Field,
    q: &Quad,
    samples: usize,
    hint: Option<&Quad>,
    o: &Options,
) -> Score {
    if !q.is_convex() {
        return INVALID;
    }
    let frame = f.w as f64 * f.h as f64;
    let share = q.area() / frame;
    if share < o.min_area_share || share > MAX_AREA_SHARE {
        return INVALID;
    }
    let mut shape = 1.0;
    for i in 0..4 {
        let (p, c, n) = (at(q, i + 3), at(q, i), at(q, i + 1));
        let (ax, ay) = (p.x - c.x, p.y - c.y);
        let (bx, by) = (n.x - c.x, n.y - c.y);
        let (la, lb) = ((ax * ax + ay * ay).sqrt(), (bx * bx + by * by).sqrt());
        if la < 4.0 || lb < 4.0 {
            return INVALID;
        }
        let cosine = (ax * bx + ay * by).abs() / (la * lb);
        if cosine > MAX_CORNER_COS {
            return INVALID;
        }
        if cosine > FREE_CORNER_COS {
            shape *= 1.0 - 0.5 * (cosine - FREE_CORNER_COS) / (MAX_CORNER_COS - FREE_CORNER_COS);
        }
    }
    let ratio = estimate_aspect(q, f.w as f64, f.h as f64).long_over_short();
    if ratio.is_nan() || ratio > MAX_LONG_OVER_SHORT {
        return INVALID;
    }
    let aspect_factor = if ratio <= PLAIN_LONG_OVER_SHORT {
        1.0
    } else {
        1.0 - 0.5 * (ratio - PLAIN_LONG_OVER_SHORT) / (MAX_LONG_OVER_SHORT - PLAIN_LONG_OVER_SHORT)
    };
    let size_factor = 0.85 + 0.15 * (share / 0.3).min(1.0);

    let (mut sum, mut minimum) = (0.0, f64::MAX);
    let (mut covered, mut brighter) = (0, 0);
    for i in 0..4 {
        let Some(side) = side_score(f, q, i, samples) else {
            continue;
        };
        sum += side.0;
        minimum = minimum.min(side.0);
        covered += 1;
        if side.1 > 0.0 {
            brighter += 1;
        }
    }
    if covered < 3 {
        return INVALID;
    }
    let edge = 0.5 * (sum / covered as f64) + 0.5 * minimum;
    let mut total = edge * shape * aspect_factor * size_factor;
    if covered < 4 {
        total *= PARTIAL_FACTOR;
    }
    if brighter != 0 && brighter != covered {
        total *= MIXED_POLARITY_FACTOR;
    }
    if match_paper(ratio, o.paper_aspect_tolerance, &MATCHED).is_some() {
        total += PAPER_BONUS;
    }
    if let Some(h) = hint
        && corner_distance(q, h) <= HINT_SHARE * f.diagonal()
    {
        total += HINT_BONUS;
    }
    Score {
        total,
        edge,
        min_side: minimum,
        valid: true,
        partial: covered < 4,
    }
}

/// How clearly edge `index` of `q` is a sheet's edge, 0 to 1, with its sign; `None` when it lies outside the frame or along
/// its border (BORDER_BAND). Its support (`measure_side`) times CONTRAST_FLOOR plus the rest in proportion to its contrast
/// (full at CONTRAST_FULL), less up to EXTENSION_PENALTY of that when the edge runs on past its corners.
pub(crate) fn side_score(f: &Field, q: &Quad, index: usize, samples: usize) -> Option<(f64, f64)> {
    let (p, e) = (at(q, index), at(q, index + 1));
    let m = measure_side(f, p, e, samples);
    if !m.covered {
        return None;
    }
    let value = m.support
        * (CONTRAST_FLOOR + (1.0 - CONTRAST_FLOOR) * (m.contrast / CONTRAST_FULL).min(1.0));
    let len = p.distance_to(e);
    let ext = EXTENSION_MIN.max(EXTENSION_SHARE * len);
    let (ux, uy) = ((e.x - p.x) / len, (e.y - p.y) / len);
    let ext_samples = 6.max(samples / 4);
    let before = measure_side(
        f,
        Point::new(p.x - ux * ext, p.y - uy * ext),
        p,
        ext_samples,
    );
    let after = measure_side(
        f,
        e,
        Point::new(e.x + ux * ext, e.y + uy * ext),
        ext_samples,
    );
    let (mut through, mut ends) = (0.0, 0);
    if before.covered {
        through += before.support;
        ends += 1;
    }
    if after.covered {
        through += after.support;
        ends += 1;
    }
    if ends == 0 {
        return Some((value, m.sign));
    }
    through /= ends as f64;
    let penalty = EXTENSION_PENALTY * ((through - 0.15) / 0.45).clamp(0.0, 1.0);
    Some((value * (1.0 - penalty), m.sign))
}

/// What `measure_side` finds along an edge; `sign` is +1 when the outside is brighter, -1 when darker.
pub(crate) struct SideMeasure {
    pub support: f64,
    pub contrast: f64,
    pub covered: bool,
    pub sign: f64,
}

const UNCOVERED: SideMeasure = SideMeasure {
    support: 0.0,
    contrast: 0.0,
    covered: false,
    sign: 0.0,
};

/// At `samples` points along the edge (6 % to 94 %) at least BORDER_BAND px inside the frame, the gradient across it within
/// SUPPORT_REACH px; the edge's sign and offset are those with the largest sum; a point supports the edge when, at that offset
/// or one pixel either side, the gradient of that sign is at least EDGE_THRESHOLD and TEXTURE_RATIO times the texture outside
/// (the median along the edge of the mean gradient TEXTURE_NEAR to TEXTURE_FAR px outside, of the points where some of that
/// strip lies in the frame; none when it lies outside everywhere); the contrast is the mean colour difference between 4 + 0.4 %
/// of the diagonal px inside and outside, nearer where the frame ends first. Covered when at least a quarter of the points are
/// measured.
pub(crate) fn measure_side(f: &Field, p: Point, q: Point, samples: usize) -> SideMeasure {
    let (dx, dy) = (q.x - p.x, q.y - p.y);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 1.0 {
        return UNCOVERED;
    }
    let (ux, uy) = (dx / len, dy / len);
    // outward for a clockwise outline
    let (nx, ny) = (uy, -ux);
    let offset = 4.0 + 0.004 * f.diagonal();
    let (w, h) = (f.w, f.h);
    let r = SUPPORT_REACH;
    let span = (2 * r + 1) as usize;
    let mut g = vec![0.0f64; samples * span];
    let mut texture = vec![0.0f64; samples];
    let mut used = vec![false; samples];
    let mut textured = vec![false; samples];
    let mut pos = vec![0.0f64; span];
    let mut neg = vec![0.0f64; span];
    let (mut in_frame, mut textured_count) = (0usize, 0usize);
    let mut contrast_sum = 0.0;
    for s in 0..samples {
        let t = 0.06 + 0.88 * (s as f64 + 0.5) / samples as f64;
        let (x, y) = (p.x + dx * t, p.y + dy * t);
        if !(x >= BORDER_BAND
            && y >= BORDER_BAND
            && x <= w as f64 - BORDER_BAND
            && y <= h as f64 - BORDER_BAND)
        {
            continue;
        }
        used[s] = true;
        in_frame += 1;
        for k in -r..=r {
            let i = pixel_index(f, x + nx * k as f64, y + ny * k as f64);
            let v = f.gx[i] * nx + f.gy[i] * ny;
            g[s * span + (k + r) as usize] = v;
            if v > 0.0 {
                pos[(k + r) as usize] += v;
            } else {
                neg[(k + r) as usize] -= v;
            }
        }
        // the texture strip where it lies within the gradient's area (not the outermost pixels)
        let (mut tex, mut probes) = (0.0, 0);
        for k in TEXTURE_NEAR..=TEXTURE_FAR {
            let fx = (x + nx * k as f64).floor();
            let fy = (y + ny * k as f64).floor();
            if fx < 1.0 || fy < 1.0 || fx > (w - 2) as f64 || fy > (h - 2) as f64 {
                continue;
            }
            let i = fy as usize * w + fx as usize;
            tex += (f.gx[i] * nx + f.gy[i] * ny).abs();
            probes += 1;
        }
        if probes > 0 {
            texture[s] = tex / probes as f64;
            textured[s] = true;
            textured_count += 1;
        }
        let inner = probe_reach(f, x, y, -nx, -ny, offset);
        let outer = probe_reach(f, x, y, nx, ny, offset);
        let a = pixel_index(f, x - nx * inner, y - ny * inner);
        let b = pixel_index(f, x + nx * outer, y + ny * outer);
        let diff = (f.r[a] - f.r[b]).abs() + (f.g[a] - f.g[b]).abs() + (f.b[a] - f.b[b]).abs();
        contrast_sum += diff as f64 / 768.0;
    }
    if in_frame == 0 || in_frame * 4 < samples {
        return UNCOVERED;
    }
    let (mut best_k, mut sign, mut best_sum) = (0usize, 1.0, -1.0);
    for k in 0..span {
        if pos[k] > best_sum {
            best_sum = pos[k];
            best_k = k;
            sign = 1.0;
        }
        if neg[k] > best_sum {
            best_sum = neg[k];
            best_k = k;
            sign = -1.0;
        }
    }
    let mut level = 0.0;
    if textured_count > 0 {
        let mut levels: Vec<f64> = (0..samples)
            .filter(|&s| textured[s])
            .map(|s| texture[s])
            .collect();
        levels.sort_by(|a, b| a.total_cmp(b));
        level = levels[textured_count / 2];
    }
    let mut support = 0usize;
    for s in 0..samples {
        if !used[s] {
            continue;
        }
        let mut e = -f64::MAX;
        for k in best_k.saturating_sub(1)..=(span - 1).min(best_k + 1) {
            e = e.max(sign * g[s * span + k]);
        }
        if e >= EDGE_THRESHOLD && e >= TEXTURE_RATIO * level {
            support += 1;
        }
    }
    SideMeasure {
        support: support as f64 / in_frame as f64,
        contrast: contrast_sum / in_frame as f64,
        covered: true,
        sign,
    }
}

/// The index of the analysis pixel holding the continuous point (x, y), clamped into the frame.
pub(crate) fn pixel_index(f: &Field, x: f64, y: f64) -> usize {
    let px = x.floor().max(0.0).min((f.w - 1) as f64) as usize;
    let py = y.floor().max(0.0).min((f.h - 1) as f64) as usize;
    py * f.w + px
}

/// How far from (x, y) along the unit direction (ux, uy) a probe can go, at most `limit`, and stay half a pixel short of where
/// the frame ends (0 at least).
fn probe_reach(f: &Field, x: f64, y: f64, ux: f64, uy: f64, limit: f64) -> f64 {
    let mut room = f64::MAX;
    if ux > 0.0 {
        room = room.min((f.w as f64 - x) / ux);
    } else if ux < 0.0 {
        room = room.min(x / -ux);
    }
    if uy > 0.0 {
        room = room.min((f.h as f64 - y) / uy);
    } else if uy < 0.0 {
        room = room.min(y / -uy);
    }
    0.0f64.max(limit.min(room - 0.5))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(x: f64, y: f64, size: f64) -> Quad {
        Quad::from_xy([[x, y], [x + size, y], [x + size, y + size], [x, y + size]])
    }

    #[test]
    fn corner_distance_ignores_which_corner_comes_first() {
        let q = square(10.0, 20.0, 100.0);
        let turned = Quad([q.0[1], q.0[2], q.0[3], q.0[0]]);
        assert!(corner_distance(&q, &turned).abs() < 1e-9);
        assert!((corner_distance(&q, &square(13.0, 20.0, 100.0)) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn separation_needs_a_line_between_the_outlines() {
        let q = square(10.0, 20.0, 100.0);
        assert!(!separated(&q, &q.scaled(0.5, 0.5), 0.0));
        assert!(!separated(&q, &square(60.0, 60.0, 100.0), 0.0));
        let apart = square(400.0, 0.0, 100.0);
        assert!(separated(&q, &apart, 5.0));
        assert!(!separated(&q, &apart, 400.0));
    }

    #[test]
    fn the_angle_table_turns_one_degree_at_a_time() {
        let (c, s) = angle_tables();
        assert_eq!((c[0], s[0]), (1.0, 0.0));
        assert!((c[90]).abs() < 1e-12 && (s[90] - 1.0).abs() < 1e-12);
        assert!((c[180] + 1.0).abs() < 1e-12 && s[180].abs() < 1e-12);
    }

    #[test]
    fn an_image_under_16_px_has_no_page() {
        let tiny = Rgba::filled(15, 40, 200, 200, 200);
        assert_eq!(detect(&tiny, None, &Options::default()), Detection::NOTHING);
        let empty = Rgba::new(0, 0, Vec::new());
        assert_eq!(
            detect(&empty, None, &Options::default()),
            Detection::NOTHING
        );
    }
}
