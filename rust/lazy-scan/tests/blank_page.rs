// Lazy Scan's BlankPageDetectorTests: the same pages and expectations.
mod common;

use common::pages::{self, HEIGHT, Rng, WIDTH};
use lazy_image::Rgba;
use lazy_scan::blank_page::{
    BlankPageResult, Options, Sensitivity, analyze, analyze_rgba, threshold,
};
use lazy_scan::gray_image::GrayImage;

const LEVELS: [Sensitivity; 3] = [Sensitivity::Low, Sensitivity::Medium, Sensitivity::High];

fn options(sensitivity: Sensitivity) -> Options {
    Options {
        sensitivity,
        ..Default::default()
    }
}

fn check(page: &GrayImage, sensitivity: Sensitivity) -> BlankPageResult {
    analyze(page, None, &options(sensitivity))
}

fn medium(page: &GrayImage) -> BlankPageResult {
    check(page, Sensitivity::Medium)
}

#[test]
fn the_fixtures_are_the_same_bytes_on_every_platform() {
    let sums: Vec<u32> = pages::all()
        .iter()
        .map(|(_, p)| pages::checksum(p))
        .collect();
    assert_eq!(
        sums,
        [
            0xB28D_863D,
            0xC790_1B30,
            0x41EC_9527,
            0x7F42_A318,
            0xD7C4_DB8C,
            0xC37B_1A14,
            0x0E42_C008,
            0xC739_A64F
        ]
    );
}

#[test]
fn blank_pages_as_scanners_deliver_them_are_blank_at_every_level() {
    for page in [
        pages::clean_blank(),
        pages::noisy_blank(),
        pages::show_through(),
    ] {
        for level in LEVELS {
            let result = check(&page, level);
            assert!(
                result.is_blank,
                "{level:?}: coverage {}",
                result.ink_coverage
            );
            assert!(!result.has_large_dark_area);
        }
    }
}

#[test]
fn clean_paper_has_no_ink_at_all() {
    let result = medium(&pages::clean_blank());
    assert_eq!((result.ink_coverage, result.ink_components), (0.0, 0));
}

#[test]
fn dust_noise_shadows_holes_and_show_through_add_no_ink() {
    assert!(medium(&pages::noisy_blank()).ink_coverage < 0.00002);
    assert_eq!(medium(&pages::show_through()).ink_coverage, 0.0);
}

#[test]
fn pages_with_little_on_them_are_never_blank() {
    for page in [
        pages::one_text_line(),
        pages::signature(),
        pages::photo(),
        pages::tinted_with_text(),
    ] {
        for level in LEVELS {
            let result = check(&page, level);
            assert!(
                !result.is_blank,
                "{level:?}: coverage {}",
                result.ink_coverage
            );
        }
    }
}

#[test]
fn one_text_line_and_a_signature_cover_more_than_the_highest_threshold() {
    for page in [pages::one_text_line(), pages::signature()] {
        let coverage = medium(&page).ink_coverage;
        assert!((0.0015..=0.004).contains(&coverage), "{coverage}");
    }
    assert!(medium(&pages::one_text_line()).ink_components > 50);
}

#[test]
fn a_lone_page_number_is_blank_at_medium_and_high_but_not_at_low() {
    let page = pages::page_number();
    let result = medium(&page);
    assert!(result.is_blank);
    assert!((0.00008..=0.00024).contains(&result.ink_coverage));
    assert_eq!(result.ink_components, 2);
    assert!(!check(&page, Sensitivity::Low).is_blank);
    assert!(check(&page, Sensitivity::High).is_blank);
}

#[test]
fn a_thin_slanted_pencil_note_is_one_mark_per_stroke_not_dust() {
    // four zigzag lines one pixel wide at 55 % of the paper: each step is diagonal, so the strokes only hold together
    // 8-connected
    let page = pages::noisy(0x009E_9C11, 245, |p| {
        for line in 0..4 {
            for x in 0..250 {
                let y = 300 + line * 30 + if x / 6 % 2 == 0 { x % 6 } else { 6 - x % 6 };
                p[y * WIDTH + 200 + x] = (245 * 55 / 100) as u8;
            }
        }
    });
    for level in LEVELS {
        let result = check(&page, level);
        assert!(
            !result.is_blank,
            "{level:?}: coverage {}",
            result.ink_coverage
        );
        assert_eq!(result.ink_components, 4);
    }
}

#[test]
fn a_photo_is_a_large_dark_area() {
    let result = check(&pages::photo(), Sensitivity::High);
    assert!(result.has_large_dark_area && !result.is_blank);
    assert!(!result.is_blank_at(Sensitivity::High));
}

#[test]
fn tinted_paper_is_paper_and_its_text_is_ink() {
    let result = medium(&pages::tinted_with_text());
    assert!(!result.has_large_dark_area);
    assert!(result.ink_coverage > 0.005);
    assert!(medium(&pages::noisy(0x8181_818A, 217, |_| {})).is_blank);
}

