// Lazy Scan's MultiQuadPickerTests: the same outlines and picks.
use lazy_scan::Quad;
use lazy_scan::multi_quad::{Kind, Options, QuadCandidate, overlap, pick, pick_kind};

fn rect(x: f64, y: f64, w: f64, h: f64) -> Quad {
    Quad::from_xy([[x, y], [x + w, y], [x + w, y + h], [x, y + h]])
}

fn candidate(quad: Quad, confidence: f64) -> QuadCandidate {
    QuadCandidate { quad, confidence }
}

#[test]
fn overlap_and_intersection_area() {
    let a = rect(0.0, 0.0, 100.0, 100.0);
    assert!((a.intersection_area(&a) - 10000.0).abs() < 1e-9);
    assert!((a.intersection_area(&rect(50.0, 50.0, 100.0, 100.0)) - 2500.0).abs() < 1e-9);
    assert_eq!(a.intersection_area(&rect(200.0, 0.0, 50.0, 50.0)), 0.0);
    assert!((overlap(&a, &rect(50.0, 50.0, 100.0, 100.0)) - 2500.0 / 17500.0).abs() < 1e-12);
    assert!((overlap(&a, &a) - 1.0).abs() < 1e-12);
    // a diamond inside the square: half its area
    let diamond = Quad::from_xy([[50.0, 0.0], [100.0, 50.0], [50.0, 100.0], [0.0, 50.0]]);
    assert!((a.intersection_area(&diamond) - 5000.0).abs() < 1e-9);
    assert!((diamond.intersection_area(&a) - 5000.0).abs() < 1e-9);
    // turned 45 degrees about the square's centre: the regular octagon, (2 sqrt 2 - 2) x 100 squared
    let h = 50.0 * 2.0f64.sqrt();
    let turned = Quad::from_xy([
        [50.0, 50.0 - h],
        [50.0 + h, 50.0],
        [50.0, 50.0 + h],
        [50.0 - h, 50.0],
    ]);
    assert!((a.intersection_area(&turned) - 8284.271247461902).abs() < 1e-6);
}

#[test]
fn receipts_are_kept_by_shape_size_and_in_reading_order() {
    // three receipts in two rows, a speck and the whole table
    let candidates = [
        candidate(rect(600.0, 120.0, 160.0, 700.0), 0.9),
        candidate(rect(100.0, 100.0, 150.0, 600.0), 0.8),
        candidate(rect(350.0, 1100.0, 140.0, 500.0), 0.95),
        candidate(rect(10.0, 10.0, 20.0, 50.0), 0.99),
        candidate(rect(0.0, 0.0, 1600.0, 2000.0), 0.99),
    ];
    assert_eq!(
        pick_kind(&candidates, 1600, 2000, Kind::Receipt),
        [
            rect(100.0, 100.0, 150.0, 600.0),
            rect(600.0, 120.0, 160.0, 700.0),
            rect(350.0, 1100.0, 140.0, 500.0)
        ]
    );
}

#[test]
fn short_slips_are_kept_as_receipts() {
    let slip = rect(200.0, 200.0, 570.0, 630.0);
    assert_eq!(
        pick_kind(&[candidate(slip, 0.9)], 1600, 2000, Kind::Receipt),
        [slip]
    );
}

#[test]
fn overlapping_outlines_are_one_object_the_surer_stays() {
    let sure = rect(100.0, 100.0, 400.0, 600.0);
    let shifted = rect(130.0, 120.0, 400.0, 600.0);
    let beside = rect(600.0, 100.0, 400.0, 600.0);
    let picked = pick_kind(
        &[
            candidate(shifted, 0.6),
            candidate(sure, 0.9),
            candidate(beside, 0.5),
        ],
        1200,
        900,
        Kind::Photo,
    );
    assert_eq!(picked, [sure, beside]);
    // on a tie the larger stays, then the earlier
    let larger = rect(100.0, 100.0, 420.0, 620.0);
    let options = Options {
        max_long_over_short: 2.0,
        min_area_fraction: 0.01,
        ..Options::default()
    };
    assert_eq!(
        pick(
            &[candidate(shifted, 0.7), candidate(larger, 0.7)],
            1200,
            900,
            &options
        ),
        [larger]
    );
}

