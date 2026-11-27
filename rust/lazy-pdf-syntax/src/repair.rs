//! Repair: when the cross-reference data fails, the file is scanned once for "n g obj" and for trailers, and the table
//! built from what is there, the newest definition of an object winning. One pass over the bytes; a stream is skipped
//! by its length or its "endstream", so nothing is looked at twice.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::document::{Document, Entry, MAX_OBJECT_NUMBER};
use crate::error::ReadError;
use crate::lexer::{Lexer, is_regular, is_white_space};
use crate::object::{Dict, Object, Ref};

const MAX_ROOT_HOPS: usize = 32;

/// What a scan of the whole file found.
#[derive(Default)]
pub(crate) struct Scan {
    /// Where each object starts (the first digit of "n g obj") and its generation.
    pub objects: HashMap<u32, (usize, u16)>,
    pub trailers: Vec<Dict>,
    pub object_streams: Vec<u32>,
    pub catalogs: Vec<u32>,
    pub page_trees: Vec<u32>,
    pub pages: Vec<u32>,
    pub highest: u32,
    pub wasted: usize,
}

impl Document {
    pub(crate) fn ensure_scan(&mut self) -> Arc<Scan> {
        if self.scan.is_none() {
            self.scan = Some(Arc::new(self.run_scan()));
        }
        self.scan.clone().unwrap()
    }

    fn run_scan(&self) -> Scan {
        let mut scan = Scan::default();
        let mut pos = 0;
        // Where the next keyword is; usize::MAX when there is none, and none before the first look.
        let (mut next_object, mut next_trailer): (Option<usize>, Option<usize>) = (None, None);
        loop {
            if next_object.is_none_or(|n| n < pos) {
                next_object =
                    Some(crate::bytes::find(&self.data, b"obj", pos).unwrap_or(usize::MAX));
            }
            if next_trailer.is_none_or(|n| n < pos) {
                next_trailer =
                    Some(crate::bytes::find(&self.data, b"trailer", pos).unwrap_or(usize::MAX));
            }
            let (object, trailer) = (next_object.unwrap(), next_trailer.unwrap());
            if object == usize::MAX && trailer == usize::MAX {
                break;
            }
            if trailer < object {
                pos = trailer + "trailer".len();
                if let Some((value, end)) = self.try_read(&mut scan, pos, trailer) {
                    if let Object::Dict(dict) = value {
                        scan.trailers.push(dict);
                    }
                    pos = end;
                }
                continue;
            }
            pos = object + 3;
            let Some((start, number, generation)) = self.try_object_header(object) else {
                continue;
            };
            scan.objects.insert(number, (start, generation));
            scan.highest = scan.highest.max(number);
            let Some((value, value_end)) = self.try_read(&mut scan, pos, object) else {
                continue;
            };
            pos = pos.max(value_end);
            if let Object::Stream(stream) = &value {
                pos = pos.max(self.scan_stream_end(stream));
            }
            classify(&mut scan, number, &value);
        }
        scan
    }

    /// Reads an object at `position`. What a failed read cost is added up, and once it passes the file's size nothing
    /// more is parsed: a stray "(" that swallows the rest of the file would otherwise be paid for again at every
    /// later header.
    fn try_read(&self, scan: &mut Scan, position: usize, origin: usize) -> Option<(Object, usize)> {
        if scan.wasted > self.data.len() {
            return None;
        }
        let mut lexer = Lexer::new(&self.data, position);
        match lexer.read_object(true) {
            Ok(value) => Some((value, lexer.pos)),
            Err(_) => {
                scan.wasted += lexer.pos.saturating_sub(origin);
                None
            }
        }
    }

    /// "n g obj" ending at the "obj" at `at`; the digits before it are the number and generation.
    fn try_object_header(&self, at: usize) -> Option<(usize, u32, u16)> {
        if at + 3 < self.data.len() && is_regular(self.data[at + 3] as i32) {
            return None;
        }
        let mut p = at as isize - 1;
        if !self.step_back_over(&mut p, true, 8) {
            return None;
        }
        let generation_end = p + 1;
        if !self.step_back_over(&mut p, false, 6) || p + 1 == generation_end {
            return None;
        }
        let generation_start = p + 1;
        let gap_end = p + 1;
        if !self.step_back_over(&mut p, true, 8) || p + 1 == gap_end {
            return None;
        }
        let number_end = p + 1;
        if !self.step_back_over(&mut p, false, 10) || p + 1 == number_end {
            return None;
        }
        let start = (p + 1) as usize;
        let digits = |from: usize, to: isize| {
            self.data[from..to as usize]
                .iter()
                .fold(0u64, |n, d| n * 10 + (*d - b'0') as u64)
        };
        let n = digits(start, number_end);
        let g = digits(generation_start as usize, generation_end);
        if n < 1 || n > MAX_OBJECT_NUMBER as u64 || g > 65535 {
            return None;
        }
        Some((start, n as u32, g as u16))
    }

