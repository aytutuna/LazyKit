mod common;

use common::fixtures::{edit, make_save_fixture, table};
use common::open;
use common::pdf::TestPdf;
use lazy_pdf_syntax::{Dict, Lexer, Name, Object, PdfString, SaveOptions};

fn text(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| b as char).collect()
}

fn rewrite(file: Vec<u8>, packed: bool) -> Vec<u8> {
    open(file)
        .save_rewrite(&SaveOptions {
            use_object_streams: packed,
            document_id: None,
        })
        .bytes
}

#[test]
fn names_and_strings_read_back_as_they_were_written() {
    for b in 0..=255u8 {
        let name = Object::Name(Name(vec![b'A', b, b'z']));
        let mut out = Vec::new();
        name.write_to(&mut out);
        assert_eq!(
            Lexer::new(&out, 0).read_object(false).unwrap(),
            name,
            "name byte {b}"
        );
        for hex in [false, true] {
            let string = Object::String(PdfString {
                bytes: vec![b, b'(', b, b')', b'\\'],
                hex,
            });
            let mut out = Vec::new();
            string.write_to(&mut out);
            assert_eq!(
                Lexer::new(&out, 0).read_object(false).unwrap(),
                string,
                "string byte {b}"
            );
        }
    }
}

#[test]
fn a_rewrite_reads_back_the_same_pages() {
    for packed in [false, true] {
        let mut document = open(rewrite(table(), packed));
        assert!(document.repairs().is_empty(), "{:?}", document.repairs());
        assert_eq!(document.xref_is_stream(), packed);
        let pages = document.pages();
        assert_eq!(pages.len(), 2);
        assert_eq!(
            text(&pages[1].contents(&mut document)),
            "BT /F1 12 Tf 20 100 Td (Page two) Tj ET"
        );
        assert_eq!(document.version(), if packed { "1.5" } else { "1.4" });
    }
}

#[test]
fn a_rewrite_keeps_the_first_id_and_is_the_same_bytes_every_time() {
    let original = open(table()).id()[0].clone();
    let once = rewrite(table(), false);
    assert_eq!(once, rewrite(table(), false));
    let mut document = open(once);
    let ids = document.id();
    assert_eq!(ids[0], original);
    assert_ne!(ids[1], original);
}

#[test]
fn a_rewrite_can_take_the_first_id_it_is_given() {
    let mut document = open(table());
    let saved = document
        .save_rewrite(&SaveOptions {
            use_object_streams: false,
            document_id: Some(vec![9; 16]),
        })
        .bytes;
    assert_eq!(open(saved).id()[0], vec![9; 16]);
}

#[test]
fn a_rewrite_leaves_out_what_the_trailer_cannot_reach() {
    let mut pdf = TestPdf::with_pages(1);
    pdf.obj(9, "<</Orphan true>>");
    let saved = rewrite(pdf.finish(), false);
    assert!(!text(&saved).contains("Orphan"));
}

#[test]
fn a_rewrite_counts_the_objects_it_could_not_read() {
    let mut pdf = TestPdf::with_pages(1);
    pdf.obj(9, "<</Broken (unclosed>>");
    pdf.obj(8, "<</Type /Pages /Kids [3 0 R] /Count 1 /Extra 9 0 R>>");
    let file = pdf.finish();
    // The page tree is object 2; point the catalog at object 8, which refers to the unreadable one.
    let file = TestPdf::replace(&file, "/Pages 2 0 R", "/Pages 8 0 R");
    let mut document = open(file);
    let result = document.save_rewrite(&SaveOptions::default());
    assert_eq!(result.dropped, 1);
}

#[test]
fn an_update_without_changes_is_the_file_itself() {
    let file = table();
    let mut document = open(file.clone());
    assert_eq!(document.save_incremental().unwrap(), file);
}

