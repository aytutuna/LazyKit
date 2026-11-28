//! Walks the operations of a page or form with the full graphics state (ISO 32000-1 8, 9) and reports glyphs, paths,
//! images, shadings and marked content to a `ContentVisitor`. Form XObjects and tiling patterns are entered, each
//! once per use for forms and once per run for patterns. Everything is bounded: the q stack, form depth, the number
//! of operations and the size of a path; when the operation limit stops a run, `is_truncated` says so.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use lazy_pdf_syntax::{Dict, Document, Name, Number, Object, Page, Rect};

use crate::color::{ColorSpaceInfo, PdfColor};
use crate::content_parser::{ContentOperation, InlineImage, parse, parse_limited};
use crate::font::{FontChar, PdfFont};
use crate::matrix::Matrix;

const MAX_STACK: usize = 256;
const MAX_FORM_DEPTH: usize = 20;
const MAX_SEGMENTS: usize = 1_000_000;
const MAX_CACHED_FORMS: usize = 1024;
const MAX_OPERATIONS: i64 = 10_000_000;
const MAX_FORM_OPERATIONS: usize = 1_000_000;
const MAX_CACHED_OPERATIONS: usize = 4_000_000;
const MAX_FORM_SIZE: usize = 64 << 20;
const MAX_MARKED_DEPTH: usize = 1024;

const UNBOUNDED: Rect = Rect {
    left: -1e9,
    bottom: -1e9,
    right: 1e9,
    top: 1e9,
};

/// The graphics state (8.4) as it stands while an operator runs, with the text state (9.3).
#[derive(Clone)]
pub struct GraphicsState {
    /// Maps the current user space to the space the page content started in.
    pub ctm: Matrix,
    /// The bounding box of the clipping path, in the space the content started in.
    pub clip: Rect,
    pub fill_color: PdfColor,
    pub stroke_color: PdfColor,
    pub line_width: f64,
    pub line_cap: i64,
    pub line_join: i64,
    pub miter_limit: f64,
    pub dash: Vec<f64>,
    pub dash_phase: f64,
    pub char_spacing: f64,
    pub word_spacing: f64,
    /// /Tz as a fraction: 1 for 100%.
    pub horizontal_scale: f64,
    pub leading: f64,
    pub font: Option<Arc<PdfFont>>,
    pub font_size: f64,
    /// 0 fill, 1 stroke, 2 both, 3 invisible, 4 to 7 the same and added to the clip.
    pub render_mode: i64,
    pub rise: f64,
    pub fill_alpha: f64,
    pub stroke_alpha: f64,
    pub blend_mode: String,
    /// The /SMask of an ExtGState: a dictionary, or none.
    pub soft_mask: Option<Dict>,
}

impl GraphicsState {
    pub fn new() -> GraphicsState {
        GraphicsState {
            ctm: Matrix::IDENTITY,
            clip: Rect {
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
                top: 0.0,
            },
            fill_color: PdfColor::black(),
            stroke_color: PdfColor::black(),
            line_width: 1.0,
            line_cap: 0,
            line_join: 0,
            miter_limit: 10.0,
            dash: Vec::new(),
            dash_phase: 0.0,
            char_spacing: 0.0,
            word_spacing: 0.0,
            horizontal_scale: 1.0,
            leading: 0.0,
            font: None,
            font_size: 0.0,
            render_mode: 0,
            rise: 0.0,
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            blend_mode: "Normal".into(),
            soft_mask: None,
        }
    }
}

impl Default for GraphicsState {
    fn default() -> Self {
        Self::new()
    }
}

/// An open marked-content sequence (14.6): `BMC` or `BDC` up to its `EMC`.
#[derive(Debug)]
pub struct MarkedContent {
    /// The tag, such as `Span`, `OC` or `Artifact`.
    pub tag: String,
    /// The property dictionary of a `BDC`, inline or looked up in /Properties.
    pub properties: Option<Dict>,
    /// /MCID of the properties, the key into the structure tree.
    pub mcid: Option<i64>,
    /// For tag `OC`: the optional content group or membership dictionary the properties name.
    pub optional_content: Option<Dict>,
    overflow: bool,
}

/// Where a callback comes from and what was true when it came.
pub struct ContentContext {
    /// The graphics state before the operator ran, except for text, where it is the state the glyph is drawn in.
    pub state: GraphicsState,
    /// The form XObjects (and tiling patterns) being run, outermost first; empty on the page itself.
    pub forms: Vec<Arc<Object>>,
    /// Index of the running operation in the parse of the content stream it belongs to.
    pub operator_index: usize,
    /// Open marked-content sequences, outermost first, those of enclosing streams included.
    pub marked: Vec<Arc<MarkedContent>>,
    /// The text matrix and text line matrix (9.4.2); identity outside a text object.
    pub text_matrix: Matrix,
    pub text_line_matrix: Matrix,
}

impl ContentContext {
    /// The innermost /MCID among the open sequences.
    pub fn mcid(&self) -> Option<i64> {
        self.marked.iter().rev().find_map(|m| m.mcid)
    }

    /// The innermost optional content dictionary among the open sequences.
    pub fn optional_content(&self) -> Option<&Dict> {
        self.marked
            .iter()
            .rev()
            .find_map(|m| m.optional_content.as_ref())
    }
}

/// A glyph shown by `Tj`, `TJ`, `'` or `"`.
#[derive(Clone)]
pub struct GlyphInfo {
    pub character: FontChar,
    pub font: Arc<PdfFont>,
    /// Maps the em square of the glyph to the space the content started in: the text rendering matrix (9.4.4), with
    /// a vertical font's origin shift applied.
    pub matrix: Matrix,
    /// The box of the glyph's advance by the font's ascent and descent, in that space.
    pub bounds: Rect,
    /// How far the text matrix moves after the glyph, in text space.
    pub advance: f64,
    /// Index of the string among the operands: 0 for Tj, the array item for TJ, 0 for ' and 1 for ".
    pub element: usize,
    /// Byte position of the code in that string.
    pub offset: usize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathSegmentKind {
    Move,
    Line,
    /// A cubic Bezier: control points 1 and 2, then the end point.
    Curve,
    Close,
}

/// One segment of a path, with the points already transformed by the matrix in force when it was added.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathSegment {
    pub kind: PathSegmentKind,
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    pub x3: f64,
    pub y3: f64,
}

