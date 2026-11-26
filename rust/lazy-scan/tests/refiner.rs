mod common;

use common::scenes::{map_pixels, photo_of, text_page};
use lazy_image::Rgba;
use lazy_scan::{Quad, Refinement, build_field, placement, refine};

fn desk() -> (Rgba, Quad) {
    let sheet = map_pixels(&text_page(480, 640, 7), |v| if v == 255 { 225 } else { v });
    let page = Quad::from_xy([
        [260.0, 180.0],
        [700.0, 210.0],
        [740.0, 760.0],
        [220.0, 720.0],
    ]);
    (photo_of(&sheet, &page, 1000, 900), page)
}

const DIAGONAL: f64 = 1345.362404707371; // hypot(1000, 900)

fn off(q: &Quad, pattern: usize) -> Quad {
    let mut out = *q;
    for (i, p) in out.0.iter_mut().enumerate() {
        let angle = i as f64 * 1.7 + pattern as f64 * 2.3;
        p.x += 0.01 * DIAGONAL * angle.cos();
        p.y += 0.01 * DIAGONAL * angle.sin();
    }
    out
}

#[test]
fn an_outline_off_the_page_is_moved_back_onto_its_edges() {
    let (photo, page) = desk();
    for pattern in 0..4 {
        let r = refine(&photo, &off(&page, pattern));
        assert!(r.refined, "pattern {pattern}");
        assert!(
            r.quad.max_corner_distance(&page) <= 0.003 * DIAGONAL,
            "pattern {pattern}"
        );
        assert!(r.support >= 0.95, "pattern {pattern}: {}", r.support);
    }
}

#[test]
fn a_page_already_on_its_edges_stays() {
    let (photo, page) = desk();
    assert!(refine(&photo, &page).quad.max_corner_distance(&page) <= 1.0);
}

#[test]
fn a_table_with_no_page_leaves_the_outline_alone() {
    let photo = Rgba::filled(1000, 900, 140, 140, 140);
    let start = off(&desk().1, 0);
    let r = refine(&photo, &start);
    assert!(!r.refined);
    assert_eq!(r.quad, start);
    assert!(r.support < 0.2);
}

#[test]
fn a_large_photo_is_searched_small_and_answered_in_its_own_pixels() {
    let sheet = map_pixels(&text_page(480, 640, 7), |v| if v == 255 { 225 } else { v });
    let page = Quad::from_xy([
        [780.0, 540.0],
        [2100.0, 630.0],
        [2220.0, 2280.0],
        [660.0, 2160.0],
    ]);
    let photo = photo_of(&sheet, &page, 3000, 2700);
    let mut start = page;
    for (i, p) in start.0.iter_mut().enumerate() {
        p.x += if i % 2 == 1 { 25.0 } else { -25.0 };
        p.y += 20.0;
    }
    let r = refine(&photo, &start);
    assert!(r.refined);
    assert!(r.quad.max_corner_distance(&page) <= 0.003 * 3.0 * DIAGONAL);
    assert!(r.quad.signed_area() > 0.0);
}

#[test]
fn a_flat_image_has_no_gradient() {
    let f = build_field(&Rgba::filled(20, 20, 255, 255, 255));
    assert!(f.gx.iter().all(|&v| v == 0.0));
}

#[test]
fn a_page_is_placed_straight_away_only_when_the_model_and_the_edges_are_both_sure() {
    let given = Quad::from_xy([
        [100.0, 100.0],
        [400.0, 100.0],
        [400.0, 500.0],
        [100.0, 500.0],
    ]);
    let edge = Quad::from_xy([
        [102.0, 99.0],
        [401.0, 101.0],
        [399.0, 501.0],
        [101.0, 499.0],
    ]);
    let found = |quad, refined, support| Refinement {
        quad,
        refined,
        support,
    };
    assert_eq!(
        placement(Some(&found(edge, true, 0.9)), &given, 0.92, 640.0, 640.0),
        (edge, true)
    );
    assert!(!placement(Some(&found(edge, true, 0.9)), &given, 0.6, 640.0, 640.0).1);
    assert_eq!(
        placement(Some(&found(given, false, 0.5)), &given, 0.92, 640.0, 640.0),
        (given, false)
    );
    assert_eq!(placement(None, &given, 0.92, 640.0, 640.0), (given, false));
}

