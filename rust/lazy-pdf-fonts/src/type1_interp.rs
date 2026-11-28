//! A Type 1 charstring interpreter: outline and advance width of one glyph, flex and seac included.

use crate::bytes::{Span, to_int32};
use crate::names::standard_encoding;
use crate::outline::{Outline, OutlineOp, Segment};
use crate::type1::{Glyph, Type1Font};

const MAX_DEPTH: i64 = 10;
const MAX_OPERATORS: i64 = 200_000;

pub(crate) struct Interpreter<'a> {
    font: &'a Type1Font,
    allow_seac: bool,
    s: [f64; 64],
    ps: Vec<f64>,
    flex: Vec<(f64, f64)>,
    out: Outline,
    n: usize,
    x: f64,
    y: f64,
    sbx: f64,
    width: f64,
    path_open: bool,
    flexing: bool,
    done: bool,
    ops: i64,
}

impl<'a> Interpreter<'a> {
    pub fn new(font: &'a Type1Font, allow_seac: bool) -> Self {
        Interpreter {
            font,
            allow_seac,
            s: [0.0; 64],
            ps: Vec::new(),
            flex: Vec::new(),
            out: Outline::default(),
            n: 0,
            x: 0.0,
            y: 0.0,
            sbx: 0.0,
            width: 0.0,
            path_open: false,
            flexing: false,
            done: false,
            ops: 0,
        }
    }

    pub fn run(mut self, name: &str) -> Glyph {
        let font = self.font;
        if let Some(code) = font.char_string(name) {
            self.exec(code, 0);
        }
        self.close();
        Glyph {
            outline: self.out,
            width: self.width,
        }
    }

    fn exec(&mut self, bytes: &'a [u8], depth: i64) {
        let font = self.font;
        let code = Span(bytes);
        let mut p = 0i64;
        while p < code.len() && !self.done {
            self.ops += 1;
            if self.ops > MAX_OPERATORS {
                self.done = true;
                return;
            }
            let b = code.0[p as usize] as i64;
            p += 1;
            if b >= 32 {
                let v = if b <= 246 {
                    (b - 139) as f64
                } else if b <= 250 {
                    p += 1;
                    ((b - 247) * 256 + code.u8(p - 1) + 108) as f64
                } else if b <= 254 {
                    p += 1;
                    (-(b - 251) * 256 - code.u8(p - 1) - 108) as f64
                } else {
                    p += 4;
                    code.u32(p - 4) as i32 as f64
                };
                self.push(v);
                continue;
            }

            let n = self.n;
            let s = self.s;
            match b {
                13 if n >= 2 => {
                    self.sbx = s[0];
                    self.width = s[1];
                    self.x = self.sbx;
                    self.y = 0.0;
                    self.n = 0;
                }
                4 if n >= 1 => {
                    self.move_to(0.0, s[n - 1]);
                    self.n = 0;
                }
                21 if n >= 2 => {
                    self.move_to(s[n - 2], s[n - 1]);
                    self.n = 0;
                }
                22 if n >= 1 => {
                    self.move_to(s[n - 1], 0.0);
                    self.n = 0;
                }
                5 if n >= 2 => {
                    self.line_to(s[0], s[1]);
                    self.n = 0;
                }
                6 if n >= 1 => {
                    self.line_to(s[0], 0.0);
                    self.n = 0;
                }
                7 if n >= 1 => {
                    self.line_to(0.0, s[0]);
                    self.n = 0;
                }
                8 if n >= 6 => {
                    self.curve(s[0], s[1], s[2], s[3], s[4], s[5]);
                    self.n = 0;
                }
                30 if n >= 4 => {
                    self.curve(0.0, s[0], s[1], s[2], s[3], 0.0);
                    self.n = 0;
                }
                31 if n >= 4 => {
                    self.curve(s[0], 0.0, s[1], s[2], 0.0, s[3]);
                    self.n = 0;
                }
                9 => {
                    self.close();
                    self.n = 0;
                }
                10 if n >= 1 => {
                    self.n -= 1;
                    let index = to_int32(s[self.n]);
                    if depth < MAX_DEPTH
                        && index >= 0
                        && (index as usize) < font.subrs.len()
                        && let Some(sub) = &font.subrs[index as usize]
                    {
                        self.exec(sub, depth + 1);
                    }
                }
                11 => return,
                14 => self.done = true,
                12 => {
                    self.escape(code.u8(p));
                    p += 1;
                }
                _ => self.n = 0,
            }
        }
    }