#[test]
fn is_blank_at_agrees_with_the_thresholds() {
    assert!(threshold(Sensitivity::Low) < threshold(Sensitivity::Medium));
    assert!(threshold(Sensitivity::Medium) < threshold(Sensitivity::High));
    assert_eq!(threshold(Sensitivity::Low), 0.00008);
    assert_eq!(threshold(Sensitivity::Medium), 0.00024);
    assert_eq!(threshold(Sensitivity::High), 0.0008);
    for (_, page) in pages::all() {
        for level in LEVELS {
            let result = check(&page, level);
            assert_eq!(result.is_blank, result.is_blank_at(level));
        }
    }
}

fn colored(
    page: &GrayImage,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    c: [u8; 3],
    factor: usize,
) -> Rgba {
    let mut scan = pages::enlarged(page, factor);
    for py in y * factor..(y + h) * factor {
        for px in x * factor..(x + w) * factor {
            let i = (py * scan.width + px) * 4;
            scan.data[i..i + 3].copy_from_slice(&c);
        }
    }
    scan
}

#[test]
fn a_yellow_highlighter_line_is_ink_though_as_light_as_paper() {
    let scan = colored(&pages::noisy_blank(), 200, 500, 450, 15, [255, 245, 100], 1);
    for level in LEVELS {
        let result = analyze_rgba(&scan, 100, &options(level));
        assert!(
            !result.is_blank,
            "{level:?}: coverage {}",
            result.ink_coverage
        );
    }
    assert!(medium(&GrayImage::from_rgba(&scan, 100)).is_blank);
}

#[test]
fn a_pale_printed_stamp_is_ink_though_as_light_as_paper() {
    let scan = colored(&pages::clean_blank(), 300, 600, 150, 60, [200, 215, 245], 1);
    for level in LEVELS {
        let result = analyze_rgba(&scan, 100, &options(level));
        assert!(
            !result.is_blank,
            "{level:?}: coverage {}",
            result.ink_coverage
        );
    }
    assert!(medium(&GrayImage::from_rgba(&scan, 100)).is_blank);
}

#[test]
fn paper_that_is_colored_all_over_is_still_paper() {
    let scan = colored(
        &pages::noisy_blank(),
        0,
        0,
        WIDTH,
        HEIGHT,
        [250, 245, 150],
        1,
    );
    assert!(analyze_rgba(&scan, 100, &Options::default()).is_blank);
}

#[test]
fn a_note_along_the_bottom_edge_040_inches_in_is_not_blank() {
    let page = pages::noisy(0x0B07_E5E5, 245, |p| {
        pages::text_line(p, 100, 1050, 650, 50, &mut Rng::new(0x7E7E_0004));
    });
    for level in LEVELS {
        let result = check(&page, level);
        assert!(
            !result.is_blank,
            "{level:?}: coverage {}",
            result.ink_coverage
        );
    }
}

#[test]
fn the_margin_keeps_edge_shadows_and_holes_out() {
    let no_margin = analyze(
        &pages::noisy_blank(),
        None,
        &Options {
            margin_inches: 0.0,
            ..Default::default()
        },
    );
    assert!(!no_margin.is_blank);
    assert!(no_margin.ink_coverage > 0.01);
}

#[test]
fn a_higher_resolution_scan_gives_the_same_result() {
    for page in [
        pages::noisy_blank(),
        pages::page_number(),
        pages::one_text_line(),
    ] {
        let scan = pages::enlarged(&page, 3);
        let gray = GrayImage::from_rgba(&scan, 300);
        assert_eq!(gray.dpi, 100.0);
        assert_eq!((gray.width(), gray.height()), (page.width(), page.height()));
        assert_eq!(gray.gray.data, page.gray.data);
        assert_eq!(medium(&page), analyze_rgba(&scan, 300, &Options::default()));
    }
}

#[test]
fn a_low_resolution_scan_is_analyzed_at_its_own_resolution() {
    // 50 dpi: half the pixels; marks and tiles scale with it
    let blank = GrayImage::new(
        lazy_image::Gray {
            width: 425,
            height: 550,
            data: vec![240; 425 * 550],
        },
        50.0,
    );
    assert!(medium(&blank).is_blank);
    let text = pages::one_text_line().downsampled(425, 550);
    assert_eq!(text.dpi, 50.0);
    assert!(!medium(&text).is_blank);
}

#[test]
fn a_small_card_uses_a_share_of_its_short_side_as_margin() {
    let card = GrayImage::filled(350, 200, 245);
    assert!(medium(&card).is_blank);
    let mut data = card.gray.data.clone();
    for x in 60..290 {
        for y in 95..105 {
            data[y * 350 + x] = if (x / 4) % 2 == 0 { 40 } else { 245 };
        }
    }
    let marked = GrayImage::new(
        lazy_image::Gray {
            width: 350,
            height: 200,
            data,
        },
        100.0,
    );
    assert!(!medium(&marked).is_blank);
}

#[test]
fn a_tiny_image_is_not_blank() {
    assert!(!medium(&GrayImage::filled(3, 3, 255)).is_blank);
    assert!(!medium(&GrayImage::filled(1, 1, 255)).is_blank);
}

#[test]
fn a_dark_band_across_the_page_is_never_blank() {
    // a scanner black backing showing past a short sheet
    let page = pages::noisy(0x00BA_DBAD, 245, |p| {
        pages::fill(p, 0, 900, WIDTH as i64, 200, 20)
    });
    let result = check(&page, Sensitivity::High);
    assert!(result.has_large_dark_area && !result.is_blank);
}
