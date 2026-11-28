use std::sync::Arc;

use super::*;
use crate::write_to_unicode;

const HEADER: &str = "/CIDInit /ProcSet findresource begin 12 dict begin begincmap /CMapName /Test def /CMapType 2 def\n";

fn parse(text: &str) -> CMap {
    CMap::parse(text.as_bytes(), None)
}

fn with_header(text: &str) -> CMap {
    parse(&format!("{HEADER}{text}"))
}

fn length(map: &CMap, bytes: &[u8]) -> usize {
    map.next_code(bytes).1
}

fn u(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn reads_bf_char_and_bf_range() {
    let map = with_header(
        "1 begincodespacerange <00> <FF> endcodespacerange
2 beginbfchar <01> <0041> <02> <00660069> endbfchar
2 beginbfrange <10> <12> <0061> <20> <21> [<0058> <005900590059>] endbfrange
endcmap",
    );
    assert_eq!(map.name.as_deref(), Some("Test"));
    assert_eq!(map.unicode_of(1, 0).as_deref(), Some("A"));
    assert_eq!(map.unicode_of(2, 0).as_deref(), Some("fi"));
    assert_eq!(map.unicode_of(0x10, 0).as_deref(), Some("a"));
    assert_eq!(map.unicode_of(0x12, 0).as_deref(), Some("c"));
    assert_eq!(map.unicode_of(0x20, 0).as_deref(), Some("X"));
    assert_eq!(map.unicode_of(0x21, 0).as_deref(), Some("YYY"));
    assert_eq!(map.unicode_of(0x13, 0), None);
    assert_eq!(map.unicode_of(0x22, 0), None);
}

#[test]
fn surrogate_pairs_and_two_byte_codes() {
    let map = with_header(
        "1 begincodespacerange <0000> <FFFF> endcodespacerange
1 beginbfchar <0003> <D83DDE00> endbfchar
1 beginbfrange <0100> <0102> <D83DDE00> endbfrange",
    );
    assert_eq!(map.unicode_of(3, 0).as_deref(), Some("\u{1F600}"));
    assert_eq!(map.unicode_of(0x101, 0).as_deref(), Some("\u{1F601}"));
    assert_eq!(map.next_code(&[0x01, 0x02, 0x03]), (0x0102, 2));
}

#[test]
fn bf_char_may_name_a_glyph() {
    let map = with_header("1 beginbfchar <05> /fi endbfchar");
    assert_eq!(map.unicode_of(5, 0).as_deref(), Some("\u{FB01}"));
}

#[test]
fn tolerates_comments_strings_and_odd_spacing() {
    let map = parse(
        "%!PS-Adobe\n/CIDSystemInfo << /Registry (Ado)be) /Ordering (UCS) >> def\n\
1 begincodespacerange<00><FF>endcodespacerange 1 beginbfchar % c\n<41>\n<00 42>endbfchar",
    );
    assert_eq!(map.unicode_of(0x41, 0).as_deref(), Some("B"));
}

#[test]
fn reads_cid_mappings_and_notdef() {
    let map = with_header(
        "1 begincodespacerange <0000> <FFFF> endcodespacerange
1 beginnotdefrange <0000> <001F> 1 endnotdefrange
2 begincidrange <0020> <007E> 100 <8000> <80FF> 500 endcidrange
1 begincidchar <0100> 7 endcidchar",
    );
    assert_eq!(map.cid_of(0x20, 0), Some(100));
    assert_eq!(map.cid_of(0x41, 0), Some(100 + 0x41 - 0x20));
    assert_eq!(map.cid_of(0x80FF, 0), Some(500 + 0xFF));
    assert_eq!(map.cid_of(0x100, 0), Some(7));
    assert_eq!(map.cid_of(0x05, 0), Some(1));
    assert_eq!(map.cid_of(0x200, 0), None);
}

#[test]
fn mixed_length_codespaces() {
    let map = with_header(
        "4 begincodespacerange <00> <80> <8140> <9FFC> <A0> <DF> <E040> <FCFC> endcodespacerange",
    );
    let s = [0x41, 0x82, 0xA0, 0xB1, 0xE0, 0x40];
    let mut pos = 0;
    let (mut codes, mut lengths) = (Vec::new(), Vec::new());
    while pos < s.len() {
        let (code, len) = map.next_code(&s[pos..]);
        codes.push(code);
        lengths.push(len);
        pos += len;
    }
    assert_eq!(codes, [0x41, 0x82A0, 0xB1, 0xE040]);
    assert_eq!(lengths, [1, 2, 1, 2]);
}

#[test]
fn invalid_bytes_stay_in_step() {
    let map = with_header("2 begincodespacerange <00> <80> <8140> <9FFC> endcodespacerange");
    assert_eq!(map.next_code(&[0x81, 0x20, 0x41]), (0x8120, 2));
    assert_eq!(map.next_code(&[0xFF, 0x41]), (0xFF, 1));
    assert_eq!(length(&map, &[0x81]), 1);
    assert_eq!(length(&map, &[]), 0);
}

#[test]
fn without_a_codespace_the_mapping_lengths_decide() {
    let map = with_header("1 beginbfchar <0041> <0041> endbfchar");
    assert_eq!(map.next_code(&[0x00, 0x41, 0x00]), (0x0041, 2));
}

#[test]
fn codes_of_different_lengths_are_different_codes() {
    let map = with_header(
        "2 begincodespacerange <00> <7F> <8000> <FFFF> endcodespacerange
1 begincidchar <41> 1 endcidchar
1 begincidchar <0041> 2 endcidchar",
    );
    assert_eq!(map.cid_of(0x41, 1), Some(1));
    assert_eq!(map.cid_of(0x41, 2), Some(2));
    assert_eq!(map.cid_of(0x41, 0), Some(1));
}

#[test]
fn later_mappings_win_over_earlier_ones() {
    let map = with_header("2 begincidrange <00> <FF> 0 <10> <1F> 900 endcidrange");
    assert_eq!(map.cid_of(0x15, 0), Some(905));
    assert_eq!(map.cid_of(0x30, 0), Some(0x30));
}

#[test]
fn use_cmap_takes_the_parents_mappings_and_codespace() {
    let map = with_header("/90ms-RKSJ-H usecmap 1 begincidchar <82A0> 9999 endcidchar");
    assert_eq!(map.cid_of(0x82A0, 0), Some(9999));
    assert_eq!(map.cid_of(0x41, 0), Some(231 + 0x41 - 0x20));
    assert_eq!(length(&map, &[0x82, 0xA0]), 2);
}

#[test]
fn use_cmap_from_the_stream_dictionary() {
    let parent = with_header(
        "1 begincodespacerange <00> <FF> endcodespacerange 1 begincidchar <41> 5 endcidchar",
    );
    let child = CMap::parse(
        format!("{HEADER}1 begincidchar <42> 6 endcidchar").as_bytes(),
        Some(Arc::new(parent)),
    );
    assert_eq!(child.cid_of(0x41, 0), Some(5));
    assert_eq!(child.cid_of(0x42, 0), Some(6));
}

#[test]
fn unknown_use_cmap_is_ignored() {
    let map = with_header("/No-Such-CMap usecmap 1 begincidchar <0041> 5 endcidchar");
    assert_eq!(map.cid_of(0x41, 0), Some(5));
    assert_eq!(map.cid_of(0x42, 0), None);
}

#[test]
fn reads_writing_mode() {
    assert!(with_header("/WMode 1 def").is_vertical);
    assert!(!with_header("/WMode 0 def").is_vertical);
}

#[test]
fn identity_maps() {
    let h = CMap::predefined("Identity-H");
    assert!(!h.is_vertical);
    assert!(CMap::predefined("Identity-V").is_vertical);
    assert_eq!(h.next_code(&[0x12, 0x34, 0x56]), (0x1234, 2));
    assert_eq!(h.cid_of(0x1234, 0), Some(0x1234));
    assert_eq!(h.cid_of(0, 0), Some(0));
    assert_eq!(h.cid_of(0x10000, 0), None);
    assert_eq!(h.unicode_of(0x41, 0), None);
    assert!(!h.is_fallback);
}

#[test]
fn shift_jis_predefined() {
    let map = CMap::predefined("90ms-RKSJ-H");
    assert_eq!(map.cid_of(0x82A0, 0), Some(843));
    assert_eq!(length(&map, &[0x82, 0xA0]), 2);
    assert_eq!(length(&map, &[0x41, 0x82, 0xA0]), 1);
    assert!(Arc::ptr_eq(&map, &CMap::predefined("90ms-RKSJ-H")));
    assert!(CMap::predefined("90ms-RKSJ-V").is_vertical);
}

#[test]
fn unicode_predefined_cmaps_map_codes_to_cids_and_text() {
    let map = CMap::predefined("UniJIS-UCS2-H");
    assert_eq!(map.cid_of(0x3042, 0), Some(843));
    assert_eq!(map.unicode_of(0x3042, 0).as_deref(), Some("\u{3042}"));
    let v = CMap::predefined("UniJIS-UTF16-V");
    assert!(v.is_vertical);
    assert_eq!(
        v.cid_of(0x3042, 0),
        CMap::predefined("UniJIS-UTF16-H").cid_of(0x3042, 0)
    );
    assert_eq!(
        CMap::predefined("UniJIS-UTF16-H")
            .unicode_of(i64::from(0xD842DF9Fu32 as i32), 0)
            .as_deref(),
        Some("\u{20B9F}")
    );
}

#[test]
fn cjk_predefined_cmaps_load() {
    for name in [
        "GBK-EUC-H",
        "UniGB-UCS2-H",
        "B5pc-H",
        "ETen-B5-V",
        "UniCNS-UTF16-H",
        "KSC-EUC-H",
        "KSCms-UHC-H",
        "UniKS-UCS2-V",
        "EUC-H",
        "H",
        "V",
    ] {
        let map = CMap::predefined(name);
        assert!(!map.is_fallback, "{name}");
        assert_eq!(map.name.as_deref(), Some(name));
        assert!((1..=2).contains(&length(&map, &[0x41, 0x42])), "{name}");
    }
}

#[test]
fn unknown_predefined_name_is_two_byte_codes() {
    let map = CMap::predefined("Adobe-Japan1-UCS2");
    assert!(map.is_fallback);
    assert_eq!(map.next_code(&[0x30, 0x42]), (0x3042, 2));
    assert_eq!(map.cid_of(0x3042, 0), Some(0x3042));
}

#[test]
fn garbage_gives_an_empty_map() {
    for text in [
        "",
        "\0\0\0",
        "<<<<>>>>",
        "((((",
        "begincidrange <",
        "1 begincidrange <00> endcidrange",
        "begincodespacerange <0000000000> <FF> endcodespacerange",
        "beginbfrange <00> <10> [ <41>",
    ] {
        let map = parse(text);
        assert_eq!(map.cid_of(0x41, 0), None, "{text}");
        assert_eq!(map.unicode_of(0x41, 0), None, "{text}");
        assert!((1..=3).contains(&length(&map, &[1, 2, 3])), "{text}");
    }
}

#[test]
fn reversed_and_oversized_ranges_are_skipped() {
    let map = with_header(
        "1 begincodespacerange <0000> <FFFF> endcodespacerange
1 begincidrange <0010> <0001> 5 endcidrange
1 beginbfrange <0000> <FFFF> <0000> endbfrange",
    );
    assert_eq!(map.cid_of(5, 0), None);
    assert_eq!(map.unicode_of(0x41, 0).as_deref(), Some("A"));
    assert_eq!(map.unicode_of(0xFFFF, 0).as_deref(), Some("\u{FFFF}"));
}

#[test]
fn random_bytes_never_panic() {
    let mut state = 7u64;
    let mut draw = |limit: u64| {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) % limit
    };
    let words = [
        "begincidrange",
        "endcidrange",
        "beginbfrange",
        "beginbfchar",
        "begincodespacerange",
        "/usecmap",
        "usecmap",
        "[",
        "]",
        "<",
        ">",
        "<41>",
        "<0041> <0043> 12",
        "(",
        "%",
        "/WMode",
        "99999999999999999999",
        "-5",
    ];
    for _ in 0..300 {
        let mut data: Vec<u8> = Vec::new();
        for _ in 0..200 {
            if draw(3) == 0 {
                data.push(draw(256) as u8);
            } else {
                data.extend(words[draw(words.len() as u64) as usize].bytes());
            }
            data.push(0x20);
        }
        let map = CMap::parse(&data, None);
        assert!((1..=4).contains(&length(&map, &[0x41, 0x42, 0x43, 0x44, 0x45])));
        let _ = map.cid_of(0x41, 0);
        let _ = map.unicode_of(0x41, 0);
    }
}