#[test]
fn an_outline_inside_a_larger_one_is_part_of_it() {
    // a print with a white border: the picture inside it is found too, and surer
    let printed = rect(200.0, 200.0, 600.0, 400.0);
    let picture = rect(230.0, 230.0, 540.0, 340.0);
    assert_eq!(
        pick_kind(
            &[candidate(picture, 0.95), candidate(printed, 0.7)],
            1200,
            900,
            Kind::Photo
        ),
        [printed]
    );
    // two corners a pixel outside still count as inside
    let almost = rect(199.0, 230.0, 540.0, 340.0);
    assert_eq!(
        pick_kind(
            &[candidate(almost, 0.95), candidate(printed, 0.7)],
            1200,
            900,
            Kind::Photo
        ),
        [printed]
    );
}

#[test]
fn an_outline_holding_several_is_the_background() {
    // an album page holding four prints, one of them with its picture found too: the prints
    let page = rect(250.0, 300.0, 1000.0, 1400.0);
    let prints = [
        rect(300.0, 350.0, 400.0, 300.0),
        rect(750.0, 350.0, 400.0, 300.0),
        rect(300.0, 800.0, 400.0, 300.0),
        rect(750.0, 800.0, 400.0, 300.0),
    ];
    let picture = rect(320.0, 370.0, 360.0, 260.0);
    let album = [
        candidate(page, 0.99),
        candidate(prints[3], 0.8),
        candidate(picture, 0.95),
        candidate(prints[1], 0.7),
        candidate(prints[0], 0.6),
        candidate(prints[2], 0.9),
    ];
    assert_eq!(pick_kind(&album, 1500, 2000, Kind::Photo), prints);
    // the same page holding one print cannot be told from a print with a border: one object
    assert_eq!(
        pick_kind(
            &[candidate(page, 0.5), candidate(prints[0], 0.9)],
            1500,
            2000,
            Kind::Photo
        ),
        [page]
    );
    // receipts on an A4 sheet laid under them for contrast: the receipts
    let sheet = rect(200.0, 200.0, 1000.0, 1414.0);
    let receipts = [
        rect(250.0, 250.0, 150.0, 600.0),
        rect(500.0, 250.0, 160.0, 700.0),
        rect(800.0, 300.0, 140.0, 500.0),
    ];
    assert_eq!(
        pick_kind(
            &[
                candidate(receipts[2], 0.7),
                candidate(sheet, 0.99),
                candidate(receipts[0], 0.8),
                candidate(receipts[1], 0.9)
            ],
            1600,
            2000,
            Kind::Receipt
        ),
        receipts
    );
    // wanting one object, the outermost outline is it, whatever it holds
    let document = rect(100.0, 100.0, 700.0, 990.0);
    let top = rect(150.0, 150.0, 600.0, 400.0);
    let bottom = rect(150.0, 600.0, 600.0, 400.0);
    let one = Options {
        min_area_fraction: 0.01,
        ..Options::default()
    };
    let three = [
        candidate(top, 0.9),
        candidate(bottom, 0.9),
        candidate(document, 0.5),
    ];
    assert_eq!(pick(&three, 2400, 1600, &one), [document]);
    let two = Options {
        max_count: 2,
        ..one
    };
    assert_eq!(pick(&three, 2400, 1600, &two), [top, bottom]);
}

#[test]
fn features_inside_one_print_leave_it_whole() {
    // a 4 x 6 in print of a house over 40 % of the shot; two of its windows are found too, each under 2 % of the shot and
    // photo-shaped, together 8 % of the print: the print
    let print = rect(300.0, 325.0, 900.0, 1350.0);
    let windows = [
        rect(450.0, 500.0, 200.0, 250.0),
        rect(850.0, 500.0, 200.0, 250.0),
    ];
    assert_eq!(
        pick_kind(
            &[
                candidate(windows[0], 0.9),
                candidate(print, 0.8),
                candidate(windows[1], 0.85)
            ],
            1500,
            2000,
            Kind::Photo
        ),
        [print]
    );
    // the print's picture found as well, holding the windows: still the print
    let picture = rect(330.0, 355.0, 840.0, 1290.0);
    assert_eq!(
        pick_kind(
            &[
                candidate(windows[0], 0.9),
                candidate(picture, 0.95),
                candidate(print, 0.8),
                candidate(windows[1], 0.85)
            ],
            1500,
            2000,
            Kind::Photo
        ),
        [print]
    );
    // the picture found twice, a little apart: one object seen twice, so the print
    let again = rect(326.0, 351.0, 840.0, 1290.0);
    assert_eq!(
        pick_kind(
            &[
                candidate(picture, 0.95),
                candidate(again, 0.9),
                candidate(print, 0.8)
            ],
            1500,
            2000,
            Kind::Photo
        ),
        [print]
    );
    // two prints laid on the same outline cover enough of it to make it the background
    let left = rect(330.0, 355.0, 420.0, 600.0);
    let right = rect(760.0, 355.0, 420.0, 600.0);
    assert_eq!(
        pick_kind(
            &[
                candidate(print, 0.99),
                candidate(left, 0.8),
                candidate(right, 0.7)
            ],
            1500,
            2000,
            Kind::Photo
        ),
        [left, right]
    );
}

