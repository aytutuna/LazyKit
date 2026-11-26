// Lazy Scan's LiveQuadTrackerTests (the page search part), built from the same recipes: the quality frame's sheet, a search
// that reads the frame's index from its first pixel's alpha, and 100 ms between frames.
mod common;

use common::scenes::quality_frame;
use lazy_image::Rgba;
use lazy_scan::{
    CaptureState, Detection, Found, LiveFrame, LiveQuadFilter, LiveQuadTracker, LiveUpdate, Quad,
};
use std::cell::Cell;
use std::sync::Arc;

fn sheet() -> Quad {
    moved(0.0)
}

fn moved(dx: f64) -> Quad {
    Quad::from_xy([
        [80.0 + dx, 40.0],
        [240.0 + dx, 40.0],
        [240.0 + dx, 200.0],
        [80.0 + dx, 200.0],
    ])
}

fn tagged(image: &Rgba, index: usize) -> Rgba {
    let mut out = image.clone();
    out.data[3] = index as u8;
    out
}

fn sure(quad: Option<Quad>) -> Detection {
    Detection {
        quad,
        confidence: if quad.is_some() { 1.0 } else { 0.0 },
        cut_off: false,
    }
}

fn nothing(_: &Rgba, _: Option<&Quad>) -> Detection {
    Detection::default()
}

fn step(
    tracker: &mut LiveQuadTracker,
    image: &Rgba,
    t: f64,
    search: impl FnOnce(&Rgba, Option<&Quad>) -> Detection,
) -> Option<LiveUpdate> {
    tracker.process(
        &LiveFrame {
            image: Arc::new(image.clone()),
            timestamp_ms: t,
            model: None,
        },
        search,
    )
}

// the sheet 1 px either way by index, and `shift(index)` further
fn trembling(index: usize, shift: f64) -> Detection {
    sure(Some(moved(
        if index.is_multiple_of(2) { 1.0 } else { -1.0 } + shift,
    )))
}

fn run_sheet(
    tracker: &mut LiveQuadTracker,
    image: &Rgba,
    range: std::ops::Range<usize>,
    start_ms: f64,
    gone: std::ops::Range<usize>,
    mut each: impl FnMut(&mut LiveQuadTracker, usize, &LiveUpdate),
) {
    for i in range {
        let frame = tagged(image, i);
        let found = |image: &Rgba, _: Option<&Quad>| {
            let index = image.data[3] as usize;
            if gone.contains(&index) {
                Detection::default()
            } else {
                trembling(index, 0.0)
            }
        };
        if let Some(update) = step(tracker, &frame, start_ms + i as f64 * 100.0, found) {
            each(tracker, i, &update);
        }
    }
}

#[test]
fn the_filter_damps_jitter() {
    let mut filter = LiveQuadFilter::default();
    for i in 0..12 {
        let dx = if i % 2 == 0 { 2.0 } else { -2.0 };
        let out = filter.update(Some(moved(dx)), i as f64 * 100.0, 400.0);
        if i == 0 {
            assert_eq!(out.shown, None);
            assert_eq!(out.page, Some(moved(2.0)));
        } else {
            assert_eq!(out.page, out.shown);
            if i >= 3 {
                assert!(
                    out.shown.unwrap().max_corner_distance(&sheet()) <= 1.0,
                    "at {i}"
                );
            }
        }
    }
}

#[test]
fn the_filter_follows_a_real_move_within_two_frames() {
    let mut f = LiveQuadFilter::default();
    f.update(Some(sheet()), 0.0, 400.0);
    f.update(Some(sheet()), 100.0, 400.0);
    // slid 10 px: more than jitter, less than a jump
    let slid = f.update(Some(moved(10.0)), 200.0, 400.0);
    assert_eq!(
        (slid.shown, slid.page),
        (Some(moved(10.0)), Some(moved(10.0)))
    );
    // jumped 40 px: passed over once, then followed
    let jumped = f.update(Some(moved(50.0)), 300.0, 400.0);
    assert_eq!((jumped.page, jumped.shown), (None, Some(moved(10.0))));
    let again = f.update(Some(moved(51.0)), 400.0, 400.0);
    assert_eq!(
        (again.shown, again.page),
        (Some(moved(51.0)), Some(moved(51.0)))
    );
    assert_eq!(
        f.update(Some(moved(51.0)), 500.0, 400.0).shown,
        Some(moved(51.0))
    );
    // a jump seen once only changes nothing
    let stray = f.update(Some(moved(-60.0)), 600.0, 400.0);
    assert_eq!((stray.page, stray.shown), (None, Some(moved(51.0))));
    assert_eq!(
        f.update(Some(moved(51.0)), 700.0, 400.0).shown,
        Some(moved(51.0))
    );
    // corners named from another edge are the same page
    let s = moved(51.0);
    let renamed = Quad([s.0[3], s.0[0], s.0[1], s.0[2]]);
    assert_eq!(f.update(Some(renamed), 800.0, 400.0).shown, Some(s));
}

