// The packing half of Lazy Scan's BilevelOutputTests: layout, threshold, padding bits and the round trip.
use lazy_image::Rgba;
use lazy_scan::bilevel::{is_black, pack, stride, unpack};

fn gray(width: usize, height: usize, f: impl Fn(usize, usize) -> u8) -> Rgba {
    let mut data = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let v = f(x, y);
            data.extend_from_slice(&[v, v, v, 255]);
        }
    }
    Rgba::new(width, height, data)
}

#[test]
fn rows_start_on_a_byte_and_black_is_zero() {
    assert_eq!((stride(0), stride(1), stride(8), stride(9)), (0, 1, 1, 2));
    let image = gray(
        10,
        2,
        |x, y| if x == 0 || (y == 1 && x == 9) { 0 } else { 255 },
    );
    let packed = pack(&image);
    // the bits past the last pixel are 1
    assert_eq!(packed, [0b0111_1111, 0b1111_1111, 0b0111_1111, 0b1011_1111]);
    assert!(is_black(&packed, 10, 0, 0) && is_black(&packed, 10, 9, 1));
    assert!(!is_black(&packed, 10, 1, 0) && !is_black(&packed, 10, 9, 0));
}

#[test]
fn the_threshold_is_on_the_luma() {
    let at = |v: u8| is_black(&pack(&gray(1, 1, |_, _| v)), 1, 0, 0);
    assert!(at(127) && !at(128));
    // Rec. 601 weights: pure green is light (150), magenta dark (105)
    assert!(!is_black(
        &pack(&Rgba::new(1, 1, vec![0, 255, 0, 255])),
        1,
        0,
        0
    ));
    assert!(is_black(
        &pack(&Rgba::new(1, 1, vec![255, 0, 255, 255])),
        1,
        0,
        0
    ));
}

#[test]
fn a_black_and_white_page_survives_packing_and_unpacking() {
    let image = gray(37, 11, |x, y| if (x / 3 + y) % 2 == 0 { 0 } else { 255 });
    let back = unpack(&pack(&image), 37, 11);
    assert_eq!(back, image);
}
