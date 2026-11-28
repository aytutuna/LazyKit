use std::collections::HashMap;

use lazy_pdf_filters::flate::inflate;
use lazy_pdf_syntax::{DecodeError, Document, Filters, Parm};
use lazy_pdf_write::{
    Conformance, Date, G4Image, Image, OcrLine, OcrWord, Options, PageInput, PageSize, Rect,
    glyphless, make, num, pdfa, read_jpeg_header,
};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn data(name: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pdf-write");
    let bytes = std::fs::read(dir.join(name)).unwrap();
    let sums = std::fs::read_to_string(dir.join("SHA256SUMS")).unwrap();
    assert!(
        sums.contains(&format!("{} *{name}", sha(&bytes).to_lowercase())),
        "{name}"
    );
    bytes
}

fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}

fn hex_bytes(text: &str) -> Vec<u8> {
    (0..text.len() / 2)
        .map(|i| u8::from_str_radix(&text[i * 2..i * 2 + 2], 16).unwrap())
        .collect()
}

#[test]
fn numbers_are_written_as_the_reference_writes_them() {
    let table = String::from_utf8(data("number-format.txt")).unwrap();
    let mut count = 0;
    for line in table.lines() {
        let (bits, expected) = line.split_once(' ').unwrap();
        let value = f64::from_bits(u64::from_str_radix(bits, 16).unwrap());
        assert_eq!(num(value), expected, "{value:e}");
        count += 1;
    }
    assert!(count > 400);
}

#[test]
fn the_glyphless_font_is_the_reference_font() {
    let font = glyphless::program();
    assert_eq!(font, data("glyphless-font.ttf"));
    assert_eq!(font.len(), 636);
    assert_eq!(
        sha(font),
        "103B87CF98EE3A3E5FD752F3DB29975611605F785B36C879FAA61ABD439CC391"
    );
    // The file sums to the magic number, as a font must.
    let sum = font.chunks(4).fold(0u32, |s, c| {
        let mut w = [0u8; 4];
        w[..c.len()].copy_from_slice(c);
        s.wrapping_add(u32::from_be_bytes(w))
    });
    assert_eq!(sum, 0xB1B0_AFBA);
}

#[test]
fn the_cid_to_gid_map_inflates_to_glyph_one_for_every_code() {
    let compressed = glyphless::cid_to_gid_map();
    assert_eq!(compressed, data("glyphless-cidtogid.z"));
    assert_eq!(compressed.len(), 837);
    assert_eq!(
        sha(compressed),
        "2B554AACC4A744E7F14EF4BE52FA515B70E9AE4A60402C38B1D404E8EC7AE17E"
    );
    let map = inflate(compressed, 1 << 20, true).unwrap();
    assert_eq!(map, glyphless::cid_to_gid_map_data());
    assert_eq!(map.len(), glyphless::CODE_COUNT * 2);
    assert!(map.chunks(2).all(|c| c == [0, 1]));
}

#[test]
fn the_srgb_profile_is_the_pinned_file() {
    assert_eq!(pdfa::SRGB_PROFILE.len(), 736);
    assert_eq!(
        sha(pdfa::SRGB_PROFILE),
        "AF4EFE28F6D311799865F325BA39184A2E978B113CA74124D60AF7DE22B105F4"
    );
}

#[test]
fn glyphless_codes_give_supplementary_characters_codes_of_their_own() {
    let codes = glyphless::Codes::for_texts(["a\u{1F4C4}b\u{20000}\u{1F4C4}"]);
    assert_eq!(codes.supplementary(), [0x1F4C4, 0x20000]);
    assert_eq!(
        codes.codes("a\u{1F4C4}b\u{20000}"),
        [0x61, 0xD800, 0x62, 0xD801]
    );
    assert_eq!(codes.codes("\u{10FFFF}"), [0xFFFD]);
    assert_eq!(codes.hex_string("Hi"), "<00480069>");
    let cmap = codes.to_unicode_cmap();
    assert!(cmap.contains("2 beginbfchar\n<D800> <D83DDCC4>\n<D801> <D840DC00>\nendbfchar\n"));
    assert!(cmap.contains(
        "<0000> <00FF> <0000>
"
    ));
    assert!(!cmap.contains("<D800> <D8FF>"));
}

