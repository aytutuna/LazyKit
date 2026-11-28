//! Turns glyphs into ordered lines: orientation groups, baselines, segments split at wide gaps, tables, paragraph
//! chains, then a recursive cut of the blocks into reading order.

use std::collections::HashMap;

use crate::bidi_text;
use crate::collector::FlowGlyph;
use crate::geometry::PageGeometry;
use crate::model::TextBreak;
use crate::order;
use crate::quad::TextQuad;
use crate::tables;
use crate::unicode::compare_doubles;

/// A line of the result: glyphs (indexes into the arena) in logical order and how it joins the line before it.
#[derive(Debug, Default)]
pub struct FlowLine {
    pub glyphs: Vec<usize>,
    pub before: TextBreak,
    pub block: usize,
    pub table: bool,
    pub vertical: bool,
    pub rtl: bool,
}

/// Glyphs on one baseline with no wide gap, in the frame of their orientation group.
#[derive(Debug)]
pub struct FlowSeg {
    pub glyphs: Vec<usize>,
    pub x0: f64,
    pub x1: f64,
    pub ya: f64,
    pub yb: f64,
    pub y: f64,
    pub em: f64,
    pub row: usize,
    pub text: usize,
    pub words: usize,
}

impl FlowSeg {
    pub fn new() -> FlowSeg {
        FlowSeg {
            glyphs: Vec::new(),
            x0: f64::MAX,
            x1: -f64::MAX,
            ya: f64::MAX,
            yb: -f64::MAX,
            y: 0.0,
            em: 0.0,
            row: 0,
            text: 0,
            words: 0,
        }
    }

    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }

    fn finish(&mut self, arena: &[FlowGlyph]) {
        let ink: Vec<usize> = self
            .glyphs
            .iter()
            .copied()
            .filter(|&g| !arena[g].space)
            .collect();
        let first = self.glyphs.iter().position(|&g| !arena[g].space);
        let last = self.glyphs.iter().rposition(|&g| !arena[g].space);
        self.glyphs = match (first, last) {
            (Some(first), Some(last)) => self.glyphs[first..=last].to_vec(),
            _ => Vec::new(),
        };
        self.text = ink.len();
        self.words = 1;
        for i in 1..self.glyphs.len() {
            let (a, b) = (&arena[self.glyphs[i - 1]], &arena[self.glyphs[i]]);
            if !b.space && (a.space || b.x0 - a.x1 > 0.1 * (a.em + b.em) / 2.0) {
                self.words += 1;
            }
        }
        for &g in &ink {
            let g = &arena[g];
            self.x0 = self.x0.min(g.x0);
            self.x1 = self.x1.max(g.x1);
            self.ya = self.ya.min(g.ya);
            self.yb = self.yb.max(g.yb);
        }
        if ink.is_empty() {
            return;
        }
        self.y = median(ink.iter().map(|&g| arena[g].y).collect());
        self.em = median(ink.iter().map(|&g| arena[g].em).collect());
    }
}

pub fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(|a, b| compare_doubles(*a, *b));
    if values.is_empty() {
        0.0
    } else {
        values[values.len() / 2]
    }
}

/// A paragraph-like chain of segments, or a table.
#[derive(Debug)]
pub struct FlowBlock {
    pub segs: Vec<usize>,
    pub x0: f64,
    pub x1: f64,
    pub ya: f64,
    pub yb: f64,
    pub table: bool,
}

impl FlowBlock {
    pub fn new() -> FlowBlock {
        FlowBlock {
            segs: Vec::new(),
            x0: f64::MAX,
            x1: -f64::MAX,
            ya: f64::MAX,
            yb: -f64::MAX,
            table: false,
        }
    }

    pub fn add(&mut self, id: usize, s: &FlowSeg) {
        self.segs.push(id);
        self.x0 = self.x0.min(s.x0);
        self.x1 = self.x1.max(s.x1);
        self.ya = self.ya.min(s.ya);
        self.yb = self.yb.max(s.yb);
    }
}

