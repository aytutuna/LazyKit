// Lazy Scan's PhotoImport on its photo scenes: orientation, the plan the corner model gives, the page cut out and cleaned up.
mod common;

use common::photo_scenes::*;
use lazy_image::Rgba;
use lazy_scan::detector::{Detection, Options as DetectorOptions, Rejection, detect};
use lazy_scan::{
    Action, CornerModel, EnhanceMode, Filter, Heatmaps, ModelError, Orientation, Point, Quad,
    Reason, clamped_to_photo, corrected_page, enhance, import_photo, moved_corner, oriented,
    page_dpi, plan, plan_detection, refine, whole_photo_dpi, whole_photo_page,
};

// a model that answers with a bright spot at each corner of a quad given in shares of the photo
struct Spots(Option<Quad>);

impl CornerModel for Spots {
    fn run(&self, _: &[f32]) -> Result<Heatmaps, ModelError> {
        let quad = self.0.ok_or(ModelError::Shape)?;
        let n = 64;
        let mut values = vec![0f32; 4 * n * n];
        for (ch, p) in quad.0.iter().enumerate() {
            let (cx, cy) = ((p.x * n as f64) as usize, (p.y * n as f64) as usize);
            for dy in 0..3 {
                for dx in 0..3 {
                    let x = (cx + dx).saturating_sub(1).min(n - 1);
                    let y = (cy + dy).saturating_sub(1).min(n - 1);
                    values[ch * n * n + y * n + x] = 0.95;
                }
            }
        }
        Ok(Heatmaps {
            values,
            width: n,
            height: n,
        })
    }
}

fn shares(q: &Quad) -> Quad {
    q.scaled(1.0 / WIDTH as f64, 1.0 / HEIGHT as f64)
}

fn pattern(w: usize, h: usize) -> Rgba {
    let mut data = Vec::new();
    for y in 0..h {
        for x in 0..w {
            data.extend_from_slice(&[(x * 40) as u8, (y * 40) as u8, 7, 255]);
        }
    }
    Rgba::new(w, h, data)
}

#[test]
fn the_exif_tags_turn_a_photo_upright() {
    let upright = pattern(3, 2);
    // the tag says how the stored pixels are turned to show them
    let turned = |tag| oriented(&upright, Orientation::from_exif(tag));
    assert_eq!(turned(1), upright);
    assert_eq!(turned(2), upright.mirrored());
    assert_eq!(turned(3), upright.rotated_quarter_turns(2));
    assert_eq!(turned(4), upright.mirrored().rotated_quarter_turns(2));
    assert_eq!(turned(5), upright.mirrored().rotated_quarter_turns(3));
    assert_eq!(turned(6), upright.rotated_quarter_turns(1));
    assert_eq!(turned(7), upright.mirrored().rotated_quarter_turns(1));
    assert_eq!(turned(8), upright.rotated_quarter_turns(3));
    for tag in 1..=8 {
        assert_eq!(Orientation::from_exif(tag).is_upright(), tag == 1);
    }
    for odd in [0, 9, 100] {
        assert_eq!(turned(odd), upright);
    }
}

#[test]
fn resolutions_are_rounded_and_kept_in_range() {
    assert_eq!(page_dpi(None), 200);
    assert_eq!(page_dpi(Some(f64::NAN)), 200);
    assert_eq!(page_dpi(Some(-3.0)), 200);
    assert_eq!(page_dpi(Some(10.0)), 50);
    assert_eq!(page_dpi(Some(299.5)), 300);
    assert_eq!(page_dpi(Some(299.49)), 299);
    assert_eq!(page_dpi(Some(9000.0)), 1200);
    // an A4-shaped photo takes its dpi from A4's long side, a shape that matches no paper from the usual paper's
    assert_eq!(whole_photo_dpi(2100, 2970, Some("DE")), 254);
    assert_eq!(whole_photo_dpi(3000, 3000, Some("US")), 273);
    assert_eq!(whole_photo_dpi(3000, 3000, Some("de")), 257);
    assert_eq!(whole_photo_dpi(3000, 3000, None), 257);
    assert_eq!(whole_photo_dpi(0, 5, None), 200);
}