#[test]
fn an_update_changes_what_was_edited_and_leaves_the_rest() {
    for kind in ["table", "stream", "hybrid", "objects"] {
        let original = make_save_fixture(kind);
        let mut document = open(original.clone());
        edit(&mut document);
        let saved = document.save_incremental().unwrap();
        assert_eq!(saved[..original.len()], original[..], "{kind}");

        let mut reopened = open(saved);
        assert!(
            reopened.repairs().is_empty(),
            "{kind}: {:?}",
            reopened.repairs()
        );
        let info = reopened.info().unwrap();
        let title = match info.as_dict().unwrap().get("Title") {
            Some(Object::String(s)) => text(&s.bytes),
            other => panic!("{other:?}"),
        };
        assert_eq!(title, "New title", "{kind}");
        let pages = reopened.pages();
        assert_eq!(
            text(&pages[1].contents(&mut reopened)),
            "BT /F1 12 Tf 20 100 Td (Changed) Tj ET",
            "{kind}"
        );
        assert_eq!(
            text(&pages[0].contents(&mut reopened)),
            "BT /F1 12 Tf 20 100 Td (Page one) Tj ET",
            "{kind}"
        );
        let catalog = reopened.catalog().unwrap();
        let extra = catalog.as_dict().unwrap().get("Extra").unwrap().clone();
        assert!(reopened.resolve(&extra).is_some(), "{kind}");
        assert_eq!(
            reopened.xref_is_stream(),
            matches!(kind, "stream" | "objects"),
            "{kind}"
        );
    }
}

#[test]
fn a_deleted_object_stays_deleted_after_an_update() {
    let mut document = open(table());
    document.delete(7);
    document.trailer_mut().remove("Info");
    let saved = document.save_incremental().unwrap();
    let mut reopened = open(saved);
    assert!(reopened.get(7).is_none());
    assert!(reopened.info().is_none());
    assert_eq!(reopened.generation_of(7), 1);
}

#[test]
fn an_update_after_a_rebuild_lists_every_object() {
    let original = TestPdf::with_pages(2).finish_with("", 0, "");
    let mut document = open(original);
    assert!(document.xref_rebuilt());
    let note = document.add(Object::Dict(Dict::new())).unwrap();
    let saved = document.save_incremental().unwrap();
    let mut reopened = open(saved);
    assert!(!reopened.xref_rebuilt(), "{:?}", reopened.repairs());
    assert_eq!(reopened.pages().len(), 2);
    assert!(reopened.get(note.number).is_some());
}

#[test]
fn an_update_adds_a_line_break_when_the_file_does_not_end_in_one() {
    let mut file = table();
    file.extend_from_slice(b"%%EOF");
    // The last "startxref" still points at the table; what follows is trailing junk.
    let mut document = open(file.clone());
    document.trailer_mut().insert("Extra", Object::int(1));
    let saved = document.save_incremental().unwrap();
    assert_eq!(saved[file.len()], b'\n');
}

#[test]
fn a_changed_trailer_alone_is_enough_for_an_update() {
    let mut document = open(table());
    document.trailer_mut().insert("Extra", Object::int(1));
    assert!(document.trailer_changed());
    let saved = document.save_incremental().unwrap();
    let reopened = open(saved);
    assert!(matches!(
        reopened.trailer().get("Extra"),
        Some(Object::Number(_))
    ));
}

#[test]
fn many_updates_are_each_a_section_of_their_own() {
    let mut file = table();
    for round in 0..5 {
        let mut document = open(file);
        document.trailer_mut().insert("Round", Object::int(round));
        file = document.save_incremental().unwrap();
    }
    let document = open(file);
    assert!(document.repairs().is_empty());
    assert!(
        matches!(document.trailer().get("Round"), Some(Object::Number(n)) if n.integer() == Some(4))
    );
}

#[test]
fn an_object_opened_for_change_and_left_as_it_was_is_not_written_again() {
    let file = TestPdf::simple(1);
    let mut document = open(file.clone());
    document.get_mut(1).unwrap();
    assert_eq!(document.save_incremental().unwrap(), file);

    let mut document = open(file.clone());
    let Some(Object::Dict(catalog)) = document.get_mut(1) else {
        panic!("no catalog")
    };
    catalog.insert("Type", Object::Name(Name::new("Catalog")));
    assert_eq!(document.save_incremental().unwrap(), file);

    let mut document = open(file.clone());
    let Some(Object::Dict(catalog)) = document.get_mut(1) else {
        panic!("no catalog")
    };
    catalog.insert("Lang", Object::Name(Name::new("en")));
    assert!(document.save_incremental().unwrap().len() > file.len());
}
