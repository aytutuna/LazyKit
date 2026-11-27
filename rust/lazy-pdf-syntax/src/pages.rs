//! The page tree (ISO 32000-1 7.7.3), walked from the catalog without trusting it: /Count, /Type and /Parent are not
//! needed, a loop or junk in /Kids costs only the entries it spoils.

use std::collections::HashSet;
use std::sync::Arc;

use crate::document::Document;
use crate::object::{Dict, Object, Ref};

const MAX_TREE_DEPTH: usize = 256;

/// Nodes one walk looks at, direct dictionaries included: a /Kids array shared by its own children has no end.
const MAX_TREE_NODES: usize = 1 << 20;

/// Entries one walk may copy into the values pages inherit; a direct /Resources on each of a million shared nodes
/// would otherwise be copied a million times.
const MAX_TREE_COPIED: usize = 32 << 20;

/// Content streams of one page that may fail to read; each failure can have cost a full decode.
const MAX_CONTENT_FAILURES: usize = 16;

/// No page has more content than this; the limit is shared by all of its streams.
const MAX_CONTENT_SIZE: usize = 256 << 20;

const PAGE_EVIDENCE: [&str; 4] = ["Contents", "MediaBox", "Resources", "Parent"];

/// A rectangle in PDF user space (origin bottom-left, y up), never upside down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
    pub top: f64,
}

impl Rect {
    pub fn new(x1: f64, y1: f64, x2: f64, y2: f64) -> Rect {
        Rect {
            left: x1.min(x2),
            bottom: y1.min(y2),
            right: x1.max(x2),
            top: y1.max(y2),
        }
    }

    pub fn width(&self) -> f64 {
        self.right - self.left
    }

    pub fn height(&self) -> f64 {
        self.top - self.bottom
    }

    pub fn is_empty(&self) -> bool {
        self.width() <= 0.0 || self.height() <= 0.0
    }

    /// The part both rectangles cover; empty (and at the corner of this one) when they miss.
    pub fn intersect(&self, other: &Rect) -> Rect {
        let left = self.left.max(other.left);
        let bottom = self.bottom.max(other.bottom);
        let right = self.right.min(other.right);
        let top = self.top.min(other.top);
        if right > left && top > bottom {
            Rect::new(left, bottom, right, top)
        } else {
            Rect::new(self.left, self.bottom, self.left, self.bottom)
        }
    }
}

/// US Letter, what a viewer shows for a page without a usable /MediaBox.
const DEFAULT_BOX: Rect = Rect {
    left: 0.0,
    bottom: 0.0,
    right: 612.0,
    top: 792.0,
};

/// What a page takes from the nodes above it when it has no value of its own.
#[derive(Debug, Clone, Default)]
pub(crate) struct Inherited {
    resources: Option<Arc<Object>>,
    media_box: Option<Arc<Object>>,
    crop_box: Option<Arc<Object>>,
    rotate: Option<Arc<Object>>,
}

/// One page of the tree.
#[derive(Debug, Clone)]
pub struct Page {
    pub reference: Ref,
    /// Position in reading order, from 0.
    pub index: usize,
    inherited: Inherited,
}

struct Walk {
    pages: Vec<Page>,
    visited: HashSet<u32>,
    skipped: usize,
    nodes: usize,
    copied: usize,
}

/// How much a copy of the value costs: one for each object in it.
fn weight(object: &Object) -> usize {
    let dict_weight = |dict: &Dict| {
        1 + dict
            .entries()
            .iter()
            .map(|(_, v)| 1 + weight(v))
            .sum::<usize>()
    };
    match object {
        Object::Array(items) => 1 + items.iter().map(weight).sum::<usize>(),
        Object::Dict(dict) => dict_weight(dict),
        Object::Stream(stream) => dict_weight(&stream.dict),
        _ => 1,
    }
}

