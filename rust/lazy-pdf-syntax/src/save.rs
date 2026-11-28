//! Saving, the two ways out: an incremental update, and a rewrite of what the trailer can reach.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::document::{Document, EntryKind, MAX_OBJECT_NUMBER};
use crate::error::ReadError;
use crate::object::{Dict, Name, Object};
use crate::writer::{ObjectWriter, XrefEntry, digest16, id_pair, write_stream, write_table};
use crate::xref::MAX_SECTIONS;

/// The size qpdf and most writers pack to; an object stream is parsed whole to read one object.
const OBJECTS_PER_STREAM: usize = 100;

/// How many times its stored size a stream may decode to for a rewrite to try deflating it again.
const MAX_REPACK_RATIO: usize = 32;

/// Filters that can make data many times longer; a chain with two of them is a bomb or a mistake.
const EXPANDING: [&[u8]; 6] = [
    b"FlateDecode",
    b"Fl",
    b"LZWDecode",
    b"LZW",
    b"RunLengthDecode",
    b"RL",
];

const REPACKABLE: [&[u8]; 10] = [
    b"FlateDecode",
    b"Fl",
    b"LZWDecode",
    b"LZW",
    b"ASCII85Decode",
    b"A85",
    b"ASCIIHexDecode",
    b"AHx",
    b"RunLengthDecode",
    b"RL",
];

/// How a rewrite writes. `use_object_streams` packs the plain objects into object streams and lists them in a
/// cross-reference stream (PDF 1.5). `document_id` is the first string of the new /ID; the file's own is kept when
/// there is none.
#[derive(Debug, Clone, Default)]
pub struct SaveOptions {
    pub use_object_streams: bool,
    pub document_id: Option<Vec<u8>>,
}

/// The result of a rewrite.
#[derive(Debug, Clone)]
pub struct Rewritten {
    pub bytes: Vec<u8>,
    /// How many objects the rewrite reached that the file's table lists but that could not be read, and so are left
    /// out. A free or unlisted object is not counted.
    pub dropped: usize,
}

impl Document {
    /// The bytes read, then an update (ISO 32000-1 7.5.6) holding only what changed since: the objects that
    /// differ, the new ones, free entries for deleted ones and a new cross-reference section. The bytes read are
    /// untouched, which is what signatures rest on; anything before the %PDF header was cut off on reading and is
    /// not in them. With no change the result is the bytes read.
    pub fn save_incremental(&mut self) -> Result<Vec<u8>, ReadError> {
        let update = self.build_update()?;
        let mut file = Vec::with_capacity(self.data.len() + update.len());
        file.extend_from_slice(&self.data);
        file.extend_from_slice(&update);
        Ok(file)
    }

    /// A new file of the objects reachable from the trailer, renumbered. Signatures do not survive it.
    pub fn save_rewrite(&mut self, options: &SaveOptions) -> Rewritten {
        let mut rewriter = Rewriter {
            document: self,
            options,
            numbers: HashMap::new(),
            items: Vec::new(),
            followed: 0,
            dropped: HashSet::new(),
            read: 0,
        };
        let bytes = rewriter.run();
        Rewritten {
            bytes,
            dropped: rewriter.dropped.len(),
        }
    }

