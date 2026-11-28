//! Converts between a page's user space (points, origin bottom-left) and the display points the app uses (origin
//! top-left, y down) for the page as shown: the crop box, /Rotate (clockwise) and /UserUnit applied.

use lazy_pdf_syntax::Rect;

/// A rectangle in display points: origin top-left, y down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DisplayRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone)]
pub struct PageGeometry {
    m: [f64; 6],
    i: [f64; 6],
    pub width: f64,
    pub height: f64,
    pub rotation: i32,
}

impl PageGeometry {
    /// `bounds` is the visible region in user space, `rotate` clockwise degrees (a multiple of 90) and `user_unit` the
    /// user space units per point.
    pub fn new(bounds: &Rect, rotate: i32, user_unit: f64) -> PageGeometry {
        let u = if user_unit > 0.0 { user_unit } else { 1.0 };
        let rotation = rotate.rem_euclid(360) / 90 * 90;
        let turned = rotation == 90 || rotation == 270;
        let (w, h) = (bounds.width(), bounds.height());
        let width = (if turned { h } else { w }) * u;
        let height = (if turned { w } else { h }) * u;
        let m = match rotation {
            90 => [0.0, u, u, 0.0, -bounds.bottom * u, -bounds.left * u],
            180 => [-u, 0.0, 0.0, u, bounds.right * u, -bounds.bottom * u],
            270 => [0.0, -u, -u, 0.0, bounds.top * u, bounds.right * u],
            _ => [u, 0.0, 0.0, -u, -bounds.left * u, bounds.top * u],
        };
        let det = m[0] * m[3] - m[1] * m[2];
        let i = [
            m[3] / det,
            -m[1] / det,
            -m[2] / det,
            m[0] / det,
            (m[2] * m[5] - m[3] * m[4]) / det,
            (m[1] * m[4] - m[0] * m[5]) / det,
        ];
        PageGeometry {
            m,
            i,
            width,
            height,
            rotation,
        }
    }

    /// The matrix [a b c d e f] taking user space to display points.
    pub fn matrix(&self) -> [f64; 6] {
        self.m
    }

    pub fn to_display(&self, x: f64, y: f64) -> (f64, f64) {
        let m = &self.m;
        (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
    }

    pub fn to_user(&self, x: f64, y: f64) -> (f64, f64) {
        let i = &self.i;
        (i[0] * x + i[2] * y + i[4], i[1] * x + i[3] * y + i[5])
    }

    /// The display rectangle that covers a user-space one.
    pub fn rect_to_display(&self, rect: &Rect) -> DisplayRect {
        let p1 = self.to_display(rect.left, rect.bottom);
        let p2 = self.to_display(rect.right, rect.top);
        DisplayRect {
            x: p1.0.min(p2.0),
            y: p1.1.min(p2.1),
            width: (p2.0 - p1.0).abs(),
            height: (p2.1 - p1.1).abs(),
        }
    }
}