#[test]
fn hostile_input_is_bounded() {
    let mut text = format!("{HEADER}1 begincodespacerange <00> <FF> endcodespacerange\n");
    text += &"1 beginbfrange <00> <FF> <0041> endbfrange\n".repeat(200_000);
    text += &format!("1 beginbfchar <00> <{}> endbfchar\n", "A".repeat(2_000_000));
    text += &"<".repeat(100_000);
    text += &"(".repeat(100_000);
    let start = std::time::Instant::now();
    let map = parse(&text);
    assert!(start.elapsed().as_secs() < 60);
    assert_eq!(map.unicode_of(0, 0), Some("\u{AAAA}".repeat(128)));
}

#[test]
fn many_entries_are_capped() {
    let mut text =
        String::from("1 begincodespacerange <0000> <FFFF> endcodespacerange 1 begincidchar\n");
    for i in 0..1_200_000u32 {
        text += &format!("<{:04X}> {i}\n", i & 0xFFFF);
    }
    text += "endcidchar";
    let map = parse(&text);
    assert!(map.cid_of(0x10, 0).is_some());
    assert_eq!(map.cids.count(), 1_000_000);
}

fn repeated(section: &str, line: &str, count: usize, end: &str) -> String {
    format!(
        "1 begincodespacerange <00000000> <FFFFFFFF> endcodespacerange\n{section}\n{}{end}",
        format!("{line}\n").repeat(count)
    )
}

