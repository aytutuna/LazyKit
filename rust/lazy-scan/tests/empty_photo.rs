use lazy_image::Rgba;
use lazy_scan::{CornerModel, Heatmaps, ModelError, locate};

struct Unused;

impl CornerModel for Unused {
    fn run(&self, _input: &[f32]) -> Result<Heatmaps, ModelError> {
        panic!("an empty photo never reaches the model");
    }
}

#[test]
fn a_photo_without_pixels_has_no_page() {
    for (w, h) in [(0, 0), (0, 40), (40, 0)] {
        assert_eq!(locate(&Rgba::new(w, h, vec![]), &Unused).unwrap(), None);
    }
}
