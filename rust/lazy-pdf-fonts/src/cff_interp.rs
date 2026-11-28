//! A Type 2 charstring interpreter (Adobe TN 5177): outline and advance width of one glyph.

use crate::bytes::{Span, to_int32};
use crate::cff::{CffFont, Glyph, Private};
use crate::names::standard_encoding;
use crate::outline::{Outline, OutlineOp, Segment};

const MAX_DEPTH: i64 = 10;
const MAX_OPERATORS: i64 = 200_000;

pub(crate) struct Interpreter<'a> {
    font: &'a CffFont,
    private: &'a Private,
    width_only: bool,
    allow_seac: bool,
    s: [f64; 96],
    out: Outline,
    transient: Option<[f64; 32]>,
    n: usize,
    x: f64,
    y: f64,
    path_open: bool,
    have_width: bool,
    done: bool,
    stems: i64,
    ops: i64,
    width: f64,
}

fn bias(count: i64) -> i64 {
    if count < 1240 {
        107
    } else if count < 33900 {
        1131
    } else {
        32768
    }
}

impl<'a> Interpreter<'a> {
    pub fn new(
        font: &'a CffFont,
        private: &'a Private,
        width_only: bool,
        allow_seac: bool,
    ) -> Self {
        Interpreter {
            font,
            private,
            width_only,
            allow_seac,
            s: [0.0; 96],
            out: Outline::default(),
            transient: None,
            n: 0,
            x: 0.0,
            y: 0.0,
            path_open: false,
            have_width: false,
            done: false,
            stems: 0,
            ops: 0,
            width: private.default_width,
        }
    }

    pub fn run(mut self, glyph_id: i64) -> Glyph {
        let font = self.font;
        self.exec(font.char_string(glyph_id), 0);
        self.end_path();
        Glyph {
            outline: self.out,
            width: self.width,
        }
    }

