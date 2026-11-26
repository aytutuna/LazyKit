// Lazy Scan's PatchCodeTests: the same pages and expectations.
mod common;

use common::pages::{self, Turn};
use lazy_scan::gray_image::GrayImage;
use lazy_scan::patch_code::{Bounds, PatchCode, detect, render_sheet};

const EXPECTED_PATCH_T: u32 = 0x1C4A_46D4;
const EXPECTED_PATCH_2_VERTICAL: u32 = 0x405A_20EB;
const EXPECTED_PATCH_T_PLUS_2_POINT_5: u32 = 0xC526_D510;
const EXPECTED_RULED_TABLE: u32 = 0xDC4E_ED7D;
const EXPECTED_SHEET_LETTER_100: u32 = 0xDA73_C7FA;

fn patch(code: PatchCode, vertical: bool, turn: Turn) -> GrayImage {
    pages::patch_page(code, vertical, turn, 300)
}

#[test]
fn codes_their_bars_and_labels() {
    let labels: Vec<_> = PatchCode::ALL.iter().map(|c| c.label()).collect();
    assert_eq!(
        labels,
        [
            "Patch 1", "Patch 2", "Patch 3", "Patch 4", "Patch 6", "Patch T"
        ]
    );
    let bars: Vec<String> = PatchCode::ALL
        .iter()
        .map(|c| {
            c.bars()
                .iter()
                .map(|&b| if b { '1' } else { '0' })
                .collect()
        })
        .collect();
    assert_eq!(bars, ["1100", "1001", "1010", "0110", "0011", "0101"]);
    for code in PatchCode::ALL {
        assert_eq!(PatchCode::from_bars(code.bars()), Some(code));
    }
    assert_eq!(PatchCode::from_bars([true, true, true, false]), None);
    assert_eq!(PatchCode::from_bars([false; 4]), None);
    // read from the other end: 1 and 6, 3 and T swap; 2 and 4 read the same
    let reversed: Vec<_> = PatchCode::ALL.iter().map(|c| c.reversed()).collect();
    assert_eq!(
        reversed,
        [
            PatchCode::Patch6,
            PatchCode::Patch2,
            PatchCode::PatchT,
            PatchCode::Patch4,
            PatchCode::Patch1,
            PatchCode::Patch3
        ]
    );
}

#[test]
fn fixtures_are_the_same_bytes_on_every_platform() {
    let made = [
        patch(PatchCode::PatchT, false, pages::STRAIGHT),
        patch(PatchCode::Patch2, true, pages::STRAIGHT),
        patch(PatchCode::PatchT, false, pages::PLUS_2_POINT_5),
        pages::ruled_table(),
        render_sheet(PatchCode::PatchT, 8.5, 11.0, 100).unwrap(),
    ];
    let sums: Vec<u32> = made.iter().map(pages::checksum).collect();
    assert_eq!(
        sums,
        [
            EXPECTED_PATCH_T,
            EXPECTED_PATCH_2_VERTICAL,
            EXPECTED_PATCH_T_PLUS_2_POINT_5,
            EXPECTED_RULED_TABLE,
            EXPECTED_SHEET_LETTER_100
        ]
    );
}

#[test]
fn every_code_across_and_down_the_page() {
    for code in PatchCode::ALL {
        let across = detect(&patch(code, false, pages::STRAIGHT)).unwrap();
        assert_eq!(across.code, code);
        assert!(!across.vertical_bars);
        // the bars: x 275 to 574; y from 200, 80 px deep (2 wide, 2 narrow, 3 gaps)
        assert_eq!(
            across.bounds,
            Bounds {
                x: 275,
                y: 200,
                width: 300,
                height: 80
            }
        );
        assert_eq!(across.length, 300);

        let down = detect(&patch(code, true, pages::STRAIGHT)).unwrap();
        assert_eq!(down.code, code);
        assert!(down.vertical_bars);
        assert_eq!(
            down.bounds,
            Bounds {
                x: 200,
                y: 400,
                width: 80,
                height: 300
            }
        );
    }
}

#[test]
fn a_slant_of_a_few_degrees_is_read() {
    for turn in [
        pages::PLUS_2_POINT_5,
        pages::MINUS_2_POINT_5,
        pages::PLUS_3,
        pages::MINUS_3,
    ] {
        for code in [PatchCode::PatchT, PatchCode::Patch2, PatchCode::Patch1] {
            for vertical in [false, true] {
                let hit = detect(&patch(code, vertical, turn))
                    .unwrap_or_else(|| panic!("{code:?} vertical {vertical}"));
                assert_eq!(hit.code, code);
                assert_eq!(hit.vertical_bars, vertical);
                assert!((280..=300).contains(&hit.length), "{}", hit.length);
            }
        }
    }
}