#[test]
fn a_photo_goes_to_a_finished_page_in_one_call() {
    let photo = wood_scene();
    let corners = WOOD_MAP.corners();
    let model = Spots(Some(shares(&corners)));
    let got = import_photo(
        &photo,
        Orientation::UPRIGHT,
        Some(&model),
        Filter::Original,
        Some("CA"),
    )
    .unwrap();
    assert!(got.plan.by_model && !got.model_failed);
    assert_eq!(got.plan.action, Action::Correct, "{:?}", got.plan);
    assert_eq!(got.plan.reason, Reason::None);
    for (a, b) in got.plan.corners.0.iter().zip(corners.0) {
        assert!(a.distance_to(b) < 6.0, "{a:?} {b:?}");
    }
    assert!(!got.page.whole_photo);
    assert_eq!(got.page.mode, None);
    // 333 px along A4's 11.69 in is 28 dpi, under the lowest a page is given
    assert_eq!(
        (got.page.image.width, got.page.image.height, got.page.dpi),
        (235, 333, 50)
    );
}

#[test]
fn the_filter_is_applied_to_the_cut_out_page() {
    let photo = wood_scene();
    let model = Spots(Some(shares(&WOOD_MAP.corners())));
    let plain = import_photo(
        &photo,
        Orientation::UPRIGHT,
        Some(&model),
        Filter::Original,
        None,
    )
    .unwrap();
    for (filter, mode) in [
        (Filter::Enhanced, EnhanceMode::Auto),
        (Filter::Grayscale, EnhanceMode::Gray),
        (Filter::BlackAndWhite, EnhanceMode::BlackAndWhite),
        (Filter::Whiteboard, EnhanceMode::Whiteboard),
    ] {
        let got = import_photo(&photo, Orientation::UPRIGHT, Some(&model), filter, None).unwrap();
        let expected = enhance(&plain.page.image, mode, 0.5);
        assert_eq!(got.page.image, expected.image, "{filter:?}");
        assert_eq!(got.page.mode, Some(expected.mode));
        assert_eq!(got.page.corners, plain.page.corners);
    }
}

#[test]
fn a_photo_stored_turned_comes_out_as_the_same_page() {
    let photo = wood_scene();
    let model = Spots(Some(shares(&WOOD_MAP.corners())));
    let direct = import_photo(
        &photo,
        Orientation::UPRIGHT,
        Some(&model),
        Filter::Original,
        None,
    )
    .unwrap();
    // a camera held on its side stores the pixels turned, with tag 6 to turn them back
    let stored = photo.rotated_quarter_turns(-1);
    let turned = import_photo(
        &stored,
        Orientation::from_exif(6),
        Some(&model),
        Filter::Original,
        None,
    )
    .unwrap();
    assert_eq!(turned.page.image, direct.page.image);
    assert_eq!(turned.plan, direct.plan);
    let flipped = import_photo(
        &photo.mirrored(),
        Orientation::from_exif(2),
        Some(&model),
        Filter::Original,
        None,
    )
    .unwrap();
    assert_eq!(flipped.page.image, direct.page.image);
}

#[test]
fn without_a_model_or_a_page_the_editor_starts_from_the_inset_rectangle() {
    let photo = plain_scene();
    let broken = Spots(None);
    for model in [None, Some(&broken as &dyn CornerModel)] {
        let got =
            import_photo(&photo, Orientation::UPRIGHT, model, Filter::Original, None).unwrap();
        assert_eq!(got.plan.action, Action::Review);
        assert_eq!(got.plan.corners, Quad::start(WIDTH as f64, HEIGHT as f64));
        assert!(!got.plan.by_model);
        assert_eq!(got.model_failed, model.is_some());
    }
}

#[test]
fn a_photo_without_pixels_makes_no_page() {
    let empty = Rgba::new(0, 0, vec![]);
    assert!(import_photo(&empty, Orientation::UPRIGHT, None, Filter::Enhanced, None).is_none());
    assert!(whole_photo_page(&empty, Filter::Original, None).is_none());
}

