//! A page of another PDF taken over as a form XObject: its content and everything its resources reach are copied into the
//! document being made, once, however many times the form is placed. The page's /Rotate and the corner of its crop box are
//! taken out of the content, so the form's space is the page as it is shown, y up, from the bottom-left of the part
//! asked for. Annotations with an appearance are drawn into the form, as printing them would.

use std::collections::HashMap;

use lazy_pdf_filters::encode::flate;
use lazy_pdf_syntax::{Dict, Document, Name, Object, Page, Rect as PageRect, Ref};

use crate::collect::Collector;
use crate::number::num;

const MAX_OBJECTS: usize = 100_000;
const MAX_BYTES: usize = 256 << 20;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// The document has no such page.
    NoPage,
    /// The page reaches more than a page should.
    TooLarge,
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ImportError::NoPage => "The PDF has no such page.",
            ImportError::TooLarge => "The page reaches too much to copy.",
        })
    }
}

impl std::error::Error for ImportError {}

/// A page's box within its media box and its /Rotate, with the size it is shown at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageGeometry {
    pub crop: PageRect,
    /// Quarter turns clockwise, 0 to 3.
    pub rotate: u8,
    pub shown_width: f64,
    pub shown_height: f64,
}

pub fn geometry(document: &mut Document, page: &Page) -> PageGeometry {
    let crop = page.crop_box(document);
    let rotate = (page.rotate(document) / 90) as u8 & 3;
    let (w, h) = (crop.width(), crop.height());
    let (shown_width, shown_height) = if rotate % 2 == 1 { (h, w) } else { (w, h) };
    PageGeometry {
        crop,
        rotate,
        shown_width,
        shown_height,
    }
}

impl PageGeometry {
    /// A point of the page, in its own space (y up), as it is shown: y down from the top-left of the shown page.
    pub fn to_shown(&self, x: f64, y: f64) -> (f64, f64) {
        let (lx, ly) = (x - self.crop.left, y - self.crop.bottom);
        let (w, h) = (self.crop.width(), self.crop.height());
        match self.rotate {
            1 => (ly, lx),
            2 => (w - lx, ly),
            3 => (h - ly, w - lx),
            _ => (lx, h - ly),
        }
    }

    /// The inverse of `to_shown`.
    pub fn from_shown(&self, sx: f64, sy: f64) -> (f64, f64) {
        let (w, h) = (self.crop.width(), self.crop.height());
        let (lx, ly) = match self.rotate {
            1 => (sy, sx),
            2 => (w - sx, sy),
            3 => (w - sy, h - sx),
            _ => (sx, h - sy),
        };
        (lx + self.crop.left, ly + self.crop.bottom)
    }
}

/// A form made from a page: the object, and the size of the part it shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Form {
    pub object: usize,
    pub width: f64,
    pub height: f64,
}

/// Copies the objects of one source document into the output, each once.
#[derive(Default)]
pub struct Copier {
    map: HashMap<u32, usize>,
    queue: Vec<(u32, usize)>,
    /// What the import in progress added to `map`, to take back if it fails.
    journal: Vec<u32>,
    /// Where the output stood when the import in progress began.
    start: usize,
    /// Raw bytes the import in progress has copied.
    bytes: usize,
    /// Content decoded for all the pages so far.
    decoded: usize,
}

impl Copier {
    pub fn new() -> Copier {
        Copier::default()
    }

    fn target(&mut self, number: u32, out: &mut Collector) -> usize {
        if let Some(&found) = self.map.get(&number) {
            return found;
        }
        let new = out.alloc();
        self.map.insert(number, new);
        self.journal.push(number);
        self.queue.push((number, new));
        new
    }

