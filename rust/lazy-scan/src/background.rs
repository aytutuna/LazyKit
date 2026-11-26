use lazy_image::Gray;

/// How bright the paper is across a page: a high percentile of each tile's luma, interpolated between tile centres.
#[derive(Clone, Debug)]
pub struct BackgroundMap {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
    columns: usize,
    rows: usize,
    pub tiles: Vec<u8>,
    across: Interpolation,
    down: Interpolation,
}

#[derive(Clone, Debug)]
struct Interpolation {
    first: Vec<usize>,
    second: Vec<usize>,
    weight: Vec<f64>,
}

/// The smallest value with at least ceil(fraction x count) values at or below it.
pub fn percentile(histogram: &[i32], count: usize, fraction: f64) -> usize {
    let rank = ((fraction * count as f64).ceil() as i64).clamp(1, count.max(1) as i64);
    let mut seen = 0i64;
    for (v, n) in histogram.iter().enumerate() {
        seen += *n as i64;
        if seen >= rank {
            return v;
        }
    }
    histogram.len() - 1
}

fn lerp(a: u8, b: u8, t: f64) -> f64 {
    a as f64 + (b as f64 - a as f64) * t
}

// For each of `length` pixels split into `tiles` equal tiles: the tiles whose centres surround the pixel's centre and the second's weight.
fn interpolation(length: usize, tiles: usize) -> Interpolation {
    let mut first = vec![0; length];
    let mut second = vec![0; length];
    let mut weight = vec![0.0; length];
    let centres: Vec<f64> = (0..tiles)
        .map(|t| ((t * length / tiles + (t + 1) * length / tiles) as f64) / 2.0)
        .collect();
    let mut k = 0;
    for p in 0..length {
        let c = p as f64 + 0.5;
        while k < tiles - 1 && centres[k + 1] <= c {
            k += 1;
        }
        if c <= centres[0] {
            first[p] = 0;
            second[p] = 0;
        } else if k >= tiles - 1 {
            first[p] = tiles - 1;
            second[p] = tiles - 1;
        } else {
            first[p] = k;
            second[p] = k + 1;
            weight[p] = (c - centres[k]) / (centres[k + 1] - centres[k]);
        }
    }
    Interpolation {
        first,
        second,
        weight,
    }
}

impl BackgroundMap {
    fn with_tiles(
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        columns: usize,
        rows: usize,
        tiles: Vec<u8>,
    ) -> Self {
        BackgroundMap {
            x,
            y,
            width,
            height,
            columns,
            rows,
            tiles,
            across: interpolation(width, columns),
            down: interpolation(height, rows),
        }
    }

    fn same_size(&self, tiles: Vec<u8>) -> Self {
        BackgroundMap {
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            columns: self.columns,
            rows: self.rows,
            tiles,
            across: self.across.clone(),
            down: self.down.clone(),
        }
    }

    pub fn estimate(gray: &Gray, tile_size: usize, fraction: f64) -> Option<Self> {
        Self::estimate_in(gray, 0, 0, gray.width, gray.height, tile_size, fraction)
    }

    /// The map of the area of `gray` at (x, y), `width` by `height` pixels (kept inside the image), in whole tiles of about
    /// `tile_size` pixels (the last ones a little larger), each one's background the `fraction` percentile of its pixels;
    /// `None` when the area is empty.
    pub fn estimate_in(
        gray: &Gray,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        tile_size: usize,
        fraction: f64,
    ) -> Option<Self> {
        let x0 = x.min(gray.width);
        let y0 = y.min(gray.height);
        let x1 = x.saturating_add(width).clamp(x0, gray.width);
        let y1 = y.saturating_add(height).clamp(y0, gray.height);
        let (w, h) = (x1 - x0, y1 - y0);
        if w == 0 || h == 0 {
            return None;
        }
        let size = tile_size.max(1);
        let columns = (w / size).max(1);
        let rows = (h / size).max(1);
        let mut tiles = vec![0u8; columns * rows];
        let mut histogram = [0i32; 256];
        for ty in 0..rows {
            let (top, bottom) = (y0 + ty * h / rows, y0 + (ty + 1) * h / rows);
            for tx in 0..columns {
                let (left, right) = (x0 + tx * w / columns, x0 + (tx + 1) * w / columns);
                histogram.fill(0);
                for py in top..bottom {
                    for px in left..right {
                        histogram[gray.data[py * gray.width + px] as usize] += 1;
                    }
                }
                tiles[ty * columns + tx] =
                    percentile(&histogram, (bottom - top) * (right - left), fraction) as u8;
            }
        }
        Some(Self::with_tiles(x0, y0, w, h, columns, rows, tiles))
    }

