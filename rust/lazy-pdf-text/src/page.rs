//! The text of one page, characters with their places, grouped into words, lines and blocks in reading order. Built
//! from the page's content by `ContentInterpreter`; the result never changes. Coordinates are page user space, as in the
//! content.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, OnceLock};

use lazy_pdf_fonts::{ContentInterpreter, Matrix};
use lazy_pdf_syntax::{Document, Object, Page, Rect};

use crate::collector::TextCollector;
use crate::flow::{FlowLine, TextFlow};
use crate::geometry::PageGeometry;
use crate::layers::LayerVisibility;
use crate::model::{
    PageLine, TextBlock, TextBreak, TextChar, TextPageOptions, TextRange, TextWord,
};
use crate::quad::{QuadBox, TextQuad};
use crate::unicode::{is_letter, is_lower};

pub struct TextPage {
    pub(crate) geometry: PageGeometry,
    pub(crate) extent: OnceLock<(f64, f64)>,
    text: OnceLock<String>,
    pub page_index: usize,
    /// The crop box of the page.
    pub bounds: Rect,
    /// The page ran past the interpreter's operation limit or 1,000,000 glyphs; the text stops there.
    pub is_truncated: bool,
    /// Built with `TextPageOptions::include_annotations`: some characters belong to appearance streams, and their
    /// operator indexes mean nothing in the page's own content.
    pub includes_annotations: bool,
    pub chars: Vec<TextChar>,
    pub words: Vec<TextWord>,
    pub lines: Vec<PageLine>,
    pub blocks: Vec<TextBlock>,
}

fn union(a: &Rect, b: &Rect) -> Rect {
    Rect {
        left: a.left.min(b.left),
        bottom: a.bottom.min(b.bottom),
        right: a.right.max(b.right),
        top: a.top.max(b.top),
    }
}

// The normal appearances of a page's annotations that are shown, each with the matrix that carries it onto the
// annotation's /Rect. Hidden and no-view annotations and popups are left out.
fn appearances(doc: &mut Document, page: &Page) -> Vec<(Arc<Object>, Matrix)> {
    let mut result = Vec::new();
    let dict = page.dict(doc);
    let items = match doc
        .lookup(&dict, "Annots")
        .map(|v| v.into_shared())
        .as_deref()
    {
        Some(Object::Array(items)) => items.clone(),
        _ => return result,
    };
    for item in &items {
        let Some(annotation) = doc.resolve(item).map(|v| v.into_shared()) else {
            continue;
        };
        let Object::Dict(annotation) = &*annotation else {
            continue;
        };
        let Some(rect) = annotation.get("Rect").and_then(|r| doc.rect(r)) else {
            continue;
        };
        let flags = doc.integer_of(annotation, "F").unwrap_or(0);
        if flags & 0b100010 != 0 || doc.name_of(annotation, "Subtype").as_deref() == Some(b"Popup")
        {
            continue;
        }
        let Some(ap) = doc.dict_of(annotation, "AP") else {
            continue;
        };
        let normal = ap
            .get("N")
            .and_then(|n| doc.resolve(n))
            .map(|v| v.into_shared());
        let found = match normal.as_deref() {
            Some(Object::Stream(_)) => normal.clone(),
            Some(Object::Dict(states)) => doc.name_of(annotation, "AS").and_then(|state| {
                let state = String::from_utf8_lossy(&state).into_owned();
                let entry = states.get(&state)?;
                doc.resolve(entry).map(|v| v.into_shared())
            }),
            _ => None,
        };
        let Some(form) = found else { continue };
        let Object::Stream(stream) = &*form else {
            continue;
        };
        let Some(bbox) = stream.dict.get("BBox").and_then(|b| doc.rect(b)) else {
            continue;
        };
        let own = Matrix::from_array(doc, stream.dict.get("Matrix")).unwrap_or(Matrix::IDENTITY);
        let moved = own.transform_rect(&bbox);
        if moved.width() < 1e-6 || moved.height() < 1e-6 {
            continue;
        }
        let sx = rect.width() / moved.width();
        let sy = rect.height() / moved.height();
        let matrix = Matrix::new(
            sx,
            0.0,
            0.0,
            sy,
            rect.left - moved.left * sx,
            rect.bottom - moved.bottom * sy,
        );
        result.push((form.clone(), matrix));
    }
    result
}