#[test]
fn dates_are_written_in_both_notations() {
    let date = Date {
        year: 2026,
        month: 10,
        day: 19,
        hour: 21,
        minute: 30,
        second: 5,
        offset_minutes: 180,
    };
    assert_eq!(date.pdf(), "D:20261019213005+03'00'");
    assert_eq!(date.xmp(), "2026-10-19T21:30:05+03:00");
    let west = Date {
        offset_minutes: -330,
        ..date
    };
    assert_eq!(west.pdf(), "D:20261019213005-05'30'");
    assert_eq!(west.xmp(), "2026-10-19T21:30:05-05:30");
    let now = Date::now().unwrap();
    assert!(now.year >= 2026 && (1..=12).contains(&now.month) && (1..=31).contains(&now.day));
}

#[test]
fn text_strings_and_titles() {
    assert_eq!(pdfa::text_string("a (b) \\ c"), "(a \\(b\\) \\\\ c)");
    assert_eq!(
        pdfa::text_string("\u{e7}a\u{1F4C4}"),
        "<FEFF00E70061D83DDCC4>"
    );
    assert_eq!(pdfa::clean_title("a\u{1}b\u{7F}c\u{FFFE}d\u{FFFF}"), "abcd");
}

#[test]
fn the_jpeg_header_is_read_past_other_segments() {
    let gray = data("jpeg-tiny-gray.jpg");
    assert_eq!(read_jpeg_header(&gray), Some((1, 1, 1)));
    assert_eq!(
        read_jpeg_header(&data("jpeg-rgb-header.jpg")),
        Some((16, 12, 3))
    );
    assert_eq!(read_jpeg_header(&data("jpeg-unreadable.bin")), None);
    assert_eq!(read_jpeg_header(&[0xFF, 0xD8, 0xFF, 0xDA, 0, 2]), None);
    assert_eq!(read_jpeg_header(&[]), None);
}

fn image_files(cases: &Value) -> HashMap<String, Vec<u8>> {
    cases["images"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(key, file)| (key.clone(), data(file.as_str().unwrap())))
        .collect()
}

fn rect(value: &Value) -> Rect {
    let n: Vec<f64> = value
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_f64().unwrap())
        .collect();
    Rect::new(n[0], n[1], n[2], n[3])
}

fn date(text: &str) -> Date {
    let (day, rest) = text.split_once('T').unwrap();
    let d: Vec<u32> = day.split('-').map(|p| p.parse().unwrap()).collect();
    let split = rest.rfind(['+', '-']).unwrap();
    let t: Vec<u32> = rest[..split]
        .split(':')
        .map(|p| p.parse().unwrap())
        .collect();
    let (sign, offset) = rest[split..].split_at(1);
    let (oh, om) = offset.split_once(':').unwrap();
    let minutes = oh.parse::<i32>().unwrap() * 60 + om.parse::<i32>().unwrap();
    Date {
        year: d[0] as i32,
        month: d[1],
        day: d[2],
        hour: t[0],
        minute: t[1],
        second: t[2],
        offset_minutes: if sign == "-" { -minutes } else { minutes },
    }
}