const COLUMN_GAP: f64 = 1.2;
const SPACE_GAP: f64 = 0.1;
const STEP: f64 = std::f64::consts::PI / 12.0;

pub struct FlowResult {
    pub glyphs: Vec<FlowGlyph>,
    pub lines: Vec<FlowLine>,
}

pub struct TextFlow<'a> {
    geometry: &'a PageGeometry,
    glyphs: Vec<FlowGlyph>,
    segs: Vec<FlowSeg>,
    block: usize,
    dx: f64,
    dy: f64,
    nx: f64,
    ny: f64,
}

fn bin_of(angle: f64) -> i64 {
    (angle / STEP).round_ties_even() as i64
}

impl<'a> TextFlow<'a> {
    pub fn new(geometry: &'a PageGeometry) -> Self {
        TextFlow {
            geometry,
            glyphs: Vec::new(),
            segs: Vec::new(),
            block: 0,
            dx: 0.0,
            dy: 0.0,
            nx: 0.0,
            ny: 0.0,
        }
    }

    pub fn run(mut self, input: Vec<FlowGlyph>) -> FlowResult {
        let mut result = Vec::new();
        if input.is_empty() {
            return FlowResult {
                glyphs: input,
                lines: result,
            };
        }
        self.glyphs = input;
        let m = self.geometry.matrix();
        let scale = (m[0] * m[3] - m[1] * m[2]).abs().sqrt();
        for i in 0..self.glyphs.len() {
            self.prepare(i, scale);
        }
        let usable: Vec<usize> = (0..self.glyphs.len())
            .filter(|&i| {
                let g = &self.glyphs[i];
                let d = &g.display;
                g.em.is_finite()
                    && (d.x0 + d.y0 + d.x1 + d.y1 + d.x2 + d.y2 + d.x3 + d.y3).is_finite()
            })
            .collect();
        if usable.is_empty() {
            return self.finish(result);
        }

        // Groups come in the order they were first met; the biggest goes first, ties by that order.
        let mut keys: Vec<(bool, i64)> = Vec::new();
        let mut groups: HashMap<(bool, i64), Vec<usize>> = HashMap::new();
        for &i in &usable {
            let g = &self.glyphs[i];
            let bin = bin_of(g.angle);
            let key = (g.vertical, ((bin % 24) + 24) % 24);
            if !groups.contains_key(&key) {
                keys.push(key);
            }
            groups.entry(key).or_default().push(i);
        }
        let mut order: Vec<(usize, (bool, i64))> = keys.iter().copied().enumerate().collect();
        order.sort_by(|a, b| {
            let (ca, cb) = (groups[&a.1].len(), groups[&b.1].len());
            cb.cmp(&ca).then(a.0.cmp(&b.0))
        });
        for (_, key) in order {
            let group = groups.remove(&key).unwrap_or_default();
            self.layout(group, &mut result);
        }
        self.finish(result)
    }

    fn finish(self, lines: Vec<FlowLine>) -> FlowResult {
        FlowResult {
            glyphs: self.glyphs,
            lines,
        }
    }

    fn prepare(&mut self, i: usize, scale: f64) {
        let geometry = self.geometry;
        let g = &mut self.glyphs[i];
        let q = g.quad;
        let (x0, y0) = geometry.to_display(q.x0, q.y0);
        let (x1, y1) = geometry.to_display(q.x1, q.y1);
        let (x2, y2) = geometry.to_display(q.x2, q.y2);
        let (x3, y3) = geometry.to_display(q.x3, q.y3);
        g.display = TextQuad::new(x0, y0, x1, y1, x2, y2, x3, y3);
        g.em = g.size * scale;
        g.run = i;
        // The reading direction follows the baseline, so that a sheared matrix (fake italic) stays upright. Vertical
        // fonts and zero-width glyphs use the up vector turned a quarter clockwise.
        let bx = x1 - x0;
        let by = y1 - y0;
        let mut ux = x3 - x0;
        let mut uy = y3 - y0;
        let length = (ux * ux + uy * uy).sqrt();
        if length < 1e-12 {
            (ux, uy) = (0.0, -1.0);
        }
        g.angle = if !g.vertical && bx * bx + by * by > 1e-12 {
            by.atan2(bx)
        } else if g.vertical {
            (-uy).atan2(-ux)
        } else {
            ux.atan2(-uy)
        };
    }