#[test]
fn a_card_is_picked_by_its_shape_and_only_one() {
    let card = rect(300.0, 300.0, 856.0, 540.0);
    let page = rect(100.0, 100.0, 700.0, 990.0);
    let second = rect(1300.0, 300.0, 850.0, 536.0);
    assert_eq!(
        pick_kind(
            &[
                candidate(page, 0.99),
                candidate(card, 0.8),
                candidate(second, 0.7)
            ],
            2400,
            1600,
            Kind::IdCard
        ),
        [card]
    );
    // a document takes the surest page-shaped outline, a card's shape included
    assert_eq!(
        pick_kind(
            &[candidate(page, 0.9), candidate(card, 0.5)],
            2400,
            1600,
            Kind::Document
        ),
        [page]
    );
    assert_eq!(
        pick_kind(
            &[candidate(page, 0.5), candidate(card, 0.9)],
            2400,
            1600,
            Kind::Document
        ),
        [card]
    );
}

#[test]
fn photos_are_capped_and_odd_input_left_out() {
    let candidates: Vec<QuadCandidate> = (0..12)
        .map(|i| {
            let (x, y) = ((20 + i % 4 * 300) as f64, (20 + i / 4 * 300) as f64);
            candidate(rect(x, y, 240.0, 180.0), 0.5 + i as f64 / 100.0)
        })
        .collect();
    let picked = pick_kind(&candidates, 1240, 920, Kind::Photo);
    assert_eq!(picked.len(), 8);
    // the surest eight (the last eight given), in reading order
    assert_eq!(picked.first(), Some(&rect(20.0, 320.0, 240.0, 180.0)));
    assert_eq!(picked.last(), Some(&rect(920.0, 620.0, 240.0, 180.0)));
    // corners given counterclockwise are named again; a bow tie is no outline; no photo, no pick
    let counter = rect(100.0, 100.0, 300.0, 200.0);
    let reversed = Quad([counter.0[0], counter.0[3], counter.0[2], counter.0[1]]);
    assert_eq!(
        pick_kind(&[candidate(reversed, 0.9)], 1240, 920, Kind::Photo),
        [counter]
    );
    let bow_tie = Quad::from_xy([
        [100.0, 100.0],
        [400.0, 300.0],
        [400.0, 100.0],
        [100.0, 300.0],
    ]);
    assert!(pick_kind(&[candidate(bow_tie, 0.9)], 1240, 920, Kind::Photo).is_empty());
    assert!(pick_kind(&[candidate(counter, 0.9)], 0, 920, Kind::Photo).is_empty());
    assert!(pick_kind(&[], 1240, 920, Kind::Photo).is_empty());
}

#[test]
fn every_kind_has_options() {
    for kind in [Kind::Document, Kind::Receipt, Kind::Photo, Kind::IdCard] {
        let o = Options::for_kind(kind);
        assert!(o.min_long_over_short >= 1.0 && o.max_long_over_short > o.min_long_over_short);
        assert!(o.min_area_fraction >= 0.0 && o.max_area_fraction > o.min_area_fraction);
        assert!(o.max_count >= 1);
    }
    assert_eq!(Options::for_kind(Kind::Receipt).max_count, 6);
    assert_eq!(Options::for_kind(Kind::Photo).max_count, 8);
    assert_eq!(Options::for_kind(Kind::IdCard).max_count, 1);
    assert_eq!(Options::for_kind(Kind::Document).max_count, 1);
}