#[test]
fn the_whole_photo_page_keeps_the_photo_and_sizes_it_by_its_shape() {
    let photo = plain_scene();
    let page = whole_photo_page(&photo, Filter::Original, Some("US")).unwrap();
    assert_eq!(page.image, photo);
    assert!(page.whole_photo);
    assert_eq!(page.dpi, whole_photo_dpi(360, 480, Some("US")));
}

fn quad(a: (f64, f64), b: (f64, f64), c: (f64, f64), d: (f64, f64)) -> Quad {
    Quad::from_xy([[a.0, a.1], [b.0, b.1], [c.0, c.1], [d.0, d.1]])
}

fn detection(
    q: Option<Quad>,
    confidence: f64,
    rejection: Rejection,
    candidate: Option<Quad>,
) -> Detection {
    Detection {
        quad: q,
        confidence,
        rejection,
        candidate,
    }
}

#[test]
fn a_page_found_clearly_is_cut_out_without_asking() {
    let photo = wood_scene();
    let found = detect(&photo, None, &DetectorOptions::default());
    assert!(found.confidence >= 0.7);
    let (plan, failed) = plan(&photo, None);
    assert!(!failed);
    assert_eq!(plan.action, Action::Correct);
    assert_eq!(plan.reason, Reason::None);
    // the page found, moved onto the photo's edges
    let quad = found.quad.unwrap();
    assert_eq!(plan.corners, refine(&photo, &quad).quad);
    assert!(plan.corners.max_corner_distance(&quad) < 1.0);
}

#[test]
fn an_unsure_page_goes_to_the_corner_editor_with_its_corners() {
    let outline = quad((40.0, 30.0), (300.0, 50.0), (290.0, 420.0), (30.0, 400.0));
    let unsure = detection(Some(outline), 0.69, Rejection::None, None);
    let got = plan_detection(&unsure, 360, 480);
    assert_eq!(
        (got.action, got.reason, got.corners),
        (Action::Review, Reason::Unsure, outline)
    );
    let sure = detection(Some(outline), 0.7, Rejection::None, None);
    assert_eq!(plan_detection(&sure, 360, 480).action, Action::Correct);
    // an outline seen but not taken is the editor's start too
    let low = detection(None, 0.0, Rejection::LowConfidence, Some(outline));
    let got = plan_detection(&low, 360, 480);
    assert_eq!(
        (got.action, got.reason, got.corners),
        (Action::Review, Reason::Unsure, outline)
    );
}

#[test]
fn a_page_cut_off_by_the_frame_starts_from_what_is_seen_within_the_photo() {
    let photo = cut_off_scene();
    let (got, _) = plan(&photo, None);
    assert_eq!((got.action, got.reason), (Action::Review, Reason::CutOff));
    assert!(got.corners.is_inside(WIDTH as f64, HEIGHT as f64, 0.0));
    assert!(got.corners.usable());
    assert_ne!(got.corners, Quad::start(WIDTH as f64, HEIGHT as f64));
}

#[test]
fn no_page_or_two_pages_ask_the_user() {
    let got = plan_detection(&Detection::NOTHING, 360, 480);
    assert_eq!(
        (got.action, got.reason),
        (Action::Review, Reason::NothingFound)
    );
    assert_eq!(
        got.corners,
        quad((36.0, 48.0), (324.0, 48.0), (324.0, 432.0), (36.0, 432.0))
    );
    // a table with no page on it: nothing found, or nothing clear enough (the detector may answer either)
    for scene in [no_page_scene(), plain_scene()] {
        let (asked, _) = plan(&scene, None);
        assert_eq!(asked.action, Action::Review);
        assert!(matches!(
            asked.reason,
            Reason::NothingFound | Reason::Unsure
        ));
        assert!(
            asked.corners.usable() && asked.corners.is_inside(WIDTH as f64, HEIGHT as f64, 0.0)
        );
    }
    let (two, _) = plan(&two_pages_scene(), None);
    assert_eq!((two.action, two.reason), (Action::Review, Reason::TwoPages));
    assert!(two.corners.usable());
}

