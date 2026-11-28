//! A transformation matrix [a b c d e f] (ISO 32000-1 8.3.4): x' = ax + cy + e, y' = bx + dy + f. `multiply`
//! applies this matrix first and the other second, the order the spec writes its products in.

use lazy_pdf_syntax::{Document, Object, Rect};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

// A NaN stays a NaN, as Math.Min and Math.Max do.
fn nan_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

fn nan_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.max(b)
    }
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix::new(1.0, 0.0, 0.0, 1.0, 0.0, 0.0);

    pub const fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Matrix {
        Matrix { a, b, c, d, e, f }
    }

    pub fn translate(x: f64, y: f64) -> Matrix {
        Matrix::new(1.0, 0.0, 0.0, 1.0, x, y)
    }

    pub fn determinant(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// The square root of the area scale, a fair single number for line widths and font sizes.
    pub fn scale_factor(&self) -> f64 {
        self.determinant().abs().sqrt()
    }

    pub fn multiply(&self, m: &Matrix) -> Matrix {
        Matrix::new(
            self.a * m.a + self.b * m.c,
            self.a * m.b + self.b * m.d,
            self.c * m.a + self.d * m.c,
            self.c * m.b + self.d * m.d,
            self.e * m.a + self.f * m.c + m.e,
            self.e * m.b + self.f * m.d + m.f,
        )
    }

    pub fn transform(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// The box that covers the four transformed corners.
    pub fn transform_rect(&self, r: &Rect) -> Rect {
        let p1 = self.transform(r.left, r.bottom);
        let p2 = self.transform(r.right, r.bottom);
        let p3 = self.transform(r.right, r.top);
        let p4 = self.transform(r.left, r.top);
        Rect {
            left: nan_min(nan_min(p1.0, p2.0), nan_min(p3.0, p4.0)),
            bottom: nan_min(nan_min(p1.1, p2.1), nan_min(p3.1, p4.1)),
            right: nan_max(nan_max(p1.0, p2.0), nan_max(p3.0, p4.0)),
            top: nan_max(nan_max(p1.1, p2.1), nan_max(p3.1, p4.1)),
        }
    }

    /// The inverse, or none when the matrix flattens the plane.
    pub fn invert(&self) -> Option<Matrix> {
        let det = self.determinant();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        Some(Matrix::new(
            self.d / det,
            -self.b / det,
            -self.c / det,
            self.a / det,
            (self.c * self.f - self.d * self.e) / det,
            (self.b * self.e - self.a * self.f) / det,
        ))
    }

    /// The matrix an array of six numbers stands for; none for anything else.
    pub fn from_array(doc: &mut Document, array: Option<&Object>) -> Option<Matrix> {
        let array = doc.resolve(array?)?.into_shared();
        let items = array.as_array().filter(|a| a.len() >= 6)?;
        let mut n = [0.0; 6];
        for (slot, item) in n.iter_mut().zip(items) {
            let v = doc.resolve(item)?.as_number()?.value();
            if !v.is_finite() {
                return None;
            }
            *slot = v;
        }
        Some(Matrix::new(n[0], n[1], n[2], n[3], n[4], n[5]))
    }
}
