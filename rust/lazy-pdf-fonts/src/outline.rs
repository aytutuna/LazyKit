//! A glyph's outline in font units, y up.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutlineOp {
    Move,
    Line,
    Quad,
    Cubic,
    Close,
}

/// One step of an outline. (x, y) is where it ends; (x1, y1) is the control point of a quadratic or the first
/// of a cubic, (x2, y2) the second of a cubic.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub op: OutlineOp,
    pub x: f64,
    pub y: f64,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub x_min: f64,
    pub y_min: f64,
    pub x_max: f64,
    pub y_max: f64,
}

/// TrueType gives lines and quadratics, CFF and Type 1 lines and cubics.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outline {
    pub segments: Vec<Segment>,
}

impl Outline {
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    fn push(&mut self, op: OutlineOp, x: f64, y: f64, x1: f64, y1: f64, x2: f64, y2: f64) {
        self.segments.push(Segment {
            op,
            x,
            y,
            x1,
            y1,
            x2,
            y2,
        });
    }

    pub fn move_to(&mut self, x: f64, y: f64) {
        self.push(OutlineOp::Move, x, y, 0.0, 0.0, 0.0, 0.0);
    }

    pub fn line_to(&mut self, x: f64, y: f64) {
        self.push(OutlineOp::Line, x, y, 0.0, 0.0, 0.0, 0.0);
    }

    pub fn quad_to(&mut self, x1: f64, y1: f64, x: f64, y: f64) {
        self.push(OutlineOp::Quad, x, y, x1, y1, 0.0, 0.0);
    }

    pub fn cubic_to(&mut self, x1: f64, y1: f64, x2: f64, y2: f64, x: f64, y: f64) {
        self.push(OutlineOp::Cubic, x, y, x1, y1, x2, y2);
    }

    pub fn close(&mut self) {
        self.push(OutlineOp::Close, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    }

    /// The box around the points, control points included, or none when the outline is empty.
    pub fn bounds(&self) -> Option<Bounds> {
        let mut b = Bounds {
            x_min: f64::MAX,
            y_min: f64::MAX,
            x_max: -f64::MAX,
            y_max: -f64::MAX,
        };
        let mut any = false;
        let mut grow = |x: f64, y: f64| {
            b.x_min = b.x_min.min(x);
            b.x_max = b.x_max.max(x);
            b.y_min = b.y_min.min(y);
            b.y_max = b.y_max.max(y);
        };
        for s in &self.segments {
            if s.op == OutlineOp::Close {
                continue;
            }
            any = true;
            grow(s.x, s.y);
            if matches!(s.op, OutlineOp::Quad | OutlineOp::Cubic) {
                grow(s.x1, s.y1);
            }
            if s.op == OutlineOp::Cubic {
                grow(s.x2, s.y2);
            }
        }
        any.then_some(b)
    }
}
