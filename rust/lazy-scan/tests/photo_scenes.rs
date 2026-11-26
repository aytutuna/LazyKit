// Lazy Scan's PerspectiveCorrectorTests on its PhotoScenes: the same photos and quads, the corrected bytes pinned by the
// apps' checksums.
mod common;

use common::photo_scenes::*;
use lazy_scan::warp::{self, Options, Paper, Supersampling};

fn forced(supersample: Supersampling) -> Options {
    Options {
        output: Some((PAGE_WIDTH as usize, PAGE_HEIGHT as usize)),
        supersample,
        ..Options::default()
    }
}

#[test]
fn a_warped_sheet_comes_back_as_it_was() {
    let sheet = sheet_image(&block_raster(0x9A6E_000C));
    let quad = ROUND_TRIP_MAP.corners();
    let (page, _) = warp::correct(&round_trip_scene(), &quad, &forced(Supersampling::On)).unwrap();
    assert_eq!((page.width, page.height), (210, 297));
    let (mut sum, mut count) = (0i64, 0i64);
    for y in 2..page.height - 2 {
        for x in 2..page.width - 2 {
            let i = (y * page.width + x) * 4;
            for c in 0..3 {
                sum += (page.data[i + c] as i64 - sheet.data[i + c] as i64).abs();
                count += 1;
            }
        }
    }
    assert!(sum as f64 / count as f64 <= 6.0);
    // every block's centre is its own colour
    for by in (5..297).step_by(10) {
        for bx in (5..210).step_by(10) {
            let i = (by * 210 + bx) * 4;
            for c in 0..3 {
                let delta = page.data[i + c] as i32 - sheet.data[i + c] as i32;
                assert!((-6..=6).contains(&delta), "block ({bx}, {by}) channel {c}: {delta}");
            }
        }
    }
}

#[test]
fn the_corrected_bytes_are_the_apps() {
    let quad = ROUND_TRIP_MAP.corners();
    let photo = round_trip_scene();
    let (on, _) = warp::correct(&photo, &quad, &forced(Supersampling::On)).unwrap();
    assert_eq!(checksum(&on), 0xF8DE_2BA5);
    let (off, _) = warp::correct(&photo, &quad, &forced(Supersampling::Off)).unwrap();
    assert_eq!(checksum(&off), 0x9950_9981);
    let (wood, plan) = warp::correct(&wood_scene(), &WOOD_MAP.corners(), &Options::default()).unwrap();
    assert_eq!((wood.width, wood.height), (235, 333));
    assert_eq!(plan.paper.map(|p| p.paper), Some(Paper::A4));
    assert_eq!(checksum(&wood), 0x32B3_6A66);
}
