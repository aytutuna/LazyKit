mod common;

use common::scenes::{QUALITY_H, QUALITY_W, quality_frame, quality_sheet};
use lazy_image::Gray;
use lazy_scan::{
    CaptureQuality, CaptureStability, CaptureState, Observation, Point, Quad, frame_quality,
};

fn region(v: &serde_json::Value) -> Option<Quad> {
    v.as_array().map(|c| {
        Quad::from_xy(std::array::from_fn(|i| {
            [c[i][0].as_f64().unwrap(), c[i][1].as_f64().unwrap()]
        }))
    })
}

fn judged(sharpness: f64, brightness: f64) -> CaptureQuality {
    CaptureStability::judge(Some(sharpness), Some(brightness), &Default::default())
}

#[test]
fn the_frames_and_readings_match_the_apps() {
    let data = common::json("quality.json");
    for f in data["frames"].as_array().unwrap() {
        let frame = quality_frame(
            f["printed"].as_bool().unwrap(),
            f["blur"].as_u64().unwrap() as usize,
            f["dim"].as_u64().unwrap() as u32,
        );
        let name = f["name"].as_str().unwrap();
        if let Some(fnv) = f["fnv"].as_str() {
            assert_eq!(common::sum(&frame.data), fnv, "{name}");
        }
        let Some(sharpness) = f["sharpness"].as_f64() else {
            continue;
        };
        let quad = (f["region"] == "sheet").then(quality_sheet);
        let r = frame_quality::measure(&frame, quad.as_ref());
        assert!((r.sharpness - sharpness).abs() < 1e-9, "{name}");
        assert!(
            (r.brightness - f["brightness"].as_f64().unwrap()).abs() < 1e-9,
            "{name}"
        );
        if let Some(q) = f["quality"].as_str() {
            let want = match q {
                "good" => CaptureQuality::Good,
                "blurry" => CaptureQuality::Blurry,
                _ => CaptureQuality::Dark,
            };
            assert_eq!(judged(r.sharpness, r.brightness), want, "{name}");
        }
    }
}

#[test]
fn the_area_is_the_pages_box_with_a_margin() {
    let data = common::json("quality.json");
    for a in data["areas"].as_array().unwrap() {
        let got = frame_quality::area(region(&a["quad"]).as_ref(), QUALITY_W, QUALITY_H);
        let want: Vec<usize> = a["area"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_u64().unwrap() as usize)
            .collect();
        assert_eq!([got.x0, got.y0, got.x1, got.y1], want[..], "{}", a["name"]);
    }
    let broken = Quad::from_xy([[f64::NAN, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]]);
    let got = frame_quality::area(Some(&broken), QUALITY_W, QUALITY_H);
    assert_eq!((got.x1, got.y1), (QUALITY_W, QUALITY_H));
}

#[test]
fn the_sharpness_is_the_variance_of_the_laplacian() {
    // one bright pixel in 5 x 5: the inner 3 x 3 Laplacian is 400, four times -100 and four zeros, mean 0
    let mut data = vec![0u8; 25];
    data[12] = 100;
    let r = frame_quality::measure_gray(
        &Gray {
            width: 5,
            height: 5,
            data,
        },
        None,
    );
    assert!((r.sharpness - 200_000.0 / 9.0).abs() < 1e-9);
    assert_eq!(r.brightness, 4.0);
    let flat = |w, h| Gray {
        width: w,
        height: h,
        data: vec![90; w * h],
    };
    assert_eq!(
        frame_quality::measure_gray(&flat(8, 6), None).sharpness,
        0.0
    );
    let tiny = frame_quality::measure_gray(&flat(2, 2), None);
    assert_eq!((tiny.sharpness, tiny.brightness), (0.0, 90.0));
}

#[test]
fn the_shutter_fires_only_for_a_steady_sharp_lit_page() {
    let blurred = quality_frame(false, 2, 255);
    let dark = quality_frame(true, 0, 40);
    let sharp = quality_frame(true, 0, 255);
    let sheet = quality_sheet();
    let mut stability = CaptureStability::default();
    let mut fired = Vec::new();
    let mut log = Vec::new();
    for i in 0..60usize {
        let frame = match i {
            0..12 => &blurred,
            12..18 => &dark,
            _ => &sharp,
        };
        let dx = if i.is_multiple_of(2) { 1.0 } else { -1.0 } + if i >= 40 { 70.0 } else { 0.0 };
        let quad = Quad(sheet.0.map(|p| Point::new(p.x + dx, p.y)));
        let r = frame_quality::measure(frame, Some(&quad));
        let d = stability.update(&Observation {
            quad: Some(quad),
            confidence: 0.9,
            timestamp_ms: i as f64 * 100.0,
            frame_width: QUALITY_W as u32,
            frame_height: QUALITY_H as u32,
            sharpness: Some(r.sharpness),
            brightness: Some(r.brightness),
        });
        if d.should_capture {
            fired.push(i);
        }
        log.push(d);
    }
    let want: Vec<usize> = common::json("quality.json")["shutter"]["fired"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as usize)
        .collect();
    assert_eq!(fired, want);
    assert_eq!(log[7].quality, CaptureQuality::Blurry);
    assert_eq!(log[11].state, CaptureState::Steadying);
    assert_eq!(log[11].progress, 1.0);
    assert_eq!(log[12].quality, CaptureQuality::Dark);
    assert_eq!(log[30].state, CaptureState::Captured);
}
