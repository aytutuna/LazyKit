mod common;

use lazy_scan::{
    CapturePlacement, CaptureQuality, CaptureStability, CaptureState, Decision, Observation,
    Options, Point, Quad,
};

const W: u32 = 1000;
const H: u32 = 750;
const FRAME_MS: f64 = 33.0;

fn page() -> Quad {
    Quad::from_xy([
        [200.0, 100.0],
        [780.0, 130.0],
        [800.0, 650.0],
        [180.0, 640.0],
    ])
}

fn shifted(q: &Quad, dx: f64, dy: f64) -> Quad {
    Quad(q.0.map(|p| Point::new(p.x + dx, p.y + dy)))
}

type Frames = Vec<(Option<Quad>, f64)>;

// the page held with a tremor of one pixel, `count` frames
fn held(q: &Quad, count: usize, confidence: f64) -> Frames {
    (0..count)
        .map(|i| {
            (
                Some(shifted(
                    q,
                    if i % 2 == 0 { 1.0 } else { -1.0 },
                    (i % 3) as f64 - 1.0,
                )),
                confidence,
            )
        })
        .collect()
}

fn gap(count: usize) -> Frames {
    vec![(None, 0.0); count]
}

fn frame(index: usize, quad: Option<Quad>, confidence: f64) -> Observation {
    Observation {
        quad,
        confidence,
        timestamp_ms: index as f64 * FRAME_MS,
        frame_width: W,
        frame_height: H,
        sharpness: None,
        brightness: None,
    }
}

fn run(
    s: &mut CaptureStability,
    start: usize,
    frames: &[(Option<Quad>, f64)],
) -> (Vec<usize>, Vec<Decision>) {
    let mut fired = Vec::new();
    let mut log = Vec::new();
    for (i, (quad, confidence)) in frames.iter().enumerate() {
        let d = s.update(&frame(start + i, *quad, *confidence));
        if d.should_capture {
            fired.push(start + i);
        }
        log.push(d);
    }
    (fired, log)
}

fn fired(s: &mut CaptureStability, start: usize, frames: &[(Option<Quad>, f64)]) -> Vec<usize> {
    run(s, start, frames).0
}

fn readings(sharpness: f64, brightness: f64, frames: &Frames, start: usize) -> Vec<Observation> {
    frames
        .iter()
        .enumerate()
        .map(|(i, (q, c))| Observation {
            sharpness: Some(sharpness),
            brightness: Some(brightness),
            ..frame(start + i, *q, *c)
        })
        .collect()
}

fn state_name(s: CaptureState) -> &'static str {
    match s {
        CaptureState::NoPage => "noPage",
        CaptureState::Moving => "moving",
        CaptureState::Steadying => "steadying",
        CaptureState::Captured => "captured",
    }
}

#[test]
fn the_scripted_sequences_fire_where_the_apps_do() {
    let data = common::json("stability.json");
    let q = {
        let c = &data["page"];
        Quad::from_xy(std::array::from_fn(|i| {
            [c[i][0].as_f64().unwrap(), c[i][1].as_f64().unwrap()]
        }))
    };
    for scenario in data["scenarios"].as_array().unwrap() {
        let mut frames: Frames = Vec::new();
        for seg in scenario["segments"].as_array().unwrap() {
            let num = |k: &str, d: f64| seg[k].as_f64().unwrap_or(d);
            if let Some(n) = seg["held"].as_u64() {
                let base = shifted(&q, num("dx", 0.0), num("dy", 0.0));
                frames.extend(held(&base, n as usize, num("confidence", 0.9)));
            } else if let Some(n) = seg["gap"].as_u64() {
                frames.extend(gap(n as usize));
            } else if let Some(n) = seg["drift"].as_u64() {
                frames.extend(
                    (0..n).map(|i| (Some(shifted(&q, i as f64 * num("step", 1.0), 0.0)), 0.9)),
                );
            } else if let Some(n) = seg["slide"].as_u64() {
                let wrap = num("wrap", 1.0) as u64;
                frames.extend((0..n).map(|i| {
                    (
                        Some(shifted(&q, num("step", 0.0) * (i % wrap) as f64, 0.0)),
                        0.9,
                    )
                }));
            }
        }
        let mut s = CaptureStability::default();
        let got = fired(&mut s, 0, &frames);
        let want: Vec<usize> = scenario["fired"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as usize)
            .collect();
        assert_eq!(got, want, "{}", scenario["name"]);
        assert_eq!(
            state_name(s.state()),
            scenario["state"].as_str().unwrap(),
            "{}",
            scenario["name"]
        );
    }
}