impl Page {
    /// The page's dictionary as it is now: a change made to the page object shows at once. Empty when the object is
    /// gone or no longer a dictionary.
    pub fn dict(&self, document: &mut Document) -> Dict {
        match self.object(document).as_dict() {
            Some(dict) => dict.clone(),
            None => Dict::new(),
        }
    }

    fn object(&self, document: &mut Document) -> Arc<Object> {
        match document.resolve(&Object::Ref(self.reference)) {
            Some(object) if matches!(*object, Object::Dict(_)) => object.into_shared(),
            _ => Arc::new(Object::Dict(Dict::new())),
        }
    }

    /// The page's /Resources, or those of the nearest node above that has any.
    pub fn resources(&self, document: &mut Document) -> Option<Dict> {
        let object = self.object(document);
        if let Some(own) = document.dict_of(object.as_dict()?, "Resources") {
            return Some(own);
        }
        let inherited = document.resolve(self.inherited.resources.as_deref()?)?;
        match &*inherited {
            Object::Dict(d) => Some(d.clone()),
            _ => None,
        }
    }

    /// The paper: /MediaBox, or US Letter when it is missing or has no area.
    pub fn media_box(&self, document: &mut Document) -> Rect {
        self.boxed(document, "MediaBox", self.inherited.media_box.as_deref())
            .unwrap_or(DEFAULT_BOX)
    }

    /// What is shown: /CropBox clipped to the media box; the media box without one.
    pub fn crop_box(&self, document: &mut Document) -> Rect {
        let media = self.media_box(document);
        let Some(crop) = self.boxed(document, "CropBox", self.inherited.crop_box.as_deref()) else {
            return media;
        };
        let clipped = crop.intersect(&media);
        if clipped.is_empty() { media } else { clipped }
    }

    /// The finished page size; the crop box when the page has none.
    pub fn trim_box(&self, document: &mut Document) -> Rect {
        self.own_box(document, "TrimBox")
    }

    /// The extent that production clips to; the crop box when the page has none.
    pub fn bleed_box(&self, document: &mut Document) -> Rect {
        self.own_box(document, "BleedBox")
    }

    /// The meaningful content; the crop box when the page has none.
    pub fn art_box(&self, document: &mut Document) -> Rect {
        self.own_box(document, "ArtBox")
    }

    fn own_box(&self, document: &mut Document, key: &str) -> Rect {
        match self.boxed(document, key, None) {
            Some(found) => found,
            None => self.crop_box(document),
        }
    }

    /// Clockwise degrees the page is turned when shown: 0, 90, 180 or 270 (anything else rounds to the nearest).
    pub fn rotate(&self, document: &mut Document) -> i32 {
        let object = self.object(document);
        let own = object
            .as_dict()
            .and_then(|d| d.get("Rotate"))
            .and_then(|v| document.resolve(v));
        let value = match own {
            Some(own) => Some(own.into_shared()),
            None => self
                .inherited
                .rotate
                .as_deref()
                .and_then(|v| document.resolve(v))
                .map(|v| v.into_shared()),
        };
        let Some(Object::Number(number)) = value.as_deref() else {
            return 0;
        };
        let quarters = (number.value().clamp(-1e6, 1e6) / 90.0).round_ties_even() as i32;
        quarters.rem_euclid(4) * 90
    }

    /// User space units per point (ISO 32000-1 14.11.2.3); 1 unless /UserUnit is a positive number.
    pub fn user_unit(&self, document: &mut Document) -> f64 {
        let object = self.object(document);
        match object
            .as_dict()
            .and_then(|d| document.number_of(d, "UserUnit"))
        {
            Some(unit) if unit > 0.0 && unit.is_finite() => unit,
            _ => 1.0,
        }
    }

