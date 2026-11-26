// Lazy Scan's PhotoImport on its photo scenes: orientation, the plan the corner model gives, the page cut out and cleaned up.
mod common;

use common::photo_scenes::*;
use lazy_image::Rgba;
use lazy_scan::{
    Action, CornerModel, EnhanceMode, Filter, Heatmaps, ModelError, Orientation, Quad, Reason,
    enhance, import_photo, oriented, page_dpi, whole_photo_dpi, whole_photo_page,
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
