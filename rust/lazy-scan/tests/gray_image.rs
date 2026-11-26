use lazy_image::{Gray, Rgba};
use lazy_scan::BackgroundMap;
use lazy_scan::gray_image::{ANALYSIS_DPI, GrayImage, chroma_pixels};

fn ramp(width: usize, height: usize, dpi: f64) -> GrayImage {
    let data = (0..width * height).map(|i| (i % 251) as u8).collect();
    GrayImage::new(
        Gray {
            width,
            height,
            data,
        },
        dpi,
    )
}

#[test]
fn a_finer_image_is_boxed_down_to_the_analysis_resolution() {
    let image = ramp(600, 399, 300.0);
    let small = image.at_resolution(ANALYSIS_DPI);
    assert_eq!((small.width(), small.height()), (200, 133));
    assert_eq!(small.dpi, 100.0);
    assert!((small.scale - 1.0 / 3.0).abs() < 1e-9);
    // each pixel is the mean, rounded down, of the 3 x 3 pixels it covers
    let sum: usize = (0..3)
        .flat_map(|y| (0..3).map(move |x| (y, x)))
        .map(|(y, x)| image.gray.data[y * 600 + x] as usize)
        .sum();
    assert_eq!(small.gray.data[0] as usize, sum / 9);
}

#[test]
fn a_coarser_image_is_kept_and_a_larger_size_changes_nothing() {
    let image = ramp(425, 550, 50.0);
    assert_eq!(image.at_resolution(ANALYSIS_DPI), image);
    assert_eq!(image.downsampled(425, 550), image);
    assert_eq!(image.downsampled(900, 900), image);
    let half = image.downsampled(212, 275);
    assert_eq!((half.width(), half.height()), (212, 275));
    assert!((half.dpi - 50.0 * 212.0 / 425.0).abs() < 1e-9);
}

#[test]
fn luma_and_chroma_follow_the_rec_601_and_max_min_rules() {
    let image = Rgba::new(2, 1, vec![255, 0, 0, 255, 10, 200, 30, 255]);
    let gray = GrayImage::from_luma(&image, 100.0);
    assert_eq!(gray.gray.data, [76, 124]);
    assert_eq!(chroma_pixels(&image).data, [255, 190]);
}

#[test]
fn a_map_over_an_area_reads_only_that_area() {
    // dark outside a bright 100 x 100 area at (50, 50)
    let mut data = vec![20u8; 200 * 200];
    for y in 50..150 {
        data[y * 200 + 50..y * 200 + 150].fill(230);
    }
    let gray = Gray {
        width: 200,
        height: 200,
        data,
    };
    let map = BackgroundMap::estimate_in(&gray, 50, 50, 100, 100, 25, 0.9).unwrap();
    assert_eq!((map.width(), map.height()), (100, 100));
    assert!(map.tiles.iter().all(|&t| t == 230));
    assert!(map.ink_mask(&gray, 0.3).iter().all(|&v| v == 0));
    assert!(BackgroundMap::estimate_in(&gray, 300, 0, 10, 10, 25, 0.9).is_none());
}
