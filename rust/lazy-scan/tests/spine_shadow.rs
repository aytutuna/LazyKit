// Lazy Scan's SpineShadowTests: softening the shadow along the spine on a page cut from a spread, on the same synthetic book
// pages (checked by checksum), with the same softened bytes.
mod common;

use common::book::{
    Canvas, book_page, picture_book_page, shadowed_book_page, tint_book_page, unshadowed_book_page,
    unshadowed_picture_book_page,
};
use common::pages::{self, Rng};
use lazy_image::Rgba;
use lazy_scan::gray_image::GrayImage;
use lazy_scan::spine_shadow::{MAX_GAIN, PageEdge, estimate, soften};

// the page is 550 px wide; the shadow reaches 80 px from its right edge
const SHADOW_ZONE: usize = 80;

fn in_picture(x: usize, y: usize) -> bool {
    (400..530).contains(&x) && (300..520).contains(&y)
}

fn in_tint(x: usize, y: usize) -> bool {
    x >= 430 && (640..740).contains(&y)
}

fn nowhere(_: usize, _: usize) -> bool {
    false
}

fn assert_in(value: f64, low: f64, high: f64) {
    assert!(
        (low..=high).contains(&value),
        "{value} not in {low}..{high}"
    );
}

fn luma(image: &Rgba) -> GrayImage {
    GrayImage::from_luma(image, 100.0)
}

fn to_rgba(page: &GrayImage) -> Rgba {
    pages::enlarged(page, 1)
}

// what softening did to the paper and the ink within SHADOW_ZONE of the right edge, against the page with no shadow
struct Zone {
    darkening_before: f64,
    darkening_after: f64,
    worst_column: f64,
    ink_contrast_clean: f64,
    ink_contrast_after: f64,
    ink_clean: f64,
    ink_after: f64,
}

fn measure_zone(
    clean: &GrayImage,
    shadowed: &GrayImage,
    softened: &GrayImage,
    excluded: impl Fn(usize, usize) -> bool,
) -> Zone {
    let w = clean.width();
    let (mut paper_clean, mut paper_shadow, mut paper_soft, mut ink_clean, mut ink_soft) =
        (0.0, 0.0, 0.0, 0.0, 0.0);
    let (mut paper, mut ink) = (0usize, 0usize);
    let mut column_clean = [0f64; SHADOW_ZONE];
    let mut column_soft = [0f64; SHADOW_ZONE];
    for y in 0..clean.height() {
        for d in 0..SHADOW_ZONE {
            let x = w - 1 - d;
            if excluded(x, y) {
                continue;
            }
            let i = y * w + x;
            let c = clean.gray.data[i] as i32;
            if c >= 200 {
                paper_clean += c as f64;
                paper_shadow += shadowed.gray.data[i] as f64;
                paper_soft += softened.gray.data[i] as f64;
                column_clean[d] += c as f64;
                column_soft[d] += softened.gray.data[i] as f64;
                paper += 1;
            } else if c <= 60 {
                ink_clean += c as f64;
                ink_soft += softened.gray.data[i] as f64;
                ink += 1;
            }
        }
    }
    let mut worst = 0f64;
    for d in 0..SHADOW_ZONE {
        if column_clean[d] > 0.0 {
            worst = worst.max((1.0 - column_soft[d] / column_clean[d]).abs());
        }
    }
    let (pc, ps) = (paper_clean / paper as f64, paper_soft / paper as f64);
    let (ic, is) = (ink_clean / ink as f64, ink_soft / ink as f64);
    Zone {
        darkening_before: 1.0 - paper_shadow / paper_clean,
        darkening_after: 1.0 - paper_soft / paper_clean,
        worst_column: worst,
        ink_contrast_clean: (pc - ic) / pc,
        ink_contrast_after: (ps - is) / ps,
        ink_clean: ic,
        ink_after: is,
    }
}

// the largest change of a pixel where `area` holds
fn largest_change(
    before: &GrayImage,
    after: &GrayImage,
    area: impl Fn(usize, usize) -> bool,
) -> i32 {
    let mut largest = 0;
    for y in 0..before.height() {
        for x in 0..before.width() {
            if area(x, y) {
                let i = y * before.width() + x;
                largest =
                    largest.max((after.gray.data[i] as i32 - before.gray.data[i] as i32).abs());
            }
        }
    }
    largest
}

