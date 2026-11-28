use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

use crate::error::{Problem, ReadError, latin1};
use crate::filters::Filters;
use crate::object::{Dict, Object, Ref};
use crate::pages::Page;
use crate::repair::Scan;

/// The highest object number a PDF may use (ISO 32000-1 Annex C).
pub const MAX_OBJECT_NUMBER: u32 = 8_388_607;

const MAX_REPAIRS: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Free,
    InUse,
    Compressed,
    /// Made since the file was read.
    New,
}

/// One cross-reference entry: in use at an offset, in an object stream, free, or made since the file was read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub kind: EntryKind,
    pub offset: u64,
    pub generation: u16,
    pub stream_number: u32,
}

impl Entry {
    pub(crate) fn free(generation: u16) -> Entry {
        Entry {
            kind: EntryKind::Free,
            offset: 0,
            generation,
            stream_number: 0,
        }
    }

    pub(crate) fn in_use(offset: u64, generation: u16) -> Entry {
        Entry {
            kind: EntryKind::InUse,
            offset,
            generation,
            stream_number: 0,
        }
    }

    pub(crate) fn compressed(stream_number: u32) -> Entry {
        Entry {
            kind: EntryKind::Compressed,
            offset: 0,
            generation: 0,
            stream_number,
        }
    }
}

pub(crate) struct Slot {
    pub value: Arc<Object>,
    pub generation: u16,
}

pub(crate) struct ObjectStream {
    pub data: Arc<Vec<u8>>,
    pub offsets: HashMap<u32, usize>,
    /// The distinct places members start, ascending.
    pub starts: Vec<usize>,
    pub numbers: Vec<u32>,
    pub number_index: Option<HashMap<u32, usize>>,
}

/// An object read, or one that lives in the document.
pub enum Value<'a> {
    Borrowed(&'a Object),
    Shared(Arc<Object>),
}

impl std::ops::Deref for Value<'_> {
    type Target = Object;

    fn deref(&self) -> &Object {
        match self {
            Value::Borrowed(o) => o,
            Value::Shared(o) => o,
        }
    }
}

impl Value<'_> {
    pub fn into_shared(self) -> Arc<Object> {
        match self {
            Value::Borrowed(o) => Arc::new(o.clone()),
            Value::Shared(o) => o,
        }
    }
}

/// An open PDF: the object table, the trailer and catalog, the pages and the facts about the file. Objects load on
/// first use and are cached. The reader repairs what it can ([`Document::repairs`]); what it cannot is a
/// [`ReadError`].
pub struct Document {
    pub(crate) data: Vec<u8>,
    pub(crate) filters: Box<dyn Filters>,
    pub(crate) repairs: Vec<String>,
    pub(crate) header_version_end: usize,
    pub(crate) had_info: bool,
    pub(crate) trailer: Dict,
    pub(crate) trailer_print: Vec<u8>,

    pub(crate) entries: BTreeMap<u32, Entry>,
    pub(crate) sections: Vec<u64>,
    pub(crate) hybrid_streams: HashSet<u64>,
    pub(crate) loading_xref: bool,
    pub(crate) start_xref: u64,
    pub(crate) xref_is_stream: bool,
    pub(crate) next_object_number: u32,
    pub(crate) xref_rebuilt: bool,
    pub(crate) xref_entries: u64,

    pub(crate) objects: HashMap<u32, Slot>,
    pub(crate) object_streams: HashMap<u32, ObjectStream>,
    pub(crate) object_stream_bytes: usize,
    pub(crate) bad_object_streams: HashSet<u32>,
    pub(crate) decoded_object_streams: HashSet<u32>,
    /// Bytes of object streams decoded for the first time.
    pub(crate) first_object_stream_bytes: usize,
    pub(crate) globals: HashMap<u32, Arc<Vec<u8>>>,
    pub(crate) refused_globals: HashMap<u32, (usize, ReadError)>,
    pub(crate) loading: HashSet<u32>,
    pub(crate) changed: BTreeSet<u32>,
    pub(crate) originals: HashMap<u32, Vec<u8>>,
    pub(crate) deleted: BTreeSet<u32>,
    pub(crate) first_new: u32,

    pub(crate) scan: Option<Arc<Scan>>,
    pub(crate) last_endstream: Cell<i64>,
    pub(crate) endstream_spans: std::cell::RefCell<std::collections::BTreeMap<usize, usize>>,
    pub(crate) last_endobj: Cell<i64>,
    pub(crate) endobj_spans: std::cell::RefCell<std::collections::BTreeMap<usize, usize>>,
    pub(crate) length_repaired: HashSet<usize>,
    /// Bytes read by object parses, over the document's life.
    pub(crate) parsed_bytes: Cell<usize>,
    /// The offsets of the listed objects, ascending; made on use and dropped when an entry or the scan changes.
    pub(crate) object_starts: std::cell::RefCell<Option<Vec<u64>>>,

