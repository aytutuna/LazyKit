use crate::geometry::{Point, Quad};
use crate::warp::estimate_aspect;
use lazy_image::Rgba;

// DocAligner's FastViT-T8 page corner model (Apache-2.0), read as Lazy Scan reads it: the photo stretched to 256 x 256 and
// given as blue, green and red planes of 0 to 1; the answer is four heatmaps, one per corner (top left, top right,
// bottom right, bottom left).
pub const SIDE: usize = 256;
const THRESHOLD: f32 = 0.3;
/// A page is taken from this confidence (the weakest corner's peak) up, and trusted from `SURE_CONFIDENCE` up.
pub const MIN_CONFIDENCE: f64 = 0.5;
pub const SURE_CONFIDENCE: f64 = 0.7;
const MAX_LONG_OVER_SHORT: f64 = 2.0;

/// A page the model found: corners as shares of the photo (x right, y down) and the confidence.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Found {
    pub quad: Quad,
    pub confidence: f64,
}

// bilinear with the pixel centres matched, as OpenCV's linear resize does it: the two source pixels and the second's weight
fn tap(index: usize, scale: f64, size: usize) -> (usize, usize, f64) {
    let at = ((index as f64 + 0.5) * scale - 0.5).max(0.0);
    let low = (at as usize).min(size - 1);
    (low, (low + 1).min(size - 1), at - low as f64)
}

/// The photo as the model takes it: 3 x SIDE x SIDE values, blue plane first.
pub fn model_input(photo: &Rgba) -> Vec<f32> {
    let plane = SIDE * SIDE;
    let mut input = vec![0f32; 3 * plane];
    let sx = photo.width as f64 / SIDE as f64;
    let sy = photo.height as f64 / SIDE as f64;
    let px = &photo.data;
    for y in 0..SIDE {
        let (r0, r1, wy) = tap(y, sy, photo.height);
        let (top, bottom) = (r0 * photo.width, r1 * photo.width);
        for x in 0..SIDE {
            let (c0, c1, wx) = tap(x, sx, photo.width);
            for ch in 0..3 {
                let o = 2 - ch;
                let a = px[(top + c0) * 4 + o] as f64;
                let b = px[(top + c1) * 4 + o] as f64;
                let c = px[(bottom + c0) * 4 + o] as f64;
                let d = px[(bottom + c1) * 4 + o] as f64;
                let upper = a + (b - a) * wx;
                let lower = c + (d - c) * wx;
                input[ch * plane + y * SIDE + x] = ((upper + (lower - upper) * wy) / 255.0) as f32;
            }
        }
    }
    input
}

// the heat-weighted centre of the largest 8-connected blob of values from THRESHOLD up in one channel, as shares of the
// map, and the channel's peak
fn corner(heat: &[f32], width: usize, height: usize) -> Option<(Point, f64)> {
    let plane = width * height;
    let mut seen = vec![false; plane];
    let mut stack: Vec<usize> = Vec::new();
    let mut peak = 0f32;
    let mut best_count = 0;
    let (mut best_x, mut best_y) = (0.0, 0.0);
    for start in 0..plane {
        let value = heat[start];
        peak = peak.max(value);
        if value < THRESHOLD || seen[start] {
            continue;
        }
        seen[start] = true;
        stack.push(start);
        let mut count = 0;
        let (mut sum, mut sum_x, mut sum_y) = (0.0f64, 0.0f64, 0.0f64);
        while let Some(at) = stack.pop() {
            let (x, y) = (at % width, at / width);
            let weight = heat[at] as f64;
            count += 1;
            sum += weight;
            sum_x += weight * (x as f64 + 0.5);
            sum_y += weight * (y as f64 + 0.5);
            for ny in y.saturating_sub(1)..=(y + 1).min(height - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(width - 1) {
                    let next = ny * width + nx;
                    if !seen[next] && heat[next] >= THRESHOLD {
                        seen[next] = true;
                        stack.push(next);
                    }
                }
            }
        }
        if count > best_count {
            best_count = count;
            best_x = sum_x / sum;
            best_y = sum_y / sum;
        }
    }
    (best_count > 0).then(|| {
        (
            Point::new(best_x / width as f64, best_y / height as f64),
            peak as f64,
        )
    })
}

