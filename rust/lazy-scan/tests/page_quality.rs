// Lazy Collect's scan.test.ts quality cases, on the same text page and blur.
use lazy_image::Rgba;
use lazy_scan::{Issue, Quad, frame_issues, page_issues};

fn text_page(width: usize, height: usize) -> Rgba {
    let mut s = 7u32;
    let mut rnd = || {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        s as f64 / 4294967296.0
    };
    let mut data = vec![255u8; width * height * 4];
    let mut y = 80;
    while y < height - 80 {
        let mut x = 80;
        while x < width - 120 {
            let w = 8 + (rnd() * 8.0) as usize;
            let mut mark = |x0: usize, y0: usize, mw: usize, mh: usize| {
                for j in 0..mh {
                    for i in 0..mw {
                        let o = ((y + y0 + j) * width + x + x0 + i) * 4;
                        data[o..o + 3].fill(30);
                    }
                }
            };
            mark(0, 0, 2, 16);
            mark(w - 2, 0, 2, 16);
            mark(0, if rnd() < 0.5 { 0 } else { 7 }, w, 2);
            x += w + 4 + (rnd() * 6.0) as usize;
        }
        y += 34;
    }
    Rgba::new(width, height, data)
}

fn map(img: &Rgba, f: impl Fn(u8, usize, usize) -> u8) -> Rgba {
    let mut out = img.clone();
    for p in 0..img.width * img.height {
        for c in 0..3 {
            out.data[p * 4 + c] = f(img.data[p * 4 + c], p % img.width, p / img.width);
        }
        out.data[p * 4 + 3] = 255;
    }
    out
}

fn box_blur(img: &Rgba, r: i64) -> Rgba {
    let (w, h) = (img.width as i64, img.height as i64);
    let mut from = img.data.clone();
    for horizontal in [true, false] {
        let mut to = vec![0u8; from.len()];
        for y in 0..h {
            for x in 0..w {
                let mut sum = 0f64;
                for d in -r..=r {
                    let (xx, yy) = if horizontal {
                        ((x + d).clamp(0, w - 1), y)
                    } else {
                        (x, (y + d).clamp(0, h - 1))
                    };
                    sum += from[((yy * w + xx) * 4) as usize] as f64;
                }
                let o = ((y * w + x) * 4) as usize;
                to[o..o + 3].fill((sum / (2 * r + 1) as f64) as u8);
                to[o + 3] = 255;
            }
        }
        from = to;
    }
    Rgba::new(img.width, img.height, from)
}

fn page() -> Rgba {
    map(
        &text_page(1000, 1400),
        |v, _, _| if v == 255 { 215 } else { v },
    )
}

#[test]
fn a_sharp_bright_page_passes() {
    assert!(page_issues(&page()).is_empty());
}

#[test]
fn blur_is_flagged_but_not_a_slight_softness() {
    assert!(!page_issues(&box_blur(&page(), 1)).contains(&Issue::Blur));
    assert!(page_issues(&box_blur(&page(), 12)).contains(&Issue::Blur));
}

#[test]
fn an_empty_page_is_not_blurry() {
    let blank = Rgba::new(800, 1000, vec![250; 800 * 1000 * 4]);
    assert!(page_issues(&blank).is_empty());
}

#[test]
fn a_blown_out_patch_is_glare() {
    let glared = map(&page(), |v, x, y| {
        if (300..700).contains(&x) && (300..700).contains(&y) {
            255
        } else {
            v
        }
    });
    assert!(page_issues(&glared).contains(&Issue::Glare));
}

#[test]
fn a_dark_page_is_flagged() {
    let dark = map(&page(), |v, _, _| (v as f64 * 0.25) as u8);
    assert!(page_issues(&dark).contains(&Issue::Dark));
}

#[test]
fn a_page_touching_the_frame_or_filling_little_of_it_is_flagged() {
    let q = |c: [[f64; 2]; 4]| Quad::from_xy(c);
    let ok = q([
        [100.0, 100.0],
        [700.0, 100.0],
        [700.0, 500.0],
        [100.0, 500.0],
    ]);
    assert_eq!(frame_issues(&ok, Some(0.9), 800.0, 600.0), []);
    let touching = q([[0.0, 100.0], [700.0, 100.0], [700.0, 500.0], [0.0, 500.0]]);
    assert_eq!(
        frame_issues(&touching, Some(0.9), 800.0, 600.0),
        [Issue::Cut]
    );
    assert_eq!(frame_issues(&ok, Some(0.55), 800.0, 600.0), [Issue::Cut]);
    let far = q([
        [300.0, 200.0],
        [500.0, 200.0],
        [500.0, 400.0],
        [300.0, 400.0],
    ]);
    assert_eq!(frame_issues(&far, None, 800.0, 600.0), [Issue::Small]);
}