    fn exec(&mut self, code: Span<'a>, depth: i64) {
        let font = self.font;
        let mut p = 0i64;
        while p < code.len() && !self.done {
            self.ops += 1;
            if self.ops > MAX_OPERATORS {
                self.done = true;
                return;
            }
            let b = code.0[p as usize] as i64;
            p += 1;
            if b >= 32 || b == 28 {
                let v = if b == 28 {
                    p += 2;
                    code.i16(p - 2) as f64
                } else if b <= 246 {
                    (b - 139) as f64
                } else if b <= 250 {
                    p += 1;
                    ((b - 247) * 256 + code.u8(p - 1) + 108) as f64
                } else if b <= 254 {
                    p += 1;
                    (-(b - 251) * 256 - code.u8(p - 1) - 108) as f64
                } else {
                    p += 4;
                    (code.u32(p - 4) as i32 as f64) / 65536.0
                };
                if self.n < self.s.len() {
                    self.s[self.n] = v;
                    self.n += 1;
                } else {
                    self.done = true;
                }
                continue;
            }

            let n = self.n;
            let s = self.s;
            match b {
                1 | 3 | 18 | 23 => {
                    self.take_width(n & 1 == 1);
                    self.stems += (self.n / 2) as i64;
                    self.n = 0;
                }
                19 | 20 => {
                    self.take_width(n & 1 == 1);
                    self.stems += (self.n / 2) as i64;
                    self.n = 0;
                    p += (self.stems + 7) / 8;
                }
                21 => {
                    self.take_width(n > 2);
                    if self.n >= 2 {
                        self.move_to(self.s[self.n - 2], self.s[self.n - 1]);
                    }
                    self.n = 0;
                }
                22 => {
                    self.take_width(n > 1);
                    if self.n >= 1 {
                        self.move_to(self.s[self.n - 1], 0.0);
                    }
                    self.n = 0;
                }
                4 => {
                    self.take_width(n > 1);
                    if self.n >= 1 {
                        self.move_to(0.0, self.s[self.n - 1]);
                    }
                    self.n = 0;
                }
                5 => {
                    let mut i = 0;
                    while i + 1 < n {
                        self.line_to(s[i], s[i + 1]);
                        i += 2;
                    }
                    self.n = 0;
                }
                6 | 7 => {
                    for (i, &v) in s.iter().enumerate().take(n) {
                        let horizontal = (i % 2 == 0) == (b == 6);
                        self.line_to(
                            if horizontal { v } else { 0.0 },
                            if horizontal { 0.0 } else { v },
                        );
                    }
                    self.n = 0;
                }
                8 => {
                    let mut i = 0;
                    while i + 5 < n {
                        self.curve(s[i], s[i + 1], s[i + 2], s[i + 3], s[i + 4], s[i + 5]);
                        i += 6;
                    }
                    self.n = 0;
                }
                24 => {
                    let mut i = 0;
                    while i + 7 < n {
                        self.curve(s[i], s[i + 1], s[i + 2], s[i + 3], s[i + 4], s[i + 5]);
                        i += 6;
                    }
                    if i + 1 < n {
                        self.line_to(s[i], s[i + 1]);
                    }
                    self.n = 0;
                }
                25 => {
                    let mut i = 0;
                    while i + 7 < n {
                        self.line_to(s[i], s[i + 1]);
                        i += 2;
                    }
                    if i + 5 < n {
                        self.curve(s[i], s[i + 1], s[i + 2], s[i + 3], s[i + 4], s[i + 5]);
                    }
                    self.n = 0;
                }
                26 => {
                    let mut i = 0;
                    let mut dx1 = 0.0;
                    if n & 1 == 1 {
                        dx1 = s[i];
                        i += 1;
                    }
                    while i + 3 < n {
                        self.curve(dx1, s[i], s[i + 1], s[i + 2], 0.0, s[i + 3]);
                        i += 4;
                        dx1 = 0.0;
                    }
                    self.n = 0;
                }
                27 => {
                    let mut i = 0;
                    let mut dy1 = 0.0;
                    if n & 1 == 1 {
                        dy1 = s[i];
                        i += 1;
                    }
                    while i + 3 < n {
                        self.curve(s[i], dy1, s[i + 1], s[i + 2], s[i + 3], 0.0);
                        i += 4;
                        dy1 = 0.0;
                    }
                    self.n = 0;
                }
                30 | 31 => {
                    self.alternating_curves(b == 31);
                    self.n = 0;
                }
                10 | 29 => {
                    if n == 0 {
                        continue;
                    }
                    let subrs = if b == 10 {
                        &self.private.subrs
                    } else {
                        &font.global_subrs
                    };
                    self.n -= 1;
                    let index = to_int32(s[self.n]) + bias(subrs.count);
                    if depth < MAX_DEPTH {
                        self.exec(subrs.item(font.bytes(), index), depth + 1);
                    }
                }
                11 => return,
                14 => {
                    self.take_width(n == 1 || n == 5);
                    let (s, n) = (self.s, self.n);
                    if self.allow_seac && n >= 4 {
                        self.seac(s[n - 4], s[n - 3], to_int32(s[n - 2]), to_int32(s[n - 1]));
                    }
                    self.n = 0;
                    self.done = true;
                }
                12 => {
                    self.escape(code.u8(p));
                    p += 1;
                }
                _ => self.n = 0,
            }
        }
    }

    fn take_width(&mut self, present: bool) {
        if self.have_width {
            return;
        }
        self.have_width = true;
        if present && self.n > 0 {
            self.width = self.private.nominal_width + self.s[0];
            for i in 0..self.n - 1 {
                self.s[i] = self.s[i + 1];
            }
            self.n -= 1;
        }
        if self.width_only {
            self.done = true;
        }
    }

    fn end_path(&mut self) {
        if !self.path_open {
            return;
        }
        self.out.close();
        self.path_open = false;
    }