    pub fn median_tile(&self) -> u8 {
        let mut sorted = self.tiles.clone();
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    }

    fn neighbours(&self, pick: fn(u8, u8) -> u8, start: u8) -> Self {
        let mut out = vec![0u8; self.tiles.len()];
        for ty in 0..self.rows {
            for tx in 0..self.columns {
                let mut best = start;
                for ny in ty.saturating_sub(1)..=(ty + 1).min(self.rows - 1) {
                    for nx in tx.saturating_sub(1)..=(tx + 1).min(self.columns - 1) {
                        best = pick(best, self.tiles[ny * self.columns + nx]);
                    }
                }
                out[ty * self.columns + tx] = best;
            }
        }
        self.same_size(out)
    }

    /// Each tile replaced by the brightest of itself and its eight neighbours.
    pub fn max_smoothed(&self) -> Self {
        self.neighbours(u8::max, 0)
    }

    /// Each tile replaced by the darkest of itself and its eight neighbours.
    pub fn min_smoothed(&self) -> Self {
        self.neighbours(u8::min, 255)
    }

    /// Tiles much darker than the paper beside them are filled in from it, pass after pass; a filled region that reaches
    /// the map's outer tiles is lighting (a shadow cast across the sheet) and takes its own values back.
    pub fn raised_dark_tiles(&self, share: f64) -> Self {
        let (columns, rows) = (self.columns, self.rows);
        let mut current = self.tiles.clone();
        let mut next = vec![0u8; current.len()];
        for _ in 0..columns.max(rows) {
            let mut changed = false;
            for ty in 0..rows {
                for tx in 0..columns {
                    let mut best = 0u8;
                    for ny in ty.saturating_sub(1)..=(ty + 1).min(rows - 1) {
                        for nx in tx.saturating_sub(1)..=(tx + 1).min(columns - 1) {
                            if nx == tx && ny == ty {
                                continue;
                            }
                            best = best.max(current[ny * columns + nx]);
                        }
                    }
                    let value = current[ty * columns + tx];
                    if (value as f64) < share * best as f64 {
                        next[ty * columns + tx] = best;
                        changed = true;
                    } else {
                        next[ty * columns + tx] = value;
                    }
                }
            }
            std::mem::swap(&mut current, &mut next);
            if !changed {
                break;
            }
        }
        let tiles = &self.tiles;
        let mut visited = vec![false; current.len()];
        let mut members = vec![0usize; current.len()];
        for start in 0..current.len() {
            if visited[start] || current[start] == tiles[start] {
                continue;
            }
            visited[start] = true;
            members[0] = start;
            let mut count = 1;
            let mut reaches_edge = false;
            let mut k = 0;
            while k < count {
                let tile = members[k];
                let (tx, ty) = (tile % columns, tile / columns);
                if tx == 0 || ty == 0 || tx == columns - 1 || ty == rows - 1 {
                    reaches_edge = true;
                }
                for ny in ty.saturating_sub(1)..=(ty + 1).min(rows - 1) {
                    for nx in tx.saturating_sub(1)..=(tx + 1).min(columns - 1) {
                        let n = ny * columns + nx;
                        if visited[n] || current[n] == tiles[n] {
                            continue;
                        }
                        visited[n] = true;
                        members[count] = n;
                        count += 1;
                    }
                }
                k += 1;
            }
            if reaches_edge {
                for &m in &members[..count] {
                    current[m] = tiles[m];
                }
            }
        }
        self.same_size(current)
    }

    pub fn clamped_below(&self, floor: u8) -> Self {
        self.same_size(self.tiles.iter().map(|&v| v.max(floor)).collect())
    }

    /// Bilinear between the centres of the surrounding tiles, the nearest tile's value beyond the outer centres.
    pub fn value_at(&self, x: usize, y: usize) -> f64 {
        let ax = x.min(self.width - 1);
        let ay = y.min(self.height - 1);
        let (x0, x1, fx) = (
            self.across.first[ax],
            self.across.second[ax],
            self.across.weight[ax],
        );
        let (y0, y1, fy) = (
            self.down.first[ay],
            self.down.second[ay],
            self.down.weight[ay],
        );
        let c = self.columns;
        let top = lerp(self.tiles[y0 * c + x0], self.tiles[y0 * c + x1], fx);
        let bottom = lerp(self.tiles[y1 * c + x0], self.tiles[y1 * c + x1], fx);
        top + (bottom - top) * fy
    }