/// A path painted or turned into a clip.
#[derive(Debug, Clone)]
pub struct PathInfo {
    /// The painting operator: S, s, f, F, f*, B, B*, b, b* or n.
    pub operator: String,
    pub segments: Vec<PathSegment>,
    /// The box of its points, control points of curves included.
    pub bounds: Rect,
    /// The same grown by half the line width when it is stroked.
    pub ink_bounds: Rect,
    pub fill: bool,
    pub stroke: bool,
    pub even_odd: bool,
    pub clips: bool,
}

/// An image painted by `Do` or `BI`.
#[derive(Debug, Clone)]
pub struct ImageInfo {
    /// The resource name; none for an inline image.
    pub name: Option<String>,
    pub stream: Option<Arc<Object>>,
    pub inline: Option<InlineImage>,
    /// The image dictionary with full key names.
    pub dictionary: Dict,
    pub width: i64,
    pub height: i64,
    pub is_mask: bool,
    /// Maps the unit square the image fills to the space the content started in.
    pub matrix: Matrix,
    pub bounds: Rect,
    pub optional_content: Option<Dict>,
}

/// A shading painted by `sh`.
#[derive(Debug, Clone)]
pub struct ShadingInfo {
    pub name: String,
    /// The shading dictionary, or the stream of types 4 to 7.
    pub shading: Arc<Object>,
    /// What it covers: the clipping box in force.
    pub bounds: Rect,
}

/// A form XObject or tiling pattern being entered.
#[derive(Debug, Clone)]
pub struct FormInfo {
    pub stream: Arc<Object>,
    pub name: Option<String>,
    /// Maps form (or pattern) space to the space the content started in.
    pub matrix: Matrix,
    /// Its /BBox transformed.
    pub bounds: Rect,
    pub is_pattern: bool,
    pub optional_content: Option<Dict>,
}

/// What an interpreter reports. Every method does nothing until implemented.
#[allow(unused_variables)]
pub trait ContentVisitor {
    fn show_glyph(&mut self, context: &ContentContext, glyph: &GlyphInfo) {}
    fn paint_path(&mut self, context: &ContentContext, path: &PathInfo) {}
    fn paint_image(&mut self, context: &ContentContext, image: &ImageInfo) {}
    fn paint_shading(&mut self, context: &ContentContext, shading: &ShadingInfo) {}
    fn begin_marked(&mut self, context: &ContentContext, marked: &MarkedContent) {}
    fn end_marked(&mut self, context: &ContentContext, marked: &MarkedContent) {}
    /// Called before the operations of a form or pattern run; `ContentContext::forms` already ends with it.
    fn begin_form(&mut self, context: &ContentContext, form: &FormInfo) {}
    fn end_form(&mut self, context: &ContentContext, form: &FormInfo) {}
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Number(u32),
    Text(Vec<u8>),
}

impl Key {
    // An object that is a reference is known by its number, a direct one by what it says.
    fn of(entry: &Object, resolved: &Object) -> Key {
        match entry {
            Object::Ref(r) => Key::Number(r.number),
            _ => {
                let mut text = Vec::new();
                resolved.write_to(&mut text);
                Key::Text(text)
            }
        }
    }
}

fn resolved(doc: &mut Document, value: &Object) -> Option<Arc<Object>> {
    doc.resolve(value).map(|v| v.into_shared())
}

// The resource `name` of a category, looked up in the current resources and then those of the enclosing streams;
// with the key of the entry as the file gives it.
fn lookup_resource(
    doc: &mut Document,
    resources: &[Option<Dict>],
    category: &str,
    name: &str,
) -> Option<(Key, Arc<Object>)> {
    for dict in resources.iter().rev().flatten() {
        let Some(entries) = doc.lookup(dict, category).map(|v| v.into_shared()) else {
            continue;
        };
        let Some(entries) = entries.as_dict() else {
            continue;
        };
        if let Some(entry) = entries.get(name) {
            let value = resolved(doc, entry)?;
            return Some((Key::of(entry, &value), value));
        }
    }
    None
}

pub struct ContentInterpreter<'a> {
    doc: &'a mut Document,
    visitor: &'a mut dyn ContentVisitor,
    form_operations: HashMap<u32, Arc<Vec<ContentOperation>>>,
    color_spaces: HashMap<Key, Arc<ColorSpaceInfo>>,
    fonts: HashMap<Key, Arc<PdfFont>>,
    resources: Vec<Option<Dict>>,
    active_forms: HashSet<u32>,
    walked_patterns: HashSet<u32>,
    context: ContentContext,
    stack: Vec<GraphicsState>,
    overflow: usize,
    stack_floor: usize,
    marked_floor: usize,
    base: Matrix,
    tm: Matrix,
    tlm: Matrix,
    budget: i64,
    cached_operations: usize,
    fallback_font: Option<Arc<PdfFont>>,
    path: Vec<PathSegment>,
    cur: (f64, f64),
    start: (f64, f64),
    pending_clip: u8,
    /// Whether tiling pattern cells are run when something is painted with the pattern.
    pub walk_patterns: bool,
    truncated: bool,
}