    /// The page's content streams, decoded and joined with a newline so that no token runs into the next stream
    /// (7.8.2). A stream that cannot be read is left out and noted in the document's repairs.
    pub fn contents(&self, document: &mut Document) -> Vec<u8> {
        let mut parts: Vec<Vec<u8>> = Vec::new();
        let mut room = MAX_CONTENT_SIZE;
        let mut input_room = MAX_CONTENT_SIZE;
        let mut failures = 0;
        let object = self.object(document);
        let contents = object
            .as_dict()
            .and_then(|d| d.get("Contents"))
            .and_then(|c| document.resolve(c))
            .map(|c| c.into_shared());
        let streams: Vec<Object> = match contents.as_deref() {
            Some(Object::Array(items)) => items.clone(),
            Some(other) => vec![other.clone()],
            None => Vec::new(),
        };
        for item in &streams {
            let Some(found) = document.resolve(item).map(|v| v.into_shared()) else {
                continue;
            };
            let Object::Stream(stream) = &*found else {
                continue;
            };
            // What a stream stores is read each time it is named, whatever it decodes to.
            let stored = document.raw_length(stream).max(1);
            if stored > input_room {
                break;
            }
            input_room -= stored;
            match document.stream_data_limited(stream, room, false) {
                Ok(part) => {
                    room = room.saturating_sub(part.len() + 1);
                    parts.push(part);
                }
                Err(e) => {
                    document.add_repair(format!(
                        "Page {}: a content stream could not be read ({})",
                        self.index + 1,
                        e.detail
                    ));
                    failures += 1;
                    if failures >= MAX_CONTENT_FAILURES {
                        break;
                    }
                }
            }
        }
        parts.join(&b'\n')
    }

    fn boxed(
        &self,
        document: &mut Document,
        key: &str,
        inherited: Option<&Object>,
    ) -> Option<Rect> {
        let object = self.object(document);
        let own = object
            .as_dict()
            .and_then(|d| d.get(key))
            .and_then(|v| document.rect(v));
        own.or_else(|| inherited.and_then(|v| document.rect(v)))
    }
}

impl Document {
    /// A rectangle from an array of four numbers (corners in any order); none for anything else or one with no area.
    pub fn rect(&mut self, value: &Object) -> Option<Rect> {
        let array = self.resolve(value)?.into_shared();
        let items = array.as_array().filter(|a| a.len() >= 4)?;
        let mut n = [0.0; 4];
        for (i, slot) in n.iter_mut().enumerate() {
            let v = self.resolve(&items[i])?.as_number()?.value();
            if !v.is_finite() {
                return None;
            }
            *slot = v;
        }
        let rect = Rect::new(n[0], n[1], n[2], n[3]);
        (!rect.is_empty()).then_some(rect)
    }

    /// A dictionary value of `dict`, resolved; a stream does not count.
    pub fn dict_of(&mut self, dict: &Dict, key: &str) -> Option<Dict> {
        match &*self.lookup(dict, key)? {
            Object::Dict(d) => Some(d.clone()),
            _ => None,
        }
    }

    /// The catalog's dictionary; empty when /Root names none.
    pub(crate) fn catalog_dict(&mut self) -> Dict {
        match self.catalog().as_deref() {
            Some(Object::Dict(d)) => d.clone(),
            _ => Dict::new(),
        }
    }

    /// The pages in reading order. Built on first use and kept; call [`Document::refresh_pages`] after changing the
    /// tree.
    pub fn pages(&mut self) -> Arc<Vec<Page>> {
        if let Some(pages) = &self.pages {
            return pages.clone();
        }
        let pages = Arc::new(self.walk_pages());
        self.pages = Some(pages.clone());
        pages
    }

    /// Forgets the page list so that the next use walks the tree again.
    pub fn refresh_pages(&mut self) {
        self.pages = None;
    }

