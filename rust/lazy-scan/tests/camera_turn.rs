// Lazy Send's LiveQuadTrackerTests (a live outline turned into the upright photo, corners found on the upright frame turned
// back onto the sensor's pixels) and PageFinderTests' frame candidate.
use lazy_image::Rgba;
use lazy_scan::{CornerModel, Heatmaps, ModelError, Quad, frame_candidate, photo_outline};

fn close(a: &Quad, b: &Quad) {
    for (p, q) in a.0.iter().zip(&b.0) {
        assert!(
            (p.x - q.x).abs() < 1e-12 && (p.y - q.y).abs() < 1e-12,
            "{a:?} vs {b:?}"
        );
    }
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Quad {
    Quad::from_xy([[x0, y0], [x1, y0], [x1, y1], [x0, y1]])
}

#[test]
fn a_live_outline_is_turned_into_the_upright_photo() {
    let outline = rect(0.1, 0.2, 0.5, 0.6);
    close(&photo_outline(&outline, 0).unwrap(), &outline);
    close(&photo_outline(&outline, 360).unwrap(), &outline);
    let quarter = rect(0.4, 0.1, 0.8, 0.5);
    close(&photo_outline(&outline, 90).unwrap(), &quarter);
    close(&photo_outline(&outline, -270).unwrap(), &quarter);
    close(
        &photo_outline(&outline, 180).unwrap(),
        &rect(0.5, 0.4, 0.9, 0.8),
    );
    close(
        &photo_outline(&outline, 270).unwrap(),
        &rect(0.2, 0.5, 0.6, 0.9),
    );
    assert!(photo_outline(&outline, 45).is_none());
}

#[test]
fn corners_found_on_the_upright_frame_turn_back_onto_the_sensors_pixels() {
    let (width, height) = (48usize, 36usize);
    let marks = [(5usize, 4usize), (40, 7), (37, 30), (8, 27)];
    let mut data = vec![0u8; width * height * 4];
    for (index, (x, y)) in marks.iter().enumerate() {
        let at = (y * width + x) * 4;
        data[at] = 100 + index as u8 * 40;
        data[at + 3] = 255;
    }
    let frame = Rgba::new(width, height, data);
    for turn in [0, 90, 180, 270] {
        let upright = frame.rotated_quarter_turns(turn / 90);
        let mut found = Vec::new();
        for index in 0..4 {
            let wanted = 100 + index as u8 * 40;
            for at in 0..upright.width * upright.height {
                if upright.data[at * 4] == wanted {
                    found.push(lazy_scan::Point::new(
                        ((at % upright.width) as f64 + 0.5) / upright.width as f64,
                        ((at / upright.width) as f64 + 0.5) / upright.height as f64,
                    ));
                }
            }
        }
        assert_eq!(found.len(), 4, "turn {turn}");
        let back = photo_outline(
            &Quad::from_corners(found.try_into().unwrap()),
            (360 - turn) % 360,
        )
        .unwrap();
        let sensor = back.scaled(width as f64, height as f64);
        for (x, y) in marks {
            let (cx, cy) = (x as f64 + 0.5, y as f64 + 0.5);
            let nearest = sensor
                .0
                .iter()
                .map(|p| ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt())
                .fold(f64::INFINITY, f64::min);
            assert!(nearest < 1e-6, "turn {turn}, mark {x},{y}");
        }
    }
}

struct Spots([(usize, usize); 4]);

impl CornerModel for Spots {
    fn run(&self, _: &[f32]) -> Result<Heatmaps, ModelError> {
        let n = 64;
        let mut values = vec![0f32; 4 * n * n];
        for (ch, (x, y)) in self.0.iter().enumerate() {
            for dy in 0..3 {
                for dx in 0..3 {
                    values[ch * n * n + (y + dy) * n + x + dx] = 0.9;
                }
            }
        }
        Ok(Heatmaps {
            values,
            width: n,
            height: n,
        })
    }
}

#[test]
fn the_frame_candidate_is_turned_back_onto_the_sensors_pixels() {
    let frame = Rgba::new(64, 48, [120, 120, 120, 255].repeat(64 * 48));
    // in the frame turned a quarter (48 x 64) the page spans x 0.2..0.8, y 0.1..0.9 of the 64 x 64 heat
    let spots = Spots([(12, 6), (51, 6), (51, 57), (12, 57)]);
    let found = frame_candidate(&frame, 90, &spots).unwrap();
    let quad = found.quad.unwrap();
    assert!((quad.0[0].x - 0.1 * 64.0).abs() < 2.0, "{quad:?}");
    assert!((quad.0[0].y - 0.2 * 48.0).abs() < 2.0, "{quad:?}");
    assert!((quad.0[2].x - 0.9 * 64.0).abs() < 2.0, "{quad:?}");
    assert!((quad.0[2].y - 0.8 * 48.0).abs() < 2.0, "{quad:?}");
    assert!((found.confidence - 0.9).abs() < 0.05);
    assert!(frame_candidate(&frame, 45, &spots).is_none());
}
