use lazy_pdf_fonts::{
    Standard14, code_of, encoding_by_name, glyph_name_of, mac_expert, mac_roman, pdf_doc, standard,
    symbol, unicode_of_glyph_name, win_ansi, zapf_dingbats, zapf_name_of,
};

fn all() -> [&'static lazy_pdf_fonts::Encoding; 7] {
    [
        standard(),
        win_ansi(),
        mac_roman(),
        mac_expert(),
        pdf_doc(),
        symbol(),
        zapf_dingbats(),
    ]
}

#[test]
fn glyph_names_map_to_text() {
    let cases = [
        ("A", "A"),
        ("space", " "),
        ("Euro", "\u{20AC}"),
        ("fi", "\u{FB01}"),
        ("uni0041", "A"),
        ("uni00410042", "AB"),
        ("u1F600", "\u{1F600}"),
        ("u0041", "A"),
        ("a.alt", "a"),
        ("A.swash.1", "A"),
        ("f_i", "fi"),
        ("f_f_uni0069", "ffi"),
        ("a1", "\u{2701}"),
        ("dalethatafpatah", "\u{5D3}\u{5B2}"),
    ];
    for (name, text) in cases {
        assert_eq!(unicode_of_glyph_name(name).as_deref(), Some(text), "{name}");
    }
}

#[test]
fn unmapped_names_give_none() {
    for name in [
        "", ".notdef", "g123", "cid1234", "uniD800", "uni004", "uniGGGG", "u110000", "u+041", "_",
    ] {
        assert_eq!(unicode_of_glyph_name(name), None, "{name}");
    }
    assert_eq!(unicode_of_glyph_name("a_g123_b").as_deref(), Some("ab"));
    assert_eq!(unicode_of_glyph_name(&"a".repeat(5000)), None);
}

#[test]
fn name_of_prefers_the_encoding_names_and_falls_back() {
    for (cp, name) in [
        (0x20, "space"),
        (0x41, "A"),
        (0x20AC, "Euro"),
        (0xFB01, "fi"),
        (0x2022, "bullet"),
    ] {
        assert_eq!(glyph_name_of(cp), name, "{cp:x}");
    }
    assert_eq!(glyph_name_of(0x4E2D), "uni4E2D");
    assert_eq!(glyph_name_of(0x1F600), "u1F600");
    assert_eq!(glyph_name_of(0xD800), ".notdef");
    assert_eq!(glyph_name_of(-1), ".notdef");
    assert_eq!(glyph_name_of(0x110000), ".notdef");
    assert_eq!(zapf_name_of(0x2701).as_deref(), Some("a1"));
    assert_eq!(zapf_name_of(0x41), None);
}

#[test]
fn names_and_text_round_trip() {
    let mut seen = std::collections::HashSet::new();
    for table in [win_ansi(), standard(), mac_roman()] {
        for name in table.iter().flatten() {
            if !seen.insert(*name) {
                continue;
            }
            let text = unicode_of_glyph_name(name).unwrap_or_else(|| panic!("{name}"));
            let first = text.chars().next().unwrap() as i64;
            assert_eq!(
                unicode_of_glyph_name(&glyph_name_of(first)),
                Some(text),
                "{name}"
            );
        }
    }
}

#[test]
fn tables_have_the_documented_quirks() {
    for table in all() {
        assert_eq!(table.len(), 256);
    }
    assert_eq!(standard()[39], Some("quoteright"));
    assert_eq!(standard()[96], Some("quoteleft"));
    assert_eq!(standard()[174], Some("fi"));
    assert_eq!(standard()[127], None);
    assert_eq!(win_ansi()[39], Some("quotesingle"));
    assert_eq!(win_ansi()[128], Some("Euro"));
    assert_eq!(win_ansi()[142], Some("Zcaron"));
    assert_eq!(win_ansi()[233], Some("eacute"));
    assert_eq!(win_ansi()[255], Some("ydieresis"));
    assert_eq!(win_ansi()[127], Some("bullet"));
    assert_eq!(win_ansi()[129], Some("bullet"));
    assert_eq!(win_ansi()[160], Some("space"));
    assert_eq!(win_ansi()[173], Some("hyphen"));
    assert_eq!(mac_roman()[128], Some("Adieresis"));
    assert_eq!(mac_roman()[202], Some("space"));
    assert_eq!(mac_expert()[97], Some("Asmall"));
    assert_eq!(pdf_doc()[24], Some("breve"));
    assert_eq!(pdf_doc()[25], Some("caron"));
    assert_eq!(pdf_doc()[160], Some("Euro"));
    assert_eq!(symbol()[97], Some("alpha"));
    assert_eq!(symbol()[34], Some("universal"));
    assert_eq!(zapf_dingbats()[33], Some("a1"));
    assert_eq!(zapf_dingbats()[32], Some("space"));
}

