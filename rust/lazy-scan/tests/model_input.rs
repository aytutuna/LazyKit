mod common;

use lazy_image::Rgba;
use lazy_scan::model_input;

#[test]
fn an_odd_sized_photo_gives_the_same_numbers_as_the_apps() {
    let data = common::json("model-input.json");
    let (w, h) = (
        data["photo"]["width"].as_u64().unwrap() as usize,
        data["photo"]["height"].as_u64().unwrap() as usize,
    );
    let mut pixels = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        for x in 0..w {
            pixels.extend_from_slice(&[
                ((x * 7 + y * 3) & 255) as u8,
                ((x * y) & 255) as u8,
                ((x + 2 * y) & 255) as u8,
                255,
            ]);
        }
    }
    let input = model_input(&Rgba::new(w, h, pixels));
    for v in data["values"].as_array().unwrap() {
        let expected: f32 = v["value"].as_f64().unwrap() as f32;
        assert_eq!(
            input[v["index"].as_u64().unwrap() as usize],
            expected,
            "index {}",
            v["index"]
        );
    }
    let sum: f64 = input.iter().map(|&v| v as f64).sum();
    assert_eq!(sum, data["sum"].as_f64().unwrap());
}
