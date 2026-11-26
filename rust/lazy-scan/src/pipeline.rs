use crate::corners::{acceptable, decode_heatmap, model_input};
use crate::geometry::Quad;
use crate::refiner::{placement, refine};
use lazy_image::Rgba;
use lazy_scan_model::{CornerModel, ModelError};

/// A page found in a photo: its corners in the photo's pixels, the model's confidence, and whether the model and the photo's
/// edges are both sure enough to place it without asking.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Located {
    pub quad: Quad,
    pub confidence: f64,
    pub sure: bool,
}

/// The photo through the corner model, the sanity checks and the edge refinement; `None` when no usable page is in it.
pub fn locate(photo: &Rgba, model: &dyn CornerModel) -> Result<Option<Located>, ModelError> {
    if photo.width == 0 || photo.height == 0 {
        return Ok(None);
    }
    let heat = model.run(&model_input(photo))?;
    let Some(found) = decode_heatmap(&heat.values, heat.width, heat.height) else {
        return Ok(None);
    };
    let (w, h) = (photo.width as f64, photo.height as f64);
    if !acceptable(&found, w, h) {
        return Ok(None);
    }
    let given = found.quad.scaled(w, h);
    let refined = refine(photo, &given);
    let (quad, sure) = placement(Some(&refined), &given, found.confidence, w, h);
    Ok(Some(Located {
        quad: Quad::from_corners(quad.0),
        confidence: found.confidence,
        sure,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lazy_scan_model::Heatmaps;

    struct Fixed(Option<Heatmaps>);

    impl CornerModel for Fixed {
        fn run(&self, _: &[f32]) -> Result<Heatmaps, ModelError> {
            self.0.clone().ok_or(ModelError::Shape)
        }
    }

    fn page_heat() -> Heatmaps {
        spots([(12, 10), (50, 14), (48, 56), (9, 52)])
    }

    fn spots(corners: [(usize, usize); 4]) -> Heatmaps {
        let n = 64;
        let mut values = vec![0f32; 4 * n * n];
        for (ch, (x, y)) in corners.into_iter().enumerate() {
            for dy in 0..3 {
                for dx in 0..3 {
                    values[ch * n * n + (y + dy) * n + x + dx] = 0.9;
                }
            }
        }
        Heatmaps {
            values,
            width: n,
            height: n,
        }
    }

    #[test]
    fn a_page_in_the_heat_is_placed_in_the_photo() {
        // a flat photo has no edges, so the model's corners stand and the edges do not back them
        let photo = Rgba::filled(640, 480, 120, 120, 120);
        let found = locate(&photo, &Fixed(Some(page_heat()))).unwrap().unwrap();
        assert!(!found.sure);
        assert!((found.quad.0[0].x - 13.5 / 64.0 * 640.0).abs() < 1e-9);
        assert!(found.confidence > 0.89);
    }

    #[test]
    fn a_page_turned_in_the_frame_is_named_from_its_top_left() {
        // the model's first corner is the page's top right here
        let turned = spots([(50, 14), (48, 56), (9, 52), (12, 10)]);
        let photo = Rgba::filled(640, 480, 120, 120, 120);
        let found = locate(&photo, &Fixed(Some(turned))).unwrap().unwrap();
        assert!((found.quad.0[0].x - 13.5 / 64.0 * 640.0).abs() < 1e-9);
        assert!((found.quad.0[0].y - 11.5 / 64.0 * 480.0).abs() < 1e-9);
        assert!(found.quad.0[1].x > found.quad.0[0].x);
    }

    #[test]
    fn a_long_strip_is_no_document_even_when_the_model_is_sure() {
        // 56 x 19 cells of 64 on a 640 x 480 photo: about 3.9 to 1, a quarter of the frame
        let strip = spots([(4, 24), (60, 24), (60, 43), (4, 43)]);
        let photo = Rgba::filled(640, 480, 120, 120, 120);
        assert_eq!(locate(&photo, &Fixed(Some(strip))).unwrap(), None);
    }

    #[test]
    fn no_heat_is_no_page_and_a_model_error_comes_through() {
        let photo = Rgba::filled(640, 480, 120, 120, 120);
        let empty = Heatmaps {
            values: vec![0.0; 4 * 64 * 64],
            width: 64,
            height: 64,
        };
        assert_eq!(locate(&photo, &Fixed(Some(empty))).unwrap(), None);
        assert!(locate(&photo, &Fixed(None)).is_err());
    }
}