#[test]
fn every_name_resolves_to_unicode() {
    for table in all() {
        if std::ptr::eq(table, mac_expert()) {
            continue;
        }
        for name in table.iter().flatten() {
            assert!(unicode_of_glyph_name(name).is_some(), "{name}");
        }
    }
}

#[test]
fn by_name_and_code_of() {
    assert!(std::ptr::eq(
        encoding_by_name("WinAnsiEncoding").unwrap(),
        win_ansi()
    ));
    assert!(encoding_by_name("Identity-H").is_none());
    assert_eq!(code_of(win_ansi(), "A"), 65);
    assert_eq!(code_of(standard(), "Euro"), -1);
}

#[test]
fn there_are_fourteen_standard_fonts() {
    assert_eq!(Standard14::all().len(), 14);
    let names: std::collections::HashSet<_> = Standard14::all().iter().map(|f| &f.name).collect();
    assert_eq!(names.len(), 14);
}

#[test]
fn standard_font_metrics() {
    let f = Standard14::find("Helvetica").unwrap();
    assert_eq!(f.width_of("space"), Some(278));
    assert_eq!(f.width_of("A"), Some(667));
    assert_eq!((f.ascent, f.descent), (718, -207));
    assert_eq!(
        (f.b_box.left, f.b_box.bottom, f.b_box.right, f.b_box.top),
        (-166, -225, 1000, 931)
    );
    assert_eq!(f.width_of("a1"), None);
    assert_eq!(
        Standard14::find("Times-Roman").unwrap().width_of("A"),
        Some(722)
    );
    assert_eq!(
        Standard14::find("Times-Bold").unwrap().width_of("A"),
        Some(722)
    );
    let courier = Standard14::find("Courier").unwrap();
    assert!(courier.is_fixed_pitch);
    for g in ["A", "i", "W", "space"] {
        assert_eq!(courier.width_of(g), Some(600), "{g}");
    }
    let sym = Standard14::find("Symbol").unwrap();
    assert_eq!(sym.width_of("alpha"), Some(631));
    assert!(sym.is_symbolic());
    assert_eq!(sym.ascent, 1010);
    assert!(std::ptr::eq(sym.built_in_encoding(), symbol()));
    let zapf = Standard14::find("ZapfDingbats").unwrap();
    assert_eq!(zapf.width_of("a1"), Some(974));
    assert!(std::ptr::eq(zapf.built_in_encoding(), zapf_dingbats()));
    assert!(std::ptr::eq(f.built_in_encoding(), standard()));
}

#[test]
fn standard_font_aliases() {
    let cases = [
        ("Arial", "Helvetica"),
        ("ArialMT", "Helvetica"),
        ("Arial,Bold", "Helvetica-Bold"),
        ("Arial-BoldMT", "Helvetica-Bold"),
        ("Arial-ItalicMT", "Helvetica-Oblique"),
        ("Arial-BoldItalicMT", "Helvetica-BoldOblique"),
        ("Helvetica-Oblique", "Helvetica-Oblique"),
        ("TimesNewRoman", "Times-Roman"),
        ("TimesNewRomanPSMT", "Times-Roman"),
        ("TimesNewRomanPS-BoldMT", "Times-Bold"),
        ("Times New Roman,Italic", "Times-Italic"),
        ("TimesNewRomanPS-BoldItalicMT", "Times-BoldItalic"),
        ("Times-Roman", "Times-Roman"),
        ("CourierNew", "Courier"),
        ("CourierNewPS-BoldMT", "Courier-Bold"),
        ("Courier New,BoldItalic", "Courier-BoldOblique"),
        ("SymbolMT", "Symbol"),
        ("ZapfDingbats", "ZapfDingbats"),
        ("ABCDEF+Arial", "Helvetica"),
        ("arial", "Helvetica"),
    ];
    for (alias, name) in cases {
        assert_eq!(
            Standard14::find(alias).map(|f| f.name.as_str()),
            Some(name),
            "{alias}"
        );
    }
    for name in [
        "",
        "Arial Narrow",
        "Arial Black",
        "Calibri",
        "Symbolic",
        "Times-Bogus",
    ] {
        assert!(Standard14::find(name).is_none(), "{name}");
    }
}

#[test]
fn flags_follow_the_descriptor_bits() {
    assert_eq!(Standard14::find("Courier").unwrap().flags(), 1 | 32);
    assert_eq!(
        Standard14::find("Times-Italic").unwrap().flags(),
        2 | 32 | 64
    );
    assert_eq!(Standard14::find("Symbol").unwrap().flags(), 4);
    assert_ne!(
        Standard14::find("Helvetica-Bold").unwrap().flags() & (1 << 18),
        0
    );
}

#[test]
fn every_glyph_of_standard_encoding_has_a_width() {
    for f in Standard14::all().iter().filter(|f| !f.is_symbolic()) {
        for name in standard().iter().flatten() {
            assert!(f.width_of(name).is_some(), "{} {name}", f.name);
        }
    }
}