    fn place(&mut self, i: usize) {
        let (dx, dy, nx, ny) = (self.dx, self.dy, self.nx, self.ny);
        let g = &mut self.glyphs[i];
        let d = g.display;
        let u = [
            dx * d.x0 + dy * d.y0,
            dx * d.x1 + dy * d.y1,
            dx * d.x2 + dy * d.y2,
            dx * d.x3 + dy * d.y3,
        ];
        let v = [
            nx * d.x0 + ny * d.y0,
            nx * d.x1 + ny * d.y1,
            nx * d.x2 + ny * d.y2,
            nx * d.x3 + ny * d.y3,
        ];
        g.x0 = u[0].min(u[1]).min(u[2].min(u[3]));
        g.x1 = u[0].max(u[1]).max(u[2].max(u[3]));
        g.ya = v[0].min(v[1]).min(v[2].min(v[3]));
        g.yb = v[0].max(v[1]).max(v[2].max(v[3]));
        if g.vertical {
            g.y = (g.ya + g.yb) / 2.0;
            return;
        }
        let bx = d.x0 + g.base * (d.x3 - d.x0);
        let by = d.y0 + g.base * (d.y3 - d.y0);
        g.y = nx * bx + ny * by;
    }

    fn layout(&mut self, group: Vec<usize>, result: &mut Vec<FlowLine>) {
        let Some(&head) = group.first() else { return };
        let vertical = self.glyphs[head].vertical;
        let centre = bin_of(self.glyphs[head].angle) as f64 * STEP;
        let turns = group
            .iter()
            .map(|&g| ieee_remainder(self.glyphs[g].angle - centre, 2.0 * std::f64::consts::PI))
            .collect();
        let phi = centre + median(turns);
        (self.dx, self.dy) = (phi.cos(), phi.sin());
        (self.nx, self.ny) = (-self.dy, self.dx);
        for &g in &group {
            self.place(g);
        }

        let mut rtl_count = 0;
        let mut ltr_count = 0;
        for &g in &group {
            match bidi_text::class_of(&self.glyphs[g].text) {
                bidi_text::BidiClass::R => rtl_count += 1,
                bidi_text::BidiClass::L => ltr_count += 1,
                _ => {}
            }
        }
        let page_rtl = rtl_count > ltr_count;

        let lines = self.lines(&group);
        let mut rows: Vec<Vec<usize>> = Vec::new();
        let mut all = Vec::new();
        for (i, line) in lines.into_iter().enumerate() {
            let mut ids = Vec::new();
            for s in line {
                let id = self.segs.len();
                let mut s = s;
                s.row = i;
                self.segs.push(s);
                ids.push(id);
                all.push(id);
            }
            rows.push(ids);
        }
        if all.is_empty() {
            return;
        }
        let em = median(all.iter().map(|&s| self.segs[s].em).collect());
        let mut blocks = tables::blocks(&rows, em, &self.segs);
        let ordered = order::sort(&mut blocks, &self.segs, em, page_rtl);

        let mut previous: Option<usize> = None;
        let first = result.is_empty();
        for bi in ordered {
            let number = self.block;
            self.block += 1;
            let mut last: Option<usize> = None;
            let table = blocks[bi].table;
            for si in blocks[bi].segs.clone() {
                let mut line = FlowLine {
                    block: number,
                    table,
                    vertical,
                    ..FlowLine::default()
                };
                line.before = if let Some(previous) = previous {
                    if let Some(last) = last {
                        if table && self.segs[last].row == self.segs[si].row {
                            TextBreak::Tab
                        } else {
                            TextBreak::Line
                        }
                    } else if same_row(&self.segs[previous], &self.segs[si]) {
                        TextBreak::Tab
                    } else {
                        TextBreak::Block
                    }
                } else if first {
                    TextBreak::None
                } else {
                    TextBreak::Block
                };
                let (glyphs, rtl) = self.logical(si, page_rtl);
                line.glyphs = glyphs;
                line.rtl = rtl;
                result.push(line);
                previous = Some(si);
                last = Some(si);
            }
        }
    }