impl TextPage {
    pub fn create(doc: &mut Document, page: &Page, options: &TextPageOptions) -> TextPage {
        let layers = LayerVisibility::new(doc);
        let shown = if options.include_annotations {
            appearances(doc, page)
        } else {
            Vec::new()
        };
        let full = Rc::new(Cell::new(false));
        let mut collector = TextCollector::new(*options, layers, full.clone());
        let mut truncated;
        {
            let mut interpreter = ContentInterpreter::new(doc, &mut collector);
            interpreter.walk_patterns = false;
            interpreter.run_page(page);
            truncated = interpreter.is_truncated();
            for (form, matrix) in &shown {
                // Each run starts with a fresh operation budget, so one that ran out ends the walk.
                if truncated || full.get() {
                    break;
                }
                interpreter.run_form(form, Some(*matrix));
                truncated = truncated || interpreter.is_truncated();
            }
        }
        truncated = truncated || full.get();
        if options.remove_duplicates {
            collector.remove_duplicates();
        }
        let crop = page.crop_box(doc);
        let geometry = PageGeometry::new(&crop, page.rotate(doc), page.user_unit(doc));
        let flow = TextFlow::new(&geometry).run(collector.glyphs);
        TextPage::assemble(
            page.index,
            crop,
            geometry,
            flow.glyphs,
            flow.lines,
            truncated,
            options.include_annotations,
        )
    }

    fn assemble(
        page_index: usize,
        bounds: Rect,
        geometry: PageGeometry,
        arena: Vec<crate::collector::FlowGlyph>,
        layout: Vec<FlowLine>,
        truncated: bool,
        annotations: bool,
    ) -> TextPage {
        let mut chars: Vec<TextChar> = Vec::new();
        let mut words: Vec<TextWord> = Vec::new();
        let mut lines: Vec<PageLine> = Vec::new();
        let mut blocks: Vec<TextBlock> = Vec::new();
        let mut block_first = 0;
        for (li, o) in layout.iter().enumerate() {
            let first = chars.len();
            for &gi in &o.glyphs {
                let g = &arena[gi];
                chars.push(TextChar {
                    index: chars.len(),
                    text: g.text.clone(),
                    quad: g.quad,
                    font: g.font.clone(),
                    size: g.size,
                    color: g.color,
                    render_mode: g.mode,
                    operator_index: g.op,
                    form: g.form.clone(),
                    element: g.element,
                    offset: g.offset,
                    mcid: g.mcid,
                    is_artifact: g.artifact,
                    is_space: g.space,
                    is_implied: g.implied,
                    is_rtl: g.level & 1 == 1,
                    word: None,
                    line: li,
                });
            }
            add_words(&mut chars, first, li, &mut words);
            let mut bounds_box = QuadBox::default();
            let mut text = String::new();
            for c in &chars[first..] {
                bounds_box.add(&c.quad);
                text.push_str(&c.text);
            }
            let quad = bounds_box.to_quad();
            lines.push(PageLine {
                index: li,
                range: TextRange::new(first, chars.len()),
                text,
                bounds: quad.bounds(),
                quad,
                block: blocks.len(),
                before: o.before,
                is_vertical: o.vertical,
                is_rtl: o.rtl,
            });
            if li + 1 == layout.len() || layout[li + 1].block != o.block {
                let range = TextRange::new(block_first, li + 1);
                let mut extent = lines[block_first].bounds;
                for line in &lines[block_first + 1..=li] {
                    extent = union(&extent, &line.bounds);
                }
                blocks.push(TextBlock {
                    index: blocks.len(),
                    lines: range,
                    bounds: extent,
                    is_table: o.table,
                });
                block_first = li + 1;
            }
        }
        TextPage {
            geometry,
            extent: OnceLock::new(),
            text: OnceLock::new(),
            page_index,
            bounds,
            is_truncated: truncated,
            includes_annotations: annotations,
            chars,
            words,
            lines,
            blocks,
        }
    }

    /// The whole page as text: lines separated by a line feed, blocks by a blank line, table cells by a tab.
    pub fn text(&self) -> &str {
        self.text
            .get_or_init(|| self.get_text(TextRange::new(0, self.chars.len()), false))
    }

