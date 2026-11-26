use lazy_image::{Gray, Rgba, luma};

/// The resolution the page analyses work at, so their pixel constants mean the same physical size on every scan.
pub const ANALYSIS_DPI: u32 = 100;

/// An 8-bit luminance bitmap with the resolution it was made at.
#[derive(Clone, Debug, PartialEq)]
pub struct GrayImage {
    pub gray: Gray,
    /// The resolution of these pixels, in dots per inch.
    pub dpi: f64,
    /// These pixels per pixel of the image they were made from (below 1 when downsampled).
    pub scale: f64,
}

/// The chroma of every pixel (one byte each): max(R, G, B) - min(R, G, B).
pub fn chroma_pixels(image: &Rgba) -> Gray {
    let data = image
        .data
        .chunks_exact(4)
        .map(|p| p[0].max(p[1]).max(p[2]) - p[0].min(p[1]).min(p[2]))
        .collect();
    Gray {
        width: image.width,
        height: image.height,
        data,
    }
}

impl GrayImage {
    pub fn new(gray: Gray, dpi: f64) -> Self {
        GrayImage {
            gray,
            dpi,
            scale: 1.0,
        }
    }

    /// A width x height image of one gray value at the analysis resolution.
    pub fn filled(width: usize, height: usize, value: u8) -> Self {
        Self::new(
            Gray {
                width,
                height,
                data: vec![value; width * height],
            },
            ANALYSIS_DPI as f64,
        )
    }

    pub fn width(&self) -> usize {
        self.gray.width
    }

    pub fn height(&self) -> usize {
        self.gray.height
    }

    /// `image` as gray at its own size: Rec. 601 luma, rounded.
    pub fn from_luma(image: &Rgba, dpi: f64) -> Self {
        Self::new(luma(image), dpi)
    }

    /// `image`, scanned at `source_dpi`, as gray at ANALYSIS_DPI: the luma of every pixel, then a box filter. An image no finer
    /// than ANALYSIS_DPI is kept at its own resolution.
    pub fn from_rgba(image: &Rgba, source_dpi: u32) -> Self {
        Self::from_luma(image, source_dpi.max(1) as f64).at_resolution(ANALYSIS_DPI)
    }

    /// The chroma of `image`, scanned at `source_dpi`, at ANALYSIS_DPI: the same size and box filter as `from_rgba`.
    pub fn chroma(image: &Rgba, source_dpi: u32) -> Self {
        Self::new(chroma_pixels(image), source_dpi.max(1) as f64).at_resolution(ANALYSIS_DPI)
    }

    /// This image at `dpi` (box filter); itself when it is no finer. The `scale` of the result maps back to the image this one
    /// was made from.
    pub fn at_resolution(&self, dpi: u32) -> GrayImage {
        if dpi == 0 || self.dpi <= dpi as f64 {
            return self.clone();
        }
        let w = ((self.width() as f64 * dpi as f64 / self.dpi).round() as usize).max(1);
        let h = ((self.height() as f64 * dpi as f64 / self.dpi).round() as usize).max(1);
        let small = self.downsampled(w, h);
        GrayImage {
            dpi: dpi as f64,
            scale: self.scale * w as f64 / self.width() as f64,
            ..small
        }
    }

    /// A smaller copy: each pixel is the mean (rounded down) of the source rectangle it covers. Sizes not smaller than this one
    /// on both axes return this image.
    pub fn downsampled(&self, new_width: usize, new_height: usize) -> GrayImage {
        let (width, height) = (self.width(), self.height());
        if new_width >= width && new_height >= height {
            return self.clone();
        }
        let (nw, nh) = (new_width.min(width), new_height.min(height));
        let x_ratio = width as f64 / nw as f64;
        let y_ratio = height as f64 / nh as f64;
        let x0s: Vec<usize> = (0..nw).map(|dx| (dx as f64 * x_ratio) as usize).collect();
        let x1s: Vec<usize> = (0..nw)
            .map(|dx| (x0s[dx] + 1).max(width.min(((dx + 1) as f64 * x_ratio).ceil() as usize)))
            .collect();
        let mut out = vec![0u8; nw * nh];
        for dy in 0..nh {
            let sy0 = (dy as f64 * y_ratio) as usize;
            let sy1 = (sy0 + 1).max(height.min(((dy + 1) as f64 * y_ratio).ceil() as usize));
            for dx in 0..nw {
                let (sx0, sx1) = (x0s[dx], x1s[dx]);
                let mut sum = 0usize;
                for sy in sy0..sy1 {
                    let row = sy * width;
                    sum += self.gray.data[row + sx0..row + sx1]
                        .iter()
                        .map(|&v| v as usize)
                        .sum::<usize>();
                }
                out[dy * nw + dx] = (sum / ((sy1 - sy0) * (sx1 - sx0))) as u8;
            }
        }
        GrayImage {
            gray: Gray {
                width: nw,
                height: nh,
                data: out,
            },
            dpi: self.dpi * nw as f64 / width as f64,
            scale: self.scale * nw as f64 / width as f64,
        }
    }
}