    // Lines: glyphs on one baseline, found by sorting on it and cutting where it jumps, then script glyphs (smaller,
    // shifted off the baseline) are taken into the line they sit beside. Split at gaps too wide for a word space.
    fn lines(&mut self, group: &[usize]) -> Vec<Vec<FlowSeg>> {
        let mut sorted = group.to_vec();
        {
            let glyphs = &self.glyphs;
            sorted.sort_by(|&a, &b| {
                compare_doubles(glyphs[a].y, glyphs[b].y).then(glyphs[a].run.cmp(&glyphs[b].run))
            });
        }
        let mut lines: Vec<Vec<usize>> = Vec::new();
        let mut first = 0.0;
        for (i, &gi) in sorted.iter().enumerate() {
            let g = &self.glyphs[gi];
            if i > 0 && !lines.is_empty() {
                let p = &self.glyphs[sorted[i - 1]];
                if g.y - p.y <= 0.35 * g.em.min(p.em) && g.y - first <= 0.7 * g.em {
                    lines.last_mut().unwrap().push(gi);
                    continue;
                }
            }
            lines.push(vec![gi]);
            first = g.y;
        }
        let lines = self.attach_scripts(lines);

        let mut result = Vec::new();
        for mut line in lines {
            {
                let glyphs = &self.glyphs;
                line.sort_by(|&p, &q| {
                    let (p, q) = (&glyphs[p], &glyphs[q]);
                    if p.x0 != q.x0 {
                        compare_doubles(p.x0, q.x0)
                    } else {
                        p.run.cmp(&q.run)
                    }
                });
            }
            let mut segs = Vec::new();
            let mut cur = FlowSeg::new();
            let mut right = -f64::MAX;
            let mut last_em = 0.0;
            for gi in line {
                let g = &self.glyphs[gi];
                if !cur.glyphs.is_empty() && g.x0 - right > COLUMN_GAP * g.em.max(last_em) {
                    segs.push(std::mem::replace(&mut cur, FlowSeg::new()));
                }
                cur.glyphs.push(gi);
                if !g.space {
                    right = if cur.glyphs.len() == 1 || right == -f64::MAX {
                        g.x1
                    } else {
                        right.max(g.x1)
                    };
                    last_em = g.em;
                }
            }
            segs.push(cur);
            for s in &mut segs {
                s.finish(&self.glyphs);
            }
            segs.retain(|s| !s.glyphs.is_empty());
            if !segs.is_empty() {
                result.push(segs);
            }
        }
        result
    }

