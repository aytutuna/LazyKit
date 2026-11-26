// Lazy Scan's PhotoDocumentDetectorTests: the same synthetic photos (its PhotoScenes, byte-identical, checked by checksum) and
// the same expectations.
mod common;

use common::photo_scenes::*;
use lazy_image::Rgba;
use lazy_scan::Quad;
use lazy_scan::detector::{ACCEPT_SCORE, Detection, Options, Rejection, detect};

/// 1 % of the photo's diagonal (360 x 480: 6 px).
fn tolerance() -> f64 {
    0.01 * ((WIDTH * WIDTH + HEIGHT * HEIGHT) as f64).sqrt()
}

fn found(result: &Detection, expected: &Quad, tolerance: f64) {
    let Some(quad) = result.quad else {
        panic!(
            "no page: {:?}, candidate {:?}",
            result.rejection, result.candidate
        );
    };
    assert_eq!(result.rejection, Rejection::None);
    let error = quad.max_corner_distance(expected);
    assert!(
        error <= tolerance,
        "corner error {error}: found {quad:?}, expected {expected:?}"
    );
    assert!(result.confidence >= ACCEPT_SCORE);
}

fn run(scene: &Rgba) -> Detection {
    detect(scene, None, &Options::default())
}

#[test]
fn the_scenes_are_the_apps() {
    assert_eq!(checksum(&wood_scene()), 0x5D45_6758);
    assert_eq!(checksum(&dark_scene()), 0x37A2_8CB9);
    assert_eq!(checksum(&light_scene()), 0xE9BE_D0A0);
    assert_eq!(checksum(&similar_scene()), 0xB26B_D827);
    assert_eq!(checksum(&no_page_scene()), 0xB93E_AFBA);
    assert_eq!(checksum(&plain_scene()), 0x488C_62C7);
    assert_eq!(checksum(&cut_off_scene()), 0xB990_DDDA);
    assert_eq!(checksum(&overlap_scene()), 0x231A_4514);
    assert_eq!(checksum(&two_pages_scene()), 0x999E_64F6);
    assert_eq!(checksum(&round_trip_scene()), 0xB97E_5292);
    assert_eq!(checksum(&bottom_gap_scene(3)), 0x2A26_4F0E);
    assert_eq!(checksum(&bottom_gap_scene(5)), 0x1C63_A221);
    assert_eq!(checksum(&bottom_gap_scene(8)), 0xE83D_D873);
    assert_eq!(checksum(&bottom_gap_scene(-15)), 0x7DC6_FC59);
    assert_eq!(checksum(&right_gap_scene(4)), 0xCDB3_F5EF);
}

#[test]
fn finds_a_page_on_a_wooden_table() {
    found(&run(&wood_scene()), &WOOD_MAP.corners(), tolerance());
}

#[test]
fn finds_a_page_on_a_dark_desk_with_glare_on_its_edge() {
    found(&run(&dark_scene()), &DARK_MAP.corners(), tolerance());
}

#[test]
fn finds_a_page_on_a_light_desk_in_the_phones_shadow() {
    found(&run(&light_scene()), &LIGHT_MAP.corners(), tolerance());
}

#[test]
fn finds_a_page_on_a_table_of_nearly_its_colour() {
    found(&run(&similar_scene()), &SIMILAR_MAP.corners(), tolerance());
}

#[test]
fn finds_nothing_where_there_is_no_page() {
    for scene in [no_page_scene(), plain_scene()] {
        let result = run(&scene);
        assert!(result.quad.is_none());
        assert_eq!(result.confidence, 0.0);
        assert!(
            matches!(
                result.rejection,
                Rejection::NothingFound | Rejection::LowConfidence
            ),
            "{:?}",
            result.rejection
        );
    }
}

#[test]
fn refuses_a_page_cut_off_by_the_frame() {
    let result = run(&cut_off_scene());
    assert!(result.quad.is_none());
    assert_eq!(result.rejection, Rejection::CutOff);
    let candidate = result.candidate.unwrap();
    assert!(!candidate.is_inside(WIDTH as f64, HEIGHT as f64, 0.0));
}

#[test]
fn finds_a_page_whose_edge_lies_close_to_the_frames_border() {
    // 3 to 8 px of table between the page and the frame's border: every corner is in the frame
    for gap in [3, 5, 8] {
        found(
            &run(&bottom_gap_scene(gap)),
            &bottom_gap_map(gap).corners(),
            tolerance(),
        );
    }
    found(
        &run(&right_gap_scene(4)),
        &right_gap_map(4).corners(),
        tolerance(),
    );
}

#[test]
fn refuses_a_page_running_off_the_bottom_of_the_frame() {
    let result = run(&bottom_gap_scene(-15));
    assert!(result.quad.is_none());
    assert_eq!(result.rejection, Rejection::CutOff);
    assert!(result.candidate.is_some());
}

#[test]
fn takes_the_larger_of_two_overlapping_pages() {
    found(&run(&overlap_scene()), &TOP_MAP.corners(), tolerance());
}

#[test]
fn refuses_two_pages_of_the_same_size() {
    let result = run(&two_pages_scene());
    assert!(result.quad.is_none());
    assert_eq!(result.rejection, Rejection::Ambiguous);
    assert!(result.candidate.is_some());
}

#[test]
fn analyses_a_smaller_copy_and_answers_in_the_photos_pixels() {
    // the photo enlarged twice, analysed at 240 px: the corners come back in the large photo's pixels
    let scene = wood_scene();
    let (lw, lh) = (scene.width * 2, scene.height * 2);
    let mut data = vec![0u8; lw * lh * 4];
    for y in 0..lh {
        for x in 0..lw {
            let s = ((y / 2) * scene.width + x / 2) * 4;
            let d = (y * lw + x) * 4;
            data[d..d + 4].copy_from_slice(&scene.data[s..s + 4]);
        }
    }
    let large = Rgba::new(lw, lh, data);
    let options = Options {
        analysis_long_side: 240,
        ..Options::default()
    };
    let result = detect(&large, None, &options);
    found(
        &result,
        &WOOD_MAP.corners().scaled(2.0, 2.0),
        2.0 * tolerance(),
    );
}

#[test]
fn a_hint_from_the_previous_frame_keeps_the_page() {
    let truth = LIGHT_MAP.corners();
    let plain = run(&light_scene());
    let mut hint = truth;
    for (p, (dx, dy)) in hint
        .0
        .iter_mut()
        .zip([(4.0, -3.0), (-3.0, 4.0), (2.0, 3.0), (-4.0, -2.0)])
    {
        p.x += dx;
        p.y += dy;
    }
    let hinted = detect(&light_scene(), Some(&hint), &Options::default());
    found(&hinted, &truth, tolerance());
    assert!(hinted.confidence >= plain.confidence);
}
