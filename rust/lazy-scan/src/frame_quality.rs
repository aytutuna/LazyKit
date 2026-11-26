use crate::geometry::Quad;
use lazy_image::{Gray, Rgba, luma};

/// The pixels a frame is measured over, [x0, x1) by [y0, y1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Area {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    /// Variance of the 4-neighbour Laplacian: high for crisp edges and print, low when blurred.
    pub sharpness: f64,
    /// Mean luma, 0 to 255.
    pub brightness: f64,
}

/// The page's bounding box is widened by this many pixels each way.
pub const MARGIN: usize = 2;

/// The box of `region` widened by MARGIN and kept in the frame; the whole frame when there is no region, a corner is not
/// a number or nothing of the region is in the frame.
pub fn area(region: Option<&Quad>, width: usize, height: usize) -> Area {
    let whole = Area {
        x0: 0,
        y0: 0,
        x1: width,
        y1: height,
    };
    let Some(quad) = region else { return whole };
    let (mut min_x, mut min_y) = (f64::MAX, f64::MAX);
    let (mut max_x, mut max_y) = (-f64::MAX, -f64::MAX);
    for c in quad.0 {
        if !c.x.is_finite() || !c.y.is_finite() {
            return whole;
        }
        min_x = min_x.min(c.x);
        min_y = min_y.min(c.y);
        max_x = max_x.max(c.x);
        max_y = max_y.max(c.y);
    }
    let edge = MARGIN as f64;
    let clip = |v: f64, limit: usize| v.clamp(0.0, limit as f64) as usize;
    let (x0, y0) = (
        clip(min_x.floor() - edge, width),
        clip(min_y.floor() - edge, height),
    );
    let (x1, y1) = (
        clip(max_x.ceil() + edge, width),
        clip(max_y.ceil() + edge, height),
    );
    if x1 <= x0 || y1 <= y0 {
        return whole;
    }
    Area { x0, y0, x1, y1 }
}

/// How sharp and bright a live frame is over the page's area, from integer sums divided once.
pub fn measure(frame: &Rgba, region: Option<&Quad>) -> Reading {
    measure_gray(&luma(frame), region)
}

pub fn measure_gray(gray: &Gray, region: Option<&Quad>) -> Reading {
    let (w, h) = (gray.width, gray.height);
    let px = &gray.data;
    let a = area(region, w, h);
    let (mut sum, mut count) = (0u64, 0u64);
    for y in a.y0..a.y1 {
        sum += px[y * w + a.x0..y * w + a.x1]
            .iter()
            .map(|&v| v as u64)
            .sum::<u64>();
        count += (a.x1 - a.x0) as u64;
    }
    let brightness = if count > 0 {
        sum as f64 / count as f64
    } else {
        0.0
    };

    let (mut lap_sum, mut lap_squares, mut n) = (0i64, 0i64, 0i64);
    for y in a.y0.max(1)..a.y1.min(h.saturating_sub(1)) {
        for x in a.x0.max(1)..a.x1.min(w.saturating_sub(1)) {
            let i = y * w + x;
            let l = 4 * px[i] as i64
                - px[i - w] as i64
                - px[i + w] as i64
                - px[i - 1] as i64
                - px[i + 1] as i64;
            lap_sum += l;
            lap_squares += l * l;
            n += 1;
        }
    }
    let sharpness = if n > 0 {
        let mean = lap_sum as f64 / n as f64;
        (lap_squares as f64 / n as f64 - mean * mean).max(0.0)
    } else {
        0.0
    };
    Reading {
        sharpness,
        brightness,
    }
}