#[test]
fn every_kind_of_entry_stops_at_the_cap() {
    let chars = parse(&repeated(
        "begincidchar",
        "<00000001> 5",
        1_200_000,
        "endcidchar",
    ));
    assert_eq!(chars.cids.count(), 1_000_000);
    let ranges = parse(&repeated(
        "begincidrange",
        "<00000001> <00000002> 5",
        1_200_000,
        "endcidrange",
    ));
    assert_eq!(ranges.cids.count(), 1_000_000);
    let bf_chars = parse(&repeated(
        "beginbfchar",
        "<00000001> <0041>",
        1_200_000,
        "endbfchar",
    ));
    assert_eq!(bf_chars.texts.count(), 1_000_000);
    let bf_ranges = parse(&repeated(
        "beginbfrange",
        "<00000001> <00000002> <0041>",
        1_200_000,
        "endbfrange",
    ));
    assert_eq!(bf_ranges.texts.count(), 1_000_000);
    let bf_array = parse(&repeated(
        "beginbfrange\n<00000000> <FFFFFFFF> [",
        "<0041>",
        1_200_000,
        "] endbfrange",
    ));
    assert_eq!(bf_array.texts.count(), 1_000_000);
}

#[test]
fn the_cap_counts_notdefs_and_cids_together() {
    let text = repeated("beginnotdefchar", "<00000001> 5", 600_000, "endnotdefchar")
        + "\n1 begincidchar\n"
        + &"<00000002> 7\n".repeat(600_000)
        + "endcidchar";
    let map = parse(&text);
    assert_eq!(map.notdefs.count(), 600_000);
    assert_eq!(map.cids.count(), 400_000);
}