// the largest change of a pixel where `area` holds (in the page's pixels) when the page is scanned at 300 dpi, its content
// `shift` px further right and down (off the 100 dpi grid)
fn largest_change_at_300_dpi(
    page: &GrayImage,
    area: impl Fn(usize, usize) -> bool,
    shift: usize,
) -> i32 {
    let scan = shifted(&pages::enlarged(page, 3), shift);
    let result = soften(&scan, 300, PageEdge::Right).expect("not softened at 300 dpi");
    largest_change(&luma(&scan), &luma(&result.image), |x, y| {
        x >= shift && y >= shift && area((x - shift) / 3, (y - shift) / 3)
    })
}

// the image content `shift` px further right and down, its first column and row repeated
fn shifted(image: &Rgba, shift: usize) -> Rgba {
    let (w, h) = (image.width, image.height);
    let mut p = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let from = (y.saturating_sub(shift) * w + x.saturating_sub(shift)) * 4;
            let to = (y * w + x) * 4;
            p[to..to + 4].copy_from_slice(&image.data[from..from + 4]);
        }
    }
    Rgba::new(w, h, p)
}

// a book page with a flat mid-tone picture (150) where `in_picture` holds, in a shadow 0.8 in wide and 40 % deep
fn flat_picture_page() -> GrayImage {
    let mut c = Canvas::new(550, 850, 245, 0xB00C_002A);
    c.text_block(60, 60, 460, 36, 20, 40, &mut Rng::new(0xB00C_002B));
    c.fill_rect(400, 300, 130, 220, 150);
    c.spine_shadow(80, 400, 400);
    c.gray()
}

// a text page with a flat bar `width` px wide printed all along its right edge, and no shadow
fn bar_page(width: i64, tint: u8, ramp: i64) -> GrayImage {
    let mut c = Canvas::new(550, 850, 245, 0xB00C_0030);
    c.text_block(60, 60, 430, 36, 20, 40, &mut Rng::new(0xB00C_0031));
    c.fill_rect(550 - width, 0, width, 850, tint);
    // a scanned bar edge is a ramp of a few pixels to the paper
    for k in 1..=ramp {
        let value = tint as i64 + (245 - tint as i64) * k / (ramp + 1);
        c.fill_rect(550 - width - k, 0, 1, 850, value as u8);
    }
    c.gray()
}

fn mirrored(image: &Rgba) -> Rgba {
    image.mirrored()
}

// the image transposed: pixel (x, y) moves to (y, x)
fn transposed(image: &Rgba) -> Rgba {
    let (w, h) = (image.width, image.height);
    let mut p = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (from, to) = ((y * w + x) * 4, (x * h + y) * 4);
            p[to..to + 4].copy_from_slice(&image.data[from..from + 4]);
        }
    }
    Rgba::new(h, w, p)
}

fn softened(page: &GrayImage) -> GrayImage {
    luma(&soften(&to_rgba(page), 100, PageEdge::Right).unwrap().image)
}

#[test]
fn book_pages_are_the_same_bytes_on_every_platform() {
    assert_eq!(pages::checksum(&shadowed_book_page()), 0xFBE5_8022);
    assert_eq!(pages::checksum(&unshadowed_book_page()), 0x1305_BF67);
    assert_eq!(pages::checksum(&picture_book_page()), 0xDFE3_62B3);
    assert_eq!(
        pages::checksum(&unshadowed_picture_book_page()),
        0xE9AD_3B0B
    );
    assert_eq!(pages::checksum(&tint_book_page()), 0xE58F_1808);
}

#[test]
fn softened_pages_are_the_same_bytes_on_every_platform() {
    assert_eq!(
        pages::checksum(&softened(&shadowed_book_page())),
        0xC2E3_6A2C
    );
    assert_eq!(
        pages::checksum(&softened(&picture_book_page())),
        0xF553_8C1C
    );
    assert_eq!(pages::checksum(&softened(&tint_book_page())), 0xDCF2_6252);
    let scan = soften(
        &pages::enlarged(&shadowed_book_page(), 3),
        300,
        PageEdge::Right,
    )
    .unwrap();
    assert_eq!(pages::checksum(&luma(&scan.image)), 0x5592_BA21);
    let picture = soften(
        &pages::enlarged(&picture_book_page(), 3),
        300,
        PageEdge::Right,
    )
    .unwrap();
    assert_eq!(pages::checksum(&luma(&picture.image)), 0xC77A_A721);
}

