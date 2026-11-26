use lazy_image::Rgba;
use lazy_scan::Quad;

/// A white sheet of short dark strokes in lines (a letter is a stem or two and a bar, strokes 2 px thick), from a fixed seed.
pub fn text_page(width: usize, height: usize, seed: u32) -> Rgba {
    let mut s = seed;
    let mut rnd = || {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        s as f64 / 4294967296.0
    };
    let mut data = vec![255u8; width * height * 4];
    let mut y = 80;
    while y < height - 80 {
        let mut x = 80;
        while x < width - 120 {
            let w = 8 + (rnd() * 8.0).floor() as usize;
            let mut mark = |x0: usize, y0: usize, mw: usize, mh: usize| {
                for j in 0..mh {
                    for i in 0..mw {
                        let o = ((y + y0 + j) * width + x + x0 + i) * 4;
                        data[o..o + 3].fill(30);
                    }
                }
            };
            mark(0, 0, 2, 16);
            mark(w - 2, 0, 2, 16);
            let bar = if rnd() < 0.5 { 0 } else { 7 };
            mark(0, bar, w, 2);
            x += w + 4 + (rnd() * 6.0).floor() as usize;
        }
        y += 34;
    }
    Rgba::new(width, height, data)
}

/// The homography taking the points `from` to `to` (h33 = 1); `None` when it is singular.
pub fn homography(from: &Quad, to: &[[f64; 2]; 4]) -> Option<[f64; 8]> {
    let mut m = [[0.0f64; 9]; 8];
    for i in 0..4 {
        let (x, y) = (from.0[i].x, from.0[i].y);
        let (u, v) = (to[i][0], to[i][1]);
        m[2 * i] = [x, y, 1.0, 0.0, 0.0, 0.0, -u * x, -u * y, u];
        m[2 * i + 1] = [0.0, 0.0, 0.0, x, y, 1.0, -v * x, -v * y, v];
    }
    for c in 0..8 {
        let mut pivot = c;
        for r in c + 1..8 {
            if m[r][c].abs() > m[pivot][c].abs() {
                pivot = r;
            }
        }
        if m[pivot][c].abs() < 1e-12 {
            return None;
        }
        m.swap(c, pivot);
        for r in 0..8 {
            if r == c {
                continue;
            }
            let f = m[r][c] / m[c][c];
            for k in c..9 {
                m[r][k] -= f * m[c][k];
            }
        }
    }
    let mut h = [0.0; 8];
    for i in 0..8 {
        h[i] = m[i][8] / m[i][i];
    }
    Some(h)
}

/// `page` laid onto a brown table so that `quad` (in the photo) is its outline.
pub fn photo_of(page: &Rgba, quad: &Quad, width: usize, height: usize) -> Rgba {
    let corners = [
        [0.0, 0.0],
        [page.width as f64, 0.0],
        [page.width as f64, page.height as f64],
        [0.0, page.height as f64],
    ];
    let h = homography(quad, &corners).unwrap();
    let mut data = vec![0u8; width * height * 4];
    for y in 0..height {
        for x in 0..width {
            let o = (y * width + x) * 4;
            let (px, py) = (x as f64 + 0.5, y as f64 + 0.5);
            let w = h[6] * px + h[7] * py + 1.0;
            let u = (h[0] * px + h[1] * py + h[2]) / w;
            let v = (h[3] * px + h[4] * py + h[5]) / w;
            let inside = u >= 0.0 && v >= 0.0 && u < page.width as f64 && v < page.height as f64;
            if inside {
                let p = (v.floor() as usize * page.width + u.floor() as usize) * 4;
                data[o..o + 3].copy_from_slice(&page.data[p..p + 3]);
            } else {
                data[o..o + 3].copy_from_slice(&[120, 90, 60]);
            }
            data[o + 3] = 255;
        }
    }
    Rgba::new(width, height, data)
}

/// Every colour value mapped through `f`.
pub fn map_pixels(img: &Rgba, f: impl Fn(u8) -> u8) -> Rgba {
    let mut data = img.data.clone();
    for p in data.as_chunks_mut::<4>().0.iter_mut() {
        for v in &mut p[..3] {
            *v = f(*v);
        }
        p[3] = 255;
    }
    Rgba::new(img.width, img.height, data)
}

pub const QUALITY_W: usize = 320;
pub const QUALITY_H: usize = 240;

/// Lazy Scan's QualityFrames: a wooden table and a sheet from (80, 40) to (240, 200), with lines of print if asked,
/// blurred by a box of (2 blur + 1)^2 pixels and darkened to dim/255.
pub fn quality_frame(printed: bool, blur: usize, dim: u32) -> Rgba {
    let (w, h) = (QUALITY_W, QUALITY_H);
    let mut color = vec![[0u32; 3]; w * h];
    for y in 0..h {
        for x in 0..w {
            let mut c = [88, 80, 72];
            if (80..240).contains(&x) && (40..200).contains(&y) {
                let inked = printed
                    && (96..224).contains(&x)
                    && (56..184).contains(&y)
                    && (y - 56) % 4 < 2
                    && (x - 96) % 10 < 7;
                c = if inked {
                    [95, 92, 100]
                } else {
                    [205, 200, 192]
                };
            }
            color[y * w + x] = c;
        }
    }
    let mut data = vec![255u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (mut s, mut n) = ([0u32; 3], 0u32);
            for yy in y.saturating_sub(blur)..h.min(y + blur + 1) {
                for xx in x.saturating_sub(blur)..w.min(x + blur + 1) {
                    for k in 0..3 {
                        s[k] += color[yy * w + xx][k];
                    }
                    n += 1;
                }
            }
            for k in 0..3 {
                data[(y * w + x) * 4 + k] = (((s[k] + n / 2) / n * dim + 127) / 255) as u8;
            }
        }
    }
    Rgba::new(w, h, data)
}

pub fn quality_sheet() -> Quad {
    Quad::from_xy([[80.0, 40.0], [240.0, 40.0], [240.0, 200.0], [80.0, 200.0]])
}