    fn escape(&mut self, op: i64) {
        let n = self.n;
        let s = self.s;
        match op {
            6 if n >= 5 && self.allow_seac => {
                self.seac(s[0], s[1], s[2], to_int32(s[3]), to_int32(s[4]));
                self.n = 0;
                self.done = true;
            }
            7 if n >= 4 => {
                self.sbx = s[0];
                self.width = s[2];
                self.x = self.sbx;
                self.y = s[1];
                self.n = 0;
            }
            12 if n >= 2 => {
                self.n -= 1;
                let d = s[self.n];
                self.n -= 1;
                let a = s[self.n];
                self.push(if d == 0.0 { 0.0 } else { a / d });
            }
            16 if n >= 2 => self.call_other_subr(),
            17 => {
                if let Some(last) = self.ps.pop() {
                    self.push(last);
                }
            }
            33 if n >= 2 => {
                self.x = s[0];
                self.y = s[1];
                self.n = 0;
            }
            _ => self.n = 0,
        }
    }

    fn call_other_subr(&mut self) {
        self.n -= 1;
        let which = to_int32(self.s[self.n]);
        self.n -= 1;
        let mut count = to_int32(self.s[self.n]);
        if count < 0 || count > self.n as i64 {
            count = self.n as i64;
        }

        match which {
            1 => {
                self.flexing = true;
                self.flex.clear();
                self.n -= count as usize;
            }
            2 => {
                if self.flexing {
                    self.flex.push((self.x, self.y));
                }
                self.n -= count as usize;
            }
            0 if count >= 3 => {
                let ey = self.s[self.n - 1];
                let ex = self.s[self.n - 2];
                self.n -= count as usize;
                self.flexing = false;
                if self.flex.len() >= 7 {
                    self.begin();
                    for k in [1usize, 4] {
                        let (a, b, c) = (self.flex[k], self.flex[k + 1], self.flex[k + 2]);
                        self.out.cubic_to(a.0, a.1, b.0, b.1, c.0, c.1);
                    }
                    self.x = self.flex[6].0;
                    self.y = self.flex[6].1;
                }
                self.ps.push(ey);
                self.ps.push(ex);
            }
            _ => {
                // Hint replacement (3) and unknown routines hand their arguments back through pop.
                for _ in 0..count {
                    self.n -= 1;
                    self.ps.push(self.s[self.n]);
                }
            }
        }
    }

    fn seac(&mut self, asb: f64, adx: f64, ady: f64, base_code: i64, accent_code: i64) {
        self.close();
        self.append(base_code, 0.0, 0.0);
        self.append(accent_code, self.sbx - asb + adx, ady);
    }

    fn append(&mut self, code: i64, dx: f64, dy: f64) {
        if !(0..=255).contains(&code) {
            return;
        }
        let Some(name) = standard_encoding(code as usize) else {
            return;
        };
        let Some(glyph) = self.font.run(name, false) else {
            return;
        };
        for segment in glyph.outline.segments {
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

    fn push(&mut self, v: f64) {
        if self.n < self.s.len() {
            self.s[self.n] = v;
            self.n += 1;
        } else {
            self.done = true;
        }
    }

    fn close(&mut self) {
        if !self.path_open {
            return;
        }
        self.out.close();
        self.path_open = false;
    }

    fn begin(&mut self) {
        if self.path_open {
            return;
        }
        self.out.move_to(self.x, self.y);
        self.path_open = true;
    }

    fn move_to(&mut self, dx: f64, dy: f64) {
        self.x += dx;
        self.y += dy;
        if self.flexing {
            return;
        }
        self.close();
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
}