    fn attach_scripts(&self, lines: Vec<Vec<usize>>) -> Vec<Vec<usize>> {
        struct Stat {
            y: f64,
            em: f64,
            x0: f64,
            x1: f64,
        }
        let stats: Vec<Stat> = lines
            .iter()
            .map(|l| Stat {
                y: median(l.iter().map(|&g| self.glyphs[g].y).collect()),
                em: median(l.iter().map(|&g| self.glyphs[g].em).collect()),
                x0: l
                    .iter()
                    .map(|&g| self.glyphs[g].x0)
                    .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.min(v))))
                    .unwrap_or(0.0),
                x1: l
                    .iter()
                    .map(|&g| self.glyphs[g].x1)
                    .fold(None, |m: Option<f64>, v| Some(m.map_or(v, |m| m.max(v))))
                    .unwrap_or(0.0),
            })
            .collect();
        let mut target: Vec<usize> = (0..lines.len()).collect();
        for i in 0..lines.len() {
            let mut best: Option<usize> = None;
            let mut best_distance = f64::MAX;
            for j in i.saturating_sub(3)..=(i + 3).min(lines.len() - 1) {
                if j == i || stats[j].em * 0.85 < stats[i].em || stats[i].em < 0.55 * stats[j].em {
                    continue;
                }
                let d = (stats[j].y - stats[i].y).abs();
                let beside = stats[i].x1 >= stats[j].x0 - stats[j].em
                    && stats[i].x0 <= stats[j].x1 + stats[j].em;
                if d < 0.7 * stats[j].em && beside && d < best_distance {
                    best = Some(j);
                    best_distance = d;
                }
            }
            if let Some(best) = best {
                target[i] = best;
            }
        }
        let mut result: Vec<Vec<usize>> = Vec::new();
        let mut index: HashMap<usize, usize> = HashMap::new();
        let mut taken = lines;
        for i in 0..taken.len() {
            let mut t = target[i];
            while target[t] != t {
                t = target[t];
            }
            let at = *index.entry(t).or_insert_with(|| {
                result.push(Vec::new());
                result.len() - 1
            });
            result[at].append(&mut taken[i]);
        }
        result
    }

    // A line's glyphs in logical order with a space glyph wherever a gap is wide enough to be one.
    fn logical(&mut self, seg: usize, page_rtl: bool) -> (Vec<usize>, bool) {
        let source = self.segs[seg].glyphs.clone();
        let mut visual: Vec<usize> = Vec::with_capacity(source.len() + 8);
        let mut prev: Option<usize> = None;
        for gi in source {
            if let Some(p) = prev {
                let (a, b) = (&self.glyphs[p], &self.glyphs[gi]);
                if !a.space && !b.space && b.x0 - a.x1 > SPACE_GAP * (a.em + b.em) / 2.0 {
                    let gap = self.gap(p, gi);
                    visual.push(gap);
                }
            }
            visual.push(gi);
            prev = Some(gi);
        }
        let rtl = bidi_text::to_logical(&mut visual, &mut self.glyphs, page_rtl);
        (visual, rtl)
    }

    fn gap(&mut self, ai: usize, bi: usize) -> usize {
        let (a, b) = (&self.glyphs[ai], &self.glyphs[bi]);
        let ya = (a.ya + b.ya) / 2.0;
        let yb = (a.yb + b.yb) / 2.0;
        let (u0, u1) = (a.x1, b.x0);
        let (dx, dy, nx, ny) = (self.dx, self.dy, self.nx, self.ny);
        let geometry = self.geometry;
        let p = |u: f64, v: f64| geometry.to_user(u * dx + v * nx, u * dy + v * ny);
        let (p0, p1, p2, p3) = if a.vertical {
            (p(u1, yb), p(u1, ya), p(u0, ya), p(u0, yb))
        } else {
            (p(u0, yb), p(u1, yb), p(u1, ya), p(u0, ya))
        };
        let mut g = FlowGlyph::blank();
        g.text = " ".to_string();
        g.quad = TextQuad::new(p0.0, p0.1, p1.0, p1.1, p2.0, p2.1, p3.0, p3.1);
        g.size = a.size;
        g.color = a.color;
        g.mode = a.mode;
        g.artifact = a.artifact;
        g.vertical = a.vertical;
        g.space = true;
        g.implied = true;
        g.x0 = u0;
        g.x1 = u1;
        g.y = a.y;
        g.ya = ya;
        g.yb = yb;
        g.em = a.em;
        self.glyphs.push(g);
        self.glyphs.len() - 1
    }
}

fn same_row(a: &FlowSeg, b: &FlowSeg) -> bool {
    (a.y - b.y).abs() < 0.3 * a.em.max(b.em)
}

/// IEEE 754 remainder: x - n y with n the integer nearest to x / y, ties to even.
fn ieee_remainder(x: f64, y: f64) -> f64 {
    x - (x / y).round_ties_even() * y
}