#[test]
fn codespaces_are_capped_at_100() {
    let mut text = String::new();
    for _ in 0..2 {
        text += &format!(
            "60 begincodespacerange\n{}endcodespacerange\n",
            "<00> <FF>\n".repeat(60)
        );
    }
    assert_eq!(parse(&text).space_list.len(), 100);
}

#[test]
fn four_byte_codes_wrap_where_they_overflow() {
    let ranges = parse("1 begincidrange <00000000> <7FFFFFFF> 2147483647 endcidrange");
    assert_eq!(ranges.cid_of(1, 4), Some(i64::from(i32::MIN)));
    assert_eq!(ranges.cid_of(0, 4), Some(i64::from(i32::MAX)));

    let high = parse(
        "1 begincodespacerange <00000000> <FFFFFFFF> endcodespacerange 2 beginbfchar <80000000> <0041> <FFFFFFFF> <0042> endbfchar",
    );
    assert_eq!(high.next_code(&[0x80, 0, 0, 0]), (i64::from(i32::MIN), 4));
    assert_eq!(
        high.unicode_of(i64::from(i32::MIN), 4).as_deref(),
        Some("A")
    );
    assert_eq!(high.next_code(&[0xFF, 0xFF, 0xFF, 0xFF]).0, -1);
    assert_eq!(high.unicode_of(-1, 4).as_deref(), Some("B"));
}

