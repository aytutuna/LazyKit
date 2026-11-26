#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Point { x, y }
    }

    pub fn distance_to(self, other: Point) -> f64 {
        let (dx, dy) = (self.x - other.x, self.y - other.y);
        (dx * dx + dy * dy).sqrt()
    }
}

/// Four corners clockwise on screen from the top left: top left, top right, bottom right, bottom left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad(pub [Point; 4]);

impl Quad {
    pub fn from_xy(corners: [[f64; 2]; 4]) -> Self {
        Quad(corners.map(|c| Point::new(c[0], c[1])))
    }

    /// Positive when the corners run clockwise on screen (y down).
    pub fn signed_area(&self) -> f64 {
        let mut sum = 0.0;
        for i in 0..4 {
            let (a, b) = (self.0[i], self.0[(i + 1) & 3]);
            sum += a.x * b.y - b.x * a.y;
        }
        sum / 2.0
    }

    /// Every corner turns clockwise and no three are in line.
    pub fn is_convex(&self) -> bool {
        (0..4).all(|i| {
            let (a, b, c) = (self.0[i], self.0[(i + 1) & 3], self.0[(i + 2) & 3]);
            (b.x - a.x) * (c.y - b.y) - (b.y - a.y) * (c.x - b.x) > 0.0
        })
    }

    /// The area enclosed, in square pixels.
    pub fn area(&self) -> f64 {
        self.signed_area().abs()
    }

    /// `point` lies inside the outline or within `tolerance` pixels outside it. The quad must be convex.
    pub fn contains(&self, point: Point, tolerance: f64) -> bool {
        (0..4).all(|i| {
            let (a, b) = (self.0[i], self.0[(i + 1) & 3]);
            let len = a.distance_to(b);
            if len <= 0.0 {
                return false;
            }
            // how far outside edge a to b the point is, positive outside for a clockwise quad
            let outside = ((b.x - a.x) * (a.y - point.y) - (b.y - a.y) * (a.x - point.x)) / len;
            outside.is_nan() || outside <= tolerance
        })
    }

    /// The area this outline and `other` share (both convex and clockwise): this quad clipped by each edge of the other in
    /// turn (Sutherland and Hodgman).
    pub fn intersection_area(&self, other: &Quad) -> f64 {
        let mut polygon: Vec<Point> = self.0.to_vec();
        for edge in 0..4 {
            if polygon.is_empty() {
                break;
            }
            let (a, b) = (other.0[edge], other.0[(edge + 1) & 3]);
            let input = std::mem::take(&mut polygon);
            for k in 0..input.len() {
                let current = input[k];
                let previous = input[(k + input.len() - 1) % input.len()];
                let side = |p: Point| (b.x - a.x) * (p.y - a.y) - (b.y - a.y) * (p.x - a.x);
                let (sc, sp) = (side(current), side(previous));
                let crossing = || {
                    let t = sp / (sp - sc);
                    Point::new(
                        previous.x + (current.x - previous.x) * t,
                        previous.y + (current.y - previous.y) * t,
                    )
                };
                if sc >= 0.0 {
                    if sp < 0.0 {
                        polygon.push(crossing());
                    }
                    polygon.push(current);
                } else if sp >= 0.0 {
                    polygon.push(crossing());
                }
            }
        }
        if polygon.len() < 3 {
            return 0.0;
        }
        let mut sum = 0.0;
        for i in 0..polygon.len() {
            let (p, q) = (polygon[i], polygon[(i + 1) % polygon.len()]);
            sum += p.x * q.y - q.x * p.y;
        }
        sum.abs() / 2.0
    }

    pub fn edge(&self, i: usize) -> f64 {
        self.0[i & 3].distance_to(self.0[(i + 1) & 3])
    }

    pub fn scaled(&self, sx: f64, sy: f64) -> Quad {
        Quad(self.0.map(|p| Point::new(p.x * sx, p.y * sy)))
    }

