mod common;

use lazy_image::Rgba;
use lazy_scan::Quad;
use lazy_scan::warp::{self, Options, Paper, Supersampling};
use serde_json::Value;

fn fixture(w: usize, h: usize) -> Rgba {
    let mut data = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            data[i] = ((x * 37 + y * 11) % 256) as u8;
            let checker = (x / 4 + y / 4) % 2 == 0;
            data[i + 1] = if checker {
                40 + (x * 3) % 160
            } else {
                220 - (y * 5) % 120
            } as u8;
            data[i + 2] = ((x * y + 3 * x + 7 * y) % 256) as u8;
            data[i + 3] = 255;
        }
    }
    Rgba::new(w, h, data)
}

fn quad(v: &Value) -> Quad {
    let n: Vec<f64> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    Quad::from_xy([[n[0], n[1]], [n[2], n[3]], [n[4], n[5]], [n[6], n[7]]])
}

fn options(v: &Value) -> Options {
    let mut o = Options::default();
    if v["snap"] == false {
        o.snap_to_paper = false;
    }
    if let Some(n) = v["maxLongSide"].as_u64() {
        o.max_long_side = n as usize;
    }
    o.supersample = match v["supersample"].as_str() {
        Some("off") => Supersampling::Off,
        Some("on") => Supersampling::On,
        _ => Supersampling::Auto,
    };
    if let Some(f) = v["fill"].as_array() {
        o.fill = std::array::from_fn(|i| f[i].as_u64().unwrap() as u8);
    }
    o
}

fn pair(v: &Value) -> (usize, usize) {
    let a = v.as_array().unwrap();
    (
        a[0].as_u64().unwrap() as usize,
        a[1].as_u64().unwrap() as usize,
    )
}

fn size_of(v: &Value) -> (usize, usize) {
    (
        v["width"].as_u64().unwrap() as usize,
        v["height"].as_u64().unwrap() as usize,
    )
}

fn near(a: f64, b: f64, what: &str) {
    assert!(
        a == b || (a - b).abs() <= 1e-12 * b.abs(),
        "{what}: {a} against {b}"
    );
}

#[test]
fn plans_follow_the_apps() {
    let data = common::json("warp.json");
    for p in data["plans"].as_array().unwrap() {
        let name = format!("{} {}", p["name"], p["options"]);
        let (w, h) = pair(&p["photo"]);
        let q = quad(&p["quad"]);
        let est = warp::estimate_aspect(&q, w as f64, h as f64);
        near(
            est.width_over_height,
            p["estimate"].as_f64().unwrap(),
            &name,
        );
        match p["focal"].as_f64() {
            Some(f) => near(est.focal_length.expect(&name), f, &name),
            None => assert_eq!(est.focal_length, None, "{name}"),
        }
        let plan = warp::plan(&q, w, h, &options(&p["options"])).unwrap();
        assert_eq!((plan.width, plan.height), size_of(p), "{name}");
        let paper = plan.paper.map(|m| match m.paper {
            Paper::A4 => "A4",
            Paper::Letter => "Letter",
            Paper::Legal => "Legal",
            Paper::IdCard => "IdCard",
        });
        assert_eq!(paper, p["paper"].as_str(), "{name}");
        assert_eq!(plan.dpi.is_some(), !p["dpi"].is_null(), "{name}");
        if let Some(d) = p["dpi"].as_f64() {
            near(plan.dpi.unwrap(), d, &name);
        }
        assert_eq!(plan.supersampled, p["supersampled"], "{name}");
        for (i, m) in p["map"].as_array().unwrap().iter().enumerate() {
            near(plan.output_to_source.0[i], m.as_f64().unwrap(), &name);
        }
    }
}

#[test]
fn warped_bytes_are_the_apps() {
    let data = common::json("warp.json");
    for c in data["warps"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        let (pw, ph) = pair(&c["photo"]);
        let photo = fixture(pw, ph);
        let q = quad(&c["quad"]);
        let opts = options(&c["options"]);
        let got = if c["output"].is_null() {
            warp::correct(&photo, &q, &opts).map(|(i, _)| i)
        } else {
            let (w, h) = pair(&c["output"]);
            warp::warp(&photo, &q, w, h, &opts)
        };
        match c["fnv"].as_str() {
            None => assert!(got.is_none(), "{name}"),
            Some(fnv) => {
                let got = got.unwrap_or_else(|| panic!("{name}: no output"));
                assert_eq!((got.width, got.height), size_of(c), "{name}");
                assert_eq!(common::sum(&got.data), fnv, "{name}");
            }
        }
    }
}

#[test]
fn turning_matches_the_apps() {
    let data = common::json("warp.json");
    for r in data["rotations"].as_array().unwrap() {
        let (w, h) = pair(&r["photo"]);
        let turned = fixture(w, h).rotated_quarter_turns(r["turns"].as_i64().unwrap() as i32);
        assert_eq!((turned.width, turned.height), size_of(r));
        assert_eq!(common::sum(&turned.data), r["fnv"].as_str().unwrap(), "{r}");
    }
}