    fn walk_pages(&mut self) -> Vec<Page> {
        let first = !self.pages_walked;
        self.pages_walked = true;
        let catalog = self.catalog_dict();
        let mut walk = Walk {
            pages: Vec::new(),
            visited: HashSet::new(),
            skipped: 0,
            nodes: 0,
            copied: 0,
        };
        if let Some(root) = catalog.get("Pages") {
            self.walk_node(root, &Inherited::default(), 0, &mut walk);
        }
        if first {
            if walk.skipped > 0 {
                self.add_repair(format!(
                    "The page tree has {} entries that are not pages or that loop back; they were left out.",
                    walk.skipped
                ));
            }
            if let Some(tree) = self.dict_of(&catalog, "Pages")
                && let Some(declared) = self.integer_of(&tree, "Count")
                && declared != walk.pages.len() as i64
                && !walk.pages.is_empty()
            {
                self.add_repair(format!(
                    "The page tree says {declared} pages and has {}.",
                    walk.pages.len()
                ));
            }
            if walk.pages.is_empty() {
                self.find_pages_by_scan(&mut walk.pages);
            }
        }
        walk.pages
    }

    fn walk_node(&mut self, node: &Object, inherited: &Inherited, depth: usize, walk: &mut Walk) {
        let reference = match node {
            Object::Ref(r) => Some(*r),
            _ => None,
        };
        walk.nodes += 1;
        let resolved = if depth > MAX_TREE_DEPTH
            || walk.nodes > MAX_TREE_NODES
            || reference.is_some_and(|r| !walk.visited.insert(r.number))
        {
            None
        } else {
            self.resolve(node)
        };
        let Some(Object::Dict(dict)) = resolved.as_deref() else {
            walk.skipped += 1;
            return;
        };
        walk.copied += ["Resources", "MediaBox", "CropBox", "Rotate"]
            .iter()
            .filter_map(|key| dict.get(key))
            .map(weight)
            .sum::<usize>();
        if walk.copied > MAX_TREE_COPIED {
            walk.skipped += 1;
            return;
        }
        let kind = self.name_of(dict, "Type");
        let own = |key: &str| {
            dict.get(key)
                .filter(|v| !matches!(v, Object::Null))
                .map(|v| Arc::new(v.clone()))
        };
        let inherited = Inherited {
            resources: own("Resources").or_else(|| inherited.resources.clone()),
            media_box: own("MediaBox").or_else(|| inherited.media_box.clone()),
            crop_box: own("CropBox").or_else(|| inherited.crop_box.clone()),
            rotate: own("Rotate").or_else(|| inherited.rotate.clone()),
        };
        if kind.as_deref() != Some(b"Page")
            && let Some(kids) = self.lookup(dict, "Kids")
            && let Some(kids) = kids.as_array()
        {
            for kid in kids {
                self.walk_node(kid, &inherited, depth + 1, walk);
            }
            return;
        }
        if kind.as_deref() == Some(b"Pages") {
            return;
        }
        // A page without a type is told by what pages have; a direct one cannot be referred to.
        let looks_like_page =
            kind.as_deref() == Some(b"Page") || PAGE_EVIDENCE.iter().any(|k| dict.contains_key(k));
        let typed_right = matches!(kind.as_deref(), None | Some(b"Page"));
        let Some(reference) = reference.filter(|_| looks_like_page && typed_right) else {
            walk.skipped += 1;
            return;
        };
        walk.pages.push(Page {
            reference,
            index: walk.pages.len(),
            inherited,
        });
    }

    /// A tree with no pages in it, in a file that has page objects: they are used in the order they are in the file.
    fn find_pages_by_scan(&mut self, pages: &mut Vec<Page>) {
        let scan = self.ensure_scan();
        for &number in &scan.pages {
            let reference = self.reference(number);
            let Some(object) = self.get(number) else {
                continue;
            };
            let Object::Dict(dict) = &*object else {
                continue;
            };
            if self.name_of(dict, "Type").as_deref() == Some(b"Page") {
                pages.push(Page {
                    reference,
                    index: pages.len(),
                    inherited: Inherited::default(),
                });
            }
        }
        if !pages.is_empty() {
            self.add_repair(format!(
                "The page tree holds no pages; used the {} page objects found by scanning.",
                pages.len()
            ));
        }
    }
}