    pub(crate) pages: Option<Arc<Vec<Page>>>,
    pub(crate) pages_walked: bool,
    pub(crate) linearized: Option<bool>,
    pub(crate) pdfa: Option<(Option<i32>, Option<String>)>,
}

impl Document {
    pub fn open(data: Vec<u8>, filters: Box<dyn Filters>) -> Result<Document, ReadError> {
        if data.len() > i32::MAX as usize {
            return Err(ReadError::new(
                Problem::Unsupported,
                "The file is too large.",
            ));
        }
        let mut document = Document {
            data,
            filters,
            repairs: Vec::new(),
            header_version_end: 0,
            had_info: false,
            trailer: Dict::new(),
            trailer_print: Vec::new(),
            entries: BTreeMap::new(),
            sections: Vec::new(),
            hybrid_streams: HashSet::new(),
            loading_xref: false,
            start_xref: 0,
            xref_is_stream: false,
            next_object_number: 0,
            xref_rebuilt: false,
            xref_entries: 0,
            objects: HashMap::new(),
            object_streams: HashMap::new(),
            bad_object_streams: HashSet::new(),
            decoded_object_streams: HashSet::new(),
            globals: HashMap::new(),
            refused_globals: HashMap::new(),
            object_stream_bytes: 0,
            first_object_stream_bytes: 0,
            loading: HashSet::new(),
            changed: BTreeSet::new(),
            originals: HashMap::new(),
            deleted: BTreeSet::new(),
            first_new: u32::MAX,
            scan: None,
            last_endstream: Cell::new(-2),
            endstream_spans: Default::default(),
            last_endobj: Cell::new(-2),
            endobj_spans: Default::default(),
            length_repaired: HashSet::new(),
            parsed_bytes: Cell::new(0),
            object_starts: Default::default(),
            pages: None,
            pages_walked: false,
            linearized: None,
            pdfa: None,
        };
        document.load()?;
        Ok(document)
    }

    fn load(&mut self) -> Result<(), ReadError> {
        self.find_header()?;
        let mut rebuilt = false;
        if let Err(e) = self.load_cross_references() {
            self.rebuild(&e.detail)?;
            rebuilt = true;
        }
        if !rebuilt {
            self.check_not_encrypted()?;
            if !self.catalog_usable() {
                self.rebuild("The catalog could not be read.")?;
            }
        }
        self.first_new = self.next_object_number;
        self.trailer_print = self.print_trailer();
        self.had_info = self.trailer.contains_key("Info");
        Ok(())
    }

    /// The header must come in the first 1024 bytes; what is before it is cut off, so offsets count from it as
    /// viewers count them.
    fn find_header(&mut self) -> Result<(), ReadError> {
        let window = &self.data[..self.data.len().min(1024)];
        let Some(at) = crate::bytes::find(window, b"%PDF-", 0) else {
            return Err(ReadError::new(
                Problem::NotPdf,
                "The file does not start with %PDF.",
            ));
        };
        if at > 0 {
            self.data.drain(..at);
            self.add_repair(format!("Cut {at} bytes before the %PDF header."));
        }
        let mut end = 5;
        while end < self.data.len()
            && end < 13
            && (self.data[end].is_ascii_digit() || self.data[end] == b'.')
        {
            end += 1;
        }
        self.header_version_end = end;
        Ok(())
    }

    /// The crypt layer opens encrypted files; until it is here they are refused.
    pub(crate) fn check_not_encrypted(&mut self) -> Result<(), ReadError> {
        let entry = match self.trailer.get("Encrypt") {
            None | Some(Object::Null) => return Ok(()),
            Some(entry) => entry.clone(),
        };
        match self.flatten(Some(&entry), 3, false) {
            Some(Object::Dict(_)) => {
                Err(ReadError::new(Problem::Encrypted, "The file is encrypted."))
            }
            _ => Err(ReadError::damaged(
                "The /Encrypt entry is not a dictionary.",
            )),
        }
    }

    pub(crate) fn add_repair(&mut self, text: String) {
        match self.repairs.len().cmp(&MAX_REPAIRS) {
            std::cmp::Ordering::Less => self.repairs.push(text),
            std::cmp::Ordering::Equal => self
                .repairs
                .push("More repairs were made than are listed.".into()),
            std::cmp::Ordering::Greater => {}
        }
    }