#[test]
fn the_edge_turns_with_the_page() {
    let turned: Vec<_> = (0..4).map(|q| PageEdge::Top.turned(q)).collect();
    assert_eq!(
        turned,
        [
            PageEdge::Top,
            PageEdge::Right,
            PageEdge::Bottom,
            PageEdge::Left
        ]
    );
    assert_eq!(PageEdge::Left.turned(7), PageEdge::Bottom);
    assert_eq!(PageEdge::Left.turned(-1), PageEdge::Bottom);
}

#[test]
fn a_shadowed_page_has_its_paper_back_to_white_and_text_keeps_its_contrast() {
    let shadowed = shadowed_book_page();
    let result = soften(&to_rgba(&shadowed), 100, PageEdge::Right).unwrap();
    let zone = measure_zone(
        &unshadowed_book_page(),
        &shadowed,
        &luma(&result.image),
        nowhere,
    );
    // the paper within 0.8 in of the spine was 13 % darker on average (45 % at the edge, top)
    assert_in(zone.darkening_before, 0.12, 0.15);
    assert_in(zone.darkening_after, -0.01, 0.03);
    assert!(zone.worst_column < 0.05);
    // ink keeps its share of the paper's light: the text is as dark against the paper as without the shadow
    assert_in(
        zone.ink_contrast_after / zone.ink_contrast_clean,
        0.98,
        1.02,
    );
    assert_in(zone.ink_after, zone.ink_clean - 5.0, zone.ink_clean + 5.0);
    let r = result.result;
    assert_eq!(r.edge, PageEdge::Right);
    // the shadow most of the edge agrees on: 30 to 45 % deep at the edge, faded within 0.8 in
    assert_in(r.depth, 0.25, 0.45);
    assert_in(r.width_inches, 0.4, 0.8);
    assert_in(r.max_gain, 1.4, MAX_GAIN);
    assert_eq!(r.protected_fraction, 0.0);
}

#[test]
fn a_shadowed_page_scanned_at_300_dpi_is_softened_as_at_100() {
    let scan = pages::enlarged(&shadowed_book_page(), 3);
    let result = soften(&scan, 300, PageEdge::Right).unwrap();
    let softened_page = luma(&result.image).downsampled(550, 850);
    let zone = measure_zone(
        &unshadowed_book_page(),
        &shadowed_book_page(),
        &softened_page,
        nowhere,
    );
    assert_in(zone.darkening_after, -0.01, 0.03);
    assert_in(
        zone.ink_contrast_after / zone.ink_contrast_clean,
        0.98,
        1.02,
    );
    assert_eq!(result.image.width, 1650);
    // alpha is kept, and nothing past the 1.5 in band changes
    assert!(result.image.data.chunks(4).all(|p| p[3] == 255));
    let mut changed = 0;
    for y in (0..scan.height).step_by(7) {
        for x in 0..scan.width - 3 * 150 {
            if scan.data[(y * scan.width + x) * 4] != result.image.data[(y * scan.width + x) * 4] {
                changed += 1;
            }
        }
    }
    assert_eq!(changed, 0);
}

#[test]
fn every_edge_gives_the_same_page() {
    let page = to_rgba(&shadowed_book_page());
    let right = soften(&page, 100, PageEdge::Right).unwrap().image;
    let left = soften(&mirrored(&page), 100, PageEdge::Left).unwrap().image;
    let bottom = soften(&transposed(&page), 100, PageEdge::Bottom)
        .unwrap()
        .image;
    let top = soften(&transposed(&mirrored(&page)), 100, PageEdge::Top)
        .unwrap()
        .image;
    assert_eq!(right, mirrored(&left));
    assert_eq!(right, transposed(&bottom));
    assert_eq!(right, mirrored(&transposed(&top)));
}