    /// The text of a run of characters. White space at the ends of lines is dropped. With `join_hyphens` a hyphen that
    /// ends a line and follows a letter is taken out and the line is joined to the next one; a hyphen before a capital
    /// stays (a compound name).
    pub fn get_text(&self, range: TextRange, join_hyphens: bool) -> String {
        let chars = &self.chars;
        let start = range.start.min(chars.len());
        let end = range.end.max(start).min(chars.len());
        let mut sb: Vec<u16> = Vec::new();
        let mut pending: Vec<u16> = Vec::new();
        let mut line: Option<usize> = None;
        let mut at_start = true;
        for i in start..end {
            let c = &chars[i];
            if line != Some(c.line) {
                pending.clear();
                if line.is_some() && !sb.is_empty() && !c.is_space {
                    let before = self.lines[c.line].before;
                    if !(join_hyphens && before == TextBreak::Line && try_join(&mut sb, c)) {
                        sb.extend_from_slice(separator(before));
                    }
                }
                line = Some(c.line);
                at_start = true;
            }
            if c.is_space {
                if !at_start {
                    pending.extend(c.text.encode_utf16());
                }
                continue;
            }
            if c.text.is_empty() {
                continue;
            }
            if c.text == "\u{AD}" && i + 1 < end && chars[i + 1].line == c.line {
                continue;
            }
            sb.append(&mut pending);
            sb.extend(c.text.encode_utf16());
            at_start = false;
        }
        String::from_utf16_lossy(&sb)
    }

    /// The boxes that cover a run of characters, one for each stretch of a line without a break in it, for painting a
    /// selection or a search hit. Spaces at the ends of a stretch are left out.
    pub fn quads(&self, range: TextRange) -> Vec<TextQuad> {
        let chars = &self.chars;
        let start = range.start.min(chars.len());
        let end = range.end.max(start).min(chars.len());
        let mut result = Vec::new();
        let mut bounds = QuadBox::default();
        let mut pending_from: Option<usize> = None;
        let mut previous: Option<&TextChar> = None;

        fn flush(
            result: &mut Vec<TextQuad>,
            bounds: &mut QuadBox,
            pending_from: &mut Option<usize>,
            previous: &mut Option<&TextChar>,
        ) {
            if !bounds.is_empty() {
                result.push(bounds.to_quad());
            }
            *bounds = QuadBox::default();
            *pending_from = None;
            *previous = None;
        }

        for i in start..end {
            let c = &chars[i];
            if let Some(p) = previous {
                let center = c.quad.center();
                if c.line != p.line || p.quad.distance_to(center.0, center.1) > c.size {
                    flush(&mut result, &mut bounds, &mut pending_from, &mut previous);
                }
            }
            if c.is_space {
                if !bounds.is_empty() && pending_from.is_none() {
                    pending_from = Some(i);
                }
                previous = Some(c);
                continue;
            }
            if let Some(from) = pending_from.take() {
                for k in &chars[from..i] {
                    bounds.add(&k.quad);
                }
            }
            bounds.add(&c.quad);
            previous = Some(c);
        }
        flush(&mut result, &mut bounds, &mut pending_from, &mut previous);
        result
    }

    /// The characters of a word.
    pub fn word_range(&self, word: usize) -> TextRange {
        self.words[word].range
    }

    pub fn line_range(&self, line: usize) -> TextRange {
        self.lines[line].range
    }
}

fn add_words(chars: &mut [TextChar], first: usize, line: usize, words: &mut Vec<TextWord>) {
    let mut start: Option<usize> = None;
    for i in first..=chars.len() {
        let space = i == chars.len() || chars[i].is_space;
        if !space {
            if start.is_none() {
                start = Some(i);
            }
            continue;
        }
        let Some(from) = start.take() else { continue };
        let mut text = String::new();
        let mut bounds = chars[from].quad.bounds();
        for c in &mut chars[from..i] {
            text.push_str(&c.text);
            c.word = Some(words.len());
            bounds = union(&bounds, &c.quad.bounds());
        }
        words.push(TextWord {
            index: words.len(),
            range: TextRange::new(from, i),
            text,
            bounds,
            line,
        });
    }
}

fn separator(before: TextBreak) -> &'static [u16] {
    match before {
        TextBreak::Tab => &[0x09],
        TextBreak::Line => &[0x0A],
        TextBreak::Block => &[0x0A, 0x0A],
        TextBreak::None => &[],
    }
}

fn try_join(sb: &mut Vec<u16>, next: &TextChar) -> bool {
    let Some(letter) = next.text.encode_utf16().next() else {
        return false;
    };
    if sb.len() < 2 {
        return false;
    }
    let hyphen = sb[sb.len() - 1];
    if !matches!(hyphen, 0x2D | 0xAD | 0x2010 | 0x2011)
        || !is_letter(sb[sb.len() - 2])
        || !is_letter(letter)
    {
        return false;
    }
    if hyphen == 0xAD || is_lower(letter) {
        sb.pop();
    }
    true
}