#[test]
fn progress_climbs_while_steadying() {
    let mut s = CaptureStability::default();
    let (fired, log) = run(&mut s, 0, &held(&page(), 60, 0.9));
    assert_eq!(fired, vec![20]);
    assert_eq!(log[0].state, CaptureState::Moving);
    assert_eq!(log[1].state, CaptureState::Steadying);
    assert!(log[10].progress > 0.45 && log[10].progress < 0.6);
    for i in 2..20 {
        assert!(log[i].progress >= log[i - 1].progress);
    }
}

#[test]
fn the_page_is_the_same_whichever_corner_its_quad_names_first() {
    let p = page();
    let renamed = Quad([p.0[1], p.0[2], p.0[3], p.0[0]]);
    assert!(p.max_corner_distance(&renamed) > 500.0);
    assert_eq!(p.movement(&renamed), 0.0);
    let frames: Frames = held(&p, 120, 0.9)
        .into_iter()
        .enumerate()
        .map(|(i, (q, c))| {
            let q = q.unwrap();
            (
                Some(if i % 2 == 0 {
                    q
                } else {
                    Quad([q.0[1], q.0[2], q.0[3], q.0[0]])
                }),
                c,
            )
        })
        .collect();
    let mut s = CaptureStability::default();
    assert_eq!(fired(&mut s, 0, &frames), vec![20]);
}

#[test]
fn a_sheet_turned_where_its_corners_are_renamed_is_still_held_still() {
    // an A4 sheet turned 54.6 and 54.8 degrees, on either side of the angle where from_corners starts from other corners
    let turned = |degrees: f64| {
        let a = degrees.to_radians();
        let (c, s) = (a.cos(), a.sin());
        let corners = [
            (-210.0, -297.0),
            (210.0, -297.0),
            (210.0, 297.0),
            (-210.0, 297.0),
        ]
        .map(|(u, v): (f64, f64)| Point::new(500.0 + u * c - v * s, 375.0 + u * s + v * c));
        Quad::from_corners(corners)
    };
    let (a, b) = (turned(54.6), turned(54.8));
    assert!(a.max_corner_distance(&b) > 200.0);
    assert!(a.movement(&b) < 3.0);
    let frames: Frames = (0..61)
        .map(|i| (Some(if i % 2 == 0 { a } else { b }), 0.9))
        .collect();
    assert_eq!(
        fired(&mut CaptureStability::default(), 0, &frames),
        vec![20]
    );
}

#[test]
fn a_pause_between_frames_does_not_take_the_same_page_again() {
    let mut s = CaptureStability::default();
    let mut frames = held(&page(), 61, 0.9);
    frames.extend(held(&shifted(&page(), 200.0, 0.0), 40, 0.9));
    let mut fired = Vec::new();
    for (i, (q, c)) in frames.iter().enumerate() {
        let t = i as f64 * FRAME_MS + if i > 20 { 300.0 } else { 0.0 };
        let d = s.update(&Observation {
            timestamp_ms: t,
            ..frame(0, *q, *c)
        });
        if d.should_capture {
            fired.push(i);
        }
    }
    assert_eq!(fired, vec![20, 81]);
}

#[test]
fn reset_arms_the_shutter_again() {
    let mut s = CaptureStability::default();
    assert_eq!(fired(&mut s, 0, &held(&page(), 30, 0.9)), vec![20]);
    s.reset();
    assert_eq!(s.state(), CaptureState::NoPage);
    assert_eq!(fired(&mut s, 30, &held(&page(), 30, 0.9)), vec![50]);
}