/// The page's corners as shares of the photo and the confidence; `None` when a channel has no value from the threshold
/// up or the four corners are not a convex outline in their order.
pub fn decode_heatmap(heat: &[f32], width: usize, height: usize) -> Option<Found> {
    let plane = width * height;
    if width == 0 || height == 0 || heat.len() < plane * 4 {
        return None;
    }
    let mut points = [Point::default(); 4];
    let mut confidence = 1.0f64;
    for (ch, point) in points.iter_mut().enumerate() {
        let (p, peak) = corner(&heat[ch * plane..(ch + 1) * plane], width, height)?;
        *point = p;
        confidence = confidence.min(peak);
    }
    let quad = Quad(points);
    quad.is_convex().then_some(Found { quad, confidence })
}

/// The sanity checks on a page of `width` x `height` pixels: a confidence from `MIN_CONFIDENCE` up, corners the editor can
/// cut along, covering at least a tenth of the photo and no more than 2 to 1 once the perspective is undone (Lazy Scan's
/// document kind and Lazy Send's page finder; Lazy Collect's TypeScript copy allows 4 for receipts).
pub fn acceptable(found: &Found, width: f64, height: f64) -> bool {
    if found.confidence < MIN_CONFIDENCE || found.confidence.is_nan() {
        return false;
    }
    let q = found.quad.scaled(width, height);
    if !q.usable() {
        return false;
    }
    let share = q.signed_area() / (width * height);
    if !(0.1..=1.0).contains(&share) {
        return false;
    }
    estimate_aspect(&Quad::from_corners(q.0), width, height).long_over_short()
        <= MAX_LONG_OVER_SHORT
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: usize = 64;

    struct Drawn(Vec<f32>);

    impl Drawn {
        fn new() -> Self {
            Drawn(vec![0.0; SIZE * SIZE * 4])
        }

        // a Gaussian spot centred on (x, y) in the values' own units, as the model draws a corner
        fn spot(&mut self, channel: usize, x: f64, y: f64, peak: f64, sigma: f64) {
            for row in 0..SIZE {
                for column in 0..SIZE {
                    let (dx, dy) = (column as f64 + 0.5 - x, row as f64 + 0.5 - y);
                    let value =
                        (peak * (-(dx * dx + dy * dy) / (2.0 * sigma * sigma)).exp()) as f32;
                    let i = channel * SIZE * SIZE + row * SIZE + column;
                    self.0[i] = self.0[i].max(value);
                }
            }
        }

        fn peak(&self, channel: usize) -> f64 {
            self.0[channel * SIZE * SIZE..(channel + 1) * SIZE * SIZE]
                .iter()
                .cloned()
                .fold(0.0f32, f32::max) as f64
        }
    }

    const PAGE: [(f64, f64); 4] = [(12.0, 10.0), (50.0, 14.0), (48.0, 56.0), (9.0, 52.0)];

    fn page(peaks: [f64; 4]) -> Drawn {
        let mut heat = Drawn::new();
        for (ch, &(x, y)) in PAGE.iter().enumerate() {
            heat.spot(ch, x, y, peaks[ch], 2.0);
        }
        heat
    }

    #[test]
    fn four_spots_give_the_corners_in_order_as_shares() {
        let heat = page([0.9; 4]);
        let found = decode_heatmap(&heat.0, SIZE, SIZE).unwrap();
        for (i, &(x, y)) in PAGE.iter().enumerate() {
            assert!((found.quad.0[i].x * SIZE as f64 - x).abs() < 0.01);
            assert!((found.quad.0[i].y * SIZE as f64 - y).abs() < 0.01);
        }
        assert!((found.confidence - heat.peak(0)).abs() < 1e-6);
    }

    #[test]
    fn the_confidence_is_the_weakest_peak() {
        let heat = page([0.95, 0.6, 0.8, 0.9]);
        assert!(
            (decode_heatmap(&heat.0, SIZE, SIZE).unwrap().confidence - heat.peak(1)).abs() < 1e-6
        );
    }

    #[test]
    fn the_largest_blob_wins_over_a_smaller_brighter_one() {
        let mut heat = page([0.9; 4]);
        heat.spot(0, 40.5, 40.5, 1.0, 0.8);
        let found = decode_heatmap(&heat.0, SIZE, SIZE).unwrap();
        assert!((found.quad.0[0].x * SIZE as f64 - 12.0).abs() < 0.01);
        assert!((found.quad.0[0].y * SIZE as f64 - 10.0).abs() < 0.01);
    }

    #[test]
    fn an_empty_channel_or_corners_out_of_order_give_nothing() {
        let mut heat = Drawn::new();
        heat.spot(0, 12.0, 10.0, 0.9, 2.0);
        heat.spot(1, 50.0, 14.0, 0.9, 2.0);
        heat.spot(3, 9.0, 52.0, 0.9, 2.0);
        assert!(decode_heatmap(&heat.0, SIZE, SIZE).is_none());
        heat.spot(2, 48.0, 56.0, 0.25, 2.0);
        assert!(decode_heatmap(&heat.0, SIZE, SIZE).is_none());

        let mut swapped = Drawn::new();
        for (ch, (x, y)) in [(12.0, 10.0), (48.0, 56.0), (50.0, 14.0), (9.0, 52.0)]
            .into_iter()
            .enumerate()
        {
            swapped.spot(ch, x, y, 0.9, 2.0);
        }
        assert!(decode_heatmap(&swapped.0, SIZE, SIZE).is_none());
    }

    #[test]
    fn a_spot_between_values_is_found_to_a_fraction() {
        let mut heat = Drawn::new();
        heat.spot(0, 20.25, 10.75, 0.9, 2.0);
        heat.spot(1, 50.6, 14.3, 0.9, 2.0);
        heat.spot(2, 48.0, 56.0, 0.9, 2.0);
        heat.spot(3, 9.0, 52.0, 0.9, 2.0);
        let q = decode_heatmap(&heat.0, SIZE, SIZE).unwrap().quad;
        assert!((q.0[0].x * SIZE as f64 - 20.25).abs() < 0.1);
        assert!((q.0[0].y * SIZE as f64 - 10.75).abs() < 0.1);
        assert!((q.0[1].x * SIZE as f64 - 50.6).abs() < 0.1);
    }

    #[test]
    fn a_non_square_heatmap_is_read_as_shares_of_each_side() {
        let (w, h) = (32, 16);
        let mut values = vec![0f32; w * h * 4];
        for (ch, (x, y)) in [(4, 3), (27, 3), (27, 12), (4, 12)].into_iter().enumerate() {
            values[ch * w * h + y * w + x] = 0.8;
        }
        let q = decode_heatmap(&values, w, h).unwrap().quad;
        assert_eq!(q.0[0].x, 4.5 / 32.0);
        assert_eq!(q.0[0].y, 3.5 / 16.0);
        assert_eq!(q.0[2].x, 27.5 / 32.0);
        assert_eq!(q.0[2].y, 12.5 / 16.0);
        assert!(decode_heatmap(&values[..10], w, h).is_none());
    }

    #[test]
    fn the_sanity_checks() {
        let sure = decode_heatmap(&page([0.9; 4]).0, SIZE, SIZE).unwrap();
        assert!(acceptable(&sure, 800.0, 600.0));
        assert!(!acceptable(
            &Found {
                confidence: 0.4,
                ..sure
            },
            800.0,
            600.0
        ));
        let tiny = Found {
            quad: Quad(
                sure.quad
                    .0
                    .map(|p| Point::new(0.4 + p.x * 0.1, 0.4 + p.y * 0.1)),
            ),
            confidence: 0.9,
        };
        assert!(!acceptable(&tiny, 800.0, 600.0));
    }

    #[test]
    fn an_almost_square_page_with_little_perspective_is_kept() {
        // the Zhang-He estimate of this outline reads 0.42 (a focal length of 39 diagonals); the reference keeps it
        let corners = [(110.0, 30.0), (490.0, 10.0), (430.0, 420.0), (40.0, 440.0)];
        let found = Found {
            quad: Quad(corners.map(|(x, y)| Point::new(x / 640.0, y / 480.0))),
            confidence: 0.95,
        };
        assert!(acceptable(&found, 640.0, 480.0));
    }

    #[test]
    fn a_strip_of_paper_longer_than_two_to_one_is_not_a_document() {
        let strip = |width: f64| Found {
            quad: Quad(
                [
                    (0.3, 0.3),
                    (0.3 + width, 0.3),
                    (0.3 + width, 0.6),
                    (0.3, 0.6),
                ]
                .map(|(x, y)| Point::new(x, y)),
            ),
            confidence: 0.9,
        };
        // 800 x 600 photo: 0.3 x 0.3 of it is 240 x 180, so a width share of 0.6 / 0.4 gives 480 x 180 (2.7 to 1) and 288 x 180 (1.6 to 1)
        assert!(!acceptable(&strip(0.6), 800.0, 600.0));
        assert!(acceptable(&strip(0.36), 800.0, 600.0));
    }

    #[test]
    fn the_input_is_blue_green_red_planes() {
        let input = model_input(&Rgba::filled(300, 200, 255, 128, 0));
        assert_eq!(input.len(), 3 * 256 * 256);
        for (x, y) in [(0, 0), (255, 0), (17, 93), (255, 255)] {
            assert_eq!(input[y * SIDE + x], 0.0);
            assert_eq!(input[SIDE * SIDE + y * SIDE + x], (128.0f64 / 255.0) as f32);
            assert_eq!(input[2 * SIDE * SIDE + y * SIDE + x], 1.0);
        }
    }

    #[test]
    fn halving_takes_the_mean_of_each_pair() {
        let mut data = Vec::new();
        for y in 0..256usize {
            for x in 0..512usize {
                data.extend_from_slice(&[(x & 255) as u8, y as u8, 0, 255]);
            }
        }
        let input = model_input(&Rgba::new(512, 256, data));
        let red = |x: usize, y: usize| input[2 * SIDE * SIDE + y * SIDE + x];
        assert_eq!(red(10, 7), (20.5f64 / 255.0) as f32);
        assert_eq!(red(127, 7), (254.5f64 / 255.0) as f32);
        assert_eq!(red(128, 7), (0.5f64 / 255.0) as f32);
        assert_eq!(input[SIDE * SIDE + 7 * SIDE + 10], (7.0f64 / 255.0) as f32);
    }

    #[test]
    fn enlarging_blends_the_nearest_pixels_and_clamps_at_the_edges() {
        let mut data = Vec::new();
        for y in 0..2u8 {
            for x in 0..2u8 {
                data.extend_from_slice(&[255 * x, 255 * y, 0, 255]);
            }
        }
        let input = model_input(&Rgba::new(2, 2, data));
        let red = |x: usize, y: usize| input[2 * SIDE * SIDE + y * SIDE + x];
        assert_eq!(red(0, 0), 0.0);
        assert_eq!(red(63, 0), 0.0);
        assert_eq!(red(64, 0), (0.99609375f64 / 255.0) as f32);
        assert_eq!(red(128, 0), (128.49609375f64 / 255.0) as f32);
        assert_eq!(red(255, 0), 1.0);
        assert_eq!(
            input[SIDE * SIDE + 128 * SIDE],
            (128.49609375f64 / 255.0) as f32
        );
    }
}