    /// What was wrong with the file and how it was got round, one line each (at most 100).
    pub fn repairs(&self) -> &[String] {
        &self.repairs
    }

    /// The bytes the document was read from, starting at the header.
    pub fn length(&self) -> usize {
        self.data.len()
    }

    pub fn source(&self) -> &[u8] {
        &self.data
    }

    /// The newest trailer; /Root, /Info, /ID and /Encrypt come from an older one when the newest lacks them.
    pub fn trailer(&self) -> &Dict {
        &self.trailer
    }

    pub fn trailer_mut(&mut self) -> &mut Dict {
        &mut self.trailer
    }

    /// The catalog, or none when /Root names no dictionary.
    pub fn catalog(&mut self) -> Option<Arc<Object>> {
        let root = self.trailer.get("Root").cloned()?;
        let found = self.resolve(&root)?;
        matches!(&*found, Object::Dict(_)).then(|| found.into_shared())
    }

    pub fn info(&mut self) -> Option<Arc<Object>> {
        let info = self.trailer.get("Info").cloned()?;
        let found = self.resolve(&info)?;
        matches!(&*found, Object::Dict(_)).then(|| found.into_shared())
    }

    /// The trailer's two /ID strings; empty without them.
    pub fn id(&mut self) -> Vec<Vec<u8>> {
        let Some(ids) = self.trailer.get("ID").cloned() else {
            return Vec::new();
        };
        let Some(array) = self.resolve(&ids) else {
            return Vec::new();
        };
        let Some(items) = array.as_array() else {
            return Vec::new();
        };
        let mut strings = Vec::new();
        for item in items {
            if let Some(Object::String(s)) = self.resolve(item).as_deref() {
                strings.push(s.bytes.clone());
            }
        }
        strings
    }

    pub fn is_encrypted(&self) -> bool {
        false
    }

    /// The first /ID string a save has to keep; none when the file has none.
    pub(crate) fn first_id(&mut self) -> Option<Vec<u8>> {
        self.id().into_iter().next()
    }

    /// The version of the file: the header's, or the catalog's /Version when that is newer.
    pub fn version(&mut self) -> String {
        let header = self.header_version();
        let declared = self
            .catalog()
            .and_then(|c| c.as_dict().and_then(|d| d.get("Version").cloned()))
            .and_then(|v| {
                self.resolve(&v)
                    .and_then(|n| n.as_name().map(|n| latin1(n.as_bytes())))
            });
        match declared {
            Some(declared) => match (parse_version(&declared), parse_version(&header)) {
                (Some(d), Some(h)) if d > h => declared,
                _ => header,
            },
            None => header,
        }
    }

    fn header_version(&self) -> String {
        let end = self.header_version_end;
        let text = latin1(&self.data[5..end]);
        if parse_version(&text).is_some() {
            text
        } else {
            "1.4".into()
        }
    }

    pub(crate) fn print_trailer(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.trailer.write_to(&mut out);
        out
    }

    /// The trailer holds something other than what was read.
    pub fn trailer_changed(&self) -> bool {
        self.print_trailer() != self.trailer_print
    }

    /// Where the newest cross-reference section starts, as the file said; 0 when the table was rebuilt.
    pub fn start_xref(&self) -> u64 {
        self.start_xref
    }

    /// The newest section is a cross-reference stream, so an update writes one too.
    pub fn xref_is_stream(&self) -> bool {
        self.xref_is_stream
    }

    /// The table was rebuilt from a scan, so the offsets in the file's own cross-reference data cannot be built on.
    pub fn xref_rebuilt(&self) -> bool {
        self.xref_rebuilt
    }

    /// Where each cross-reference section of the file starts, newest first; empty after a rebuild, when the file's own
    /// sections cannot be told.
    pub fn section_offsets(&self) -> &[u64] {
        if self.xref_rebuilt {
            &[]
        } else {
            &self.sections
        }
    }

    /// The first object number not used: the trailer's /Size, or one past the highest listed when that is more.
    pub fn next_object_number(&self) -> u32 {
        self.next_object_number
    }

    pub(crate) fn reference(&self, number: u32) -> Ref {
        Ref {
            number,
            generation: self.generation_of(number),
        }
    }
}

/// System.Version's rules: two to four numeric parts, compared part by part.
fn parse_version(text: &str) -> Option<Vec<u32>> {
    let parts: Vec<&str> = text.split('.').collect();
    if !(2..=4).contains(&parts.len()) {
        return None;
    }
    parts
        .iter()
        .map(|p| {
            (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse::<u32>().ok())
                .flatten()
        })
        .collect()
}