    fn move_to(&mut self, dx: f64, dy: f64) {
        self.end_path();
        self.x += dx;
        self.y += dy;
        self.out.move_to(self.x, self.y);
        self.path_open = true;
    }

    fn begin(&mut self) {
        if self.path_open {
            return;
        }
        self.out.move_to(self.x, self.y);
        self.path_open = true;
    }

    fn line_to(&mut self, dx: f64, dy: f64) {
        self.begin();
        self.x += dx;
        self.y += dy;
        self.out.line_to(self.x, self.y);
    }

    fn curve(&mut self, dx1: f64, dy1: f64, dx2: f64, dy2: f64, dx3: f64, dy3: f64) {
        self.begin();
        let x1 = self.x + dx1;
        let y1 = self.y + dy1;
        let x2 = x1 + dx2;
        let y2 = y1 + dy2;
        self.x = x2 + dx3;
        self.y = y2 + dy3;
        self.out.cubic_to(x1, y1, x2, y2, self.x, self.y);
    }

    fn alternating_curves(&mut self, horizontal_first: bool) {
        let mut horizontal = horizontal_first;
        let s = self.s;
        let n = self.n;
        let mut i = 0;
        while i + 3 < n {
            let extra = if n - i == 5 { s[i + 4] } else { 0.0 };
            if horizontal {
                self.curve(s[i], 0.0, s[i + 1], s[i + 2], extra, s[i + 3]);
            } else {
                self.curve(0.0, s[i], s[i + 1], s[i + 2], s[i + 3], extra);
            }
            i += 4;
            horizontal = !horizontal;
        }
    }

    fn seac(&mut self, adx: f64, ady: f64, base_code: i64, accent_code: i64) {
        self.end_path();
        self.append(base_code, 0.0, 0.0);
        self.append(accent_code, adx, ady);
    }

    fn append(&mut self, standard_code: i64, dx: f64, dy: f64) {
        if !(0..=255).contains(&standard_code) {
            return;
        }
        let Some(name) = standard_encoding(standard_code as usize) else {
            return;
        };
        let gid = self.font.glyph_for_name(name);
        if gid == 0 {
            return;
        }
        for segment in self.font.interpret(gid, false, false).outline.segments {
            if segment.op == OutlineOp::Close {
                self.out.segments.push(segment);
            } else {
                self.out.segments.push(Segment {
                    x: segment.x + dx,
                    y: segment.y + dy,
                    x1: segment.x1 + dx,
                    y1: segment.y1 + dy,
                    x2: segment.x2 + dx,
                    y2: segment.y2 + dy,
                    ..segment
                });
            }
        }
    }

    fn escape(&mut self, op: i64) {
        let s = self.s;
        let n = self.n;
        match op {
            35 => {
                if n >= 12 {
                    self.curve(s[0], s[1], s[2], s[3], s[4], s[5]);
                    self.curve(s[6], s[7], s[8], s[9], s[10], s[11]);
                }
                self.n = 0;
            }
            34 => {
                if n >= 7 {
                    self.curve(s[0], 0.0, s[1], s[2], s[3], 0.0);
                    self.curve(s[4], 0.0, s[5], -s[2], s[6], 0.0);
                }
                self.n = 0;
            }
            36 => {
                if n >= 9 {
                    self.curve(s[0], s[1], s[2], s[3], s[4], 0.0);
                    self.curve(s[5], 0.0, s[6], s[7], s[8], -(s[1] + s[3] + s[7]));
                }
                self.n = 0;
            }
            37 => {
                if n >= 11 {
                    let dx = s[0] + s[2] + s[4] + s[6] + s[8];
                    let dy = s[1] + s[3] + s[5] + s[7] + s[9];
                    self.curve(s[0], s[1], s[2], s[3], s[4], s[5]);
                    if dx.abs() > dy.abs() {
                        self.curve(s[6], s[7], s[8], s[9], s[10], -dy);
                    } else {
                        self.curve(s[6], s[7], s[8], s[9], -dx, s[10]);
                    }
                }
                self.n = 0;
            }
            _ => self.arithmetic(op),
        }
    }