#[test]
fn the_filter_follows_a_fast_move_from_its_second_frame() {
    let mut f = LiveQuadFilter::default();
    f.update(Some(sheet()), 0.0, 400.0);
    f.update(Some(sheet()), 100.0, 400.0);
    let first = f.update(Some(moved(40.0)), 200.0, 400.0);
    assert_eq!((first.page, first.shown), (None, Some(sheet())));
    for step in 2..=5 {
        let dx = 40.0 * step as f64;
        let out = f.update(Some(moved(dx)), 100.0 + 100.0 * step as f64, 400.0);
        assert_eq!(
            (out.shown, out.page),
            (Some(moved(dx)), Some(moved(dx))),
            "step {step}"
        );
    }
}

#[test]
fn the_filter_holds_a_lost_page_then_clears_it() {
    let mut f = LiveQuadFilter::default();
    f.update(Some(sheet()), 0.0, 400.0);
    f.update(Some(sheet()), 100.0, 400.0);
    for t in [200.0, 300.0, 400.0] {
        let missed = f.update(None, t, 400.0);
        assert_eq!((missed.page, missed.shown), (None, Some(sheet())), "at {t}");
    }
    assert_eq!(f.update(None, 500.0, 400.0).shown, None);
    // found again after that: a new page, drawn from its second sighting
    let first = f.update(Some(sheet()), 600.0, 400.0);
    assert_eq!((first.shown, first.page), (None, Some(sheet())));
    // a first sighting missing on the next frame is forgotten
    f.update(None, 700.0, 400.0);
    assert_eq!(f.update(Some(sheet()), 800.0, 400.0).shown, None);
    assert_eq!(f.update(Some(sheet()), 900.0, 400.0).shown, Some(sheet()));
}

#[test]
fn a_frame_too_soon_is_passed_over_and_a_new_clock_starts_again() {
    let asked = Cell::new(0);
    let counting = |_: &Rgba, _: Option<&Quad>| {
        asked.set(asked.get() + 1);
        Detection::default()
    };
    let plain = |w, h| Rgba::filled(w, h, 90, 90, 90);
    let mut t = LiveQuadTracker::default();
    let img = plain(64, 48);
    assert!(step(&mut t, &img, 1000.0, counting).is_some());
    assert!(step(&mut t, &img, 1033.0, counting).is_none());
    assert!(step(&mut t, &img, 1089.9, counting).is_none());
    assert!(step(&mut t, &img, 1090.0, counting).is_some());
    assert_eq!(asked.get(), 2);
    // the camera started again: its clock begins anew, and so does the tracker
    assert!(step(&mut t, &img, 20.0, counting).is_some());
    assert!(step(&mut t, &img, 40.0, counting).is_none());
    assert_eq!(asked.get(), 3);
    // a frame of another size starts over too
    assert!(step(&mut t, &plain(48, 64), 200.0, counting).is_some());
    assert_eq!(asked.get(), 4);
}

#[test]
fn frames_with_no_page_show_no_outline() {
    let mut t = LiveQuadTracker::default();
    let empty = Rgba::filled(320, 240, 100, 100, 100);
    for i in 0..20 {
        let u = step(&mut t, &empty, i as f64 * 100.0, nothing).unwrap();
        assert_eq!(
            (u.outline, u.state, u.progress),
            (None, CaptureState::NoPage, 0.0)
        );
    }
}

#[test]
fn a_page_moved_after_it_held_still_is_not_steady_until_it_holds_still_again() {
    // 320 x 240 (diagonal 400): still within 10 px, holding still within 4 px a frame, a new page 60 px away
    let offset = |i: usize| match i {
        0..10 => 0.0,
        10..18 => 6.0 * (i - 9) as f64,
        _ => 48.0,
    };
    let image = quality_frame(false, 0, 255);
    let mut t = LiveQuadTracker::default();
    let mut updates = Vec::new();
    for i in 0..21 {
        let found = |_: &Rgba, _: Option<&Quad>| sure(Some(moved(offset(i))));
        updates.push(step(&mut t, &image, i as f64 * 100.0, found).expect("not passed over"));
    }
    assert!(!updates[0].is_steady);
    assert_eq!(updates[1].state, CaptureState::Steadying);
    assert!(updates[1].is_steady);
    assert_eq!(updates[7].state, CaptureState::Captured);
    assert!(updates[7].is_steady && updates[9].is_steady && updates[10].is_steady);
    for (i, u) in updates.iter().enumerate().skip(11) {
        assert_eq!(u.state, CaptureState::Captured, "at {i}");
        assert_eq!(u.is_steady, i >= 18, "at {i}");
    }
}