#[test]
fn a_bf_range_array_past_the_last_code_wraps() {
    let map = parse("1 beginbfrange <FFFFFFFE> <FFFFFFFF> [<0041> <0042> <0043>] endbfrange");
    assert_eq!(map.unicode_of(-2, 4).as_deref(), Some("A"));
    assert_eq!(map.unicode_of(-1, 4).as_deref(), Some("B"));
    assert_eq!(map.unicode_of(0, 4).as_deref(), Some("C"));
}

#[test]
fn lone_surrogate_halves_read_as_replacement_characters() {
    let map = parse(
        "1 begincodespacerange <00> <FF> endcodespacerange 2 beginbfchar <01> <D83D> <02> <DE00> endbfchar",
    );
    assert_eq!(map.unicode_of(1, 0).as_deref(), Some("\u{FFFD}"));
    assert_eq!(map.unicode_of(2, 0).as_deref(), Some("\u{FFFD}"));
}

#[test]
fn unknown_names_give_a_fallback_that_is_not_kept() {
    let first = CMap::predefined("No-Such-CMap-H");
    assert!(first.is_fallback);
    assert!(!Arc::ptr_eq(&first, &CMap::predefined("No-Such-CMap-H")));
    assert!(Arc::ptr_eq(
        &CMap::predefined("Identity-H"),
        &CMap::predefined("Identity-H")
    ));
}

#[test]
fn to_unicode_round_trips_through_the_parser() {
    let pairs: Vec<(i64, &str)> = vec![
        (0x0001, "A"),
        (0x0002, "B"),
        (0x0003, "C"),
        (0x0010, "fi"),
        (0x0020, "\u{1F600}"),
        (0x0021, "\u{1F601}"),
        (0x0100, "\u{FE}"),
        (0x0101, "\u{FF}"),
        (0x0102, "\u{100}"),
        (0xFFFF, "\u{4E2D}"),
    ];
    let map: Vec<(i64, Vec<u16>)> = pairs.iter().map(|(c, t)| (*c, u(t))).collect();
    let cmap = CMap::parse(&write_to_unicode(&map, 2).unwrap(), None);
    for (code, expected) in &pairs {
        assert_eq!(cmap.unicode_of(*code, 0).as_deref(), Some(*expected));
    }
    assert_eq!(cmap.unicode_of(4, 0), None);
    assert_eq!(cmap.next_code(&[0x00, 0x01]), (1, 2));
}