impl<'a> ContentInterpreter<'a> {
    pub fn new(doc: &'a mut Document, visitor: &'a mut dyn ContentVisitor) -> Self {
        ContentInterpreter {
            doc,
            visitor,
            form_operations: HashMap::new(),
            color_spaces: HashMap::new(),
            fonts: HashMap::new(),
            resources: Vec::new(),
            active_forms: HashSet::new(),
            walked_patterns: HashSet::new(),
            context: ContentContext {
                state: GraphicsState::new(),
                forms: Vec::new(),
                operator_index: 0,
                marked: Vec::new(),
                text_matrix: Matrix::IDENTITY,
                text_line_matrix: Matrix::IDENTITY,
            },
            stack: Vec::new(),
            overflow: 0,
            stack_floor: 0,
            marked_floor: 0,
            base: Matrix::IDENTITY,
            tm: Matrix::IDENTITY,
            tlm: Matrix::IDENTITY,
            budget: 0,
            cached_operations: 0,
            fallback_font: None,
            path: Vec::new(),
            cur: (0.0, 0.0),
            start: (0.0, 0.0),
            pending_clip: 0,
            walk_patterns: true,
            truncated: false,
        }
    }

    /// The last run stopped at the operation limit.
    pub fn is_truncated(&self) -> bool {
        self.truncated
    }

    /// Runs a page's content, the user space of the page as the starting space and its crop box as the clip.
    pub fn run_page(&mut self, page: &Page) {
        let crop = page.crop_box(self.doc);
        self.begin(crop);
        let resources = page.resources(self.doc);
        self.resources.push(resources);
        let contents = page.contents(self.doc);
        let operations = parse(&contents);
        self.execute(&operations);
    }

    /// Runs a form XObject as if it were painted at the origin of the starting space, with `matrix` applied first.
    pub fn run_form(&mut self, form: &Arc<Object>, matrix: Option<Matrix>) {
        self.begin(UNBOUNDED);
        self.context.state.ctm = matrix.unwrap_or(Matrix::IDENTITY);
        self.base = self.context.state.ctm;
        self.enter_form(form, None, None);
    }

    fn begin(&mut self, clip: Rect) {
        let mut state = GraphicsState::new();
        state.clip = clip;
        self.context = ContentContext {
            state,
            forms: Vec::new(),
            operator_index: 0,
            marked: Vec::new(),
            text_matrix: Matrix::IDENTITY,
            text_line_matrix: Matrix::IDENTITY,
        };
        self.stack.clear();
        self.overflow = 0;
        self.stack_floor = 0;
        self.marked_floor = 0;
        self.base = Matrix::IDENTITY;
        self.tm = Matrix::IDENTITY;
        self.tlm = Matrix::IDENTITY;
        self.budget = MAX_OPERATIONS;
        self.truncated = false;
        self.resources.clear();
        self.active_forms.clear();
        self.walked_patterns.clear();
        self.reset_path();
    }

    fn execute(&mut self, operations: &[ContentOperation]) {
        let outer = (self.marked_floor, self.stack_floor, self.overflow);
        self.marked_floor = self.context.marked.len();
        self.stack_floor = self.stack.len();
        self.overflow = 0;
        for (i, operation) in operations.iter().enumerate() {
            self.budget -= 1;
            if self.budget < 0 {
                self.truncated = true;
                break;
            }
            self.context.operator_index = i;
            self.dispatch(operation);
        }
        while self.context.marked.len() > self.marked_floor {
            self.end_marked_content();
        }
        (self.marked_floor, self.stack_floor, self.overflow) = outer;
    }

    fn dispatch(&mut self, op: &ContentOperation) {
        let state = &mut self.context.state;
        match op.operator.as_str() {
            "q" => self.push(),
            "Q" => self.pop(),
            "cm" => {
                if let Some(m) = numbers(op, 6) {
                    state.ctm =
                        Matrix::new(m[0], m[1], m[2], m[3], m[4], m[5]).multiply(&state.ctm);
                }
            }
            "w" => {
                if let Some(w) = number(op) {
                    state.line_width = w;
                }
            }
            "J" => {
                if let Some(v) = number(op) {
                    state.line_cap = v as i32 as i64;
                }
            }
            "j" => {
                if let Some(v) = number(op) {
                    state.line_join = v as i32 as i64;
                }
            }
            "M" => {
                if let Some(v) = number(op) {
                    state.miter_limit = v;
                }
            }
            "d" => self.set_dash(op),
            "gs" => self.set_ext_gstate(op),
            "m" | "l" | "c" | "v" | "y" | "h" | "re" => self.build_path(op),
            "S" | "s" | "f" | "F" | "f*" | "B" | "B*" | "b" | "b*" | "n" => {
                self.paint_path(&op.operator)
            }
            "W" => self.pending_clip = 1,
            "W*" => self.pending_clip = 2,
            "g" | "G" | "rg" | "RG" | "k" | "K" => self.set_device_color(op),
            "cs" | "CS" => self.set_color_space(op),
            "sc" | "scn" | "SC" | "SCN" => self.set_color(op),
            "BT" => {
                self.tm = Matrix::IDENTITY;
                self.tlm = Matrix::IDENTITY;
                self.sync_text();
            }
            "Tc" => {
                if let Some(v) = number(op) {
                    state.char_spacing = v;
                }
            }
            "Tw" => {
                if let Some(v) = number(op) {
                    state.word_spacing = v;
                }
            }
            "Tz" => {
                if let Some(v) = number(op) {
                    state.horizontal_scale = v / 100.0;
                }
            }
            "TL" => {
                if let Some(v) = number(op) {
                    state.leading = v;
                }
            }
            "Tf" => self.set_font(op),
            "Tr" => {
                if let Some(v) = number(op) {
                    state.render_mode = v as i32 as i64;
                }
            }
            "Ts" => {
                if let Some(v) = number(op) {
                    state.rise = v;
                }
            }
            "Td" | "TD" | "Tm" | "T*" => self.move_text(op),
            "Tj" | "TJ" | "'" | "\"" => self.show_text(op),
            "Do" => self.paint(op),
            "BI" => self.paint_inline_image(op),
            "sh" => self.paint_shading(op),
            "BMC" | "BDC" => self.begin_marked_content(op),
            "EMC" => self.end_marked_content(),
            _ => {}
        }
    }