    /// The value with every reference pointing at the copy of what it named.
    fn rewrite(&mut self, value: &Object, out: &mut Collector) -> Object {
        match value {
            Object::Ref(r) => Object::reference(self.target(r.number, out) as u32, 0),
            Object::Array(items) => {
                Object::Array(items.iter().map(|i| self.rewrite(i, out)).collect())
            }
            Object::Dict(dict) => Object::Dict(self.rewrite_dict(dict, out, None)),
            Object::Stream(stream) => Object::Dict(self.rewrite_dict(&stream.dict, out, None)),
            other => other.clone(),
        }
    }

    fn rewrite_dict(&mut self, dict: &Dict, out: &mut Collector, skip: Option<&str>) -> Dict {
        let mut new = Dict::new();
        for (key, value) in dict.entries() {
            if skip.is_some_and(|s| key.is(s)) {
                continue;
            }
            new.add(key.clone(), self.rewrite(value, out));
        }
        new
    }

    /// Copies everything reached so far.
    fn drain(&mut self, document: &mut Document, out: &mut Collector) -> Result<(), ImportError> {
        while let Some((source, target)) = self.queue.pop() {
            if out.count() - self.start > MAX_OBJECTS {
                return Err(ImportError::TooLarge);
            }
            let object = document.get(source);
            match object.as_deref() {
                Some(Object::Stream(stream)) => {
                    let data = document.raw_data(stream);
                    self.bytes += data.len();
                    if self.bytes > MAX_BYTES {
                        return Err(ImportError::TooLarge);
                    }
                    let dict = self.rewrite_dict(&stream.dict, out, Some("Length"));
                    let mut text = Vec::new();
                    dict.write_to(&mut text);
                    // The writer adds the << >> and the length.
                    let inner = text[2..text.len() - 2].to_vec();
                    out.set_stream(target, inner, data);
                }
                Some(other) => {
                    let value = self.rewrite(other, out);
                    let mut text = Vec::new();
                    value.write_to(&mut text);
                    out.set(target, text);
                }
                None => out.set(target, "null"),
            }
        }
        Ok(())
    }
}

fn matrix_text(m: [f64; 6]) -> String {
    m.iter().map(|v| num(*v)).collect::<Vec<_>>().join(" ")
}

/// The matrix taking page space to the form's: the part's bottom-left is the origin, y up.
fn form_matrix(geo: &PageGeometry, part: (f64, f64, f64, f64)) -> [f64; 6] {
    let (px, py, _, ph) = part;
    let map = |x: f64, y: f64| {
        let (sx, sy) = geo.to_shown(x, y);
        (sx - px, py + ph - sy)
    };
    let origin = map(0.0, 0.0);
    let ux = map(1.0, 0.0);
    let uy = map(0.0, 1.0);
    [
        ux.0 - origin.0,
        ux.1 - origin.1,
        uy.0 - origin.0,
        uy.1 - origin.1,
        origin.0,
        origin.1,
    ]
}

fn numbers(document: &mut Document, value: Option<&Object>, count: usize) -> Option<Vec<f64>> {
    let array = document.resolve(value?)?.into_shared();
    let items = array.as_array().filter(|a| a.len() >= count)?;
    let mut out = Vec::new();
    for item in &items[..count] {
        out.push(document.resolve(item)?.as_number()?.value());
    }
    out.iter().all(|v| v.is_finite()).then_some(out)
}