    /// The ink of the area: 1 where a pixel of `gray` is darker than (1 - `contrast`) of the background there, 0 elsewhere. One
    /// byte per pixel, the area's width by its height, row-major.
    pub fn ink_mask(&self, gray: &Gray, contrast: f64) -> Vec<u8> {
        let keep = 1.0 - contrast;
        let mut mask = vec![0u8; self.width * self.height];
        for ay in 0..self.height {
            let row = (self.y + ay) * gray.width + self.x;
            for ax in 0..self.width {
                if (gray.data[row + ax] as f64) < self.value_at(ax, ay) * keep {
                    mask[ay * self.width + ax] = 1;
                }
            }
        }
        mask
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(width: usize, height: usize, f: impl Fn(usize, usize) -> u8) -> Gray {
        let mut data = Vec::new();
        for y in 0..height {
            for x in 0..width {
                data.push(f(x, y));
            }
        }
        Gray {
            width,
            height,
            data,
        }
    }

    fn map(columns: usize, rows: usize, tiles: &[u8]) -> BackgroundMap {
        BackgroundMap::with_tiles(columns * 4, rows * 4, columns, rows, tiles.to_vec())
    }

    #[test]
    fn percentile_takes_the_value_at_the_rank() {
        let mut histogram = [0i32; 256];
        histogram[10] = 5;
        histogram[200] = 5;
        assert_eq!(percentile(&histogram, 10, 0.5), 10);
        assert_eq!(percentile(&histogram, 10, 0.51), 200);
        assert_eq!(percentile(&histogram, 10, 0.0), 10);
        assert_eq!(percentile(&histogram, 10, 1.0), 200);
        assert_eq!(percentile(&[0; 256], 0, 0.9), 255);
    }

    #[test]
    fn tiles_hold_the_percentile_of_their_luma() {
        // two 8 x 8 tiles: the left all 100, the right 100 with one bright pixel in ten
        let g = gray(16, 8, |x, y| {
            if x >= 8 && (x + y) % 10 == 0 {
                250
            } else {
                100
            }
        });
        let m = BackgroundMap::estimate(&g, 8, 0.9).unwrap();
        assert_eq!((m.columns, m.rows), (2, 1));
        assert_eq!(m.tiles, vec![100, 100]);
        let top = BackgroundMap::estimate(&g, 8, 1.0).unwrap();
        assert_eq!(top.tiles, vec![100, 250]);
    }

    #[test]
    fn values_run_between_tile_centres_and_hold_beyond_them() {
        let m = map(2, 1, &[100, 200]);
        assert_eq!(m.value_at(0, 0), 100.0);
        assert_eq!(m.value_at(7, 0), 200.0);
        // width 8, tiles 4 wide: centres at 2 and 6; pixel 3 (centre 3.5) is 3/8 of the way
        assert_eq!(m.value_at(3, 0), 100.0 + 100.0 * 0.375);
        assert_eq!(m.value_at(100, 100), 200.0);
    }

    #[test]
    fn dark_tiles_are_filled_from_the_paper_beside_them() {
        let mut tiles = [200u8; 25];
        tiles[12] = 60;
        let kept = map(5, 5, &tiles).raised_dark_tiles(0.7);
        assert_eq!(kept.tiles[12], 200);
    }

    #[test]
    fn a_dark_region_that_reaches_the_edge_is_a_shadow_and_stays() {
        let mut tiles = [200u8; 25];
        for ty in 0..5 {
            for tx in 3..5 {
                tiles[ty * 5 + tx] = 60;
            }
        }
        let kept = map(5, 5, &tiles).raised_dark_tiles(0.7);
        assert_eq!(kept.tiles, tiles.to_vec());
    }

    #[test]
    fn neighbours_and_floor() {
        let mut tiles = [100u8; 9];
        tiles[4] = 30;
        let m = map(3, 3, &tiles);
        assert_eq!(m.max_smoothed().tiles, vec![100; 9]);
        assert_eq!(m.min_smoothed().tiles, vec![30; 9]);
        assert_eq!(m.clamped_below(50).tiles[4], 50);
        assert_eq!(m.median_tile(), 100);
    }
}
