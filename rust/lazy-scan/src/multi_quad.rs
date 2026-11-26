use crate::geometry::Quad;
use crate::warp::estimate_aspect;

// Picks the objects in one camera shot (several receipts or photos, a card, a page) from the outlines a finder proposed, as
// Lazy Scan's MultiQuadPicker does: each named and checked for a convex outline, kept when its size and shape fit the kind of
// object, dropped when it is the only object inside a larger outline of the right size and shape (a picture printed on a
// photo), while a larger outline holding several separate objects that cover enough of it is the background and dropped
// instead (an album page, a sheet under receipts); then taken surest first unless it overlaps one already taken, up to the
// kind's count, and returned in reading order.

/// Two outlines overlapping by more than this (intersection over union) are the same object.
pub const MAX_OVERLAP: f64 = 0.3;
/// An outline is inside another when each of its corners is at most this many pixels outside it.
pub const INSIDE_TOLERANCE: f64 = 2.0;
/// An outline holding several separate objects is the background behind them only when they cover at least this share of it.
pub const MIN_BACKGROUND_COVERAGE: f64 = 0.15;
/// The ID-1 card's long side over its short side (85.6 by 53.98 mm).
pub const ID_CARD_LONG_OVER_SHORT: f64 = 3.370 / 2.125;
/// A card's shape may differ from the ID-1 shape by this much.
pub const ID_CARD_SHAPE_SLACK: f64 = 0.12;

/// An outline a finder proposed, with how sure it is (0 to 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuadCandidate {
    pub quad: Quad,
    pub confidence: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Document,
    Receipt,
    Photo,
    IdCard,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Options {
    pub min_long_over_short: f64,
    pub max_long_over_short: f64,
    pub min_area_fraction: f64,
    pub max_area_fraction: f64,
    pub max_overlap: f64,
    pub max_count: usize,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            min_long_over_short: 1.0,
            max_long_over_short: 2.0,
            min_area_fraction: 0.0,
            max_area_fraction: 1.0,
            max_overlap: MAX_OVERLAP,
            max_count: 1,
        }
    }
}

impl Options {
    /// What is kept for `kind`: receipts (short over long side 0.12 to 1) of at least 0.5 % of the photo, up to 6; photos up to
    /// twice as long as wide of at least 1 %, up to 8; one card of the ID-1 shape of at least 2 %; one document page up to
    /// twice as long as wide of at least 10 %.
    pub fn for_kind(kind: Kind) -> Options {
        let base = Options::default();
        match kind {
            Kind::Receipt => Options {
                max_long_over_short: 1.0 / 0.12,
                min_area_fraction: 0.005,
                max_area_fraction: 0.9,
                max_count: 6,
                ..base
            },
            Kind::Photo => Options {
                min_area_fraction: 0.01,
                max_area_fraction: 0.95,
                max_count: 8,
                ..base
            },
            Kind::IdCard => Options {
                min_long_over_short: ID_CARD_LONG_OVER_SHORT - ID_CARD_SHAPE_SLACK,
                max_long_over_short: ID_CARD_LONG_OVER_SHORT + ID_CARD_SHAPE_SLACK,
                min_area_fraction: 0.02,
                ..base
            },
            Kind::Document => Options {
                min_area_fraction: 0.1,
                ..base
            },
        }
    }
}

struct Fitting {
    quad: Quad,
    confidence: f64,
    area: f64,
    index: usize,
}

// surer first, then larger, then earlier (a total order)
fn surer_first(a: &Fitting, b: &Fitting) -> std::cmp::Ordering {
    if a.confidence != b.confidence {
        return b.confidence.total_cmp(&a.confidence);
    }
    if a.area != b.area {
        return b.area.total_cmp(&a.area);
    }
    a.index.cmp(&b.index)
}

/// The share of their joint area two convex outlines have in common (intersection over union, 0 to 1).
pub fn overlap(a: &Quad, b: &Quad) -> f64 {
    let shared = a.intersection_area(b);
    let union = a.area() + b.area() - shared;
    if union > 0.0 { shared / union } else { 0.0 }
}

fn is_inside(inner: &Quad, outer: &Quad) -> bool {
    inner.0.iter().all(|&p| outer.contains(p, INSIDE_TOLERANCE))
}

// `held` are objects laid on an outline of `container_area`, not features inside one object: at least two of them are
// separate (surest first, one overlapping one counted by more than `max_overlap` is the same object) and those cover at least
// MIN_BACKGROUND_COVERAGE of the outline
fn is_background(
    fitting: &[Fitting],
    held: &[usize],
    container_area: f64,
    max_overlap: f64,
) -> bool {
    if held.len() < 2 {
        return false;
    }
    let mut surest: Vec<usize> = held.to_vec();
    surest.sort_by(|&a, &b| surer_first(&fitting[a], &fitting[b]));
    let mut counted: Vec<Quad> = Vec::new();
    let mut covered = 0.0;
    for k in surest {
        if counted
            .iter()
            .any(|q| overlap(&fitting[k].quad, q) > max_overlap)
        {
            continue;
        }
        counted.push(fitting[k].quad);
        covered += fitting[k].area;
    }
    counted.len() >= 2 && covered >= MIN_BACKGROUND_COVERAGE * container_area
}

