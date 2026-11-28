//! Four corners in page user space, bottom-left, bottom-right, top-right and top-left of a glyph box in the glyph's own
//! frame, so rotated and skewed text keeps its slant.

use lazy_pdf_syntax::Rect;

// Swift's min and max: the second argument wins only when it passes the comparison.
pub(crate) fn smin(a: f64, b: f64) -> f64 {
    if b < a { b } else { a }
}

pub(crate) fn smax(a: f64, b: f64) -> f64 {
    if b > a { b } else { a }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextQuad {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub x3: f64,
    pub y3: f64,
}

impl TextQuad {
    pub const ZERO: TextQuad = TextQuad::new(0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);

    #[allow(clippy::too_many_arguments)]
    pub const fn new(
        x0: f64,
        y0: f64,
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        x3: f64,
        y3: f64,
    ) -> Self {
        TextQuad {
            x0,
            y0,
            x1,
            y1,
            x2,
            y2,
            x3,
            y3,
        }
    }

    pub fn bounds(&self) -> Rect {
        Rect {
            left: smin(smin(self.x0, self.x1), smin(self.x2, self.x3)),
            bottom: smin(smin(self.y0, self.y1), smin(self.y2, self.y3)),
            right: smax(smax(self.x0, self.x1), smax(self.x2, self.x3)),
            top: smax(smax(self.y0, self.y1), smax(self.y2, self.y3)),
        }
    }

    pub fn center(&self) -> (f64, f64) {
        (
            (self.x0 + self.x1 + self.x2 + self.x3) / 4.0,
            (self.y0 + self.y1 + self.y2 + self.y3) / 4.0,
        )
    }

    /// The eight numbers of a /QuadPoints entry (ISO 32000-1 12.5.6.10): top-left, top-right, bottom-left, bottom-right.
    pub fn to_quad_points(&self) -> [f64; 8] {
        [
            self.x3, self.y3, self.x2, self.y2, self.x0, self.y0, self.x1, self.y1,
        ]
    }

    pub fn contains(&self, x: f64, y: f64) -> bool {
        let a = cross(self.x0, self.y0, self.x1, self.y1, x, y);
        let b = cross(self.x1, self.y1, self.x2, self.y2, x, y);
        let c = cross(self.x2, self.y2, self.x3, self.y3, x, y);
        let d = cross(self.x3, self.y3, self.x0, self.y0, x, y);
        (a >= 0.0 && b >= 0.0 && c >= 0.0 && d >= 0.0)
            || (a <= 0.0 && b <= 0.0 && c <= 0.0 && d <= 0.0)
    }

    /// Zero inside, else the distance to the nearest edge.
    pub fn distance_to(&self, x: f64, y: f64) -> f64 {
        if self.contains(x, y) {
            return 0.0;
        }
        smin(
            smin(
                edge(self.x0, self.y0, self.x1, self.y1, x, y),
                edge(self.x1, self.y1, self.x2, self.y2, x, y),
            ),
            smin(
                edge(self.x2, self.y2, self.x3, self.y3, x, y),
                edge(self.x3, self.y3, self.x0, self.y0, x, y),
            ),
        )
    }
}

fn cross(ax: f64, ay: f64, bx: f64, by: f64, px: f64, py: f64) -> f64 {
    (bx - ax) * (py - ay) - (by - ay) * (px - ax)
}

fn edge(ax: f64, ay: f64, bx: f64, by: f64, px: f64, py: f64) -> f64 {
    let dx = bx - ax;
    let dy = by - ay;
    let length = dx * dx + dy * dy;
    let t = if length == 0.0 {
        0.0
    } else {
        smin(smax(((px - ax) * dx + (py - ay) * dy) / length, 0.0), 1.0)
    };
    let ex = px - (ax + t * dx);
    let ey = py - (ay + t * dy);
    (ex * ex + ey * ey).sqrt()
}

/// Grows a quad around quads in the frame of the first one added.
#[derive(Debug, Clone, Default)]
pub struct QuadBox {
    ux: f64,
    uy: f64,
    vx: f64,
    vy: f64,
    umin: f64,
    umax: f64,
    vmin: f64,
    vmax: f64,
    started: bool,
}

impl QuadBox {
    pub fn is_empty(&self) -> bool {
        !self.started
    }

    pub fn add(&mut self, q: &TextQuad) {
        if !self.started {
            let mut dx = q.x1 - q.x0;
            let mut dy = q.y1 - q.y0;
            let mut length = (dx * dx + dy * dy).sqrt();
            if length < 1e-9 {
                // A zero-width glyph: take the frame from its height.
                let hx = q.x3 - q.x0;
                let hy = q.y3 - q.y0;
                let height = (hx * hx + hy * hy).sqrt();
                (dx, dy, length) = if height < 1e-9 {
                    (1.0, 0.0, 1.0)
                } else {
                    (hy, -hx, height)
                };
            }
            self.ux = dx / length;
            self.uy = dy / length;
            self.vx = -self.uy;
            self.vy = self.ux;
            if (q.x3 - q.x0) * self.vx + (q.y3 - q.y0) * self.vy < 0.0 {
                self.vx = -self.vx;
                self.vy = -self.vy;
            }
            self.umin = f64::MAX;
            self.vmin = f64::MAX;
            self.umax = -f64::MAX;
            self.vmax = -f64::MAX;
            self.started = true;
        }
        self.include(q.x0, q.y0);
        self.include(q.x1, q.y1);
        self.include(q.x2, q.y2);
        self.include(q.x3, q.y3);
    }

    fn include(&mut self, x: f64, y: f64) {
        let u = x * self.ux + y * self.uy;
        let v = x * self.vx + y * self.vy;
        self.umin = smin(self.umin, u);
        self.umax = smax(self.umax, u);
        self.vmin = smin(self.vmin, v);
        self.vmax = smax(self.vmax, v);
    }

    pub fn to_quad(&self) -> TextQuad {
        let p = |u: f64, v: f64| (u * self.ux + v * self.vx, u * self.uy + v * self.vy);
        let (a, b, c, d) = (
            p(self.umin, self.vmin),
            p(self.umax, self.vmin),
            p(self.umax, self.vmax),
            p(self.umin, self.vmax),
        );
        TextQuad::new(a.0, a.1, b.0, b.1, c.0, c.1, d.0, d.1)
    }
}
