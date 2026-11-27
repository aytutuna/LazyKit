#![no_main]

use lazy_pdf_syntax::{DecodeError, Document, EntryKind, Filters, Object, Parm, SaveOptions};
use libfuzzer_sys::fuzz_target;
use std::collections::HashSet;

/// Flate and nothing else, with output capped far below the library's own limit so a bomb costs little.
struct Flate;

impl Filters for Flate {
    fn decode(
        &self,
        filter: &[u8],
        _parms: &[(Vec<u8>, Parm)],
        data: &[u8],
        max_size: usize,
        _strict: bool,
    ) -> Result<Vec<u8>, DecodeError> {
        match filter {
            b"FlateDecode" | b"Fl" => {
                miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, max_size.min(1 << 20))
                    .map_err(|e| DecodeError::Damaged(format!("{e:?}")))
            }
            _ => Err(DecodeError::Unsupported("filter".into())),
        }
    }

    fn flate(&self, data: &[u8]) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(data, 1)
    }
}

fn reachable_from_trailer(document: &mut Document) -> HashSet<u32> {
    fn refs(object: &Object, found: &mut Vec<u32>) {
        match object {
            Object::Ref(r) => found.push(r.number),
            Object::Array(items) => items.iter().for_each(|i| refs(i, found)),
            Object::Dict(d) => d.entries().iter().for_each(|(_, v)| refs(v, found)),
            Object::Stream(s) => s.dict.entries().iter().for_each(|(_, v)| refs(v, found)),
            _ => {}
        }
    }
    let mut todo = Vec::new();
    for (_, value) in document.trailer().entries() {
        refs(value, &mut todo);
    }
    let mut seen = HashSet::new();
    while let Some(number) = todo.pop() {
        if seen.len() < 100_000
            && seen.insert(number)
            && let Some(object) = document.get(number)
        {
            refs(&object, &mut todo);
            if let Some(entry) = document.entry(number)
                && entry.kind == EntryKind::Compressed
            {
                todo.push(entry.stream_number);
            }
        }
    }
    seen
}

fuzz_target!(|data: &[u8]| {
    let Ok(mut document) = Document::open(data.to_vec(), Box::new(Flate)) else {
        return;
    };
    let pages = document.pages();
    for page in pages.iter().take(50) {
        page.media_box(&mut document);
        page.crop_box(&mut document);
        page.rotate(&mut document);
        page.user_unit(&mut document);
        page.contents(&mut document);
    }
    for number in document.object_numbers().into_iter().take(500) {
        document.get(number);
    }
    document.is_signed();
    document.is_linearized();
    for packed in [false, true] {
        let saved = document.save_rewrite(&SaveOptions {
            use_object_streams: packed,
            document_id: None,
        });
        // What the writer made has to be something the reader accepts.
        assert!(Document::open(saved.bytes, Box::new(Flate)).is_ok());
    }
    // An update that changes something, a new object and a freed one; it has to open and read back what was done.
    let root = match document.trailer().get("Root") {
        Some(Object::Ref(r)) => Some(r.number),
        _ => None,
    };
    let marked = root.is_some_and(|number| match document.get_mut(number) {
        Some(Object::Dict(catalog)) => {
            catalog.insert("FuzzMark", Object::int(7));
            true
        }
        _ => false,
    });
    let added = document.add(Object::int(42)).ok();
    // Only an object nothing reachable names is freed: losing the page tree, or an object stream with members in
    // it, is a change the file does not survive.
    let reachable = reachable_from_trailer(&mut document);
    let freed = document
        .object_numbers()
        .into_iter()
        .find(|n| !reachable.contains(n) && Some(*n) != added.map(|r| r.number));
    if let Some(number) = freed {
        document.delete(number);
    }
    let Ok(saved) = document.save_incremental() else {
        return;
    };
    assert!(saved.len() >= data.len().min(document.length()));
    let mut reopened = Document::open(saved, Box::new(Flate)).expect("an update reopens");
    if let Some(added) = added {
        assert_eq!(
            reopened.get(added.number).as_deref(),
            Some(&Object::int(42))
        );
    }
    if marked && let Some(Object::Dict(catalog)) = reopened.catalog().as_deref() {
        assert_eq!(catalog.get("FuzzMark"), Some(&Object::int(7)));
    }
    if let Some(number) = freed
        && !reopened.xref_rebuilt()
    {
        assert!(reopened.get(number).is_none());
    }
});