    /// Moves `p` back over white space or digits, at most `limit` of them; false when it ran into the limit.
    fn step_back_over(&self, p: &mut isize, white: bool, limit: usize) -> bool {
        let mut steps = 0;
        while *p >= 0 {
            let b = self.data[*p as usize];
            let wanted = if white {
                is_white_space(b as i32)
            } else {
                b.is_ascii_digit()
            };
            if !wanted {
                break;
            }
            steps += 1;
            if steps > limit {
                return false;
            }
            *p -= 1;
        }
        true
    }

    fn scan_stream_end(&self, stream: &crate::object::Stream) -> usize {
        let start = stream.data_offset.unwrap_or(0);
        let declared = match stream.dict.get("Length") {
            Some(Object::Number(n)) => n.integer(),
            _ => None,
        };
        if let Some(declared) = declared
            && declared >= 0
            && declared <= (self.data.len() - start) as i64
            && self.endstream_follows(start + declared as usize)
        {
            return start + declared as usize;
        }
        self.find_stream_end(start)
    }

    pub(crate) fn rebuild(&mut self, reason: &str) -> Result<(), ReadError> {
        self.add_repair(format!(
            "{reason} Rebuilt the cross-reference table by scanning the file."
        ));
        let scan = self.ensure_scan();
        self.xref_rebuilt = true;
        self.start_xref = 0;
        self.xref_is_stream = false;
        self.entries.clear();
        self.objects.clear();
        self.object_streams.clear();
        self.bad_object_streams.clear();
        // Where each object was found: an object stream's members count as written where the stream is.
        let mut written: HashMap<u32, u64> = HashMap::with_capacity(scan.objects.len());
        for (number, (offset, generation)) in &scan.objects {
            self.entries
                .insert(*number, Entry::in_use(*offset as u64, *generation));
            written.insert(*number, *offset as u64);
        }

        let mut trailer = Dict::new();
        let mut size: i64 = 0;
        for found in &scan.trailers {
            for key in ["Root", "Info", "ID", "Encrypt"] {
                if let Some(value) = found.get(key) {
                    trailer.insert(key, value.clone());
                }
            }
            if let Some(Object::Number(n)) = found.get("Size")
                && let Some(s) = n.integer()
            {
                size = size.max(s);
            }
        }
        if scan.trailers.is_empty() {
            self.add_repair("The file has no trailer.".into());
        }
        self.trailer = trailer;
        self.next_object_number =
            (scan.highest as i64 + 1).max(size.min(MAX_OBJECT_NUMBER as i64)) as u32;

        self.check_not_encrypted()?;
        // Members of object streams in the order met, for the catalog search.
        let mut stored: Vec<u32> = Vec::new();
        let mut met: HashSet<u32> = scan.objects.keys().copied().collect();
        for &stream_number in &scan.object_streams {
            let Some(members) = self
                .load_object_stream(stream_number)
                .map(|m| m.numbers.clone())
            else {
                continue;
            };
            let at = written[&stream_number];
            for number in members {
                if written.get(&number).is_some_and(|existing| *existing >= at) {
                    continue;
                }
                self.entries
                    .insert(number, Entry::compressed(stream_number));
                written.insert(number, at);
                if met.insert(number) {
                    stored.push(number);
                }
                self.next_object_number = self.next_object_number.max(number + 1);
            }
        }

        if !self.catalog_usable() {
            self.find_catalog(&scan, &stored)?;
        }
        let size = self.next_object_number as i64;
        self.trailer.insert("Size", Object::int(size));
        Ok(())
    }