    /// Four corners in any order around the outline as a quad running clockwise from the top left: the first corner of
    /// the edge whose midpoint is highest (the smaller x on a tie).
    pub fn from_corners(corners: [Point; 4]) -> Quad {
        let mut p = corners;
        if Quad(p).signed_area() < 0.0 {
            p.swap(1, 3);
        }
        let (mut start, mut best_y, mut best_x) = (0, f64::INFINITY, f64::INFINITY);
        for i in 0..4 {
            let (a, b) = (p[i], p[(i + 1) & 3]);
            let (my, mx) = ((a.y + b.y) / 2.0, (a.x + b.x) / 2.0);
            if my < best_y || (my == best_y && mx < best_x) {
                best_y = my;
                best_x = mx;
                start = i;
            }
        }
        Quad([
            p[start],
            p[(start + 1) & 3],
            p[(start + 2) & 3],
            p[(start + 3) & 3],
        ])
    }

    /// The corners the editor can cut a page out along: convex, every edge at least 16 pixels long.
    pub fn usable(&self) -> bool {
        self.is_convex() && (0..4).all(|i| self.edge(i) >= 16.0)
    }

    /// Whether the page can be cut out and straightened: a convex outline whose shape comes out as a finite sheet (the
    /// reference's `PerspectiveCorrector.plan` returns nothing otherwise).
    pub fn correctable(&self, width: f64, height: f64) -> bool {
        crate::warp::plan(self, width as usize, height as usize, &Default::default()).is_some()
    }

    /// Every corner moved into the photo so the editor's handles stay where they can be seen.
    pub fn clamped_to_photo(&self, width: f64, height: f64) -> Quad {
        if !(width >= 0.0 && height >= 0.0 && width.is_finite() && height.is_finite()) {
            return *self;
        }
        Quad(
            self.0
                .map(|p| Point::new(p.x.clamp(0.0, width), p.y.clamp(0.0, height))),
        )
    }

    /// A rectangle 10 % in from the photo's edges, for a photo where no page was found.
    pub fn start(width: f64, height: f64) -> Quad {
        let inset = 0.1;
        Quad::from_xy([
            [width * inset, height * inset],
            [width * (1.0 - inset), height * inset],
            [width * (1.0 - inset), height * (1.0 - inset)],
            [width * inset, height * (1.0 - inset)],
        ])
    }

    /// The largest distance between corners, paired from whichever corner of `other` fits best.
    pub fn movement(&self, other: &Quad) -> f64 {
        let mut best = f64::MAX;
        for shift in 0..4 {
            let mut worst = 0.0f64;
            for i in 0..4 {
                worst = worst.max(self.0[i].distance_to(other.0[(i + shift) & 3]));
            }
            best = best.min(worst);
        }
        best
    }

    /// Whether every corner is at least `margin` inside a frame of this size.
    pub fn is_inside(&self, width: f64, height: f64, margin: f64) -> bool {
        self.0.iter().all(|p| {
            p.x >= margin && p.y >= margin && p.x <= width - margin && p.y <= height - margin
        })
    }