#[test]
fn colour_pages_are_lifted_evenly_keeping_the_papers_tint() {
    // cream paper: every channel is brightened by the same factor, so the tint stays
    let gray = shadowed_book_page();
    let mut p = vec![0u8; gray.width() * gray.height() * 4];
    for (i, &v) in gray.gray.data.iter().enumerate() {
        let v = v as usize;
        p[i * 4] = v as u8;
        p[i * 4 + 1] = (v * 96 / 100) as u8;
        p[i * 4 + 2] = (v * 82 / 100) as u8;
        p[i * 4 + 3] = 255;
    }
    let color = Rgba::new(gray.width(), gray.height(), p.clone());
    let result = soften(&color, 100, PageEdge::Right).unwrap();
    // paper at the edge, in the middle of the page (between two lines of text): red, green and blue in the cream's proportions
    let at = (430 * gray.width() + gray.width() - 3) * 4;
    let o = &result.image.data;
    assert!(o[at] as i32 > p[at] as i32 + 40);
    assert_in(o[at + 1] as f64 / o[at] as f64, 0.93, 0.99);
    assert_in(o[at + 2] as f64 / o[at] as f64, 0.79, 0.85);
}

#[test]
fn pictures_beside_the_spine_are_left_alone() {
    let shadowed = picture_book_page();
    let result = soften(&to_rgba(&shadowed), 100, PageEdge::Right).unwrap();
    let softened_page = luma(&result.image);
    // the picture keeps its pixels (and the shadow on it): its tones are not washed out
    assert_eq!(largest_change(&shadowed, &softened_page, in_picture), 0);
    // the paper around it, and the text, are softened as on a page without a picture
    let zone = measure_zone(
        &unshadowed_picture_book_page(),
        &shadowed,
        &softened_page,
        in_picture,
    );
    assert_in(zone.darkening_before, 0.13, 0.17);
    assert_in(zone.darkening_after, -0.01, 0.03);
    assert_in(
        zone.ink_contrast_after / zone.ink_contrast_clean,
        0.98,
        1.02,
    );
    // the picture is the band's protected part: 1.3 x 2.2 in of 1.5 x 8.5 in
    assert_in(result.result.protected_fraction, 0.2, 0.25);
    // scanned at 300 dpi, the picture keeps every pixel too, to its outermost rows and columns, and the paper around it is
    // softened as at 100 dpi
    assert_eq!(largest_change_at_300_dpi(&shadowed, in_picture, 0), 0);
    let scan = soften(&pages::enlarged(&shadowed, 3), 300, PageEdge::Right).unwrap();
    let down = luma(&scan.image).downsampled(550, 850);
    let zone300 = measure_zone(
        &unshadowed_picture_book_page(),
        &shadowed,
        &down,
        in_picture,
    );
    assert_in(zone300.darkening_after, -0.01, 0.03);
    assert_in(
        zone300.ink_contrast_after / zone300.ink_contrast_clean,
        0.98,
        1.02,
    );
}

#[test]
fn a_picture_off_the_analysis_grid_keeps_every_pixel() {
    // at 300 dpi a picture's edges need not fall on the 100 dpi grid the shadow is measured on: an analysis pixel only partly
    // on a flat mid-tone picture is lighter than the picture, and left out of its box; the picture's outermost rows and
    // columns in that pixel keep their values all the same
    let page = flat_picture_page();
    let result = soften(&to_rgba(&page), 100, PageEdge::Right).unwrap();
    assert_eq!(largest_change(&page, &luma(&result.image), in_picture), 0);
    for shift in [0, 1, 2] {
        assert_eq!(
            largest_change_at_300_dpi(&page, in_picture, shift),
            0,
            "flat picture, shift {shift}"
        );
        assert_eq!(
            largest_change_at_300_dpi(&picture_book_page(), in_picture, shift),
            0,
            "picture, shift {shift}"
        );
    }
}