#[test]
fn a_known_focal_length_is_used() {
    let data = common::json("warp.json");
    for f in data["focal"].as_array().unwrap() {
        let (w, h) = pair(&f["photo"]);
        let e = warp::estimate_aspect_with_focal(
            &quad(&f["quad"]),
            w as f64,
            h as f64,
            f["focal"].as_f64().unwrap(),
        );
        near(
            e.width_over_height,
            f["estimate"].as_f64().unwrap(),
            "focal",
        );
        assert_eq!(e.focal_length, f["used"].as_f64());
    }
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Quad {
    Quad::from_xy([[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

#[test]
fn the_prefilter_starts_above_twice_smaller_and_takes_the_whole_part() {
    let q = Quad::from_xy([
        [20.5, 15.25],
        [220.0, 30.75],
        [205.25, 170.5],
        [12.0, 150.0],
    ]);
    assert_eq!(warp::prefilter_factor(&q, 48, 36), 4);
    assert_eq!(warp::prefilter_factor(&q, 120, 90), 1);
    assert_eq!(warp::prefilter_factor(&q, 90, 60), 2);
    assert_eq!(warp::prefilter_factor(&q, 400, 300), 1);
    assert_eq!(warp::prefilter_factor(&q, 0, 36), 1);
    // exactly twice smaller is not more than twice
    let square = rect(0.0, 0.0, 100.0, 100.0);
    assert_eq!(warp::prefilter_factor(&square, 50, 50), 1);
    assert_eq!(warp::prefilter_factor(&square, 49, 49), 2);
    assert_eq!(warp::prefilter_factor(&square, 33, 50), 3);
}

#[test]
fn box_reduction_averages_whole_and_edge_blocks_rounding_halves_up() {
    // 5 x 3 in blocks of 2: a 3 x 2 result whose last column and row hold fewer pixels
    let mut p = vec![0u8; 5 * 3 * 4];
    for i in 0..15 {
        p[i * 4] = (i * 10) as u8;
        p[i * 4 + 1] = (i % 2) as u8;
        p[i * 4 + 2] = 255;
        p[i * 4 + 3] = if i == 0 { 0 } else { 255 };
    }
    let reduced = warp::box_reduced(&Rgba::new(5, 3, p), 2);
    assert_eq!((reduced.width, reduced.height), (3, 2));
    let px = |x: usize, y: usize| reduced.data[(y * 3 + x) * 4..(y * 3 + x) * 4 + 4].to_vec();
    // block (0, 0): pixels 0, 1, 5, 6 give R 30, G 0.5 to 1, A 191.25 to 191
    assert_eq!(px(0, 0), [30, 1, 255, 191]);
    assert_eq!(px(2, 0), [65, 1, 255, 255]);
    assert_eq!(px(1, 1), [125, 1, 255, 255]);
    assert_eq!(px(2, 1), [140, 0, 255, 255]);
    let image = fixture(7, 5);
    assert_eq!(warp::box_reduced(&image, 1), image);
}

#[test]
fn a_block_of_many_pixels_does_not_overflow_a_32_bit_sum() {
    // 4200 x 4200 is 17.6M pixels of 255, over u32::MAX once added up
    let image = Rgba::filled(4200, 4200, 255, 255, 255);
    let reduced = warp::box_reduced(&image, 4200);
    assert_eq!(reduced.data, [255, 255, 255, 255]);
}

#[test]
fn no_warp_for_an_outline_that_is_not_convex_or_a_size_out_of_range() {
    let photo = fixture(32, 24);
    let o = Options::default();
    let bent = Quad::from_xy([[0.0, 0.0], [30.0, 0.0], [10.0, 5.0], [0.0, 20.0]]);
    assert!(warp::warp(&photo, &bent, 10, 10, &o).is_none());
    let backwards = Quad::from_xy([[0.0, 20.0], [30.0, 20.0], [30.0, 0.0], [0.0, 0.0]]);
    assert!(warp::warp(&photo, &backwards, 10, 10, &o).is_none());
    let ok = rect(0.0, 0.0, 30.0, 20.0);
    assert!(warp::warp(&photo, &ok, 0, 10, &o).is_none());
    assert!(warp::warp(&photo, &ok, 10, warp::MAX_OUTPUT_SIDE + 1, &o).is_none());
}

#[test]
fn a_page_made_much_smaller_is_averaged_not_aliased() {
    // a fine 2 px checkerboard seen four times larger than the output averages to an even grey
    let (w, h) = (400, 300);
    let mut p = vec![255u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let v = if (x / 2 + y / 2) % 2 == 0 { 30 } else { 230 };
            p[(y * w + x) * 4..(y * w + x) * 4 + 3].fill(v);
        }
    }
    let photo = Rgba::new(w, h, p);
    let q = Quad::from_xy([[10.0, 10.0], [390.0, 18.0], [385.0, 290.0], [14.0, 284.0]]);
    assert_eq!(warp::prefilter_factor(&q, 90, 66), 4);
    let warped = warp::warp(&photo, &q, 90, 66, &Options::default()).unwrap();
    let mut worst = 0i32;
    for y in 2..64 {
        for x in 2..88 {
            for c in 0..3 {
                worst = worst.max((warped.data[(y * 90 + x) * 4 + c] as i32 - 130).abs());
            }
        }
    }
    assert!(worst <= 12, "{worst}");
}