    fn arithmetic(&mut self, op: i64) {
        match op {
            3 | 4 | 10 | 11 | 12 | 15 | 24 | 28 => {
                if self.n < 2 {
                    self.n = 0;
                    return;
                }
                self.n -= 1;
                let b = self.s[self.n];
                self.n -= 1;
                let a = self.s[self.n];
                match op {
                    3 => self.push(if a != 0.0 && b != 0.0 { 1.0 } else { 0.0 }),
                    4 => self.push(if a != 0.0 || b != 0.0 { 1.0 } else { 0.0 }),
                    10 => self.push(a + b),
                    11 => self.push(a - b),
                    12 => self.push(if b == 0.0 { 0.0 } else { a / b }),
                    15 => self.push(if a == b { 1.0 } else { 0.0 }),
                    24 => self.push(a * b),
                    _ => {
                        self.push(b);
                        self.push(a);
                    }
                }
            }
            5 | 9 | 14 | 26 | 18 | 27 | 21 | 29 => {
                if self.n < 1 {
                    self.n = 0;
                    return;
                }
                self.n -= 1;
                let a = self.s[self.n];
                match op {
                    5 => self.push(if a == 0.0 { 1.0 } else { 0.0 }),
                    9 => self.push(a.abs()),
                    14 => self.push(-a),
                    26 => self.push(a.abs().sqrt()),
                    27 => {
                        self.push(a);
                        self.push(a);
                    }
                    21 => match &self.transient {
                        Some(t) if a >= 0.0 && a < t.len() as f64 => {
                            let v = t[to_int32(a) as usize];
                            self.push(v);
                        }
                        _ => self.push(0.0),
                    },
                    29 => {
                        let limit = crate::bytes::fmin(a, self.n as f64 - 1.0);
                        let from = self.n as i64 - 1 - if a < 0.0 { 0 } else { to_int32(limit) };
                        let v = if from >= 0 {
                            self.s[from as usize]
                        } else {
                            0.0
                        };
                        self.push(v);
                    }
                    _ => {}
                }
            }
            20 => {
                if self.n < 2 {
                    self.n = 0;
                    return;
                }
                self.n -= 1;
                let index = self.s[self.n];
                self.n -= 1;
                let value = self.s[self.n];
                let t = self.transient.get_or_insert([0.0; 32]);
                if (0.0..32.0).contains(&index) {
                    t[to_int32(index) as usize] = value;
                }
            }
            22 => {
                if self.n < 4 {
                    self.n = 0;
                    return;
                }
                self.n -= 1;
                let v2 = self.s[self.n];
                self.n -= 1;
                let v1 = self.s[self.n];
                self.n -= 1;
                let s2 = self.s[self.n];
                self.n -= 1;
                let s1 = self.s[self.n];
                self.push(if v1 <= v2 { s1 } else { s2 });
            }
            23 => self.push(0.5),
            30 => {
                if self.n < 2 {
                    self.n = 0;
                    return;
                }
                self.n -= 1;
                let j = to_int32(self.s[self.n]);
                self.n -= 1;
                let count = to_int32(self.s[self.n]);
                if count <= 0 || count > self.n as i64 {
                    return;
                }
                let count = count as usize;
                let top = self.n - count;
                let copy: Vec<f64> = self.s[top..top + count].to_vec();
                for (i, &v) in copy.iter().enumerate() {
                    // int arithmetic: i + j wraps when j is near the limit.
                    let sum = i64::from((i as i32).wrapping_add(j as i32));
                    let c = count as i64;
                    self.s[top + (sum % c + c).rem_euclid(c) as usize] = v;
                }
            }
            _ => self.n = 0,
        }
    }

    fn push(&mut self, v: f64) {
        if self.n < self.s.len() {
            self.s[self.n] = v;
            self.n += 1;
        }
    }
}