#[test]
fn a_photo_taken_counts_the_page_as_taken_until_a_new_page_comes() {
    let image = quality_frame(false, 0, 255);
    let mut t = LiveQuadTracker::default();
    let mut fired = Vec::new();
    for i in 0..60 {
        let found = |_: &Rgba, _: Option<&Quad>| trembling(i, if i >= 40 { 100.0 } else { 0.0 });
        if let Some(u) = step(&mut t, &image, i as f64 * 100.0, found)
            && u.should_capture
        {
            fired.push(i);
        }
        if i == 3 {
            t.mark_captured();
        }
    }
    // its jump believed on the second sighting (41), steady from 42, taken at 48
    assert_eq!(fired, [48]);
}

#[test]
fn a_pause_keeps_the_page_taken_while_a_reset_forgets_it() {
    let image = quality_frame(false, 0, 255);
    let fired_in = |t: &mut LiveQuadTracker, start: f64| {
        let mut fired = 0;
        let mut last = CaptureState::NoPage;
        run_sheet(t, &image, 0..20, start, 0..0, |_, _, u| {
            fired += u.should_capture as usize;
            last = u.state;
        });
        (fired, last)
    };
    let mut t = LiveQuadTracker::default();
    assert_eq!(fired_in(&mut t, 0.0).0, 1);
    // paused with the same page still in view: not taken again
    t.pause();
    assert_eq!(fired_in(&mut t, 5000.0), (0, CaptureState::Captured));
    // stopped: forgotten, so the page is taken again
    t.reset();
    assert_eq!(fired_in(&mut t, 10000.0).0, 1);
}

#[test]
fn rearm_judges_again_only_a_page_taken_without_a_photo() {
    let image = quality_frame(false, 0, 255);
    let mut t = LiveQuadTracker::default();
    let mut fired = Vec::new();
    run_sheet(&mut t, &image, 0..40, 0.0, 0..0, |t, i, u| {
        if u.should_capture {
            fired.push(i);
        }
        match i {
            9 => t.rearm(),
            17 => t.mark_captured(),
            20 => t.rearm(),
            _ => {}
        }
    });
    assert_eq!(fired, [7, 17]);
}

fn barred(image: &Rgba) -> Rgba {
    let mut out = image.clone();
    for y in 90..150 {
        for x in 80..240 {
            let o = (y * out.width + x) * 4;
            out.data[o..o + 3].fill(40);
        }
    }
    out
}

#[test]
fn a_page_covered_for_a_second_is_not_taken_again_but_another_page_is() {
    let sharp = quality_frame(false, 0, 255);
    let other = barred(&sharp);
    let run = |from: usize, later: &Rgba| {
        let mut t = LiveQuadTracker::default();
        let mut fired = Vec::new();
        for i in 0..70 {
            let image = if i < from { &sharp } else { later };
            let frame = tagged(image, i);
            let found = |img: &Rgba, _: Option<&Quad>| {
                let index = img.data[3] as usize;
                if (20..30).contains(&index) {
                    Detection::default()
                } else {
                    trembling(index, 0.0)
                }
            };
            if let Some(u) = step(&mut t, &frame, i as f64 * 100.0, found)
                && u.should_capture
            {
                fired.push(i);
                t.mark_captured();
            }
            if i == 40 {
                t.rearm();
            }
        }
        fired
    };
    // the hand leaves and the same page holds still again: it looks like the page taken, so it waits
    assert_eq!(run(0, &sharp), [7]);
    // another page laid in its place is taken
    let fired = run(30, &other);
    assert_eq!(fired.len(), 2);
    assert_eq!(fired[0], 7);
    assert!(fired[1] > 29);
}

#[test]
fn reset_forgets_the_page_taken_so_it_can_be_taken_again() {
    let image = quality_frame(false, 0, 255);
    let mut t = LiveQuadTracker::default();
    let mut fired = Vec::new();
    run_sheet(&mut t, &image, 0..70, 0.0, 20..30, |t, i, u| {
        if u.should_capture {
            fired.push(i);
            t.mark_captured();
        }
        if i == 25 {
            t.reset();
        }
    });
    assert_eq!(fired.len(), 2);
}