    /// A catalog the trailer did not name, or failing that one made from the page tree that is there.
    fn find_catalog(&mut self, scan: &Scan, stored: &[u32]) -> Result<(), ReadError> {
        let mut candidates = scan.catalogs.clone();
        // A catalog in an object stream is not in the scan: look at the stored ones if no plain one will do.
        if !candidates.iter().any(|n| self.has_pages(*n)) {
            candidates.extend_from_slice(stored);
        }
        for &number in candidates.iter().rev() {
            let is_catalog = match self.get(number).as_deref() {
                Some(Object::Dict(dict)) => dict
                    .get("Type")
                    .is_some_and(|t| t.as_name().is_some_and(|n| n.is("Catalog"))),
                _ => false,
            };
            if is_catalog && self.has_pages(number) {
                let reference = self.reference(number);
                self.trailer.insert("Root", Object::Ref(reference));
                self.add_repair(format!("Found the catalog (object {number}) by scanning."));
                return Ok(());
            }
        }

        let mut tree: Option<Ref> = None;
        for &number in scan.page_trees.iter().rev() {
            if tree.is_some() {
                break;
            }
            // The root of the tree is the node that has no parent.
            let parent = match self.get(number).as_deref() {
                Some(Object::Dict(node)) => Some(node.get("Parent").cloned()),
                _ => None,
            };
            if let Some(parent) = parent
                && parent.is_none_or(|p| self.resolve(&p).is_none())
            {
                tree = Some(self.reference(number));
            }
        }
        if tree.is_none() && !scan.pages.is_empty() {
            let kids = scan
                .pages
                .iter()
                .map(|n| Object::Ref(self.reference(*n)))
                .collect();
            let mut root = Dict::new();
            root.add(crate::object::Name::new("Type"), Object::name("Pages"));
            root.add(crate::object::Name::new("Kids"), Object::Array(kids));
            root.add(
                crate::object::Name::new("Count"),
                Object::int(scan.pages.len() as i64),
            );
            tree = Some(self.add_repaired(Object::Dict(root))?);
            self.add_repair(format!(
                "Made a page tree for {} pages found by scanning.",
                scan.pages.len()
            ));
        }
        let Some(tree) = tree else {
            return Err(ReadError::damaged("The file has no catalog and no pages."));
        };

        // The catalog is where the /Root chain ends; it is amended only if that is a dictionary other than the tree.
        let mut root = self.trailer.get("Root").cloned();
        if let Some(Object::Ref(r)) = &root {
            let mut number = r.number;
            for _ in 0..MAX_ROOT_HOPS {
                match self.get(number).as_deref() {
                    Some(Object::Ref(next)) => number = next.number,
                    _ => break,
                }
            }
            let amend = match self.get(number).as_deref() {
                Some(Object::Dict(d)) => {
                    number != tree.number
                        || d.get("Type")
                            .is_some_and(|t| t.as_name().is_some_and(|n| n.is("Catalog")))
                }
                _ => false,
            };
            if amend && let Some(Object::Dict(existing)) = self.get_mut(number) {
                existing.insert("Pages", Object::Ref(tree));
                self.add_repair("The catalog had no page tree; set it to the one found.".into());
                return Ok(());
            }
            root = None;
        }
        if let Some(Object::Dict(mut direct)) = root {
            direct.insert("Pages", Object::Ref(tree));
            self.trailer.insert("Root", Object::Dict(direct));
            self.add_repair("The catalog had no page tree; set it to the one found.".into());
            return Ok(());
        }
        let mut made = Dict::new();
        made.add(crate::object::Name::new("Type"), Object::name("Catalog"));
        made.add(crate::object::Name::new("Pages"), Object::Ref(tree));
        let reference = self.add_repaired(Object::Dict(made))?;
        self.trailer.insert("Root", Object::Ref(reference));
        self.add_repair("The file has no catalog; made one.".into());
        Ok(())
    }

    fn add_repaired(&mut self, value: Object) -> Result<Ref, ReadError> {
        if self.next_object_number > MAX_OBJECT_NUMBER {
            return Err(ReadError::damaged(
                "The file has no object numbers left to repair it with.",
            ));
        }
        self.add(value)
    }

    fn has_pages(&mut self, number: u32) -> bool {
        let Some(object) = self.get(number) else {
            return false;
        };
        let Object::Dict(catalog) = &*object else {
            return false;
        };
        matches!(
            catalog
                .get("Pages")
                .and_then(|p| self.resolve(p))
                .as_deref(),
            Some(Object::Dict(_))
        )
    }

    pub(crate) fn catalog_usable(&mut self) -> bool {
        let Some(root) = self.trailer.get("Root").cloned() else {
            return false;
        };
        let Some(catalog) = self.resolve(&root).map(|v| v.into_shared()) else {
            return false;
        };
        let Object::Dict(dict) = &*catalog else {
            return false;
        };
        matches!(
            dict.get("Pages").and_then(|p| self.resolve(p)).as_deref(),
            Some(Object::Dict(_))
        )
    }
}

fn classify(scan: &mut Scan, number: u32, value: &Object) {
    let dict = match value {
        Object::Stream(s) => &s.dict,
        Object::Dict(d) => d,
        _ => return,
    };
    let Some(Object::Name(kind)) = dict.get("Type") else {
        return;
    };
    match kind.as_bytes() {
        b"ObjStm" => scan.object_streams.push(number),
        b"XRef" => scan.trailers.push(dict.clone()),
        b"Catalog" => scan.catalogs.push(number),
        b"Pages" => scan.page_trees.push(number),
        b"Page" => scan.pages.push(number),
        _ => {}
    }
}
