use lazy_image::Rgba;

/// A pixel whose luma is below this is black.
pub const THRESHOLD: u32 = 128;

// Black and white pages as one bit per pixel, the form CCITT G4 encodes: rows top to bottom, each starting on a byte boundary,
// pixels left to right from the most significant bit, 0 = black, 1 = white. The bits past a row's last pixel are 1.

/// Bytes per packed row.
pub fn stride(width: usize) -> usize {
    width.div_ceil(8)
}

/// The page as packed bits: luma (Rec. 601 in thousandths, rounded half up) below THRESHOLD is black. A page the black and
/// white mode made (every pixel 0 or 255) keeps every pixel; a downsampled one is thresholded again.
pub fn pack(image: &Rgba) -> Vec<u8> {
    let row_bytes = stride(image.width);
    let mut packed = vec![0xFFu8; row_bytes * image.height];
    for y in 0..image.height {
        for x in 0..image.width {
            let i = (y * image.width + x) * 4;
            let p = &image.data[i..i + 3];
            let luma = (299 * p[0] as u32 + 587 * p[1] as u32 + 114 * p[2] as u32 + 500) / 1000;
            if luma < THRESHOLD {
                packed[y * row_bytes + (x >> 3)] &= !(0x80u8 >> (x & 7));
            }
        }
    }
    packed
}

/// Whether pixel (x, y) of a packed image `width` pixels wide is black.
pub fn is_black(packed: &[u8], width: usize, x: usize, y: usize) -> bool {
    packed[y * stride(width) + (x >> 3)] & (0x80 >> (x & 7)) == 0
}

/// A packed image as an opaque RGBA image: black (0, 0, 0) and white (255, 255, 255).
pub fn unpack(packed: &[u8], width: usize, height: usize) -> Rgba {
    let row_bytes = stride(width);
    let mut data = vec![255u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            if packed[y * row_bytes + (x >> 3)] & (0x80 >> (x & 7)) == 0 {
                let i = (y * width + x) * 4;
                data[i..i + 3].fill(0);
            }
        }
    }
    Rgba::new(width, height, data)
}