#[test]
fn a_flat_tint_reaching_the_spine_is_left_alone() {
    let shadowed = tint_book_page();
    let result = soften(&to_rgba(&shadowed), 100, PageEdge::Right).unwrap();
    let softened_page = luma(&result.image);
    // a flat gray band reaching the spine looks like shadowed paper close up, but not beside the blocks around it
    assert_eq!(largest_change(&shadowed, &softened_page, in_tint), 0);
    let clean = book_page(0, 0, 80, 0xB00C_0022, false, true);
    let zone = measure_zone(&clean, &shadowed, &softened_page, in_tint);
    assert_in(zone.darkening_after, -0.01, 0.03);
    assert!(result.result.protected_fraction > 0.08);
    assert_eq!(largest_change_at_300_dpi(&shadowed, in_tint, 0), 0);
}

#[test]
fn a_bar_printed_all_along_the_spine_is_no_shadow() {
    // a tint, a thumb index or a bleed running the whole length of the spine darkens the paper there in every block, but comes
    // back to the paper in a step: it is left as printed
    for width in [10, 20] {
        for tint in [200u8, 160, 120] {
            for ramp in [0, 2] {
                let page = bar_page(width, tint, ramp);
                assert!(
                    estimate(&page, PageEdge::Right).is_none(),
                    "bar {width} px, {tint}, ramp {ramp}"
                );
                let big = pages::enlarged(&page, 3);
                assert!(
                    soften(&big, 300, PageEdge::Right).is_none(),
                    "bar {width} px, {tint}, ramp {ramp}, 300 dpi"
                );
            }
        }
    }
    // a shadow as narrow and faint (0.2 in, 10 % deep) fades gradually, and is softened
    let faint = book_page(100, 100, 20, 0xB00C_0029, false, false);
    assert!(soften(&to_rgba(&faint), 100, PageEdge::Right).is_some());
}

#[test]
fn pages_without_a_shadow_are_left_alone() {
    let plain = to_rgba(&unshadowed_book_page());
    assert!(soften(&plain, 100, PageEdge::Right).is_none());
    assert!(
        soften(
            &to_rgba(&unshadowed_picture_book_page()),
            100,
            PageEdge::Right
        )
        .is_none()
    );
    // the shadow is on the right: the left edge has none
    assert!(soften(&to_rgba(&shadowed_book_page()), 100, PageEdge::Left).is_none());
    assert!(estimate(&shadowed_book_page(), PageEdge::Top).is_none());
    // a shadow too faint to matter (3 %), a dark page, a page too narrow for a band
    let faint = book_page(30, 30, 80, 0xB00C_0023, false, false);
    assert!(soften(&to_rgba(&faint), 100, PageEdge::Right).is_none());
    assert!(soften(&Rgba::filled(550, 850, 60, 60, 60), 100, PageEdge::Right).is_none());
    assert!(soften(&Rgba::filled(40, 850, 200, 200, 200), 100, PageEdge::Right).is_none());
}

#[test]
fn dark_areas_all_across_the_band_are_no_shadow() {
    // a dark picture running the whole length of the edge, wider than the band: nothing fades, nothing is softened
    let mut c = Canvas::new(550, 850, 245, 0xB00C_0024);
    c.text_block(60, 60, 300, 36, 20, 40, &mut Rng::new(0xB00C_0025));
    c.picture(380, 0, 170, 850);
    assert!(soften(&to_rgba(&c.gray()), 100, PageEdge::Right).is_none());
    // a dark stripe 0.5 in from the edge, the paper at the edge as bright as elsewhere: not the spine shadow
    let mut stripe = Canvas::new(550, 850, 245, 0xB00C_0026);
    stripe.text_block(60, 60, 400, 36, 20, 40, &mut Rng::new(0xB00C_0027));
    stripe.fill_rect(470, 0, 30, 850, 150);
    assert!(soften(&to_rgba(&stripe.gray()), 100, PageEdge::Right).is_none());
}

#[test]
fn gains_are_capped() {
    // a fold 90 % dark at the edge: the paper there is brightened at most MAX_GAIN times
    let deep = book_page(900, 900, 60, 0xB00C_0028, false, false);
    let model = estimate(&deep, PageEdge::Right).unwrap();
    assert_eq!(model.result.max_gain, MAX_GAIN);
    assert!(model.gains.iter().all(|&g| (1.0..=MAX_GAIN).contains(&g)));
    assert_eq!([model.band, model.length, model.extent], [150, 850, 550]);
    assert_eq!(model.gains.len(), model.pictures.len());
}
