//! A picture from pixels: RGB data in a Flate stream, with a soft mask for the alpha channel when there is one.

use lazy_pdf_filters::encode::flate;

use crate::collect::Collector;

/// An image XObject of `width` x `height` pixels, `rgb` three bytes a pixel and `alpha` one, rows top to bottom. Returns
/// its object number.
pub fn add_rgb_image(
    out: &mut Collector,
    width: usize,
    height: usize,
    rgb: &[u8],
    alpha: Option<&[u8]>,
) -> usize {
    let pixels = width.checked_mul(height).expect("image size is too large");
    assert_eq!(
        rgb.len(),
        pixels.checked_mul(3).expect("image size is too large"),
        "pixels do not match the size"
    );
    let mask = alpha.map(|alpha| {
        assert_eq!(alpha.len(), pixels, "alpha does not match the size");
        let number = out.alloc();
        out.set_stream(
            number,
            format!(
                "/Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /FlateDecode"
            ),
            flate(alpha, 6),
        );
        number
    });
    let number = out.alloc();
    let soft_mask = mask.map_or(String::new(), |m| format!(" /SMask {m} 0 R"));
    out.set_stream(
        number,
        format!(
            "/Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /FlateDecode{soft_mask}"
        ),
        flate(rgb, 6),
    );
    number
}