fn run_with(s: &mut CaptureStability, obs: &[Observation], log: &mut Vec<Decision>) -> Vec<usize> {
    let mut fired = Vec::new();
    for o in obs {
        let d = s.update(o);
        if d.should_capture {
            fired.push(o.timestamp_ms as usize / FRAME_MS as usize);
        }
        log.push(d);
    }
    fired
}

#[test]
fn a_held_page_waits_for_a_sharp_frame() {
    let mut s = CaptureStability::default();
    let mut log = Vec::new();
    assert!(
        run_with(
            &mut s,
            &readings(10.0, 180.0, &held(&page(), 40, 0.9), 0),
            &mut log
        )
        .is_empty()
    );
    for d in &log[20..40] {
        assert_eq!(d.state, CaptureState::Steadying);
        assert_eq!(d.progress, 1.0);
        assert_eq!(d.quality, CaptureQuality::Blurry);
    }
    assert_eq!(
        run_with(
            &mut s,
            &readings(400.0, 180.0, &held(&page(), 30, 0.9), 40),
            &mut log
        ),
        vec![40]
    );
    assert_eq!(log[40].quality, CaptureQuality::Good);
    assert_eq!(s.state(), CaptureState::Captured);
}

#[test]
fn a_held_page_waits_for_light() {
    let mut s = CaptureStability::default();
    let mut log = Vec::new();
    assert!(
        run_with(
            &mut s,
            &readings(400.0, 20.0, &held(&page(), 30, 0.9), 0),
            &mut log
        )
        .is_empty()
    );
    assert_eq!(log[29].quality, CaptureQuality::Dark);
    assert_eq!(log[29].state, CaptureState::Steadying);
    assert_eq!(
        run_with(
            &mut s,
            &readings(400.0, 120.0, &held(&page(), 10, 0.9), 30),
            &mut log
        ),
        vec![30]
    );
    let mut moving = CaptureStability::default();
    let d = moving.update(&Observation {
        sharpness: Some(400.0),
        brightness: Some(20.0),
        ..frame(0, Some(page()), 0.9)
    });
    assert_eq!(d.quality, CaptureQuality::Dark);
    assert_eq!(d.state, CaptureState::Moving);
}

#[test]
fn a_steady_blurred_page_suggests_the_shutter_after_two_seconds() {
    let mut s = CaptureStability::default();
    let mut log = Vec::new();
    assert!(
        run_with(
            &mut s,
            &readings(10.0, 180.0, &held(&page(), 100, 0.9), 0),
            &mut log
        )
        .is_empty()
    );
    assert!(log[20..81].iter().all(|d| !d.suggests_shutter));
    assert!(log[81..100].iter().all(|d| d.suggests_shutter));
    // darkness is another wait: no suggestion, and the time starts over
    assert!(
        run_with(
            &mut s,
            &readings(400.0, 20.0, &held(&page(), 5, 0.9), 100),
            &mut log
        )
        .is_empty()
    );
    assert!(!log[104].suggests_shutter);
    assert!(
        run_with(
            &mut s,
            &readings(10.0, 180.0, &held(&page(), 5, 0.9), 105),
            &mut log
        )
        .is_empty()
    );
    assert!(!log[109].suggests_shutter);
    assert_eq!(
        run_with(
            &mut s,
            &readings(400.0, 180.0, &held(&page(), 5, 0.9), 110),
            &mut log
        ),
        vec![110]
    );
}

#[test]
fn readings_are_judged_dark_first() {
    let o = Options::default();
    let judge = |s, b| CaptureStability::judge(s, b, &o);
    assert_eq!(judge(Some(10.0), Some(20.0)), CaptureQuality::Dark);
    assert_eq!(judge(Some(10.0), Some(120.0)), CaptureQuality::Blurry);
    assert_eq!(judge(Some(40.0), Some(50.0)), CaptureQuality::Good);
    assert_eq!(judge(Some(39.99), Some(50.0)), CaptureQuality::Blurry);
    assert_eq!(judge(Some(400.0), Some(49.99)), CaptureQuality::Dark);
    assert_eq!(judge(None, None), CaptureQuality::Good);
    assert_eq!(judge(None, Some(120.0)), CaptureQuality::Good);
    let strict = Options {
        min_sharpness: 500.0,
        min_brightness: 10.0,
        ..o
    };
    assert_eq!(
        CaptureStability::judge(Some(400.0), Some(20.0), &strict),
        CaptureQuality::Blurry
    );
}