fn found(quad: Quad, w: f64, h: f64, confidence: f64) -> Found {
    Found {
        quad: quad.scaled(1.0 / w, 1.0 / h),
        confidence,
    }
}

#[test]
fn the_models_page_is_taken_when_sure_and_the_search_waits_while_the_model_answers() {
    let asked = Cell::new(0);
    let counting = |_: &Rgba, _: Option<&Quad>| {
        asked.set(asked.get() + 1);
        Detection::default()
    };
    let stub = Quad::from_xy([[10.0, 8.0], [50.0, 8.0], [50.0, 40.0], [10.0, 40.0]]);
    let image = Arc::new(Rgba::filled(64, 48, 90, 90, 90));
    let mut t = LiveQuadTracker::default();
    let feed = |t: &mut LiveQuadTracker, ms: f64, model: Option<Found>| {
        t.process(
            &LiveFrame {
                image: Arc::clone(&image),
                timestamp_ms: ms,
                model,
            },
            counting,
        )
    };
    let page = |c| Some(found(stub, 64.0, 48.0, c));
    assert_eq!(feed(&mut t, 0.0, page(0.9)).unwrap().outline, None);
    let expected = stub.scaled(1.0 / 64.0, 1.0 / 48.0);
    let second = feed(&mut t, 100.0, page(0.9)).unwrap();
    for (a, b) in second.outline.unwrap().0.iter().zip(expected.0) {
        assert!(a.distance_to(b) < 1e-9);
    }
    assert_eq!(asked.get(), 0);
    // the model misses it for up to 300 ms: the search waits and the outline is held
    assert!(feed(&mut t, 200.0, page(0.3)).unwrap().outline.is_some());
    let sliver = Quad::from_xy([[10.0, 20.0], [54.0, 20.0], [54.0, 24.0], [10.0, 24.0]]);
    feed(&mut t, 300.0, Some(found(sliver, 64.0, 48.0, 0.9)));
    feed(&mut t, 400.0, None);
    assert_eq!(asked.get(), 0);
    feed(&mut t, 500.0, None);
    assert_eq!(asked.get(), 1);
    // less sure than holding still needs (0.6): never taken, so the search looks
    feed(&mut t, 600.0, page(0.55));
    assert_eq!(asked.get(), 2);
}

#[test]
fn the_models_corners_come_out_from_the_top_left_whatever_order_it_gave() {
    let stub = Quad::from_xy([[10.0, 8.0], [50.0, 8.0], [50.0, 40.0], [10.0, 40.0]]);
    // the model's first corner is the page's top right
    let turned = Quad([stub.0[1], stub.0[2], stub.0[3], stub.0[0]]);
    let image = Arc::new(Rgba::filled(64, 48, 90, 90, 90));
    let mut t = LiveQuadTracker::default();
    let mut last = None;
    for ms in [0.0, 100.0] {
        let frame = LiveFrame {
            image: Arc::clone(&image),
            timestamp_ms: ms,
            model: Some(found(turned, 64.0, 48.0, 0.9)),
        };
        last = t.process(&frame, nothing).unwrap().outline;
    }
    let expected = stub.scaled(1.0 / 64.0, 1.0 / 48.0);
    for (a, b) in last.unwrap().0.iter().zip(expected.0) {
        assert!(a.distance_to(b) < 1e-9);
    }
}

#[test]
fn the_model_flickering_does_not_hand_the_outline_to_the_search() {
    let asked = Cell::new(0);
    let image = Arc::new(quality_frame(false, 0, 255));
    let mut t = LiveQuadTracker::default();
    let mut outlines = Vec::new();
    for i in 0..20usize {
        let model = (i < 2 || i % 2 == 1).then(|| found(sheet(), 320.0, 240.0, 0.9));
        let u = t
            .process(
                &LiveFrame {
                    image: Arc::clone(&image),
                    timestamp_ms: i as f64 * 100.0,
                    model,
                },
                |_, _| {
                    asked.set(asked.get() + 1);
                    sure(Some(moved(5.0)))
                },
            )
            .unwrap();
        outlines.push(u.outline);
    }
    assert_eq!(asked.get(), 0);
    let want = sheet().scaled(1.0 / 320.0, 1.0 / 240.0);
    for (i, o) in outlines.iter().enumerate().skip(1) {
        let o = o.unwrap_or_else(|| panic!("no outline at {i}"));
        assert!(o.max_corner_distance(&want) < 1e-9, "at {i}");
    }
}