    fn push(&mut self) {
        if self.stack.len() >= MAX_STACK {
            self.overflow += 1;
            return;
        }
        self.stack.push(self.context.state.clone());
    }

    fn pop(&mut self) {
        if self.overflow > 0 {
            self.overflow -= 1;
            return;
        }
        if self.stack.len() <= self.stack_floor {
            return;
        }
        if let Some(top) = self.stack.pop() {
            self.context.state = top;
        }
    }

    fn find_resource(&mut self, category: &str, name: &str) -> Option<Arc<Object>> {
        lookup_resource(self.doc, &self.resources, category, name).map(|(_, v)| v)
    }

    /// The font for a font dictionary, kept for this interpreter.
    fn font_for(&mut self, key: Key, dict: &Dict) -> Arc<PdfFont> {
        if let Some(known) = self.fonts.get(&key) {
            return known.clone();
        }
        let font = Arc::new(PdfFont::new(self.doc, dict));
        self.fonts.insert(key, font.clone());
        font
    }

    fn sync_text(&mut self) {
        self.context.text_matrix = self.tm;
        self.context.text_line_matrix = self.tlm;
    }

    // Graphics

    fn reset_path(&mut self) {
        self.path.clear();
        self.pending_clip = 0;
    }

    fn set_dash(&mut self, op: &ContentOperation) {
        let n = op.operands.len();
        if n < 2 {
            return;
        }
        let (Object::Array(array), Object::Number(phase)) =
            (&op.operands[n - 2], &op.operands[n - 1])
        else {
            return;
        };
        let dash = self.dash_array(array);
        self.context.state.dash = dash;
        self.context.state.dash_phase = phase.value();
    }

    fn dash_array(&mut self, array: &[Object]) -> Vec<f64> {
        array
            .iter()
            .take(64)
            .map(|item| match resolved(self.doc, item) {
                Some(v) => match v.as_number() {
                    Some(n) if n.value().is_finite() => n.value(),
                    _ => 0.0,
                },
                None => 0.0,
            })
            .collect()
    }

    fn set_ext_gstate(&mut self, op: &ContentOperation) {
        let Some(name) = name_operand(op) else { return };
        let Some(g) = self.find_resource("ExtGState", &name) else {
            return;
        };
        let Object::Dict(g) = &*g else { return };
        let doc = &mut *self.doc;
        if let Some(v) = doc.number_of(g, "LW") {
            self.context.state.line_width = v;
        }
        if let Some(v) = doc.integer_of(g, "LC") {
            self.context.state.line_cap = v as i32 as i64;
        }
        if let Some(v) = doc.integer_of(g, "LJ") {
            self.context.state.line_join = v as i32 as i64;
        }
        if let Some(v) = doc.number_of(g, "ML") {
            self.context.state.miter_limit = v;
        }
        if let Some(d) = doc.lookup(g, "D").map(|v| v.into_shared())
            && let Some(d) = d.as_array()
            && d.len() >= 2
            && let Some(dash) = resolved(doc, &d[0])
            && let Some(dash) = dash.as_array()
            && let Some(phase) = resolved(doc, &d[1])
            && let Some(phase) = phase.as_number()
        {
            let phase = phase.value();
            let dash = self.dash_array(dash);
            self.context.state.dash = dash;
            self.context.state.dash_phase = phase;
        }
        let doc = &mut *self.doc;
        if let Some(f) = doc.lookup(g, "Font").map(|v| v.into_shared())
            && let Some(f) = f.as_array()
            && f.len() >= 2
            && let Some(entry) = f.first()
            && let Some(font) = resolved(doc, entry)
            && let Object::Dict(font) = &*font
            && let Some(size) = resolved(doc, &f[1])
            && let Some(size) = size.as_number()
        {
            let size = size.value();
            let key = Key::of(entry, &Object::Dict(font.clone()));
            let font = self.font_for(key, font);
            self.context.state.font = Some(font);
            self.context.state.font_size = size;
        }
        let doc = &mut *self.doc;
        let state = &mut self.context.state;
        if let Some(v) = doc.number_of(g, "CA") {
            state.stroke_alpha = v.clamp(0.0, 1.0);
        }
        if let Some(v) = doc.number_of(g, "ca") {
            state.fill_alpha = v.clamp(0.0, 1.0);
        }
        let mut blend = doc.lookup(g, "BM").map(|v| v.into_shared());
        if let Some(modes) = blend.as_deref().and_then(Object::as_array)
            && let Some(first) = modes.first()
        {
            blend = resolved(doc, first);
        }
        if let Some(Object::Name(mode)) = blend.as_deref() {
            state.blend_mode = mode.text();
        }
        if g.contains_key("SMask") {
            state.soft_mask = doc.dict_of(g, "SMask");
        }
    }

    fn add(&mut self, kind: PathSegmentKind, x: f64, y: f64) {
        if self.path.len() < MAX_SEGMENTS {
            self.path.push(PathSegment {
                kind,
                x1: x,
                y1: y,
                x2: 0.0,
                y2: 0.0,
                x3: 0.0,
                y3: 0.0,
            });
        }
    }

    fn curve(&mut self, c1: (f64, f64), c2: (f64, f64), end: (f64, f64)) {
        if self.path.len() < MAX_SEGMENTS {
            self.path.push(PathSegment {
                kind: PathSegmentKind::Curve,
                x1: c1.0,
                y1: c1.1,
                x2: c2.0,
                y2: c2.1,
                x3: end.0,
                y3: end.1,
            });
        }
        self.cur = end;
    }

