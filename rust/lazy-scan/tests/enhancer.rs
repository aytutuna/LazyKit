mod common;

use common::{json, sum};
use lazy_image::{Rgba, luma};
use lazy_scan::{DEFAULT_THRESHOLD, EnhanceMode, enhance, is_light_on_dark};
use serde_json::Value;

const PAPER: [i32; 3] = [232, 226, 212];
const INK: [i32; 3] = [40, 38, 44];
const STAMP: [i32; 3] = [196, 48, 44];
const DARK: [i32; 3] = [70, 74, 80];

fn is_ink(x: usize, y: usize, w: usize, h: usize) -> bool {
    x >= 24 && x < w - 24 && y >= 70 && y < h - 30 && (y - 70) % 12 < 3 && (x - 24) / 18 % 5 != 4
}

fn is_light_print(x: usize, y: usize, w: usize, h: usize) -> bool {
    x >= 24 && x < w - 24 && y >= 70 && y < h - 30 && (y - 70) % 12 < 1 && (x - 24) / 18 % 5 != 4
}

fn is_few_lines(x: usize, y: usize, w: usize) -> bool {
    x >= 24 && x < w - 24 && (70..130).contains(&y) && (y - 70) % 12 < 2 && (x - 24) / 18 % 5 != 4
}

fn drawn(w: usize, h: usize, pixel: impl Fn(usize, usize) -> [u8; 3]) -> Rgba {
    let mut data = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = pixel(x, y);
            data.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Rgba::new(w, h, data)
}

fn lit(c: [i32; 3], s: i32) -> [u8; 3] {
    [
        ((c[0] * s + 127) / 255) as u8,
        ((c[1] * s + 127) / 255) as u8,
        ((c[2] * s + 127) / 255) as u8,
    ]
}

fn page(spec: &Value) -> Rgba {
    let (w, h) = (240usize, 320usize);
    let flag = |k: &str| spec[k].as_bool().unwrap_or(false);
    match spec["kind"].as_str().unwrap() {
        "shadowed" => {
            let (stamp, picture) = (flag("stamp"), flag("picture"));
            drawn(w, h, |x, y| {
                let mut c = PAPER;
                if is_ink(x, y, w, h) {
                    c = INK;
                }
                if stamp && (150..200).contains(&x) && (24..52).contains(&y) {
                    c = STAMP;
                }
                if picture && (40..140).contains(&x) && (200..290).contains(&y) {
                    c = DARK;
                }
                lit(
                    c,
                    255 - (130 * x / (w - 1)) as i32 - (30 * y / (h - 1)) as i32,
                )
            })
        }
        "dark-ground" => drawn(w, h, |x, y| {
            let v = if is_light_print(x, y, w, h) { 235 } else { 55 };
            [v, v, v]
        }),
        "hard-shadow" => {
            let edge = spec["edge"].as_u64().unwrap() as usize;
            let print = spec["print"].as_str().unwrap();
            drawn(w, h, |x, y| {
                let printed = match print {
                    "lines" => is_ink(x, y, w, h),
                    "few-lines" => is_few_lines(x, y, w),
                    _ => false,
                };
                lit(
                    if printed { INK } else { PAPER },
                    if x < edge { 255 } else { 115 },
                )
            })
        }
        "picture-caption" => drawn(w, h, |x, y| {
            let big = (30..210).contains(&x) && (50..230).contains(&y);
            let caption = (40..200).contains(&x)
                && (262..290).contains(&y)
                && (y - 262) % 12 < 2
                && (x - 40) / 18 % 5 != 4;
            let c = if big {
                DARK
            } else if caption {
                INK
            } else {
                PAPER
            };
            [c[0] as u8, c[1] as u8, c[2] as u8]
        }),
        "ramp" => {
            let (from, to) = (
                spec["from"].as_u64().unwrap() as usize,
                spec["to"].as_u64().unwrap() as usize,
            );
            drawn(to - from + 1, 1, |x, _| {
                let v = (from + x) as u8;
                [v, v, v]
            })
        }
        other => panic!("unknown page kind {other}"),
    }
}

fn mode(name: &str) -> EnhanceMode {
    match name {
        "auto" => EnhanceMode::Auto,
        "color" => EnhanceMode::Color,
        "gray" => EnhanceMode::Gray,
        "bw" => EnhanceMode::BlackAndWhite,
        "photo" => EnhanceMode::Photo,
        "whiteboard" => EnhanceMode::Whiteboard,
        other => panic!("unknown mode {other}"),
    }
}

#[test]
fn the_pages_start_from_the_same_pixels() {
    let data = json("enhancer.json");
    for (name, spec) in data["pages"].as_object().unwrap() {
        if let Some(expected) = spec["sum"].as_str() {
            assert_eq!(sum(&page(spec).data), expected, "{name}");
        }
    }
}

#[test]
fn the_filters_give_the_same_bytes_as_the_apps() {
    let data = json("enhancer.json");
    for r in data["results"].as_array().unwrap() {
        let name = r["page"].as_str().unwrap();
        let out = enhance(
            &page(&data["pages"][name]),
            mode(r["mode"].as_str().unwrap()),
            DEFAULT_THRESHOLD,
        );
        assert_eq!(
            sum(&out.image.data),
            r["sum"].as_str().unwrap(),
            "{name} {}",
            r["mode"]
        );
        assert_eq!(
            out.mode,
            mode(r["applied"].as_str().unwrap()),
            "{name} {}",
            r["mode"]
        );
    }
}

#[test]
fn only_a_light_on_dark_page_is_claimed() {
    let data = json("enhancer.json");
    for (expected, key) in [(true, "true"), (false, "false")] {
        for name in data["lightOnDark"][key].as_array().unwrap() {
            let name = name.as_str().unwrap();
            assert_eq!(
                is_light_on_dark(&luma(&page(&data["pages"][name]))),
                expected,
                "{name}"
            );
        }
    }
    assert!(!is_light_on_dark(&luma(&Rgba::filled(
        240, 320, 55, 55, 55
    ))));
}

#[test]
fn a_shadowed_page_comes_out_one_white_paper_with_black_print() {
    let data = json("enhancer.json");
    let source = page(&data["pages"]["shadowed"]);
    let out = luma(&enhance(&source, EnhanceMode::Gray, DEFAULT_THRESHOLD).image);
    let (mut paper_min, mut ink_max) = (255u8, 0u8);
    for y in 0..320 {
        for x in 0..240 {
            let v = out.data[y * 240 + x];
            if is_ink(x, y, 240, 320) {
                ink_max = ink_max.max(v);
            } else if !((150..200).contains(&x) && (24..52).contains(&y)) {
                paper_min = paper_min.min(v);
            }
        }
    }
    assert!(paper_min >= 250, "paper {paper_min}");
    assert!(ink_max <= 10, "print {ink_max}");
}

#[test]
fn the_test_data_matches_its_sums() {
    use sha2::{Digest, Sha256};
    let sums = std::fs::read_to_string(common::testdata("SHA256SUMS")).unwrap();
    for line in sums.lines() {
        let (hash, name) = line.split_once(" *").unwrap();
        let bytes = std::fs::read(common::testdata(name)).unwrap();
        let digest: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(digest, hash, "{name}");
    }
}
