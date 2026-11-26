const SAMPLE_STEP: usize = 4;
const CHROMA_THRESHOLD: i32 = 32;
const MAX_COLORED: f64 = 0.001;

/// A page is gray when fewer than a thousandth of every 4th pixel of every 4th row has chroma above 32.
pub fn is_effectively_gray(rgba: &[u8], width: usize, height: usize) -> bool {
    let mut samples = 0u64;
    let mut colored = 0u64;
    for y in (0..height).step_by(SAMPLE_STEP) {
        for x in (0..width).step_by(SAMPLE_STEP) {
            let i = (y * width + x) * 4;
            let (r, g, b) = (rgba[i] as i32, rgba[i + 1] as i32, rgba[i + 2] as i32);
            samples += 1;
            if r.max(g).max(b) - r.min(g).min(b) > CHROMA_THRESHOLD {
                colored += 1;
            }
        }
    }
    samples > 0 && (colored as f64) < MAX_COLORED * samples as f64
}
