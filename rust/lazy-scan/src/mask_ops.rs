use std::collections::BTreeMap;

/// One 4-connected region of a binary mask: its area and, per row, the horizontal extent (minX, maxX).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MaskComponent {
    pub area: usize,
    pub rows: BTreeMap<usize, (usize, usize)>,
}

// A mask is one byte per pixel, row-major, 1 = set and 0 = clear.

/// Dilation with a (2r+1) x (2r+1) square: a pixel is set when any pixel within r (per axis) is.
pub fn dilate(mask: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut tmp = vec![0u8; w * h];
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            let any = (x.saturating_sub(r)..=(x + r).min(w - 1)).any(|xx| mask[row + xx] == 1);
            tmp[row + x] = any as u8;
        }
    }
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let any = (y.saturating_sub(r)..=(y + r).min(h - 1)).any(|yy| tmp[yy * w + x] == 1);
            out[y * w + x] = any as u8;
        }
    }
    out
}

/// Erosion with a (2r+1) x (2r+1) square: a pixel stays set when every pixel within r is. Pixels outside the image count as
/// clear, so the mask shrinks at the borders, unless `outside_set` says they count as set (a region touching the border then
/// keeps its edge there, as when closing the mask of a page laid against the glass's edge).
pub fn erode(mask: &[u8], w: usize, h: usize, r: usize, outside_set: bool) -> Vec<u8> {
    let mut tmp = vec![0u8; w * h];
    for y in 0..h {
        let row = y * w;
        for x in 0..w {
            let inside = outside_set || (x >= r && x + r < w);
            let all =
                inside && (x.saturating_sub(r)..=(x + r).min(w - 1)).all(|xx| mask[row + xx] == 1);
            tmp[row + x] = all as u8;
        }
    }
    let mut out = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let inside = outside_set || (y >= r && y + r < h);
            let all =
                inside && (y.saturating_sub(r)..=(y + r).min(h - 1)).all(|yy| tmp[yy * w + x] == 1);
            out[y * w + x] = all as u8;
        }
    }
    out
}

fn neighbours(idx: usize, width: usize, height: usize, eight: bool, out: &mut Vec<usize>) {
    out.clear();
    let (x, y) = (idx % width, idx / width);
    if x > 0 {
        out.push(idx - 1);
    }
    if x < width - 1 {
        out.push(idx + 1);
    }
    if y > 0 {
        out.push(idx - width);
    }
    if y < height - 1 {
        out.push(idx + width);
    }
    if eight {
        if x > 0 && y > 0 {
            out.push(idx - width - 1);
        }
        if x < width - 1 && y > 0 {
            out.push(idx - width + 1);
        }
        if x > 0 && y < height - 1 {
            out.push(idx + width - 1);
        }
        if x < width - 1 && y < height - 1 {
            out.push(idx + width + 1);
        }
    }
}

/// The 4-connected components of the set pixels, in the order their first pixel appears (row-major), each with its area and
/// per-row extent.
pub fn connected_components(mask: &[u8], width: usize, height: usize) -> Vec<MaskComponent> {
    let mut visited = vec![false; mask.len()];
    let mut components = Vec::new();
    let mut stack = Vec::new();
    let mut next = Vec::new();
    for start in 0..mask.len() {
        if mask[start] != 1 || visited[start] {
            continue;
        }
        let mut comp = MaskComponent::default();
        visited[start] = true;
        stack.push(start);
        while let Some(idx) = stack.pop() {
            let (x, y) = (idx % width, idx / width);
            comp.area += 1;
            let span = comp.rows.entry(y).or_insert((x, x));
            *span = (span.0.min(x), span.1.max(x));
            neighbours(idx, width, height, false, &mut next);
            for &n in &next {
                if mask[n] == 1 && !visited[n] {
                    visited[n] = true;
                    stack.push(n);
                }
            }
        }
        components.push(comp);
    }
    components
}

/// The areas of the components of the set pixels, in the order their first pixel appears; cheaper than
/// `connected_components` when only the sizes matter (dust filtering). 4-connected, or 8-connected (diagonal neighbours join
/// too, so a thin slanted pen stroke stays one mark).
pub fn component_areas(
    mask: &[u8],
    width: usize,
    height: usize,
    eight_connected: bool,
) -> Vec<usize> {
    let mut visited = vec![false; mask.len()];
    let mut areas = Vec::new();
    let mut stack = Vec::new();
    let mut next = Vec::new();
    for start in 0..mask.len() {
        if mask[start] != 1 || visited[start] {
            continue;
        }
        let mut area = 0;
        visited[start] = true;
        stack.push(start);
        while let Some(idx) = stack.pop() {
            area += 1;
            neighbours(idx, width, height, eight_connected, &mut next);
            for &n in &next {
                if mask[n] == 1 && !visited[n] {
                    visited[n] = true;
                    stack.push(n);
                }
            }
        }
        areas.push(area);
    }
    areas
}
