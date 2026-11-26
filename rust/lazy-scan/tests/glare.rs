// Lazy Scan's GlareDetectorTests: the same made-up cards, checksums and readings.
use lazy_image::Rgba;
use lazy_scan::{GlareReading, Quad, glare_has_glare, glare_measure};

const WIDTH: usize = 428;
const HEIGHT: usize = 270;

fn card(base: [i64; 3], glare: bool, specks: bool, edge: bool) -> Rgba {
    let mut data = vec![0u8; WIDTH * HEIGHT * 4];
    let (rx, ry, cx, cy) = (70i64, 40i64, 300i64, 120i64);
    let r4 = 4 * rx * rx * ry * ry;
    for y in 0..HEIGHT as i64 {
        for x in 0..WIDTH as i64 {
            let mut c = base;
            if (24..124).contains(&x) && (60..200).contains(&y) {
                c = [150, 130, 120];
            }
            for i in 0..6 {
                let top = 70 + 24 * i;
                if (150..400).contains(&x) && (top..top + 8).contains(&y) {
                    c = [40, 44, 52];
                }
            }
            if glare {
                let (dx, dy) = (2 * x + 1 - 2 * cx, 2 * y + 1 - 2 * cy);
                let d = dx * dx * ry * ry + dy * dy * rx * rx;
                if d <= r4 {
                    c = [255; 3];
                } else if d <= 2 * r4 {
                    c = c.map(|v| (v + 256) / 2);
                }
            }
            let speck_row = matches!(y, 20 | 21 | 240 | 241);
            if specks && speck_row && x >= 10 && (x - 10) % 30 < 2 {
                c = [255; 3];
            }
            if edge && x < 6 {
                c = [255; 3];
            }
            let p = (y as usize * WIDTH + x as usize) * 4;
            data[p] = c[0] as u8;
            data[p + 1] = c[1] as u8;
            data[p + 2] = c[2] as u8;
            data[p + 3] = 255;
        }
    }
    Rgba::new(WIDTH, HEIGHT, data)
}

fn plain() -> Rgba {
    card([214, 220, 226], false, false, false)
}

fn glared() -> Rgba {
    card([214, 220, 226], true, false, false)
}

fn fnv(image: &Rgba) -> u32 {
    image.data.iter().fold(0x811C_9DC5u32, |h, &v| {
        (h ^ v as u32).wrapping_mul(0x0100_0193)
    })
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Quad {
    Quad::from_xy([[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-12, "{a} vs {b}");
}

#[test]
fn the_cards_are_the_same_on_every_platform() {
    assert_eq!(fnv(&plain()), 0x0CE7_17C5);
    assert_eq!(fnv(&glared()), 0x1765_7405);
    assert_eq!(fnv(&card([214, 220, 226], false, true, false)), 0x6BF0_9B45);
    assert_eq!(fnv(&card([214, 220, 226], false, false, true)), 0xD428_B625);
    assert_eq!(
        fnv(&card([236, 238, 240], false, false, false)),
        0xC076_1E05
    );
    assert_eq!(fnv(&card([214, 220, 226], true, true, true)), 0xF39E_E005);
}

#[test]
fn a_card_without_a_reflection_has_no_glare() {
    assert_eq!(glare_measure(&plain(), None), GlareReading::default());
    assert!(!glare_has_glare(&plain(), None));
    let light = card([236, 238, 240], false, false, false);
    assert_eq!(glare_measure(&light, None), GlareReading::default());
}

#[test]
fn a_white_patch_on_the_card_is_glare() {
    let reading = glare_measure(&glared(), None);
    near(reading.clipped_share, 0.08509803921568627);
    near(reading.largest_blob_share, 0.08509803921568627);
    assert!(reading.has_glare());
}

#[test]
fn white_specks_and_the_edge_are_not_glare() {
    let specks = glare_measure(&card([214, 220, 226], false, true, false), None);
    near(specks.clipped_share, 0.0010980392156862745);
    near(specks.largest_blob_share, 3.9215686274509805e-05);
    assert!(!specks.has_glare());
    let edge = card([214, 220, 226], false, false, true);
    assert_eq!(glare_measure(&edge, None), GlareReading::default());
    let all = glare_measure(&card([214, 220, 226], true, true, true), None);
    near(all.clipped_share, 0.08619607843137254);
    near(all.largest_blob_share, 0.08509803921568627);
    assert!(all.has_glare());
}

#[test]
fn only_the_outline_given_is_measured() {
    let card = glared();
    let left = glare_measure(&card, Some(&rect(0.0, 0.0, 214.0, 270.0)));
    assert_eq!(left, GlareReading::default());
    let right = glare_measure(&card, Some(&rect(214.0, 0.0, 428.0, 270.0)));
    near(right.clipped_share, 0.1780855199222546);
    near(right.largest_blob_share, 0.1780855199222546);
    let tilted = Quad::from_xy([[190.0, 30.0], [410.0, 60.0], [390.0, 230.0], [170.0, 200.0]]);
    let reading = glare_measure(&card, Some(&tilted));
    near(reading.clipped_share, 0.2681216850576114);
    near(reading.largest_blob_share, 0.2681216850576114);
    let bow_tie = Quad::from_xy([[0.0, 0.0], [428.0, 270.0], [428.0, 0.0], [0.0, 270.0]]);
    assert_eq!(
        glare_measure(&card, Some(&bow_tie)),
        GlareReading::default()
    );
    let outside = rect(500.0, 300.0, 600.0, 400.0);
    assert_eq!(
        glare_measure(&card, Some(&outside)),
        GlareReading::default()
    );
}

#[test]
fn a_large_photo_is_measured_in_cells() {
    let small = glared();
    let (width, height) = (WIDTH * 3, HEIGHT * 3);
    let mut data = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let from = ((y / 3) * WIDTH + x / 3) * 4;
            let to = (y * width + x) * 4;
            data[to..to + 4].copy_from_slice(&small.data[from..from + 4]);
        }
    }
    let reading = glare_measure(&Rgba::new(width, height, data), None);
    assert!(reading.has_glare());
    assert!((0.08..=0.09).contains(&reading.largest_blob_share));
}