    fn build_update(&mut self) -> Result<Vec<u8>, ReadError> {
        let changed = self.changed_objects();
        let freed = self.deleted_objects();
        if changed.is_empty() && freed.is_empty() && !self.trailer_changed() {
            return Ok(Vec::new());
        }

        let source_length = self.data.len();
        let mut writer = ObjectWriter::new(source_length as u64);
        // The update starts on a line of its own.
        if source_length > 0 && !matches!(self.data[source_length - 1], 10 | 13) {
            writer.out.push(10);
        }

        let mut entries = Vec::new();
        for &number in &changed {
            // Objects that came from an object stream are written as plain objects.
            let value = self.get(number).expect("a changed object is loaded");
            let generation = self.generation_of(number);
            let at = match &*value {
                Object::Stream(stream) => {
                    let data = self.raw_data(stream);
                    writer.write_stream(number, generation, &stream.dict, &data)
                }
                other => writer.write_object(number, generation, other),
            };
            entries.push(XrefEntry::new(number, 1, at, generation as u32));
        }
        for (i, &number) in freed.iter().enumerate() {
            let next = freed.get(i + 1).copied().unwrap_or(0);
            entries.push(XrefEntry::new(
                number,
                0,
                next as u64,
                self.generation_of(number) as u32,
            ));
        }
        if let Some(&head) = freed.first() {
            entries.push(XrefEntry::new(0, 0, head as u64, 65535));
        }

        // After a rebuild the file's own sections cannot be built on, and past the reader's chain limit it would not
        // read back, so this one lists everything.
        let full = self.xref_rebuilt || self.sections.len() >= MAX_SECTIONS;
        if full {
            let written: HashSet<u32> = entries.iter().map(|e| e.number).collect();
            let listed: Vec<(u32, crate::document::Entry)> =
                self.entries.iter().map(|(n, e)| (*n, *e)).collect();
            for (number, entry) in listed {
                if written.contains(&number) {
                    continue;
                }
                match entry.kind {
                    EntryKind::InUse => {
                        entries.push(XrefEntry::new(
                            number,
                            1,
                            entry.offset,
                            entry.generation as u32,
                        ));
                    }
                    EntryKind::Compressed => {
                        let index = self.index_in_object_stream(number);
                        entries.push(XrefEntry::new(number, 2, entry.stream_number as u64, index));
                    }
                    _ => {}
                }
            }
            if freed.is_empty() {
                entries.push(XrefEntry::new(0, 0, 0, 65535));
            }
        }
        // Only the trailer changed.
        if entries.is_empty() {
            entries.push(XrefEntry::new(0, 0, 0, 65535));
        }
        entries.sort_by_key(|e| e.number);

        let as_stream = self.xref_is_stream || entries.iter().any(|e| e.kind == 2);
        let size = self
            .next_object_number
            .max(entries.last().unwrap().number + 1);
        // The cross-reference stream is an object too, and PDF has no number past the limit for it.
        if as_stream && size > MAX_OBJECT_NUMBER {
            return Err(ReadError::damaged("The file has no object numbers left."));
        }
        let mut trailer = Dict::new();
        trailer.add(
            Name::new("Size"),
            Object::int(if as_stream { size + 1 } else { size } as i64),
        );
        for (key, value) in self.trailer.entries() {
            if !matches!(key.as_bytes(), b"Size" | b"Prev" | b"XRefStm" | b"ID") {
                trailer.add(key.clone(), value.clone());
            }
        }
        if self.had_info && !self.trailer.contains_key("Info") {
            trailer.add(Name::new("Info"), Object::Null);
        }
        let first = self.first_id();
        let id_context: [&[u8]; 2] = [&self.data, &writer.out];
        let first = first.unwrap_or_else(|| digest16(&[b"first", id_context[0], id_context[1]]));
        let second = digest16(&[b"second", &first, id_context[0], id_context[1]]);
        trailer.add(Name::new("ID"), id_pair(first, second));
        if !full {
            trailer.add(Name::new("Prev"), Object::int(self.start_xref as i64));
        }

        if as_stream {
            write_stream(&mut writer, size, entries, &trailer, None);
        } else {
            write_table(&mut writer, &entries, &trailer);
        }
        Ok(writer.out)
    }
}

/// A rewrite keeps what the trailer can reach, numbers it afresh and lays the file out again, with no history in it.
/// Reachability is a walk from /Root and /Info; the numbers come out in the order the walk meets the objects, which
/// puts the catalog first.
struct Rewriter<'a> {
    document: &'a mut Document,
    options: &'a SaveOptions,
    numbers: HashMap<u32, u32>,
    items: Vec<Arc<Object>>,
    followed: usize,
    dropped: HashSet<u32>,
    /// What the streams read from the file store, together.
    read: usize,
}