    fn build_path(&mut self, op: &ContentOperation) {
        let ctm = self.context.state.ctm;
        match op.operator.as_str() {
            "m" => {
                let Some(p) = numbers(op, 2) else { return };
                let t = ctm.transform(p[0], p[1]);
                self.cur = t;
                self.start = t;
                self.add(PathSegmentKind::Move, t.0, t.1);
            }
            "l" => {
                let Some(p) = numbers(op, 2) else { return };
                self.cur = ctm.transform(p[0], p[1]);
                if self.path.is_empty() {
                    self.start = self.cur;
                    self.add(PathSegmentKind::Move, self.cur.0, self.cur.1);
                } else {
                    self.add(PathSegmentKind::Line, self.cur.0, self.cur.1);
                }
            }
            "c" => {
                let Some(p) = numbers(op, 6) else { return };
                if self.path.is_empty() {
                    return;
                }
                self.curve(
                    ctm.transform(p[0], p[1]),
                    ctm.transform(p[2], p[3]),
                    ctm.transform(p[4], p[5]),
                );
            }
            "v" => {
                let Some(p) = numbers(op, 4) else { return };
                if self.path.is_empty() {
                    return;
                }
                let first = self.cur;
                self.curve(first, ctm.transform(p[0], p[1]), ctm.transform(p[2], p[3]));
            }
            "y" => {
                let Some(p) = numbers(op, 4) else { return };
                if self.path.is_empty() {
                    return;
                }
                let end = ctm.transform(p[2], p[3]);
                self.curve(ctm.transform(p[0], p[1]), end, end);
            }
            "h" => {
                if self.path.is_empty() {
                    return;
                }
                self.add(PathSegmentKind::Close, 0.0, 0.0);
                self.cur = self.start;
            }
            "re" => {
                let Some(p) = numbers(op, 4) else { return };
                let a = ctm.transform(p[0], p[1]);
                let b = ctm.transform(p[0] + p[2], p[1]);
                let c = ctm.transform(p[0] + p[2], p[1] + p[3]);
                let d = ctm.transform(p[0], p[1] + p[3]);
                self.add(PathSegmentKind::Move, a.0, a.1);
                self.add(PathSegmentKind::Line, b.0, b.1);
                self.add(PathSegmentKind::Line, c.0, c.1);
                self.add(PathSegmentKind::Line, d.0, d.1);
                self.add(PathSegmentKind::Close, 0.0, 0.0);
                self.cur = a;
                self.start = a;
            }
            _ => {}
        }
    }

    fn paint_path(&mut self, op: &str) {
        let close = matches!(op, "s" | "b" | "b*");
        let fill = matches!(op, "f" | "F" | "f*" | "B" | "B*" | "b" | "b*");
        let stroke = matches!(op, "S" | "s" | "B" | "B*" | "b" | "b*");
        if !self.path.is_empty() {
            if close {
                self.add(PathSegmentKind::Close, 0.0, 0.0);
            }
            let bounds = self.path_bounds();
            if fill || stroke || self.pending_clip != 0 {
                let state = &self.context.state;
                let grow = if stroke {
                    state.line_width.max(0.0) * state.ctm.scale_factor() / 2.0
                } else {
                    0.0
                };
                let ink = Rect {
                    left: bounds.left - grow,
                    bottom: bounds.bottom - grow,
                    right: bounds.right + grow,
                    top: bounds.top + grow,
                };
                let info = PathInfo {
                    operator: op.into(),
                    segments: self.path.clone(),
                    bounds,
                    ink_bounds: ink,
                    fill,
                    stroke,
                    even_odd: matches!(op, "f*" | "B*" | "b*"),
                    clips: self.pending_clip != 0,
                };
                self.visitor.paint_path(&self.context, &info);
                if fill {
                    let color = self.context.state.fill_color.clone();
                    self.walk_pattern(&color);
                }
                if stroke {
                    let color = self.context.state.stroke_color.clone();
                    self.walk_pattern(&color);
                }
            }
            if self.pending_clip != 0 {
                self.context.state.clip = self.context.state.clip.intersect(&bounds);
            }
        }
        self.reset_path();
    }

    fn path_bounds(&self) -> Rect {
        let (mut x1, mut y1) = (f64::MAX, f64::MAX);
        let (mut x2, mut y2) = (-f64::MAX, -f64::MAX);
        let mut include = |x: f64, y: f64| {
            x1 = nan_min(x1, x);
            y1 = nan_min(y1, y);
            x2 = nan_max(x2, x);
            y2 = nan_max(y2, y);
        };
        for s in &self.path {
            if s.kind == PathSegmentKind::Close {
                continue;
            }
            include(s.x1, s.y1);
            if s.kind != PathSegmentKind::Curve {
                continue;
            }
            include(s.x2, s.y2);
            include(s.x3, s.y3);
        }
        if x1 > x2 {
            return Rect {
                left: 0.0,
                bottom: 0.0,
                right: 0.0,
                top: 0.0,
            };
        }
        Rect {
            left: x1,
            bottom: y1,
            right: x2,
            top: y2,
        }
    }

    fn set_device_color(&mut self, op: &ContentOperation) {
        let first = op.operator.as_bytes()[0];
        let space = match first {
            b'g' | b'G' => ColorSpaceInfo::device_gray(),
            b'r' | b'R' => ColorSpaceInfo::device_rgb(),
            _ => ColorSpaceInfo::device_cmyk(),
        };
        let Some(values) = numbers(op, space.components) else {
            return;
        };
        let color = PdfColor::new(space, values);
        if first.is_ascii_uppercase() {
            self.context.state.stroke_color = color;
        } else {
            self.context.state.fill_color = color;
        }
    }

    fn set_color_space(&mut self, op: &ContentOperation) {
        let Some(name) = name_operand(op) else { return };
        let space = self.resolve_space(&name);
        let color = PdfColor::new(space.clone(), space.initial_color());
        if op.operator == "CS" {
            self.context.state.stroke_color = color;
        } else {
            self.context.state.fill_color = color;
        }
    }