/// The content that draws the page's annotations that print, and the appearance streams it names, which are added to
/// `resources` under new names.
fn annotation_content(
    document: &mut Document,
    page: &Page,
    copier: &mut Copier,
    out: &mut Collector,
    xobjects: &mut Dict,
) -> String {
    let page_dict = page.dict(document);
    let Some(annots) = document
        .lookup(&page_dict, "Annots")
        .map(|a| a.into_shared())
    else {
        return String::new();
    };
    let Some(items) = annots.as_array() else {
        return String::new();
    };
    let mut content = String::new();
    for (n, item) in items.iter().enumerate() {
        let Some(annot) = document.resolve(item).map(|a| a.into_shared()) else {
            continue;
        };
        let Some(annot) = annot.as_dict() else {
            continue;
        };
        if document.name_of(annot, "Subtype").as_deref() == Some(b"Popup") {
            continue;
        }
        // Print flag set, Hidden and NoView clear.
        let flags = document.integer_of(annot, "F").unwrap_or(0);
        if flags & 4 == 0 || flags & (2 | 32) != 0 {
            continue;
        }
        let Some(rect) = annot.get("Rect").and_then(|r| document.rect(r)) else {
            continue;
        };
        let Some(ap) = document.dict_of(annot, "AP") else {
            continue;
        };
        let Some(normal) = ap.get("N") else { continue };
        let mut appearance = normal.clone();
        if let Some(resolved) = document.resolve(normal).map(|v| v.into_shared())
            && let Object::Dict(states) = &*resolved
        {
            let state = document.name_of(annot, "AS");
            // Without /AS the first state stands.
            let chosen = match state {
                Some(s) => states.get_bytes(&s).cloned(),
                None => states.entries().first().map(|(_, v)| v.clone()),
            };
            appearance = match chosen {
                Some(found) => found,
                None => continue,
            };
        }
        let Object::Ref(Ref { number, .. }) = appearance else {
            continue;
        };
        let Some(stream) = document.get(number) else {
            continue;
        };
        let Object::Stream(stream) = &*stream else {
            continue;
        };
        let bbox = numbers(document, stream.dict.get("BBox"), 4);
        let matrix = numbers(document, stream.dict.get("Matrix"), 6)
            .unwrap_or_else(|| vec![1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);
        let Some(b) = bbox else { continue };
        // 12.5.5: the appearance's box, transformed by its matrix, is fitted to the annotation's rectangle.
        let corners = [(b[0], b[1]), (b[2], b[1]), (b[2], b[3]), (b[0], b[3])].map(|(x, y)| {
            (
                matrix[0] * x + matrix[2] * y + matrix[4],
                matrix[1] * x + matrix[3] * y + matrix[5],
            )
        });
        let min_x = corners.iter().map(|c| c.0).fold(f64::INFINITY, f64::min);
        let max_x = corners
            .iter()
            .map(|c| c.0)
            .fold(f64::NEG_INFINITY, f64::max);
        let min_y = corners.iter().map(|c| c.1).fold(f64::INFINITY, f64::min);
        let max_y = corners
            .iter()
            .map(|c| c.1)
            .fold(f64::NEG_INFINITY, f64::max);
        if max_x - min_x <= 0.0 || max_y - min_y <= 0.0 {
            continue;
        }
        let sx = rect.width() / (max_x - min_x);
        let sy = rect.height() / (max_y - min_y);
        let a = [
            sx,
            0.0,
            0.0,
            sy,
            rect.left - min_x * sx,
            rect.bottom - min_y * sy,
        ];
        let mut name = format!("LpAnnot{n}");
        while xobjects.contains_key(&name) {
            name.push('_');
        }
        let target = copier.target(number, out);
        xobjects.add(Name::new(&name), Object::reference(target as u32, 0));
        content.push_str(&format!("q {} cm /{name} Do Q\n", matrix_text(a)));
    }
    content
}

/// How much content decoding one source may cost in all: a share of the file's own size, so one stream named by
/// thousands of pages does not cost its full expansion each time.
fn decoded_limit(document: &Document) -> usize {
    (16 << 20) + 8 * document.length()
}

/// Page `index` of `document` as a form in `out`. `part` is the area shown, `(x, y, width, height)` in points on the page
/// as it is shown (y down, after its /Rotate); None is all of it. The limits are for this page alone. A page that fails
/// leaves `out` and `copier` as they were.
pub fn import_page(
    document: &mut Document,
    copier: &mut Copier,
    index: usize,
    part: Option<(f64, f64, f64, f64)>,
    out: &mut Collector,
) -> Result<Form, ImportError> {
    copier.journal.clear();
    copier.start = out.count();
    copier.bytes = 0;
    let made = import_inner(document, copier, index, part, out);
    if made.is_err() {
        for number in copier.journal.drain(..) {
            copier.map.remove(&number);
        }
        copier.queue.clear();
        out.truncate(copier.start);
    }
    made
}

fn import_inner(
    document: &mut Document,
    copier: &mut Copier,
    index: usize,
    part: Option<(f64, f64, f64, f64)>,
    out: &mut Collector,
) -> Result<Form, ImportError> {
    let pages = document.pages();
    let page = pages.get(index).ok_or(ImportError::NoPage)?;
    let geo = geometry(document, page);
    let part = part.unwrap_or((0.0, 0.0, geo.shown_width, geo.shown_height));
    if part.2 <= 0.0 || part.3 <= 0.0 {
        return Err(ImportError::NoPage);
    }

    let mut resources = page.resources(document).unwrap_or_default();
    // /XObject may be an indirect dictionary: read it into the form's own, so the appearances can be added to it.
    let source_xobjects = document.dict_of(&resources, "XObject").unwrap_or_default();
    let mut xobjects = copier.rewrite_dict(&source_xobjects, out, None);
    let annotations = annotation_content(document, page, copier, out, &mut xobjects);
    resources.remove("XObject");
    let mut resources = copier.rewrite_dict(&resources, out, None);
    if !xobjects.is_empty() {
        resources.insert("XObject", Object::Dict(xobjects));
    }

    // One content stream and nothing to add: the form takes the stream as it is stored, with its filters.
    let page_dict = page.dict(document);
    let single = match page_dict.get("Contents") {
        Some(Object::Ref(r)) if annotations.is_empty() => match document.get(r.number).as_deref() {
            Some(Object::Stream(stream)) => Some(stream.clone()),
            _ => None,
        },
        _ => None,
    };
    let (data, stored_entries) = match single {
        Some(stream) => {
            let data = document.raw_data(&stream);
            copier.bytes += data.len();
            if copier.bytes > MAX_BYTES {
                return Err(ImportError::TooLarge);
            }
            let mut kept = Dict::new();
            for key in ["Filter", "DecodeParms"] {
                if let Some(value) = stream.dict.get(key) {
                    kept.add(Name::new(key), copier.rewrite(value, out));
                }
            }
            let mut text = Vec::new();
            kept.write_to(&mut text);
            (data, text[2..text.len() - 2].to_vec())
        }
        None => {
            if copier.decoded >= decoded_limit(document) {
                return Err(ImportError::TooLarge);
            }
            let page_content = page.contents(document);
            copier.decoded += page_content.len();
            let mut content = b"q\n".to_vec();
            content.extend(page_content);
            content.extend(b"\nQ\n");
            content.extend(annotations.as_bytes());
            (flate(&content, 6), b"/Filter /FlateDecode".to_vec())
        }
    };

    // The part, back in the page's own space, within its crop box.
    let (x1, y1) = geo.from_shown(part.0, part.1);
    let (x2, y2) = geo.from_shown(part.0 + part.2, part.1 + part.3);
    let bbox = PageRect::new(x1, y1, x2, y2).intersect(&geo.crop);
    let mut resource_text = Vec::new();
    Object::Dict(resources).write_to(&mut resource_text);
    let mut entries = format!(
        "/Type /XObject /Subtype /Form /FormType 1 /BBox [{} {} {} {}] /Matrix [{}] /Resources ",
        num(bbox.left),
        num(bbox.bottom),
        num(bbox.right),
        num(bbox.top),
        matrix_text(form_matrix(&geo, part))
    )
    .into_bytes();
    entries.extend(resource_text);
    entries.push(b' ');
    entries.extend(stored_entries);
    let object = out.alloc();
    out.set_stream(object, entries, data);
    copier.drain(document, out)?;
    Ok(Form {
        object,
        width: part.2,
        height: part.3,
    })
}