/// The PDFs the reference writer made from the cases in pdf-cases.json.
fn cases() -> Vec<(String, Vec<PageInput>, Options)> {
    let cases: Value = serde_json::from_slice(&data("pdf-cases.json")).unwrap();
    let images = image_files(&cases);
    let scripts: Vec<String> = cases["scripts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    cases["cases"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let o = &c["options"];
            let mut options = Options::new("Lazy Scan");
            if o["conformance"] == "pdfa2b" {
                options.conformance = Conformance::PdfA2b;
            }
            options.title = o["title"].as_str().map(str::to_string);
            options.date = Some(date(o["date"].as_str().unwrap()));
            options.document_id = Some(hex_bytes(o["id"].as_str().unwrap()).try_into().unwrap());
            options.page_size = match o["pageSize"].as_str() {
                Some("a4") => PageSize::A4,
                Some(_) => PageSize::Letter,
                None => PageSize::AsScanned,
            };
            let pages = c["pages"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| {
                    let file = images[p["image"].as_str().unwrap()].clone();
                    let g4 = |data: Vec<u8>| G4Image {
                        data,
                        width: p["g4"][0].as_u64().unwrap() as usize,
                        height: p["g4"][1].as_u64().unwrap() as usize,
                    };
                    let image = if let Some(mask) = p["mask"].as_str() {
                        Image::Mixed {
                            background: file,
                            mask: g4(images[mask].clone()),
                        }
                    } else if p["g4"].is_array() {
                        Image::G4(g4(file))
                    } else {
                        Image::Jpeg(file)
                    };
                    let mut lines = Vec::new();
                    if p["scripts"].is_boolean() {
                        for (i, text) in scripts.iter().enumerate() {
                            lines.push(OcrLine {
                                text: text.clone(),
                                bounds: Rect::new(0.1, 0.9 - i as f64 * 0.09, 0.8, 0.05),
                                words: vec![],
                            });
                        }
                    }
                    for l in p["lines"].as_array().into_iter().flatten() {
                        lines.push(OcrLine {
                            text: l["text"].as_str().unwrap().to_string(),
                            bounds: rect(&l["box"]),
                            words: l["words"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .map(|w| OcrWord {
                                    text: w["text"].as_str().unwrap().to_string(),
                                    bounds: rect(&w["box"]),
                                })
                                .collect(),
                        });
                    }
                    PageInput {
                        image,
                        pixel_width: p["width"].as_u64().unwrap() as u32,
                        pixel_height: p["height"].as_u64().unwrap() as u32,
                        dpi: p["dpi"].as_u64().unwrap() as u32,
                        ocr_lines: lines,
                    }
                })
                .collect();
            (c["name"].as_str().unwrap().to_string(), pages, options)
        })
        .collect()
}

#[test]
fn every_case_is_the_reference_file_byte_for_byte() {
    let all = cases();
    assert_eq!(all.len(), 13);
    for (name, pages, options) in all {
        let expected = data(&format!("pdf-{name}.pdf"));
        let pdf = make(&pages, &options).unwrap();
        assert!(
            pdf == expected,
            "{name}: {} bytes, the reference has {}",
            pdf.len(),
            expected.len()
        );
    }
}

#[test]
fn the_pinned_golden_files() {
    let golden = [
        (
            "standard-golden",
            7186,
            "79CEAD5058F5ECA74778937FF13CEBC8BBC2DD19936EB4607F8404EE1A6A8577",
        ),
        (
            "archival-golden",
            8451,
            "4D1350CC42A4A7158A9EB3CCEC9E381DDD085B4E79895BFBA7C0099363158625",
        ),
        (
            "g4-and-jpeg-golden",
            7154,
            "C9CB40F60D03C4B573EA256FA926136D60F992022CC05FEB7C978B1147E8F55B",
        ),
    ];
    for (name, length, hash) in golden {
        let (_, pages, options) = cases().into_iter().find(|c| c.0 == name).unwrap();
        let pdf = make(&pages, &options).unwrap();
        assert_eq!(pdf.len(), length, "{name}");
        assert_eq!(sha(&pdf), hash, "{name}");
    }
}

struct Inflate;