#[test]
fn text_tables_and_pages_without_a_code_are_not_read() {
    assert!(detect(&pages::text_page()).is_none());
    assert!(detect(&pages::turned(&pages::text_page(), pages::PLUS_3, 245)).is_none());
    assert!(detect(&pages::ruled_table()).is_none());
    assert!(detect(&pages::quarter_turned(&pages::ruled_table())).is_none());
    for (name, page) in pages::all() {
        assert!(detect(&page).is_none(), "{name}");
    }
    let sheet = pages::sheet_on_backdrop(pages::PLUS_2, 245, 250, 2, 0x5EE7_0001);
    assert!(detect(&sheet).is_none());
}

#[test]
fn bars_shorter_than_an_inch_and_a_half_are_not_a_code() {
    let short = pages::patch_page(PatchCode::PatchT, false, pages::STRAIGHT, 140);
    assert!(detect(&short).is_none());
    let long = pages::patch_page(PatchCode::PatchT, false, pages::STRAIGHT, 160);
    assert!(detect(&long).is_some());
}

#[test]
fn the_separator_sheet_reads_its_code_either_way_round_at_any_resolution() {
    for code in PatchCode::ALL {
        let sheet = render_sheet(code, 8.5, 11.0, 100).unwrap();
        assert_eq!((sheet.width(), sheet.height()), (850, 1100));
        let hit = detect(&sheet).unwrap();
        assert_eq!(hit.code, code);
        assert!(!hit.vertical_bars);
        // 6 in bars, centred, 1 in from the top
        let depth: usize = code
            .bars()
            .iter()
            .map(|&b| if b { 20 } else { 8 })
            .sum::<usize>()
            + 3 * 8;
        assert_eq!(
            hit.bounds,
            Bounds {
                x: 125,
                y: 100,
                width: 600,
                height: depth
            }
        );
        // fed the other way round, the copy near the other edge reads the same code
        assert_eq!(
            detect(&pages::half_turned(&sheet)).map(|h| h.code),
            Some(code)
        );
        // fed sideways, its bars run down the page
        assert_eq!(
            detect(&pages::quarter_turned(&sheet)).map(|h| h.vertical_bars),
            Some(true)
        );
    }
    // A4 at 300 dpi, read at the analysis resolution
    let a4 = render_sheet(PatchCode::PatchT, 8.27, 11.69, 300).unwrap();
    assert_eq!((a4.width(), a4.height()), (2481, 3507));
    assert_eq!(detect(&a4).map(|h| h.code), Some(PatchCode::PatchT));
}

#[test]
fn the_separator_sheet_has_the_standard_bar_widths() {
    let sheet = render_sheet(PatchCode::PatchT, 8.5, 11.0, 300).unwrap();
    // down the middle column from the top: white to 1 in, then narrow 24, gap 24, wide 60, gap 24, narrow 24, gap 24, wide
    // 60 (0.08 and 0.20 in at 300 dpi)
    let x = sheet.width() / 2;
    let mut runs: Vec<(bool, usize)> = Vec::new();
    for y in 0..600 {
        let ink = sheet.gray.data[y * sheet.width() + x] == 0;
        match runs.last_mut() {
            Some(last) if last.0 == ink => last.1 += 1,
            _ => runs.push((ink, 1)),
        }
    }
    let inks: Vec<bool> = runs.iter().map(|r| r.0).collect();
    let lengths: Vec<usize> = runs.iter().map(|r| r.1).collect();
    assert_eq!(
        inks,
        [false, true, false, true, false, true, false, true, false]
    );
    assert_eq!(lengths, [300, 24, 24, 60, 24, 24, 24, 60, 60]);
    // the words lie between the two codes, clear of their quiet zones (0.3 in)
    let row_has_ink =
        |y: usize| (0..sheet.width()).any(|x| sheet.gray.data[y * sheet.width() + x] == 0);
    assert!(!(540..630).any(row_has_ink));
    assert!(!(2670..2760).any(row_has_ink));
    assert!((1100..2200).any(row_has_ink));
}

#[test]
fn a_separator_sheet_too_big_to_hold_is_refused() {
    let t = PatchCode::PatchT;
    assert!(render_sheet(t, 1e12, 1e12, 600).is_none());
    assert!(render_sheet(t, 1e9, 1e9, u32::MAX).is_none());
    assert!(render_sheet(t, f64::INFINITY, 11.0, 300).is_none());
    assert!(render_sheet(t, 100.0, 100.0, 300).is_none());
    assert!(render_sheet(t, 8.5, 11.0, 300).is_some());
}