#[test]
fn the_placement_names_a_far_or_cut_off_page() {
    assert_eq!(
        CaptureStability::placement(&page(), W, H),
        CapturePlacement::Fine
    );
    let small = Quad::from_xy([
        [350.0, 275.0],
        [650.0, 275.0],
        [650.0, 475.0],
        [350.0, 475.0],
    ]);
    assert_eq!(
        CaptureStability::placement(&small, W, H),
        CapturePlacement::Far
    );
    assert_eq!(
        CaptureStability::placement(&shifted(&page(), -170.0, 0.0), W, H),
        CapturePlacement::CutOff
    );
    assert_eq!(
        CaptureStability::placement(&shifted(&page(), -160.0, 0.0), W, H),
        CapturePlacement::Fine
    );
    assert_eq!(
        CaptureStability::placement(&shifted(&page(), 0.0, 100.0), W, H),
        CapturePlacement::CutOff
    );
}

#[test]
fn a_photo_taken_by_hand_counts_as_taken() {
    let mut s = CaptureStability::default();
    s.mark_captured();
    assert_eq!(s.state(), CaptureState::NoPage);
    assert!(fired(&mut s, 0, &held(&page(), 10, 0.9)).is_empty());
    s.mark_captured();
    assert_eq!(s.state(), CaptureState::Captured);
    assert!(fired(&mut s, 10, &held(&page(), 60, 0.9)).is_empty());
    assert_eq!(
        fired(&mut s, 70, &held(&shifted(&page(), 200.0, 0.0), 40, 0.9)),
        vec![90]
    );
}

#[test]
fn a_pause_is_neither_time_missing_nor_time_held_still() {
    let mut s = CaptureStability::default();
    assert_eq!(fired(&mut s, 0, &held(&page(), 30, 0.9)), vec![20]);
    s.resume(200.0 * FRAME_MS);
    // the first frame back misses the page: the page taken stays taken, and held again it is not taken again
    assert_eq!(
        s.update(&frame(200, None, 0.0)).state,
        CaptureState::Captured
    );
    assert!(fired(&mut s, 201, &held(&page(), 60, 0.9)).is_empty());
    assert_eq!(s.state(), CaptureState::Captured);

    // without resume the pause counts as the page gone, and the same page is taken twice
    let mut unresumed = CaptureStability::default();
    assert_eq!(fired(&mut unresumed, 0, &held(&page(), 30, 0.9)), vec![20]);
    assert_eq!(
        unresumed.update(&frame(200, None, 0.0)).state,
        CaptureState::NoPage
    );
    assert_eq!(
        fired(&mut unresumed, 201, &held(&page(), 60, 0.9)),
        vec![221]
    );

    // a page that went meanwhile is gone once frames without it span the gap, as ever
    let mut gone = CaptureStability::default();
    fired(&mut gone, 0, &held(&page(), 30, 0.9));
    gone.resume(200.0 * FRAME_MS);
    let (_, log) = run(&mut gone, 200, &gap(9));
    assert_eq!(log[7].state, CaptureState::Captured);
    assert_eq!(log[8].state, CaptureState::NoPage);

    // a page holding still when the camera stopped goes on from where it was
    let mut steady = CaptureStability::default();
    assert!(fired(&mut steady, 0, &held(&page(), 10, 0.9)).is_empty());
    steady.resume(300.0 * FRAME_MS);
    assert_eq!(fired(&mut steady, 300, &held(&page(), 20, 0.9)), vec![311]);

    // nothing to resume before any frame
    let mut fresh = CaptureStability::default();
    fresh.resume(1000.0);
    assert_eq!(fired(&mut fresh, 0, &held(&page(), 30, 0.9)), vec![20]);
}