#[test]
fn to_unicode_runs_become_bf_ranges() {
    let pairs: Vec<(i64, Vec<u16>)> = (0..10).map(|i| (0x20 + i, vec![0x61 + i as u16])).collect();
    let cmap = String::from_utf8(write_to_unicode(&pairs, 2).unwrap()).unwrap();
    assert!(cmap.contains("1 beginbfrange\n<0020> <0029> <0061>\nendbfrange"));
    assert!(!cmap.contains("beginbfchar"));
}

#[test]
fn to_unicode_ranges_do_not_cross_a_low_byte_wrap() {
    let pairs: Vec<(i64, Vec<u16>)> = (0xFE..0xFE + 4).map(|c| (c, vec![c as u16])).collect();
    let data = write_to_unicode(&pairs, 2).unwrap();
    let map = CMap::parse(&data, None);
    for (code, text) in &pairs {
        assert_eq!(
            map.unicode_of(*code, 0),
            Some(String::from_utf16_lossy(text))
        );
    }
    assert!(!String::from_utf8(data).unwrap().contains("<00FE> <0101>"));
}

#[test]
fn to_unicode_sections_hold_at_most_100_entries() {
    let pairs: Vec<(i64, Vec<u16>)> = (0..250)
        .map(|i| (i * 2, vec![0x78, 0x4E00 + (i * 3) as u16]))
        .collect();
    let cmap = String::from_utf8(write_to_unicode(&pairs, 2).unwrap()).unwrap();
    assert!(cmap.contains("100 beginbfchar"));
    assert!(cmap.contains("50 beginbfchar"));
    let map = CMap::parse(cmap.as_bytes(), None);
    assert_eq!(
        map.unicode_of(498, 0),
        Some(String::from_utf16_lossy(&[0x78, 0x4E00 + 249 * 3]))
    );
}

#[test]
fn to_unicode_last_text_for_a_code_wins_and_empty_text_is_left_out() {
    let map = CMap::parse(
        &write_to_unicode(&[(5, u("a")), (5, u("b")), (6, u(""))], 2).unwrap(),
        None,
    );
    assert_eq!(map.unicode_of(5, 0).as_deref(), Some("b"));
    assert_eq!(map.unicode_of(6, 0), None);
}

#[test]
fn to_unicode_one_byte_codes_and_the_fixed_frame() {
    let data = write_to_unicode(&[(0x41, u("Z"))], 1).unwrap();
    assert!(String::from_utf8_lossy(&data).contains("<00> <FF>"));
    let map = CMap::parse(&data, None);
    assert_eq!(map.unicode_of(0x41, 0).as_deref(), Some("Z"));
    assert_eq!(length(&map, &[0x41, 0x42]), 1);

    let cmap = String::from_utf8(write_to_unicode(&[], 2).unwrap()).unwrap();
    assert!(cmap.starts_with("/CIDInit /ProcSet findresource begin"));
    assert!(cmap.contains("begincmap"));
    assert!(cmap.contains("1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange"));
    assert!(cmap.ends_with("end\nend\n"));
}

#[test]
fn to_unicode_rejects_what_does_not_fit() {
    assert!(write_to_unicode(&[(0x10000, u("a"))], 2).is_err());
    assert!(write_to_unicode(&[(-1, u("a"))], 2).is_err());
    assert!(write_to_unicode(&[], 5).is_err());
}

#[test]
fn lengths_past_the_mask_width_find_nothing() {
    let map = with_header(
        "1 begincodespacerange <00> <FF> endcodespacerange
1 beginbfchar <01> <0041> endbfchar
endcmap",
    );
    assert_eq!(map.unicode_of(1, 40), None);
    assert_eq!(map.cid_of(1, 40), None);
}
