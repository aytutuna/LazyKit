use crate::geometry::{Point, Quad};
use lazy_image::Rgba;

pub const PAPER_ASPECT_TOLERANCE: f64 = 0.03;
pub const MIN_FOCAL_DIAGONALS: f64 = 0.3;
pub const MAX_FOCAL_DIAGONALS: f64 = 4.0;
pub const SUPERSAMPLE_ABOVE: f64 = 1.25;
pub const MAX_PHOTO_DIAGONALS: f64 = 2.0;
pub const MAX_OUTPUT_SIDE: usize = 23170;
/// A page that shrinks by more than this when warped is averaged down first.
pub const PREFILTER_ABOVE: f64 = 2.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Paper {
    A4,
    Letter,
    Legal,
    IdCard,
}

impl Paper {
    /// Portrait width and height in inches.
    pub fn inches(self) -> (f64, f64) {
        match self {
            Paper::Letter => (8.5, 11.0),
            Paper::Legal => (8.5, 14.0),
            Paper::A4 => (8.2677, 11.6929),
            Paper::IdCard => (2.125, 3.370),
        }
    }
}

/// The sizes a photo of a page may snap to, and those a document (not an ID card) may.
pub const MATCHED: [Paper; 4] = [Paper::A4, Paper::Letter, Paper::Legal, Paper::IdCard];
pub const DOCUMENTS: [Paper; 3] = [Paper::A4, Paper::Letter, Paper::Legal];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaperMatch {
    pub paper: Paper,
    pub long_over_short: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AspectEstimate {
    pub width_over_height: f64,
    pub focal_length: Option<f64>,
}

impl AspectEstimate {
    pub fn long_over_short(&self) -> f64 {
        if self.width_over_height >= 1.0 {
            self.width_over_height
        } else {
            1.0 / self.width_over_height
        }
    }
}

/// A projective map, row by row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Homography(pub [f64; 9]);

impl Homography {
    pub fn map(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.0;
        let w = m[6] * x + m[7] * y + m[8];
        (
            (m[0] * x + m[1] * y + m[2]) / w,
            (m[3] * x + m[4] * y + m[5]) / w,
        )
    }

    pub fn determinant(&self) -> f64 {
        let m = &self.0;
        m[0] * (m[4] * m[8] - m[5] * m[7]) - m[1] * (m[3] * m[8] - m[5] * m[6])
            + m[2] * (m[3] * m[7] - m[4] * m[6])
    }

    pub fn inverse(&self) -> Option<Homography> {
        let det = self.determinant();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let m = &self.0;
        Some(Homography([
            (m[4] * m[8] - m[5] * m[7]) / det,
            (m[2] * m[7] - m[1] * m[8]) / det,
            (m[1] * m[5] - m[2] * m[4]) / det,
            (m[5] * m[6] - m[3] * m[8]) / det,
            (m[0] * m[8] - m[2] * m[6]) / det,
            (m[2] * m[3] - m[0] * m[5]) / det,
            (m[3] * m[7] - m[4] * m[6]) / det,
            (m[1] * m[6] - m[0] * m[7]) / det,
            (m[0] * m[4] - m[1] * m[3]) / det,
        ]))
    }

    /// The unit square to `quad` (top left, top right, bottom right, bottom left).
    pub fn square_to_quad(quad: &Quad) -> Option<Homography> {
        let [p0, p1, p2, p3] = quad.0;
        let (sx, sy) = (p0.x - p1.x + p2.x - p3.x, p0.y - p1.y + p2.y - p3.y);
        let (dx1, dx2, dy1, dy2) = (p1.x - p2.x, p3.x - p2.x, p1.y - p2.y, p3.y - p2.y);
        let den = dx1 * dy2 - dx2 * dy1;
        if den == 0.0 || !den.is_finite() {
            return None;
        }
        let g = (sx * dy2 - dx2 * sy) / den;
        let h = (dx1 * sy - sx * dy1) / den;
        let m = Homography([
            p1.x - p0.x + g * p1.x,
            p3.x - p0.x + h * p3.x,
            p0.x,
            p1.y - p0.y + g * p1.y,
            p3.y - p0.y + h * p3.y,
            p0.y,
            g,
            h,
            1.0,
        ]);
        let det = m.determinant();
        (det != 0.0 && det.is_finite()).then_some(m)
    }

    /// The rectangle (0, 0) to (width, height) to `quad`.
    pub fn rect_to_quad(width: f64, height: f64, quad: &Quad) -> Option<Homography> {
        if !(width > 0.0 && height > 0.0) {
            return None;
        }
        let s = Self::square_to_quad(quad)?.0;
        Some(Homography([
            s[0] / width,
            s[1] / height,
            s[2],
            s[3] / width,
            s[4] / height,
            s[5],
            s[6] / width,
            s[7] / height,
            s[8],
        ]))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Supersampling {
    #[default]
    Auto,
    Off,
    On,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub output: Option<(usize, usize)>,
    /// Longest output side; 0 for no limit.
    pub max_long_side: usize,
    pub paper_aspect_tolerance: f64,
    pub snap_to_paper: bool,
    pub supersample: Supersampling,
    pub fill: [u8; 3],
}

impl Default for Options {
    fn default() -> Self {
        Options {
            output: None,
            max_long_side: 0,
            paper_aspect_tolerance: PAPER_ASPECT_TOLERANCE,
            snap_to_paper: true,
            supersample: Supersampling::Auto,
            fill: [255, 255, 255],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plan {
    pub width: usize,
    pub height: usize,
    pub output_to_source: Homography,
    pub width_over_height: f64,
    pub paper: Option<PaperMatch>,
    pub dpi: Option<f64>,
    pub supersampled: bool,
}

struct Rays {
    n2: [f64; 3],
    n3: [f64; 3],
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn rays(quad: &Quad, image_width: f64, image_height: f64) -> Option<Rays> {
    let (cx, cy) = (image_width / 2.0, image_height / 2.0);
    // m1 top left, m2 top right, m3 bottom left, m4 bottom right, centred, homogeneous
    let c = |p: Point| [p.x - cx, p.y - cy, 1.0];
    let (m1, m2, m3, m4) = (c(quad.0[0]), c(quad.0[1]), c(quad.0[3]), c(quad.0[2]));
    let c14 = cross(m1, m4);
    let (k2den, k3den) = (dot(cross(m2, m4), m3), dot(cross(m3, m4), m2));
    if k2den == 0.0 || k3den == 0.0 {
        return None;
    }
    let k2 = dot(c14, m3) / k2den;
    let k3 = dot(c14, m2) / k3den;
    Some(Rays {
        n2: [k2 * m2[0] - m1[0], k2 * m2[1] - m1[1], k2 - 1.0],
        n3: [k3 * m3[0] - m1[0], k3 * m3[1] - m1[1], k3 - 1.0],
    })
}

fn edge_aspect(quad: &Quad) -> f64 {
    let sides = (quad.edge(3) + quad.edge(1)) / 2.0;
    if sides > 0.0 {
        (quad.edge(0) + quad.edge(2)) / 2.0 / sides
    } else {
        1.0
    }
}

/// The sheet's width over its height as it is, not as the camera sees it, from the vanishing geometry of the four corners
/// with the optical centre in the middle of the photo; the mean of opposite edges when the page is square on or the
/// solution is out of range.
pub fn estimate_aspect(quad: &Quad, image_width: f64, image_height: f64) -> AspectEstimate {
    let fallback = AspectEstimate {
        width_over_height: edge_aspect(quad),
        focal_length: None,
    };
    let Some(r) = rays(quad, image_width, image_height) else {
        return fallback;
    };
    let (n2, n3) = (r.n2, r.n3);
    let diagonal = (image_width * image_width + image_height * image_height).sqrt();
    let zz = n2[2] * n3[2];
    // seen straight on (opposite edges parallel), the camera's distance cannot be told
    let scale = n2[0].abs() + n2[1].abs() + n3[0].abs() + n3[1].abs();
    if n2[2].abs() * diagonal > 1e-3 * scale || n3[2].abs() * diagonal > 1e-3 * scale {
        let f2 = -(n2[0] * n3[0] + n2[1] * n3[1]) / zz;
        if zz != 0.0 && f2 > 0.0 && f2.is_finite() {
            let f = f2.sqrt();
            if f >= MIN_FOCAL_DIAGONALS * diagonal && f <= MAX_FOCAL_DIAGONALS * diagonal {
                let num = n2[0] * n2[0] + n2[1] * n2[1] + f2 * n2[2] * n2[2];
                let den = n3[0] * n3[0] + n3[1] * n3[1] + f2 * n3[2] * n3[2];
                if num > 0.0 && den > 0.0 {
                    return AspectEstimate {
                        width_over_height: (num / den).sqrt(),
                        focal_length: Some(f),
                    };
                }
            }
        }
    }
    fallback
}

/// As `estimate_aspect`, with the camera's focal length (in the photo's pixels) known.
pub fn estimate_aspect_with_focal(
    quad: &Quad,
    image_width: f64,
    image_height: f64,
    focal_length: f64,
) -> AspectEstimate {
    if focal_length <= 0.0 || focal_length.is_nan() || !focal_length.is_finite() {
        return estimate_aspect(quad, image_width, image_height);
    }
    let fallback = AspectEstimate {
        width_over_height: edge_aspect(quad),
        focal_length: None,
    };
    let Some(r) = rays(quad, image_width, image_height) else {
        return fallback;
    };
    let f2 = focal_length * focal_length;
    let num = r.n2[0] * r.n2[0] + r.n2[1] * r.n2[1] + f2 * r.n2[2] * r.n2[2];
    let den = r.n3[0] * r.n3[0] + r.n3[1] * r.n3[1] + f2 * r.n3[2] * r.n3[2];
    if num > 0.0 && den > 0.0 && (num / den).is_finite() {
        return AspectEstimate {
            width_over_height: (num / den).sqrt(),
            focal_length: Some(focal_length),
        };
    }
    fallback
}

/// The listed size whose long over short side is within `tolerance` (a share of it) of `long_over_short`, the nearest first.
pub fn match_paper(long_over_short: f64, tolerance: f64, sizes: &[Paper]) -> Option<PaperMatch> {
    let mut best = None;
    let mut best_diff = f64::MAX;
    for &paper in sizes {
        let (w, h) = paper.inches();
        let ratio = w.max(h) / w.min(h);
        let diff = (long_over_short - ratio).abs() / ratio;
        if diff <= tolerance && diff < best_diff {
            best_diff = diff;
            best = Some(PaperMatch {
                paper,
                long_over_short: ratio,
            });
        }
    }
    best
}

fn fits_output(width: usize, height: usize) -> bool {
    (1..=MAX_OUTPUT_SIDE).contains(&width) && (1..=MAX_OUTPUT_SIDE).contains(&height)
}

/// How a photo of the page at `quad` is cut out: the output's size (the longer edges, never more than twice the photo's
/// diagonal, in the sheet's own shape, snapped to a known paper size when it is within tolerance), the map from the output
/// back to the photo and whether to average four samples a pixel.
pub fn plan(
    quad: &Quad,
    image_width: usize,
    image_height: usize,
    options: &Options,
) -> Option<Plan> {
    if !quad.is_convex() || image_width == 0 || image_height == 0 {
        return None;
    }
    let (top, right, bottom, left) = (quad.edge(0), quad.edge(1), quad.edge(2), quad.edge(3));
    let (w0, h0) = (top.max(bottom), left.max(right));
    let estimate = estimate_aspect(quad, image_width as f64, image_height as f64);
    let mut aspect = estimate.width_over_height;
    let paper = if options.snap_to_paper {
        match_paper(
            estimate.long_over_short(),
            options.paper_aspect_tolerance,
            &DOCUMENTS,
        )
    } else {
        None
    };
    if let Some(p) = paper {
        aspect = if aspect >= 1.0 {
            p.long_over_short
        } else {
            1.0 / p.long_over_short
        };
    }
    let (width, height) = match options.output {
        Some((w, h)) if w > 0 && h > 0 => (w, h),
        _ => {
            let mut long_side = w0.max(h0);
            if options.max_long_side > 0 {
                long_side = long_side.min(options.max_long_side as f64);
            }
            let (iw, ih) = (image_width as f64, image_height as f64);
            let photo_diagonal = (iw * iw + ih * ih).sqrt();
            long_side =
                long_side.min((MAX_PHOTO_DIAGONALS * photo_diagonal).min(MAX_OUTPUT_SIDE as f64));
            let (wd, hd) = if aspect >= 1.0 {
                (long_side, long_side / aspect)
            } else {
                (long_side * aspect, long_side)
            };
            if !wd.is_finite() || !hd.is_finite() {
                return None;
            }
            (
                ((wd + 0.5).floor() as usize).max(1),
                ((hd + 0.5).floor() as usize).max(1),
            )
        }
    };
    if !fits_output(width, height) {
        return None;
    }
    let map = Homography::rect_to_quad(width as f64, height as f64, quad)?;
    let dpi = paper.map(|p| {
        let (pw, ph) = p.paper.inches();
        width.max(height) as f64 / pw.max(ph)
    });
    let supersampled = match options.supersample {
        Supersampling::On => true,
        Supersampling::Off => false,
        Supersampling::Auto => {
            w0 > width as f64 * SUPERSAMPLE_ABOVE || h0 > height as f64 * SUPERSAMPLE_ABOVE
        }
    };
    Some(Plan {
        width,
        height,
        output_to_source: map,
        width_over_height: width as f64 / height as f64,
        paper,
        dpi,
        supersampled,
    })
}

// Catmull-Rom weights for a fractional position t, in the order the apps add them.
fn catmull_rom(t: f64) -> [f64; 4] {
    [
        ((-0.5 * t + 1.0) * t - 0.5) * t,
        (1.5 * t - 2.5) * t * t + 1.0,
        ((-1.5 * t + 2.0) * t + 0.5) * t,
        (0.5 * t - 0.5) * t * t,
    ]
}

fn round_byte(v: f64) -> u8 {
    v.round_ties_even().clamp(0.0, 255.0) as u8
}

fn accumulate(image: &Rgba, m: &Homography, px: f64, py: f64, fill: [u8; 3], acc: &mut [f64; 4]) {
    let (iw, ih) = (image.width, image.height);
    let w = m.0[6] * px + m.0[7] * py + m.0[8];
    let sx = (m.0[0] * px + m.0[1] * py + m.0[2]) / w - 0.5;
    let sy = (m.0[3] * px + m.0[4] * py + m.0[5]) / w - 0.5;
    if !sx.is_finite()
        || !sy.is_finite()
        || sx < -2.0
        || sy < -2.0
        || sx > iw as f64 + 1.0
        || sy > ih as f64 + 1.0
    {
        acc[0] += fill[0] as f64;
        acc[1] += fill[1] as f64;
        acc[2] += fill[2] as f64;
        acc[3] += 255.0;
        return;
    }
    let (fx0, fy0) = (sx.floor(), sy.floor());
    let (x0, y0) = (fx0 as i64 - 1, fy0 as i64 - 1);
    let kx = catmull_rom(sx - fx0);
    let ky = catmull_rom(sy - fy0);
    for (j, ky) in ky.iter().enumerate() {
        let cy = y0 + j as i64;
        for (i, kx) in kx.iter().enumerate() {
            let cx = x0 + i as i64;
            let weight = kx * ky;
            if cx >= 0 && cx < iw as i64 && cy >= 0 && cy < ih as i64 {
                let si = (cy as usize * iw + cx as usize) * 4;
                for (a, &p) in acc.iter_mut().zip(&image.data[si..si + 4]) {
                    *a += weight * p as f64;
                }
            } else {
                acc[0] += weight * fill[0] as f64;
                acc[1] += weight * fill[1] as f64;
                acc[2] += weight * fill[2] as f64;
                acc[3] += weight * 255.0;
            }
        }
    }
}

/// The photo cut out and straightened along `plan`, bicubic (Catmull-Rom), four samples a pixel when the plan says so;
/// outside the photo is `options.fill`.
pub fn warp_plan(image: &Rgba, plan: &Plan, options: &Options) -> Rgba {
    let (w, h) = (plan.width, plan.height);
    assert!(fits_output(w, h), "a {w} x {h} output is not possible");
    let mut out = vec![0u8; w * h * 4];
    let m = &plan.output_to_source;
    for oy in 0..h {
        for ox in 0..w {
            let di = (oy * w + ox) * 4;
            let mut acc = [0.0f64; 4];
            if plan.supersampled {
                for s in 0..4 {
                    let px = ox as f64 + if s & 1 == 0 { 0.25 } else { 0.75 };
                    let py = oy as f64 + if s >> 1 == 0 { 0.25 } else { 0.75 };
                    accumulate(image, m, px, py, options.fill, &mut acc);
                }
                for k in 0..4 {
                    out[di + k] = round_byte(acc[k] / 4.0);
                }
            } else {
                accumulate(
                    image,
                    m,
                    ox as f64 + 0.5,
                    oy as f64 + 0.5,
                    options.fill,
                    &mut acc,
                );
                for k in 0..4 {
                    out[di + k] = round_byte(acc[k]);
                }
            }
        }
    }
    Rgba::new(w, h, out)
}

/// The page cut out of the photo: planned, then warped.
pub fn correct(image: &Rgba, quad: &Quad, options: &Options) -> Option<(Rgba, Plan)> {
    let plan = plan(quad, image.width, image.height, options)?;
    Some((warp_plan(image, &plan, options), plan))
}

/// How many times smaller than the photo's pixels the output is, as a whole number when above PREFILTER_ABOVE, else 1.
pub fn prefilter_factor(quad: &Quad, width: usize, height: usize) -> usize {
    if width < 1 || height < 1 {
        return 1;
    }
    let w0 = quad.edge(0).max(quad.edge(2));
    let h0 = quad.edge(1).max(quad.edge(3));
    let shrink = (w0 / width as f64).max(h0 / height as f64);
    if shrink <= PREFILTER_ABOVE || shrink.is_nan() || !shrink.is_finite() {
        return 1;
    }
    shrink.floor().min(MAX_OUTPUT_SIDE as f64) as usize
}

/// Blocks of `factor` x `factor` pixels averaged, halves rounded up; the last row and column of blocks hold what is left.
pub fn box_reduced(image: &Rgba, factor: usize) -> Rgba {
    if factor <= 1 {
        return image.clone();
    }
    let (w, h) = (image.width, image.height);
    let (rw, rh) = (w.div_ceil(factor), h.div_ceil(factor));
    let mut out = vec![0u8; rw * rh * 4];
    for oy in 0..rh {
        let (y0, y1) = (oy * factor, h.min(oy * factor + factor));
        for ox in 0..rw {
            let (x0, x1) = (ox * factor, w.min(ox * factor + factor));
            let mut sum = [0usize; 4];
            for y in y0..y1 {
                for x in x0..x1 {
                    let si = (y * w + x) * 4;
                    for (s, &p) in sum.iter_mut().zip(&image.data[si..si + 4]) {
                        *s += p as usize;
                    }
                }
            }
            let count = (y1 - y0) * (x1 - x0);
            let di = (oy * rw + ox) * 4;
            for k in 0..4 {
                out[di + k] = ((sum[k] + count / 2) / count) as u8;
            }
        }
    }
    Rgba::new(rw, rh, out)
}

/// The page at `quad` warped to exactly `width` x `height`, averaged down first when it is much larger than the output so
/// fine print does not alias; `None` for an outline that is not convex or a size out of range.
pub fn warp(
    image: &Rgba,
    quad: &Quad,
    width: usize,
    height: usize,
    options: &Options,
) -> Option<Rgba> {
    if !quad.is_convex() || !fits_output(width, height) {
        return None;
    }
    let factor = prefilter_factor(quad, width, height);
    let (reduced, source_quad);
    let (source, source_quad) = if factor > 1 {
        reduced = box_reduced(image, factor);
        let scale = 1.0 / factor as f64;
        source_quad = quad.scaled(scale, scale);
        (&reduced, &source_quad)
    } else {
        (image, quad)
    };
    let planned = Options {
        output: Some((width, height)),
        snap_to_paper: false,
        max_long_side: 0,
        ..*options
    };
    let plan = plan(source_quad, source.width, source.height, &planned)?;
    Some(warp_plan(source, &plan, &planned))
}