impl Rewriter<'_> {
    fn run(&mut self) -> Vec<u8> {
        let root_entry = self.document.trailer.get("Root").cloned();
        let root = self.intern(root_entry.as_ref());
        let info_entry = self.document.trailer.get("Info").cloned();
        let info = if self.document.info().is_none() {
            0
        } else {
            self.intern(info_entry.as_ref())
        };
        self.follow();
        // Pages the reader shows without the tree reaching them (it found them by scanning) are kept too.
        for page in self.document.pages().iter() {
            self.intern(Some(&Object::Ref(page.reference)));
        }
        self.follow();

        let mut packed = Vec::new();
        if self.options.use_object_streams {
            // The catalog stays a plain object: some readers ask for it before they can read an object stream.
            for n in 1..=self.items.len() as u32 {
                let item = &self.items[n as usize - 1];
                let is_signature = matches!(&**item, Object::Dict(d) if is_signature(d));
                if n != root && !matches!(&**item, Object::Stream(_)) && !is_signature {
                    packed.push(n);
                }
            }
        }
        let packed_set: HashSet<u32> = packed.iter().copied().collect();
        let mut next = self.items.len() as u32 + 1;
        let first_stream = next;
        next += packed.len().div_ceil(OBJECTS_PER_STREAM) as u32;
        let xref_number = if self.options.use_object_streams {
            next += 1;
            next - 1
        } else {
            0
        };

        let mut writer = ObjectWriter::new(0);
        let version = self.header_version();
        writer.write_header(&version);
        let mut entries = vec![XrefEntry::new(0, 0, 0, 65535)];

        for n in 1..=self.items.len() as u32 {
            if packed_set.contains(&n) {
                continue;
            }
            let item = self.items[n as usize - 1].clone();
            let at = match &*item {
                Object::Stream(stream) => {
                    let mut dict = self.remap_dict(&stream.dict, true);
                    let stored = self.stored(stream, &mut dict);
                    writer.write_stream(n, 0, &dict, &stored)
                }
                other => {
                    let mapped = self.remap(other);
                    writer.write_object(n, 0, &mapped)
                }
            };
            entries.push(XrefEntry::new(n, 1, at, 0));
        }
        for (s, members) in packed.chunks(OBJECTS_PER_STREAM).enumerate() {
            let number = first_stream + s as u32;
            let (dict, data) = self.pack(members);
            let at = writer.write_stream(number, 0, &dict, &data);
            entries.push(XrefEntry::new(number, 1, at, 0));
            for (i, member) in members.iter().enumerate() {
                entries.push(XrefEntry::new(*member, 2, number as u64, i as u32));
            }
        }

        let mut trailer = Dict::new();
        trailer.add(Name::new("Size"), Object::int(next as i64));
        trailer.add(Name::new("Root"), Object::reference(root, 0));
        if info > 0 {
            trailer.add(Name::new("Info"), Object::reference(info, 0));
        }
        let first = self.first_id(&writer.out);
        let second = digest16(&[b"second", &first, &writer.out]);
        trailer.add(Name::new("ID"), id_pair(first, second));
        entries.sort_by_key(|e| e.number);
        if xref_number > 0 {
            write_stream(
                &mut writer,
                xref_number,
                entries,
                &trailer,
                Some(&*self.document.filters),
            );
        } else {
            write_table(&mut writer, &entries, &trailer);
        }
        writer.out
    }

    fn follow(&mut self) {
        while self.followed < self.items.len() {
            let item = self.items[self.followed].clone();
            self.followed += 1;
            match &*item {
                Object::Stream(stream) => self.visit(&Object::Dict(stream.dict.clone()), true),
                other => self.visit(other, false),
            }
        }
    }

    /// The number the object `entry` names or is gets in the new file; 0 for nothing there.
    fn intern(&mut self, entry: Option<&Object>) -> u32 {
        let Some(entry) = entry else { return 0 };
        if let Object::Ref(reference) = entry {
            if let Some(known) = self.numbers.get(&reference.number) {
                return *known;
            }
            let value = self.document.get(reference.number);
            match value {
                Some(value) if !matches!(*value, Object::Null) => {
                    self.items.push(value);
                    let number = self.items.len() as u32;
                    self.numbers.insert(reference.number, number);
                    return number;
                }
                _ => {
                    if let Some(listed) = self.document.entry(reference.number)
                        && matches!(listed.kind, EntryKind::InUse | EntryKind::Compressed)
                    {
                        self.dropped.insert(reference.number);
                    }
                    return 0;
                }
            }
        }
        let Some(direct) = self.document.resolve(entry).map(|v| v.into_shared()) else {
            return 0;
        };
        self.items.push(direct);
        self.items.len() as u32
    }

    /// Numbers the objects a value refers to. A stream's /Length is not followed: it is written afresh.
    fn visit(&mut self, value: &Object, stream: bool) {
        match value {
            Object::Ref(_) => {
                self.intern(Some(value));
            }
            Object::Array(items) => {
                for item in items {
                    self.visit(item, false);
                }
            }
            Object::Dict(dict) => {
                for (key, item) in dict.entries() {
                    if !(stream && key.is("Length")) {
                        self.visit(item, false);
                    }
                }
            }
            _ => {}
        }
    }

    /// The value with its references renumbered; one to an object that is not there is dropped from a dictionary and
    /// null in an array.
    fn remap(&self, value: &Object) -> Object {
        match value {
            Object::Ref(reference) => match self.numbers.get(&reference.number) {
                Some(n) => Object::reference(*n, 0),
                None => Object::Null,
            },
            Object::Array(items) => Object::Array(items.iter().map(|i| self.remap(i)).collect()),
            Object::Dict(dict) => Object::Dict(self.remap_dict(dict, false)),
            other => other.clone(),
        }
    }

    fn remap_dict(&self, source: &Dict, stream: bool) -> Dict {
        let mut copy = Dict::new();
        for (key, item) in source.live_entries() {
            if stream && key.is("Length") {
                continue;
            }
            let mapped = self.remap(item);
            if mapped != Object::Null {
                copy.add(key.clone(), mapped);
            }
        }
        copy
    }

    fn pack(&mut self, members: &[u32]) -> (Dict, Vec<u8>) {
        let mut header = String::new();
        let mut body = Vec::new();
        for &number in members {
            header.push_str(&format!("{number} {} ", body.len()));
            self.remap(&self.items[number as usize - 1].clone())
                .write_to(&mut body);
            body.push(b'\n');
        }
        let mut data = header.clone().into_bytes();
        let first = data.len();
        data.extend_from_slice(&body);
        let mut dict = Dict::new();
        dict.add(Name::new("Type"), Object::name("ObjStm"));
        dict.add(Name::new("N"), Object::int(members.len() as i64));
        dict.add(Name::new("First"), Object::int(first as i64));
        dict.add(Name::new("Filter"), Object::name("FlateDecode"));
        (dict, self.document.filters.flate(&data))
    }

    /// The data to store for a stream. What is not compressed, or only by the old text filters, is deflated when that
    /// is smaller. A stream with a predictor is left (deflating without it would lose more than it gains), as are
    /// Flate streams already, metadata, and image data, which is never touched.
    fn stored(&mut self, stream: &crate::object::Stream, dict: &mut Dict) -> Vec<u8> {
        // Streams that end where the next begins share bytes (a missing /Length runs to the next endstream), so a
        // small file could make a rewrite quadratic. Streams that are apart never store more than the file holds.
        if stream.data.is_none() {
            self.read = self.read.saturating_add(self.document.raw_length(stream));
            if self.read > self.document.data.len() {
                self.document.add_repair(
                    "A stream was left empty in the rewrite: the streams store more than the file holds."
                        .to_string(),
                );
                dict.remove("Filter");
                dict.remove("DecodeParms");
                return Vec::new();
            }
        }
        let raw = self.document.raw_data(stream);
        if !self.can_repack(&stream.dict) {
            return raw;
        }
        // What a few bytes inflate to is not worth the work: past this the stream is kept as it is.
        let cap = raw
            .len()
            .saturating_mul(MAX_REPACK_RATIO)
            .saturating_add(4096);
        let Ok(plain) = self.document.stream_data_limited(stream, cap, true) else {
            return raw;
        };
        let deflated = self.document.filters.flate(&plain);
        if deflated.len() >= raw.len() {
            return raw;
        }
        dict.insert("Filter", Object::name("FlateDecode"));
        dict.remove("DecodeParms");
        deflated
    }

    fn can_repack(&mut self, dict: &Dict) -> bool {
        let kind = self.document.name_of(dict, "Type");
        if matches!(kind.as_deref(), Some(b"Metadata" | b"XRef" | b"ObjStm")) {
            return false;
        }
        let filter = dict
            .get("Filter")
            .and_then(|f| self.document.resolve(f))
            .map(|f| f.into_shared());
        let filters: Vec<Vec<u8>> = match filter.as_deref() {
            None => Vec::new(),
            Some(Object::Name(name)) => vec![name.0.clone()],
            Some(Object::Array(items)) => {
                let mut names = Vec::new();
                for item in items {
                    let name = self
                        .document
                        .resolve(item)
                        .and_then(|v| v.as_name().map(|n| n.0.clone()));
                    names.push(name.unwrap_or_default());
                }
                names
            }
            Some(_) => vec![Vec::new()],
        };
        if filters.len() == 1 && (filters[0] == b"FlateDecode" || filters[0] == b"Fl") {
            return false;
        }
        if filters
            .iter()
            .filter(|f| EXPANDING.contains(&f.as_slice()))
            .count()
            > 1
        {
            return false;
        }
        if !filters.iter().all(|f| REPACKABLE.contains(&f.as_slice())) {
            return false;
        }
        let parameters = dict
            .get("DecodeParms")
            .and_then(|p| self.document.resolve(p))
            .map(|p| p.into_shared());
        let each: Vec<Object> = match parameters.as_deref() {
            Some(Object::Array(items)) => items.clone(),
            Some(other) => vec![other.clone()],
            None => Vec::new(),
        };
        for parms in &each {
            let resolved = self.document.resolve(parms).map(|p| p.into_shared());
            if let Some(Object::Dict(d)) = resolved.as_deref()
                && self
                    .document
                    .integer_of(d, "Predictor")
                    .is_some_and(|p| p > 1)
            {
                return false;
            }
        }
        true
    }

    /// The first /ID string: a given one, else the file's own, which names the document across its versions
    /// (ISO 32000-1 14.4.1), else one made from the content.
    fn first_id(&mut self, written: &[u8]) -> Vec<u8> {
        if let Some(id) = &self.options.document_id {
            return id.clone();
        }
        match self.document.first_id() {
            Some(kept) if !kept.is_empty() => kept,
            _ => digest16(&[b"first", written]),
        }
    }

    fn header_version(&mut self) -> String {
        let version = self.document.version();
        if self.options.use_object_streams {
            raised(&version, [1, 5])
        } else {
            version
        }
    }
}

fn is_signature(dict: &Dict) -> bool {
    matches!(dict.get("Type"), Some(Object::Name(n)) if n.is("Sig") || n.is("DocTimeStamp"))
        || dict.contains_key("ByteRange")
}

/// `version`, or `least` when that is newer.
fn raised(version: &str, least: [u32; 2]) -> String {
    let parts: Option<Vec<u32>> = version.split('.').map(|p| p.parse().ok()).collect();
    match parts {
        Some(p) if p.as_slice() >= least.as_slice() => version.to_owned(),
        _ => format!("{}.{}", least[0], least[1]),
    }
}