    fn resolve_space(&mut self, name: &str) -> Arc<ColorSpaceInfo> {
        match name {
            "DeviceGray" | "G" => return ColorSpaceInfo::device_gray(),
            "DeviceRGB" | "RGB" => return ColorSpaceInfo::device_rgb(),
            "DeviceCMYK" | "CMYK" => return ColorSpaceInfo::device_cmyk(),
            _ => {}
        }
        let (key, definition) = if name == "Pattern" {
            let definition = Arc::new(Object::name(name));
            (Key::of(&Object::Null, &definition), definition)
        } else {
            match lookup_resource(self.doc, &self.resources, "ColorSpace", name) {
                Some(found) => found,
                None => return ColorSpaceInfo::device_gray(),
            }
        };
        if let Some(known) = self.color_spaces.get(&key) {
            return known.clone();
        }
        let space = ColorSpaceInfo::create(self.doc, &definition, 0);
        if self.color_spaces.len() < 256 {
            self.color_spaces.insert(key, space.clone());
        }
        space
    }

    fn set_color(&mut self, op: &ContentOperation) {
        let stroke = op.operator.starts_with('S');
        let space = if stroke {
            self.context.state.stroke_color.space.clone()
        } else {
            self.context.state.fill_color.space.clone()
        };
        let values: Vec<f64> = op
            .operands
            .iter()
            .filter_map(|o| match o {
                Object::Number(n) => Some(n.value()),
                _ => None,
            })
            .take(32)
            .collect();
        let color = if space.family == "Pattern" {
            let pattern_name = match op.operands.last() {
                Some(Object::Name(n)) => Some(n.clone()),
                _ => None,
            };
            let pattern = pattern_name
                .as_ref()
                .and_then(|n| self.find_resource("Pattern", &n.text()));
            PdfColor {
                space,
                components: values,
                pattern_name,
                pattern,
            }
        } else if !values.is_empty() {
            PdfColor::new(space, values)
        } else {
            return;
        };
        if stroke {
            self.context.state.stroke_color = color;
        } else {
            self.context.state.fill_color = color;
        }
    }

    // Text

    fn set_font(&mut self, op: &ContentOperation) {
        let n = op.operands.len();
        if n < 2 {
            return;
        }
        let (Object::Name(name), Object::Number(size)) = (&op.operands[n - 2], &op.operands[n - 1])
        else {
            return;
        };
        self.context.state.font_size = size.value();
        let found = lookup_resource(self.doc, &self.resources, "Font", &name.text());
        let font = match found {
            Some((key, value)) => match &*value {
                Object::Dict(dict) => self.font_for(key, dict),
                _ => self.fallback(),
            },
            None => self.fallback(),
        };
        self.context.state.font = Some(font);
    }

    // A viewer draws text in a missing font with something plain; the metrics are Helvetica's.
    fn fallback(&mut self) -> Arc<PdfFont> {
        if let Some(known) = &self.fallback_font {
            return known.clone();
        }
        let mut dict = Dict::new();
        dict.insert("Type", Object::name("Font"));
        dict.insert("Subtype", Object::name("Type1"));
        dict.insert("BaseFont", Object::name("Helvetica"));
        let font = Arc::new(PdfFont::new(self.doc, &dict));
        self.fallback_font = Some(font.clone());
        font
    }

    fn move_text(&mut self, op: &ContentOperation) {
        match op.operator.as_str() {
            "Td" | "TD" => {
                let Some(p) = numbers(op, 2) else { return };
                if op.operator == "TD" {
                    self.context.state.leading = -p[1];
                }
                self.tlm = Matrix::translate(p[0], p[1]).multiply(&self.tlm);
                self.tm = self.tlm;
            }
            "Tm" => {
                let Some(p) = numbers(op, 6) else { return };
                self.tm = Matrix::new(p[0], p[1], p[2], p[3], p[4], p[5]);
                self.tlm = self.tm;
            }
            _ => {
                self.tlm = Matrix::translate(0.0, -self.context.state.leading).multiply(&self.tlm);
                self.tm = self.tlm;
            }
        }
        self.sync_text();
    }