impl Filters for Inflate {
    fn decode(
        &self,
        filter: &[u8],
        _parms: &[(Vec<u8>, Parm)],
        data: &[u8],
        max_size: usize,
        strict: bool,
    ) -> Result<Vec<u8>, DecodeError> {
        match filter {
            b"FlateDecode" | b"Fl" => {
                inflate(data, max_size as u64, strict).map_err(|e| DecodeError::Damaged(e.message))
            }
            other => Err(DecodeError::Unsupported(
                String::from_utf8_lossy(other).into_owned(),
            )),
        }
    }

    fn flate(&self, data: &[u8]) -> Vec<u8> {
        lazy_pdf_filters::encode::flate(data, 6)
    }
}

#[test]
fn a_reader_opens_every_file_without_repairing_it() {
    for (name, pages, options) in cases() {
        let pdf = make(&pages, &options).unwrap();
        let mut document =
            Document::open(pdf, Box::new(Inflate)).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(
            document.repairs().is_empty(),
            "{name}: {:?}",
            document.repairs()
        );
        assert_eq!(document.pages().len(), pages.len(), "{name}");
        assert!(!document.xref_rebuilt(), "{name}");
        let expected = if options.conformance == Conformance::PdfA2b {
            "1.7"
        } else {
            "1.4"
        };
        assert_eq!(document.version(), expected, "{name}");
        assert_eq!(document.id().len(), 2, "{name}");
    }
}

#[test]
fn the_text_layer_is_read_back_through_the_font_map() {
    let (_, pages, options) = cases()
        .into_iter()
        .find(|c| c.0 == "standard-golden")
        .unwrap();
    let mut document = Document::open(make(&pages, &options).unwrap(), Box::new(Inflate)).unwrap();
    let listed = document.pages();
    let contents = String::from_utf8(listed[0].contents(&mut document)).unwrap();
    assert!(
        contents.starts_with("q\n612 0 0 792 0 0 cm\n/Im0 Do\nQ\nBT\n3 Tr\n"),
        "{contents}"
    );
    assert!(contents.contains("/F1 31.68 Tf\n"), "{contents}");
    assert!(contents.ends_with(" Tj\nET\n"));
    let plain = listed[1].contents(&mut document);
    assert_eq!(plain, b"q\n612 0 0 792 0 0 cm\n/Im1 Do\nQ\n");
}

#[test]
fn the_page_size_places_an_image_without_enlarging_it() {
    let p = PageSize::A4.placement(400.0, 300.0);
    assert_eq!((p.page_width, p.page_height), (595.0, 842.0));
    assert_eq!((p.width, p.height, p.x, p.y), (400.0, 300.0, 97.5, 271.0));
    let big = PageSize::Letter.placement(1224.0, 1584.0);
    assert_eq!(
        (big.width, big.height, big.x, big.y),
        (612.0, 792.0, 0.0, 0.0)
    );
    let as_scanned = PageSize::AsScanned.placement(100.5, 50.25);
    assert_eq!(
        (as_scanned.page_width, as_scanned.width, as_scanned.x),
        (100.5, 100.5, 0.0)
    );
}

#[test]
fn an_unset_date_and_id_are_filled_in() {
    let mut options = Options::new("Lazy Scan");
    options.title = Some("T".into());
    let first = make(&[], &options).unwrap();
    let second = make(&[], &options).unwrap();
    assert_ne!(first, second, "the identifier is random");
    assert!(first.windows(6).any(|w| w == b"/ID [<"));
}

#[test]
fn the_producer_is_escaped_in_the_xmp() {
    let date = Date::now().unwrap();
    let xmp = String::from_utf8(pdfa::xmp(None, &date, "A & B <c>")).unwrap();
    assert!(xmp.contains("<pdf:Producer>A &amp; B &lt;c&gt;</pdf:Producer>"));
    assert!(xmp.contains("<xmp:CreatorTool>A &amp; B &lt;c&gt;</xmp:CreatorTool>"));
}