#[test]
fn the_photos_corners_are_named_again_before_the_page_is_cut_out() {
    // corners dragged around the outline, starting anywhere and either way round, cut out the same page
    let photo = wood_scene();
    let (got, _) = plan(&photo, None);
    let q = got.corners.0;
    let page = |q: Quad| corrected_page(&photo, &q, Filter::Original).map(|p| checksum(&p.image));
    let expected = page(got.corners).unwrap();
    assert_eq!(page(Quad([q[2], q[3], q[0], q[1]])), Some(expected));
    assert_eq!(page(Quad([q[0], q[3], q[2], q[1]])), Some(expected));
    assert!(
        corrected_page(
            &photo,
            &quad((0.0, 0.0), (10.0, 0.0), (20.0, 0.0), (30.0, 0.0)),
            Filter::Original
        )
        .is_none()
    );
}

#[test]
fn a_whole_photo_takes_the_resolution_of_its_shape() {
    assert_eq!(whole_photo_dpi(4032, 3024, Some("US")), 367);
    assert_eq!(whole_photo_dpi(4032, 3024, Some("DE")), 345);
    let a4_short = (3508.0f64 * 8.2677 / 11.6929 + 0.5).floor() as usize;
    assert_eq!(
        whole_photo_dpi(a4_short, 3508, None),
        (3508.0f64 / 11.6929 + 0.5).floor() as u32
    );
    assert_eq!(
        whole_photo_dpi(3508, a4_short, None),
        whole_photo_dpi(a4_short, 3508, None)
    );
    // a corrected page takes the resolution of the paper size its shape matched
    let photo = wood_scene();
    let (got, _) = plan(&photo, None);
    let (_, cut) = lazy_scan::warp::correct(
        &photo,
        &Quad::from_corners(got.corners.0),
        &Default::default(),
    )
    .unwrap();
    let page = corrected_page(&photo, &got.corners, Filter::Original).unwrap();
    assert_eq!(page.dpi, page_dpi(cut.dpi));
}

#[test]
fn the_corner_editor_keeps_an_outline_the_page_can_be_cut_out_along() {
    let q = quad((50.0, 50.0), (300.0, 60.0), (290.0, 400.0), (40.0, 380.0));
    let to = |x, y| Point::new(x, y);
    let moved = moved_corner(&q, 1, to(310.0, 40.0), 360.0, 480.0);
    assert_eq!(moved.0[1], to(310.0, 40.0));
    assert_eq!(moved.0[0], q.0[0]);
    // dragged off the photo: kept on its edge
    assert_eq!(
        moved_corner(&q, 2, to(500.0, 600.0), 360.0, 480.0).0[2],
        to(360.0, 480.0)
    );
    // across the outline (a bow tie) or onto another corner: refused
    assert_eq!(moved_corner(&q, 0, to(320.0, 420.0), 360.0, 480.0), q);
    assert_eq!(moved_corner(&q, 0, to(295.0, 62.0), 360.0, 480.0), q);
    assert_eq!(moved_corner(&q, 0, to(170.0, 380.0), 360.0, 480.0), q);
    // the index wraps like the quad's
    assert_eq!(
        moved_corner(&q, 7, to(45.0, 385.0), 360.0, 480.0).0[3],
        to(45.0, 385.0)
    );
    // a guess partly off the photo is brought onto it
    let off = quad((-30.0, 20.0), (330.0, -10.0), (400.0, 500.0), (20.0, 470.0));
    assert_eq!(
        clamped_to_photo(&off, 360.0, 480.0),
        quad((0.0, 20.0), (330.0, 0.0), (360.0, 480.0), (20.0, 470.0))
    );
    let flat = quad((-30.0, -20.0), (-10.0, -20.0), (-10.0, -5.0), (-30.0, -5.0));
    assert_eq!(
        clamped_to_photo(&flat, 360.0, 480.0),
        Quad::start(360.0, 480.0)
    );
}
