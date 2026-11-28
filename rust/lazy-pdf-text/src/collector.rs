//! Gathers the glyphs a page shows, with the clip, invisibility, /ActualText and artifact rules applied.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use lazy_pdf_fonts::{
    ContentContext, ContentVisitor, FormInfo, GlyphInfo, MarkedContent, PdfFont, RgbColor,
};
use lazy_pdf_syntax::Object;
use unicode_normalization::UnicodeNormalization;

use crate::layers::LayerVisibility;
use crate::model::TextPageOptions;
use crate::quad::{QuadBox, TextQuad};
use crate::unicode::is_white_space;

/// A glyph on its way from the interpreter to the layout: user space data from the content, display space data for the
/// layout.
#[derive(Clone)]
pub struct FlowGlyph {
    pub text: String,
    pub quad: TextQuad,
    pub font: Option<Arc<PdfFont>>,
    pub size: f64,
    pub color: RgbColor,
    pub mode: i64,
    pub op: i64,
    pub form: Option<Arc<Object>>,
    pub element: usize,
    pub offset: usize,
    pub mcid: Option<i64>,
    pub artifact: bool,
    pub vertical: bool,
    pub space: bool,
    pub implied: bool,
    /// Where the baseline sits in the box, from its bottom edge, as a fraction of its height.
    pub base: f64,
    pub display: TextQuad,
    pub em: f64,
    pub angle: f64,
    pub x0: f64,
    pub x1: f64,
    pub y: f64,
    pub ya: f64,
    pub yb: f64,
    pub level: i32,
    pub run: usize,
}

impl FlowGlyph {
    pub fn blank() -> FlowGlyph {
        FlowGlyph {
            text: String::new(),
            quad: TextQuad::ZERO,
            font: None,
            size: 0.0,
            color: RgbColor { r: 0, g: 0, b: 0 },
            mode: 0,
            op: -1,
            form: None,
            element: 0,
            offset: 0,
            mcid: None,
            artifact: false,
            vertical: false,
            space: false,
            implied: false,
            base: 0.2,
            display: TextQuad::ZERO,
            em: 0.0,
            angle: 0.0,
            x0: 0.0,
            x1: 0.0,
            y: 0.0,
            ya: 0.0,
            yb: 0.0,
            level: 0,
            run: 0,
        }
    }
}

const MAX_GLYPHS: usize = 1_000_000;
const MAX_CELL_SCAN: usize = 64;

struct Span {
    marked: usize,
    start: usize,
    text: String,
}

pub struct TextCollector {
    options: TextPageOptions,
    layers: LayerVisibility,
    spans: Vec<Span>,
    hidden_scopes: HashSet<usize>,
    pub glyphs: Vec<FlowGlyph>,
    /// Set when the page had more glyphs than are kept.
    full: Rc<Cell<bool>>,
}

fn identity(marked: &MarkedContent) -> usize {
    std::ptr::from_ref(marked) as usize
}

fn form_identity(form: &FormInfo) -> usize {
    Arc::as_ptr(&form.stream) as usize
}

impl TextCollector {
    pub fn new(
        options: TextPageOptions,
        layers: LayerVisibility,
        full: Rc<Cell<bool>>,
    ) -> TextCollector {
        TextCollector {
            options,
            layers,
            spans: Vec::new(),
            hidden_scopes: HashSet::new(),
            glyphs: Vec::new(),
            full,
        }
    }

    pub fn is_blank(text: &str) -> bool {
        let mut units = text.encode_utf16().peekable();
        units.peek().is_some() && units.all(is_white_space)
    }

    // ISO 32000-1 14.9.4: the text replaces the glyphs. One piece per line the glyphs run over, the text on the first,
    // so that a hit on it still covers everything it stands for.
    fn replace(&mut self, start: usize, text: &str) {
        if self.glyphs.len() <= start {
            return;
        }
        let replaced = self.glyphs.split_off(start);
        if text.is_empty() {
            return;
        }
        let mut group = vec![replaced[0].clone()];
        let mut first = true;
        for i in 1..=replaced.len() {
            if i < replaced.len() && same_line(group.last().unwrap(), &replaced[i]) {
                group.push(replaced[i].clone());
                continue;
            }
            self.glyphs
                .push(merge(&group, if first { text } else { "" }));
            first = false;
            group = if i < replaced.len() {
                vec![replaced[i].clone()]
            } else {
                Vec::new()
            };
        }
    }