// rows from the top, left to right in each, by bounding boxes: rows are grouped with a tolerance of half the mean height
fn in_reading_order(quads: Vec<Quad>) -> Vec<Quad> {
    if quads.len() <= 1 {
        return quads;
    }
    struct Region {
        cx: f64,
        cy: f64,
        height: f64,
        quad: Quad,
    }
    let mut regions: Vec<Region> = quads
        .into_iter()
        .map(|quad| {
            let xs = quad.0.map(|p| p.x);
            let ys = quad.0.map(|p| p.y);
            let (min_x, max_x) = (
                xs.iter().cloned().fold(f64::MAX, f64::min),
                xs.iter().cloned().fold(f64::MIN, f64::max),
            );
            let (min_y, max_y) = (
                ys.iter().cloned().fold(f64::MAX, f64::min),
                ys.iter().cloned().fold(f64::MIN, f64::max),
            );
            Region {
                cx: (min_x + max_x) / 2.0,
                cy: (min_y + max_y) / 2.0,
                height: max_y - min_y,
                quad,
            }
        })
        .collect();
    let mean_height = regions.iter().map(|r| r.height).sum::<f64>() / regions.len() as f64;
    let tolerance = 1.0f64.max(mean_height / 2.0);
    regions.sort_by(|a, b| a.cy.total_cmp(&b.cy));
    let mut rows: Vec<Vec<Region>> = Vec::new();
    let mut row_y = 0.0;
    for region in regions {
        match rows.last_mut() {
            Some(row) if region.cy - row_y <= tolerance => row.push(region),
            _ => {
                row_y = region.cy;
                rows.push(vec![region]);
            }
        }
    }
    rows.into_iter()
        .flat_map(|mut row| {
            row.sort_by(|a, b| a.cx.total_cmp(&b.cx));
            row.into_iter().map(|r| r.quad)
        })
        .collect()
}

/// The outlines of the `kind` objects among `candidates`, in reading order.
pub fn pick_kind(
    candidates: &[QuadCandidate],
    image_width: usize,
    image_height: usize,
    kind: Kind,
) -> Vec<Quad> {
    pick(
        candidates,
        image_width,
        image_height,
        &Options::for_kind(kind),
    )
}

/// The outlines among `candidates` that `options` keep, in reading order.
pub fn pick(
    candidates: &[QuadCandidate],
    image_width: usize,
    image_height: usize,
    options: &Options,
) -> Vec<Quad> {
    if image_width == 0 || image_height == 0 || options.max_count == 0 {
        return Vec::new();
    }
    let photo_area = image_width as f64 * image_height as f64;

    // outlines of the right size and shape, with their input order
    let mut fitting: Vec<Fitting> = Vec::new();
    for (i, candidate) in candidates.iter().enumerate() {
        let quad = Quad::from_corners(candidate.quad.0);
        if !quad.is_convex() {
            continue;
        }
        let area = quad.area();
        let fraction = area / photo_area;
        if fraction < options.min_area_fraction || fraction > options.max_area_fraction {
            continue;
        }
        let shape =
            estimate_aspect(&quad, image_width as f64, image_height as f64).long_over_short();
        if shape < options.min_long_over_short || shape > options.max_long_over_short {
            continue;
        }
        let confidence = if candidate.confidence.is_finite() {
            candidate.confidence
        } else {
            0.0
        };
        fitting.push(Fitting {
            quad,
            confidence,
            area,
            index: i,
        });
    }

    // outlines nest: each hangs under the smallest larger one holding it (the earlier on a tie)
    let n = fitting.len();
    let mut parent: Vec<Option<usize>> = vec![None; n];
    for i in 0..n {
        for j in 0..n {
            if j == i
                || fitting[j].area <= fitting[i].area
                || !is_inside(&fitting[i].quad, &fitting[j].quad)
            {
                continue;
            }
            if parent[i].is_none_or(|p| fitting[j].area < fitting[p].area) {
                parent[i] = Some(j);
            }
        }
    }

    // folded from the smallest up: an outline is the background behind what it holds when it holds several separate objects
    // covering enough of it, unless only one object is wanted; otherwise it is one object
    let mut smallest_first: Vec<usize> = (0..n).collect();
    smallest_first.sort_by(|&a, &b| fitting[a].area.total_cmp(&fitting[b].area).then(a.cmp(&b)));
    let mut held: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut objects: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &i in &smallest_first {
        let background = options.max_count > 1
            && is_background(&fitting, &held[i], fitting[i].area, options.max_overlap);
        objects[i] = if background { held[i].clone() } else { vec![i] };
        if let Some(p) = parent[i] {
            held[p].extend(objects[i].iter().copied());
        }
    }
    let mut outer: Vec<usize> = Vec::new();
    for i in 0..n {
        if parent[i].is_none() {
            outer.extend(&objects[i]);
        }
    }

    // surest first; an outline overlapping one taken is the same object
    outer.sort_by(|&a, &b| surer_first(&fitting[a], &fitting[b]));
    let mut taken: Vec<Quad> = Vec::new();
    for k in outer {
        if taken.len() >= options.max_count {
            break;
        }
        if !taken
            .iter()
            .any(|q| overlap(&fitting[k].quad, q) > options.max_overlap)
        {
            taken.push(fitting[k].quad);
        }
    }
    in_reading_order(taken)
}