    pub fn max_corner_distance(&self, other: &Quad) -> f64 {
        (0..4)
            .map(|i| self.0[i].distance_to(other.0[i]))
            .fold(0.0, f64::max)
    }
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

/// The page's width over its height as the sheet is, not as the camera sees it. With the optical centre in the middle of
/// the photo, the four corners fix the focal length and then the sheet's shape (Zhang and He, whiteboard scanning). Falls
/// back to the mean of opposite edges when the page is square on or the solution is degenerate. This is Lazy Collect's
/// TypeScript estimator, kept for its parity tests; the checks and the cut-out use `warp::estimate_aspect`.
pub fn aspect(q: &Quad, image_width: f64, image_height: f64) -> f64 {
    let plain = (q.edge(0) + q.edge(2)) / (q.edge(1) + q.edge(3));
    let (u0, v0) = (image_width / 2.0, image_height / 2.0);
    let m = q.0.map(|p| [p.x - u0, p.y - v0, 1.0]);
    let (m1, m2, m4, m3) = (m[0], m[1], m[2], m[3]);
    let k2d = dot(cross(m2, m4), m3);
    let k3d = dot(cross(m3, m4), m2);
    if k2d.abs() < 1e-9 || k3d.abs() < 1e-9 {
        return plain;
    }
    let k2 = dot(cross(m1, m4), m3) / k2d;
    let k3 = dot(cross(m1, m4), m2) / k3d;
    if (k2 - 1.0).abs() < 1e-3 || (k3 - 1.0).abs() < 1e-3 {
        return plain;
    }
    let n2 = [0, 1, 2].map(|i| k2 * m2[i] - m1[i]);
    let n3 = [0, 1, 2].map(|i| k3 * m3[i] - m1[i]);
    let f2 = -((n2[0] * n3[0] + n2[1] * n3[1]) / (n2[2] * n3[2]));
    if f2.is_nan() || f2 <= 0.0 || !f2.is_finite() {
        return plain;
    }
    let f = f2.sqrt();
    let len = |n: [f64; 3]| ((n[0] / f).powi(2) + (n[1] / f).powi(2) + n[2].powi(2)).sqrt();
    let ratio = len(n2) / len(n3);
    if ratio.is_finite() && ratio > 0.2 && ratio < 5.0 {
        ratio
    } else {
        plain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const QUAD: [[f64; 2]; 4] = [
        [300.0, 200.0],
        [900.0, 260.0],
        [960.0, 800.0],
        [240.0, 760.0],
    ];

    #[test]
    fn corners_are_named_from_their_place() {
        let q = Quad::from_xy(QUAD);
        let named = |order: [usize; 4]| Quad::from_corners(order.map(|i| q.0[i]));
        assert_eq!(named([2, 3, 0, 1]), q);
        assert_eq!(named([0, 3, 2, 1]), q);
        assert!(q.is_convex());
        assert!(!Quad([q.0[0], q.0[2], q.0[1], q.0[3]]).is_convex());
    }

    #[test]
    fn movement_ignores_which_corner_comes_first() {
        let q = Quad::from_xy(QUAD);
        let renamed = Quad([q.0[1], q.0[2], q.0[3], q.0[0]]);
        assert_eq!(q.movement(&renamed), 0.0);
        assert!(q.max_corner_distance(&renamed) > 100.0);
    }

    #[test]
    fn the_start_rectangle_and_clamping() {
        let out = Quad::from_xy([
            [-20.0, -10.0],
            [700.0, -10.0],
            [700.0, 660.0],
            [-20.0, 660.0],
        ]);
        assert_eq!(
            out.clamped_to_photo(640.0, 640.0),
            Quad::from_xy([[0.0, 0.0], [640.0, 0.0], [640.0, 640.0], [0.0, 640.0]])
        );
        assert!(Quad::start(640.0, 640.0).usable());
        assert!(!Quad::from_xy([[0.0, 0.0], [5.0, 0.0], [5.0, 100.0], [0.0, 100.0]]).usable());
    }

    #[test]
    fn the_sheets_shape_is_found_through_the_perspective() {
        let f = 1000.0;
        let (w, h) = (210.0, 297.0);
        let project = |turn_x: f64, turn_y: f64| {
            Quad::from_xy(
                [
                    [-w / 2.0, -h / 2.0],
                    [w / 2.0, -h / 2.0],
                    [w / 2.0, h / 2.0],
                    [-w / 2.0, h / 2.0],
                ]
                .map(|[x, y]| {
                    let y1 = y * f64::cos(turn_x);
                    let z1 = y * f64::sin(turn_x);
                    let x2 = x * f64::cos(turn_y) + z1 * f64::sin(turn_y);
                    let z2 = -x * f64::sin(turn_y) + z1 * f64::cos(turn_y) + 600.0;
                    [f * x2 / z2 + 500.0, f * y1 / z2 + 400.0]
                }),
            )
        };
        for (a, b) in [(0.5, 0.3), (-0.4, 0.5), (0.3, -0.2)] {
            assert!((aspect(&project(a, b), 1000.0, 800.0) - w / h).abs() < 0.05);
        }
        assert!((aspect(&project(0.0, 0.0), 1000.0, 800.0) - w / h).abs() < 0.005);
    }
}