    /// Drops a glyph drawn again at nearly the same place (fake bold, drop shadows).
    pub fn remove_duplicates(&mut self) {
        let cell = 4.0;
        let mut grid: HashMap<(i64, i64), Vec<usize>> = HashMap::new();
        let mut kept: Vec<FlowGlyph> = Vec::with_capacity(self.glyphs.len());
        for g in std::mem::take(&mut self.glyphs) {
            if g.text.is_empty() || g.space {
                kept.push(g);
                continue;
            }
            // A C# (long) cast of a double saturates.
            let cx = (g.quad.x0 / cell).floor() as i64;
            let cy = (g.quad.y0 / cell).floor() as i64;
            // Adjacent narrow letters of the same kind (ll) sit closer than 0.2 em, so the reach is also held under a
            // quarter of the glyph's own advance.
            let advance = ((g.quad.x1 - g.quad.x0) * (g.quad.x1 - g.quad.x0)
                + (g.quad.y1 - g.quad.y0) * (g.quad.y1 - g.quad.y0))
                .sqrt();
            let reach = if advance > 1e-9 {
                (0.2 * g.size).min(0.25 * advance)
            } else {
                0.2 * g.size
            };
            let mut duplicate = false;
            // At the ends of the Int range the C# neighbour loops do not run (the sum wraps), so no neighbour is found.
            if cx > i64::MIN && cx < i64::MAX && cy > i64::MIN && cy < i64::MAX {
                'scan: for i in (cx - 1)..=(cx + 1) {
                    for j in (cy - 1)..=(cy + 1) {
                        let Some(list) = grid.get(&(i, j)) else {
                            continue;
                        };
                        let low = list.len().saturating_sub(MAX_CELL_SCAN);
                        for &k in list[low..].iter().rev() {
                            let o = &kept[k];
                            if o.text == g.text
                                && (o.size - g.size).abs() < 0.1 * g.size
                                && (o.quad.x0 - g.quad.x0).abs() < reach
                                && (o.quad.y0 - g.quad.y0).abs() < reach
                            {
                                duplicate = true;
                                break 'scan;
                            }
                        }
                    }
                }
            }
            if duplicate {
                continue;
            }
            grid.entry((cx, cy)).or_default().push(kept.len());
            kept.push(g);
        }
        self.glyphs = kept;
    }
}

fn same_line(a: &FlowGlyph, b: &FlowGlyph) -> bool {
    let (ca, cb) = (a.quad.center(), b.quad.center());
    let (rx, ry) = if a.vertical {
        (a.quad.x0 - a.quad.x3, a.quad.y0 - a.quad.y3)
    } else {
        (a.quad.x1 - a.quad.x0, a.quad.y1 - a.quad.y0)
    };
    let length = (rx * rx + ry * ry).sqrt();
    if length < 1e-9 {
        return true;
    }
    let across = ((cb.0 - ca.0) * ry - (cb.1 - ca.1) * rx).abs() / length;
    across < 0.5 * a.size
}

fn merge(group: &[FlowGlyph], text: &str) -> FlowGlyph {
    let mut bounds = QuadBox::default();
    for g in group {
        bounds.add(&g.quad);
    }
    let f = &group[0];
    let mut merged = FlowGlyph::blank();
    merged.text = text.to_string();
    merged.quad = bounds.to_quad();
    merged.font = f.font.clone();
    merged.size = f.size;
    merged.color = f.color;
    merged.mode = f.mode;
    merged.op = f.op;
    merged.form = f.form.clone();
    merged.element = f.element;
    merged.offset = f.offset;
    merged.mcid = f.mcid;
    merged.artifact = f.artifact;
    merged.vertical = f.vertical;
    merged.space = TextCollector::is_blank(text);
    merged.base = f.base;
    merged
}

