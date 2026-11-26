// Lazy Scan's PageFlattenTests: curved text lines drawn straight, pages left alone, fingers painted over.
use lazy_image::Rgba;
use lazy_scan::page_flatten::{fill_fingers, flatten};

const WIDTH: usize = 600;
const HEIGHT: usize = 400;
const PAPER: [u8; 3] = [238, 234, 224];
const LINE_SHARES: [f64; 6] = [0.15, 0.3, 0.45, 0.6, 0.75, 0.9];

// 0 at the ends of the page, 1 at its middle (the spine)
fn bump(x: usize) -> f64 {
    let u = x as f64 / (WIDTH - 1) as f64;
    4.0 * u * (1.0 - u)
}

// a page on a dark table whose top and bottom edges dip `bend` pixels towards each other at the spine, with a line of text at
// every share of the way between its edges
fn curved_page(bend: f64) -> Rgba {
    let mut data = vec![255u8; WIDTH * HEIGHT * 4];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let top = bend * bump(x);
            let bottom = (HEIGHT - 1) as f64 - bend * bump(x);
            let mut color = [50, 45, 40];
            if y as f64 >= top && y as f64 <= bottom {
                color = PAPER;
                for share in LINE_SHARES {
                    if (y as f64 - (top + share * (bottom - top))).abs() < 1.5 {
                        color = [30, 30, 30];
                    }
                }
            }
            let i = (y * WIDTH + x) * 4;
            data[i..i + 3].copy_from_slice(&color);
        }
    }
    Rgba::new(WIDTH, HEIGHT, data)
}

// the row of the dark line near `expected` in column x, to a tenth of a pixel
fn line_row(image: &Rgba, x: usize, expected: f64) -> Option<f64> {
    let (mut sum, mut weight) = (0.0, 0.0);
    let from = (expected as usize).saturating_sub(12);
    let to = (image.height - 1).min(expected as usize + 12);
    for y in from..=to {
        let dark = 128.0 - image.data[(y * image.width + x) * 4] as f64;
        if dark > 0.0 {
            sum += dark * y as f64;
            weight += dark;
        }
    }
    (weight > 0.0).then(|| sum / weight)
}

#[test]
fn flatten_draws_curved_text_lines_straight() {
    let page = curved_page(30.0);
    let straight = 0.15 * (HEIGHT - 1) as f64;
    let bent = line_row(&page, WIDTH / 2, straight + 20.0).unwrap();
    assert!((bent - straight).abs() > 0.01 * HEIGHT as f64);

    let flat = flatten(&page).unwrap();
    assert_eq!((flat.width, flat.height), (WIDTH, HEIGHT));
    for share in LINE_SHARES {
        let expected = share * (HEIGHT - 1) as f64;
        for x in (20..WIDTH - 20).step_by(10) {
            let row = line_row(&flat, x, expected).unwrap_or_else(|| panic!("x {x}, line {share}"));
            assert!(
                (row - expected).abs() <= 0.01 * HEIGHT as f64,
                "x {x}, line {share}: {row}"
            );
        }
    }
}

#[test]
fn flatten_leaves_a_page_with_a_dark_banner_along_its_top_alone() {
    let mut page = curved_page(30.0);
    for y in 0..80 {
        for x in 0..WIDTH / 2 {
            let i = (y * WIDTH + x) * 4;
            page.data[i..i + 3].copy_from_slice(&[50, 45, 40]);
        }
    }
    assert!(flatten(&page).is_none());
}

#[test]
fn flatten_is_repeatable() {
    let page = curved_page(24.0);
    assert!(flatten(&page).is_some());
    assert_eq!(flatten(&page), flatten(&page));
}

#[test]
fn flatten_leaves_straight_and_edgeless_pages_alone() {
    assert!(flatten(&curved_page(0.0)).is_none());
    assert!(flatten(&curved_page(1.0)).is_none());
    assert!(flatten(&Rgba::filled(WIDTH, HEIGHT, PAPER[0], PAPER[1], PAPER[2])).is_none());
    assert!(flatten(&Rgba::filled(WIDTH, HEIGHT, 40, 40, 40)).is_none());
}

const DPI: f64 = 150.0;

fn paper_page() -> Rgba {
    let mut page = Rgba::filled(900, 600, PAPER[0], PAPER[1], PAPER[2]);
    for row in (150..450).step_by(30) {
        for y in row..row + 3 {
            for x in 250..650 {
                let i = (y * page.width + x) * 4;
                page.data[i..i + 3].fill(40);
            }
        }
    }
    page
}

fn paint(page: &mut Rgba, color: [u8; 3], inside: impl Fn(usize, usize) -> bool) {
    for y in 0..page.height {
        for x in 0..page.width {
            if inside(x, y) {
                let i = (y * page.width + x) * 4;
                page.data[i..i + 3].copy_from_slice(&color);
            }
        }
    }
}

fn pixel(image: &Rgba, x: usize, y: usize) -> [u8; 3] {
    let i = (y * image.width + x) * 4;
    [image.data[i], image.data[i + 1], image.data[i + 2]]
}

const SKIN: [u8; 3] = [214, 160, 130];

#[test]
fn fill_fingers_paints_a_thumb_at_the_margin_with_paper() {
    let mut page = paper_page();
    paint(&mut page, SKIN, |x, y| {
        let (dx, dy) = (x as f64 / 90.0, (y as f64 - 300.0) / 70.0);
        dx * dx + dy * dy <= 1.0
    });
    assert_eq!(pixel(&page, 20, 300), SKIN);
    let filled = fill_fingers(&page, DPI).unwrap();
    for (x, y) in [(0, 300), (20, 300), (60, 280), (85, 300), (40, 340)] {
        let p = pixel(&filled, x, y);
        for c in 0..3 {
            assert!((p[c] as i32 - PAPER[c] as i32).abs() <= 4, "{x}, {y}");
        }
    }
    // the text and the rest of the page are as they were
    assert_eq!(pixel(&filled, 300, 151), [40, 40, 40]);
    assert_eq!(pixel(&filled, 800, 100), pixel(&page, 800, 100));
    assert_eq!(pixel(&filled, 450, 20), pixel(&page, 450, 20));
}

#[test]
fn fill_fingers_leaves_pictures_that_run_past_the_band_and_plain_pages_alone() {
    assert!(fill_fingers(&paper_page(), DPI).is_none());
    let mut page = paper_page();
    paint(&mut page, SKIN, |x, y| x < 220 && (100..220).contains(&y));
    assert!(fill_fingers(&page, DPI).is_none());
    // a patch that does not touch the border is no finger
    let mut island = paper_page();
    paint(&mut island, SKIN, |x, y| {
        (30..90).contains(&x) && (270..330).contains(&y)
    });
    assert!(fill_fingers(&island, DPI).is_none());
}

#[test]
fn fill_fingers_ignores_paper_that_is_warm_all_over() {
    let mut page = Rgba::filled(900, 600, 225, 205, 160);
    paint(&mut page, [40, 40, 40], |x, y| {
        (300..600).contains(&x) && (200..203).contains(&y)
    });
    assert!(fill_fingers(&page, DPI).is_none());
}