    fn show_text(&mut self, op: &ContentOperation) {
        let operands = &op.operands;
        let Some(last) = operands.last() else { return };
        match op.operator.as_str() {
            "Tj" => {
                if let Object::String(s) = last {
                    self.show(&s.bytes, 0);
                }
            }
            "'" | "\"" => {
                let n = operands.len();
                if op.operator == "\"" && n >= 3 {
                    if let Object::Number(aw) = &operands[n - 3] {
                        self.context.state.word_spacing = aw.value();
                    }
                    if let Object::Number(ac) = &operands[n - 2] {
                        self.context.state.char_spacing = ac.value();
                    }
                }
                self.tlm = Matrix::translate(0.0, -self.context.state.leading).multiply(&self.tlm);
                self.tm = self.tlm;
                self.sync_text();
                if let Object::String(line) = last {
                    self.show(&line.bytes, usize::from(op.operator == "\""));
                }
            }
            _ => {
                let Object::Array(array) = last else { return };
                for (i, item) in array.iter().enumerate() {
                    self.budget -= 1;
                    if self.budget < 0 {
                        self.truncated = true;
                        break;
                    }
                    match item {
                        Object::String(text) => self.show(&text.bytes, i),
                        Object::Number(adjust) => {
                            let state = &self.context.state;
                            if let Some(font) = &state.font {
                                let shift = -adjust.value() / 1000.0 * state.font_size;
                                let step = if font.is_vertical {
                                    Matrix::translate(0.0, shift)
                                } else {
                                    Matrix::translate(shift * state.horizontal_scale, 0.0)
                                };
                                self.tm = step.multiply(&self.tm);
                                self.sync_text();
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    fn show(&mut self, bytes: &[u8], element: usize) {
        let font = match &self.context.state.font {
            Some(font) => font.clone(),
            None => {
                let font = self.fallback();
                self.context.state.font = Some(font.clone());
                font
            }
        };
        let (size, scale_x, rise) = {
            let s = &self.context.state;
            (s.font_size, s.horizontal_scale, s.rise)
        };
        let scale = Matrix::new(size * scale_x, 0.0, 0.0, size, 0.0, rise);
        let mut offset = 0;
        while offset < bytes.len() {
            self.budget -= 1;
            if self.budget < 0 {
                self.truncated = true;
                return;
            }
            let (used, ch) = font.read(&bytes[offset..]);
            if used == 0 {
                return;
            }
            let state = &self.context.state;
            let width = ch.width / 1000.0;
            let mut trm = scale.multiply(&self.tm).multiply(&state.ctm);
            let spacing = state.char_spacing + if ch.is_space { state.word_spacing } else { 0.0 };
            let advance;
            if font.is_vertical {
                trm =
                    Matrix::translate(-ch.origin_x / 1000.0, -ch.origin_y / 1000.0).multiply(&trm);
                advance = ch.vertical_advance / 1000.0 * state.font_size + spacing;
            } else {
                advance = (width * state.font_size + spacing) * state.horizontal_scale;
            }
            let bounds = trm.transform_rect(&Rect::new(
                0.0,
                font.descent / 1000.0,
                width,
                font.ascent / 1000.0,
            ));
            let glyph = GlyphInfo {
                character: ch,
                font: font.clone(),
                matrix: trm,
                bounds,
                advance,
                element,
                offset,
            };
            self.visitor.show_glyph(&self.context, &glyph);
            let step = if font.is_vertical {
                Matrix::translate(0.0, advance)
            } else {
                Matrix::translate(advance, 0.0)
            };
            self.tm = step.multiply(&self.tm);
            self.sync_text();
            offset += used;
        }
    }

    // Images, shadings, forms, patterns and marked content

    fn operations(&mut self, stream: &lazy_pdf_syntax::Stream) -> Arc<Vec<ContentOperation>> {
        if let Some(known) = self.form_operations.get(&stream.number) {
            return known.clone();
        }
        let mut size = 0;
        let operations = match self.doc.stream_data_limited(stream, MAX_FORM_SIZE, false) {
            Ok(data) => {
                size = data.len();
                parse_limited(&data, MAX_FORM_OPERATIONS).unwrap_or_default()
            }
            Err(_) => Vec::new(),
        };
        let operations = Arc::new(operations);
        if stream.number != 0
            && self.form_operations.len() < MAX_CACHED_FORMS
            && self.cached_operations + operations.len() <= MAX_CACHED_OPERATIONS
        {
            self.form_operations
                .insert(stream.number, operations.clone());
            self.cached_operations += operations.len();
        } else {
            // Not kept, so every Do of it inflates and scans the stream again.
            self.budget -= (size >> 6) as i64;
        }
        operations
    }

    fn paint(&mut self, op: &ContentOperation) {
        let Some(name) = name_operand(op) else { return };
        let Some(xobject) = self.find_resource("XObject", &name) else {
            return;
        };
        let Object::Stream(stream) = &*xobject else {
            return;
        };
        let subtype = self.doc.name_of(&stream.dict, "Subtype");
        match subtype.as_deref() {
            Some(b"Image") => {
                let dictionary = stream.dict.clone();
                self.paint_image(Some(name), Some(xobject.clone()), None, dictionary);
            }
            Some(b"Form") => {
                let oc = self.doc.dict_of(&stream.dict, "OC");
                self.enter_form(&xobject, Some(name), oc);
            }
            None if stream.dict.contains_key("BBox") => {
                let oc = self.doc.dict_of(&stream.dict, "OC");
                self.enter_form(&xobject, Some(name), oc);
            }
            _ => {}
        }
    }

    fn paint_inline_image(&mut self, op: &ContentOperation) {
        let Some(image) = &op.image else { return };
        let doc = std::cell::RefCell::new(&mut *self.doc);
        let resources = &self.resources;
        let lookup = |name: &str| {
            let mut doc = doc.borrow_mut();
            lookup_resource(&mut doc, resources, "ColorSpace", name).map(|(_, v)| (*v).clone())
        };
        let dictionary = image.expanded(Some(&lookup));
        self.paint_image(None, None, Some(image.clone()), dictionary);
    }

    fn paint_image(
        &mut self,
        name: Option<String>,
        stream: Option<Arc<Object>>,
        inline: Option<InlineImage>,
        dictionary: Dict,
    ) {
        let doc = &mut *self.doc;
        let width = doc
            .integer_of(&dictionary, "Width")
            .unwrap_or(0)
            .clamp(0, i32::MAX as i64);
        let height = doc
            .integer_of(&dictionary, "Height")
            .unwrap_or(0)
            .clamp(0, i32::MAX as i64);
        let ctm = self.context.state.ctm;
        let bounds = ctm.transform_rect(&Rect::new(0.0, 0.0, 1.0, 1.0));
        let is_mask = doc.bool_of(&dictionary, "ImageMask") == Some(true);
        let optional_content = doc.dict_of(&dictionary, "OC");
        let info = ImageInfo {
            name,
            stream,
            inline,
            dictionary,
            width,
            height,
            is_mask,
            matrix: ctm,
            bounds,
            optional_content,
        };
        self.visitor.paint_image(&self.context, &info);
    }

    fn paint_shading(&mut self, op: &ContentOperation) {
        let Some(name) = name_operand(op) else { return };
        if let Some(shading) = self.find_resource("Shading", &name)
            && matches!(&*shading, Object::Dict(_) | Object::Stream(_))
        {
            let info = ShadingInfo {
                name,
                shading,
                bounds: self.context.state.clip,
            };
            self.visitor.paint_shading(&self.context, &info);
        }
    }

    fn enter_form(&mut self, form: &Arc<Object>, name: Option<String>, oc: Option<Dict>) {
        let Object::Stream(stream) = &**form else {
            return;
        };
        let mut inner = self.context.state.clone();
        let matrix =
            Matrix::from_array(self.doc, stream.dict.get("Matrix")).unwrap_or(Matrix::IDENTITY);
        inner.ctm = matrix.multiply(&inner.ctm);
        self.run_content(form, name, false, inner, oc);
    }

    // A tiling pattern's cell is run once, when something is first painted with it (8.7.3).
    fn walk_pattern(&mut self, color: &PdfColor) {
        if !self.walk_patterns {
            return;
        }
        let Some(pattern) = &color.pattern else {
            return;
        };
        let Object::Stream(stream) = &**pattern else {
            return;
        };
        if self.doc.integer_of(&stream.dict, "PatternType") != Some(1) {
            return;
        }
        if !self.walked_patterns.insert(stream.number) {
            return;
        }
        let matrix =
            Matrix::from_array(self.doc, stream.dict.get("Matrix")).unwrap_or(Matrix::IDENTITY);
        let mut inner = GraphicsState::new();
        inner.ctm = matrix.multiply(&self.base);
        inner.clip = UNBOUNDED;
        if self.doc.integer_of(&stream.dict, "PaintType") == Some(2) {
            let space = color
                .space
                .base
                .clone()
                .unwrap_or_else(ColorSpaceInfo::device_gray);
            inner.fill_color = PdfColor::new(space, color.components.clone());
            inner.stroke_color = inner.fill_color.clone();
        }
        let name = color.pattern_name.as_ref().map(Name::text);
        self.run_content(pattern, name, true, inner, None);
    }

    fn run_content(
        &mut self,
        form: &Arc<Object>,
        name: Option<String>,
        is_pattern: bool,
        mut inner: GraphicsState,
        optional_content: Option<Dict>,
    ) {
        let Object::Stream(stream) = &**form else {
            return;
        };
        if self.context.forms.len() >= MAX_FORM_DEPTH
            || (stream.number != 0 && self.active_forms.contains(&stream.number))
        {
            return;
        }
        let operations = self.operations(stream);
        self.active_forms.insert(stream.number);
        let outer_state = std::mem::take(&mut self.context.state);
        let outer_stack = std::mem::take(&mut self.stack);
        let outer_path = std::mem::take(&mut self.path);
        let (outer_base, outer_tm, outer_tlm, outer_clip) =
            (self.base, self.tm, self.tlm, self.pending_clip);
        let bbox = stream.dict.get("BBox").and_then(|b| self.doc.rect(b));
        let area = match &bbox {
            Some(b) => inner.ctm.transform_rect(b),
            None => inner.clip,
        };
        if bbox.is_some() {
            inner.clip = inner.clip.intersect(&area);
        }
        self.base = inner.ctm;
        let matrix = inner.ctm;
        self.context.state = inner;
        self.reset_path();
        let resources = self.doc.dict_of(&stream.dict, "Resources");
        self.resources.push(resources);
        self.context.forms.push(form.clone());
        let info = FormInfo {
            stream: form.clone(),
            name,
            matrix,
            bounds: area,
            is_pattern,
            optional_content,
        };
        let index = self.context.operator_index;
        self.visitor.begin_form(&self.context, &info);
        self.execute(&operations);
        self.context.operator_index = index;
        self.visitor.end_form(&self.context, &info);
        self.context.forms.pop();
        self.resources.pop();
        self.context.state = outer_state;
        self.stack = outer_stack;
        self.base = outer_base;
        self.tm = outer_tm;
        self.tlm = outer_tlm;
        self.path = outer_path;
        self.pending_clip = outer_clip;
        self.sync_text();
        self.active_forms.remove(&stream.number);
    }

    fn begin_marked_content(&mut self, op: &ContentOperation) {
        if self.context.marked.len() >= MAX_MARKED_DEPTH {
            self.context.marked.push(Arc::new(MarkedContent {
                tag: String::new(),
                properties: None,
                mcid: None,
                optional_content: None,
                overflow: true,
            }));
            return;
        }
        let tag = name_operand(op).unwrap_or_default();
        let mut properties = None;
        if op.operator == "BDC"
            && let Some(last) = op.operands.last()
            && op.operands.len() >= 2
        {
            match last {
                Object::Dict(inline) => properties = Some(inline.clone()),
                Object::Name(key) => {
                    properties =
                        self.find_resource("Properties", &key.text())
                            .and_then(|v| match &*v {
                                Object::Dict(d) => Some(d.clone()),
                                _ => None,
                            });
                }
                _ => {}
            }
        }
        // A value past the int range is no structure element at all.
        let mcid = properties
            .as_ref()
            .and_then(|p| self.doc.integer_of(p, "MCID"))
            .filter(|&v| i32::try_from(v).is_ok());
        let optional_content = if tag == "OC" {
            properties.clone()
        } else {
            None
        };
        let marked = Arc::new(MarkedContent {
            tag,
            properties,
            mcid,
            optional_content,
            overflow: false,
        });
        self.context.marked.push(marked.clone());
        self.visitor.begin_marked(&self.context, &marked);
    }

    fn end_marked_content(&mut self) {
        if self.context.marked.len() <= self.marked_floor {
            return;
        }
        let Some(marked) = self.context.marked.last().cloned() else {
            return;
        };
        if !marked.overflow {
            self.visitor.end_marked(&self.context, &marked);
        }
        self.context.marked.pop();
    }
}

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

fn number(op: &ContentOperation) -> Option<f64> {
    match op.operands.last()? {
        Object::Number(n) if n.value().is_finite() => Some(n.value()),
        _ => None,
    }
}

// The last `count` operands, which all have to be numbers.
fn numbers(op: &ContentOperation, count: usize) -> Option<Vec<f64>> {
    let available = op.operands.len();
    if available < count {
        return None;
    }
    let mut values = Vec::with_capacity(count);
    for operand in &op.operands[available - count..] {
        match operand {
            Object::Number(n) if n.value().is_finite() => values.push(n.value()),
            _ => return None,
        }
    }
    Some(values)
}

fn name_operand(op: &ContentOperation) -> Option<String> {
    match op.operands.first()? {
        Object::Name(n) => Some(n.text()),
        _ => None,
    }
}

#[allow(dead_code)]
fn unused(_: Number) {}