impl ContentVisitor for TextCollector {
    fn show_glyph(&mut self, context: &ContentContext, info: &GlyphInfo) {
        let state = &context.state;
        if state.render_mode == 7 || !self.hidden_scopes.is_empty() {
            return;
        }
        if self.glyphs.len() >= MAX_GLYPHS {
            self.full.set(true);
            return;
        }
        let m = &info.matrix;
        let mut size = (m.c * m.c + m.d * m.d).sqrt();
        let font = &info.font;
        let (mut ascent, descent);
        if font.is_type3() {
            // The glyphs of a Type 3 font have the size its /FontMatrix and /FontBBox give, not the font size.
            ascent = if font.ascent > 0.0 {
                font.ascent / 1000.0
            } else {
                0.8
            };
            descent = if font.descent < 0.0 {
                font.descent / 1000.0
            } else {
                -0.2
            };
            size *= (ascent - descent).max(1e-3);
        } else {
            ascent = if font.ascent > 0.0 {
                (font.ascent / 1000.0).min(1.1)
            } else {
                0.8
            };
            descent = if font.descent < 0.0 {
                (font.descent / 1000.0).max(-0.4)
            } else {
                -0.2
            };
        }
        if size.is_nan()
            || size <= 1e-6
            || !size.is_finite()
            || !m.e.is_finite()
            || !m.f.is_finite()
        {
            return;
        }

        ascent = ascent.max(if font.is_type3() { 1e-3 } else { 0.5 });
        let width = (info.character.width / 1000.0).max(0.0);
        let (x0, y0) = m.transform(0.0, descent);
        let (x1, y1) = m.transform(width, descent);
        let (x2, y2) = m.transform(width, ascent);
        let (x3, y3) = m.transform(0.0, ascent);
        let quad = TextQuad::new(x0, y0, x1, y1, x2, y2, x3, y3);
        if !(quad.x0 + quad.y0 + quad.x1 + quad.y1 + quad.x2 + quad.y2 + quad.x3 + quad.y3)
            .is_finite()
        {
            return;
        }

        let bounds = quad.bounds();
        let clip = &state.clip;
        if bounds.right < clip.left
            || bounds.left > clip.right
            || bounds.top < clip.bottom
            || bounds.bottom > clip.top
        {
            return;
        }

        let mut artifact = false;
        let mut mcid = None;
        if !context.marked.is_empty() {
            artifact = context.marked.iter().any(|m| m.tag == "Artifact");
            mcid = context.mcid();
        }
        if artifact && !self.options.include_artifacts {
            return;
        }

        let text = match &info.character.unicode {
            Some(t) => t.clone(),
            None if info.character.is_space => " ".to_string(),
            None => String::new(),
        };
        let mut g = FlowGlyph::blank();
        g.text = clean(&text);
        g.quad = quad;
        g.font = Some(font.clone());
        g.size = size;
        g.color = if state.render_mode == 1 {
            state.stroke_color.rgb()
        } else {
            state.fill_color.rgb()
        };
        g.mode = state.render_mode;
        g.op = context.operator_index as i64;
        g.form = context.forms.last().cloned();
        g.element = info.element;
        g.offset = info.offset;
        g.mcid = mcid;
        g.artifact = artifact;
        g.vertical = font.is_vertical;
        g.space = Self::is_blank(&g.text);
        g.base = -descent / (ascent - descent);
        self.glyphs.push(g);
    }

    fn begin_form(&mut self, _context: &ContentContext, form: &FormInfo) {
        if !form.is_pattern
            && let Some(layer) = &form.layer
            && self.layers.is_hidden(layer)
        {
            self.hidden_scopes.insert(form_identity(form));
        }
    }

    fn end_form(&mut self, _context: &ContentContext, form: &FormInfo) {
        self.hidden_scopes.remove(&form_identity(form));
    }

    fn begin_marked(&mut self, _context: &ContentContext, marked: &MarkedContent) {
        if let Some(layer) = &marked.layer
            && self.layers.is_hidden(layer)
        {
            self.hidden_scopes.insert(identity(marked));
        }
        if !self.spans.is_empty() {
            return;
        }
        if let Some(bytes) = &marked.actual_text {
            self.spans.push(Span {
                marked: identity(marked),
                start: self.glyphs.len(),
                text: clean(&crate::text_string::decode(bytes)),
            });
        }
    }

    fn end_marked(&mut self, _context: &ContentContext, marked: &MarkedContent) {
        self.hidden_scopes.remove(&identity(marked));
        if self
            .spans
            .first()
            .is_none_or(|s| s.marked != identity(marked))
        {
            return;
        }
        let span = self.spans.remove(0);
        self.spans.clear();
        self.replace(span.start, &span.text);
    }
}

fn dropped(v: u32) -> bool {
    v < 0x20 || (0x7F..0xA0).contains(&v) || v == 0x200B || v == 0xFEFF
}

/// Removes control characters and folds ligature and presentation form code points to the letters they stand for.
pub fn clean(text: &str) -> String {
    let mut fold = false;
    let mut drop = false;
    for c in text.chars() {
        let v = c as u32;
        if dropped(v) {
            drop = true;
        } else if (0xFB00..=0xFDFF).contains(&v) || (0xFE70..=0xFEFF).contains(&v) {
            fold = true;
        }
    }
    if !drop && !fold {
        return text.to_string();
    }
    let result: String = if drop {
        text.chars().filter(|&c| !dropped(c as u32)).collect()
    } else {
        text.to_string()
    };
    if !fold {
        return result;
    }
    result.nfkc().collect()
}
