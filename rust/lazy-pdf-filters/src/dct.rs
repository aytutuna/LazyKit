//! What the readers take from a JPEG file without decoding it: DCTDecode data passes through the
//! filter chain untouched.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JpegHeader {
    pub width: u32,
    pub height: u32,
    pub components: u8,
    /// An Adobe (APP14) marker tells a codec that the stored inks are inverted.
    pub adobe: bool,
}

fn is_adobe(jpeg: &[u8], p: usize, length: usize) -> bool {
    length >= 7 && p + 9 <= jpeg.len() && jpeg[p + 4] == b'A' && jpeg[p + 5] == b'd'
}

/// Whether an Adobe marker comes before the scan data.
pub fn has_adobe_marker(jpeg: &[u8]) -> bool {
    let mut p = 2;
    while p + 4 <= jpeg.len() && jpeg[p] == 0xff && jpeg[p + 1] != 0xda {
        let length = (jpeg[p + 2] as usize) << 8 | jpeg[p + 3] as usize;
        if jpeg[p + 1] == 0xee && is_adobe(jpeg, p, length) {
            return true;
        }
        p += 2 + length;
    }
    false
}

/// The size and component count from the first frame header, with the Adobe marker if one comes
/// before it. None for data that is not a JPEG, has no frame header or has an unusable one.
pub fn read_header(jpeg: &[u8]) -> Option<JpegHeader> {
    if jpeg.len() < 4 || jpeg[0] != 0xff || jpeg[1] != 0xd8 {
        return None;
    }
    let mut adobe = false;
    let mut p = 2;
    while p + 4 <= jpeg.len() {
        if jpeg[p] != 0xff {
            p += 1;
            continue;
        }
        let marker = jpeg[p + 1];
        if matches!(marker, 0xff | 0x00 | 0xd0..=0xd8 | 0x01) {
            p += if marker == 0xff { 1 } else { 2 };
            continue;
        }
        let length = (jpeg[p + 2] as usize) << 8 | jpeg[p + 3] as usize;
        if marker == 0xee && is_adobe(jpeg, p, length) {
            adobe = true;
        }
        if matches!(marker, 0xc0..=0xcf) && !matches!(marker, 0xc4 | 0xc8 | 0xcc) {
            if p + 10 > jpeg.len() {
                return None;
            }
            let height = (jpeg[p + 5] as u32) << 8 | jpeg[p + 6] as u32;
            let width = (jpeg[p + 7] as u32) << 8 | jpeg[p + 8] as u32;
            let components = jpeg[p + 9];
            let usable = width > 0 && height > 0 && matches!(components, 1 | 3 | 4);
            return usable.then_some(JpegHeader {
                width,
                height,
                components,
                adobe,
            });
        }
        p += 2 + length;
    }
    None
}