#[test]
fn a_page_moved_onto_a_stack_of_sheets_still_starts_from_the_refined_corners() {
    let given = Quad::from_xy([
        [100.0, 100.0],
        [400.0, 100.0],
        [400.0, 500.0],
        [100.0, 500.0],
    ]);
    let stacked = Quad::from_xy([
        [100.0, 100.0],
        [400.0, 100.0],
        [440.0, 540.0],
        [100.0, 500.0],
    ]);
    let r = Refinement {
        quad: stacked,
        refined: true,
        support: 0.31,
    };
    assert_eq!(
        placement(Some(&r), &given, 0.92, 640.0, 640.0),
        (stacked, false)
    );
}

#[test]
fn the_corners_stay_inside_the_photo_and_fall_back_to_the_start_rectangle() {
    let out = Quad::from_xy([
        [-20.0, -10.0],
        [700.0, -10.0],
        [700.0, 660.0],
        [-20.0, 660.0],
    ]);
    let r = Refinement {
        quad: out,
        refined: true,
        support: 0.9,
    };
    assert_eq!(
        placement(Some(&r), &out, 0.9, 640.0, 640.0).0,
        Quad::from_xy([[0.0, 0.0], [640.0, 0.0], [640.0, 640.0], [0.0, 640.0]])
    );
    let sliver = Quad::from_xy([[-50.0, 10.0], [-40.0, 10.0], [-40.0, 500.0], [-50.0, 500.0]]);
    let r = Refinement {
        quad: sliver,
        refined: false,
        support: 0.9,
    };
    assert_eq!(
        placement(Some(&r), &sliver, 0.9, 640.0, 640.0),
        (Quad::start(640.0, 640.0), false)
    );
}

// the scenes of Lazy Scan's QuadRefinerTests, drawn as plain rectangles: an upright page on a dark desk with other edges near it
mod second_edge {
    use super::*;

    fn scene(boxes: &[[usize; 5]]) -> Rgba {
        let mut img = Rgba::filled(420, 520, 255, 255, 255);
        for &[x0, y0, x1, y1, v] in boxes {
            for y in y0..y1 {
                for x in x0..x1 {
                    img.data[(y * 420 + x) * 4..(y * 420 + x) * 4 + 3].fill(v as u8);
                }
            }
        }
        img
    }

    fn page() -> Quad {
        Quad::from_xy([[100.0, 80.0], [331.0, 80.0], [331.0, 407.0], [100.0, 407.0]])
    }

    const DIAGONAL: f64 = 668.5; // about hypot(420, 520)

    #[test]
    fn a_header_bar_just_inside_the_page_does_not_pull_its_top_edge() {
        let photo = scene(&[
            [0, 0, 420, 520, 30],
            [100, 80, 331, 407, 235],
            [100, 86, 331, 116, 200],
        ]);
        let mut start = page();
        start.0[0].y += 0.009 * DIAGONAL;
        start.0[1].y += 0.009 * DIAGONAL;
        assert!(refine(&photo, &start).quad.max_corner_distance(&page()) <= 0.003 * DIAGONAL);
    }

    #[test]
    fn a_page_already_on_its_edges_is_not_pushed_onto_a_folder_under_it() {
        let photo = scene(&[
            [0, 0, 420, 520, 30],
            [88, 68, 343, 419, 175],
            [100, 80, 331, 407, 235],
        ]);
        let r = refine(&photo, &page());
        assert!(r.quad.max_corner_distance(&page()) <= 0.5);
        assert!(r.support >= 0.95);
    }

    #[test]
    fn an_equally_sharp_edge_just_outside_does_not_take_the_side() {
        let photo = scene(&[
            [0, 0, 420, 520, 20],
            [88, 68, 343, 419, 128],
            [100, 80, 331, 407, 232],
        ]);
        let mut inside = page();
        for (i, p) in inside.0.iter_mut().enumerate() {
            p.x += if i == 0 || i == 3 { 2.0 } else { -2.0 };
            p.y += if i < 2 { 2.0 } else { -2.0 };
        }
        let r = refine(&photo, &inside);
        assert!(r.refined);
        assert!(r.quad.max_corner_distance(&page()) <= 1.0);
        assert!(r.support >= 0.95);
    }
}
