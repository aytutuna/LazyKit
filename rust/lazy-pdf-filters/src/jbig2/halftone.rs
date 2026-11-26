// Ported from pdf.js (src/core/jbig2.js: decodePatternDictionary, decodeHalftoneRegion) by way of Lazy PDF's C#
// decoder, and changed: skipped cells, every combination operator, MMR planes read one after the other, and
// limits on patterns and the grid. See budget.rs for the licence.

use std::rc::Rc;

use super::arith::Arith;
use super::bitmap::Bitmap;
use super::budget::Budget;
use super::generic;
use super::text::code_length;
use crate::error::{FilterError, Result};

/// The most patterns one dictionary may hold.
const MAX_PATTERNS: i64 = 1 << 16;

/// The most cells a halftone grid may have.
const MAX_CELLS: i64 = 1 << 25;

/// 6.7.5: the collective bitmap of all patterns, cut up.
#[allow(clippy::too_many_arguments)]
pub(crate) fn patterns(
    budget: &mut Budget,
    data: &[u8],
    start: usize,
    end: usize,
    mmr: bool,
    template: i32,
    width: i64,
    height: i64,
    max_index: i64,
) -> Result<Vec<Rc<Bitmap>>> {
    if width < 1 || height < 1 {
        return Err(FilterError::damaged(
            "A JBIG2 pattern dictionary has a bad size.",
        ));
    }
    if max_index >= MAX_PATTERNS {
        return Err(FilterError::too_big());
    }
    let count = max_index + 1;
    budget.keep(count * (width * height + 64))?;
    let collective = if mmr {
        generic::mmr(budget, data, start, end, count * width, height)?.0
    } else {
        let mut arith = Arith::new(data, start, end, None, Some(budget))?;
        let width = width as i32;
        let at = if template == 0 {
            vec![-width, 0, -3, -1, 2, -2, -2, -2]
        } else {
            vec![-width, 0]
        };
        generic::decode(
            budget,
            &mut arith,
            count * width as i64,
            height,
            template,
            false,
            None,
            &at,
        )?
    };
    (0..count as usize)
        .map(|i| {
            collective
                .columns(i * width as usize, width as usize, budget)
                .map(Rc::new)
        })
        .collect()
}

/// 6.6.5: what a halftone region segment says (7.4.5.1).
pub(crate) struct Params {
    pub mmr: bool,
    pub template: i32,
    pub enable_skip: bool,
    pub combination_operator: i32,
    pub default_pixel: u8,
    pub width: i64,
    pub height: i64,
    pub grid_width: i64,
    pub grid_height: i64,
    pub grid_x: i64,
    pub grid_y: i64,
    pub vector_x: i64,
    pub vector_y: i64,
}

pub(crate) fn region(
    budget: &mut Budget,
    data: &[u8],
    start: usize,
    end: usize,
    p: &Params,
    patterns: &[Rc<Bitmap>],
) -> Result<Bitmap> {
    if patterns.is_empty() {
        return Err(FilterError::damaged(
            "A JBIG2 halftone region has no patterns.",
        ));
    }
    if p.grid_width.saturating_mul(p.grid_height) > MAX_CELLS
        || p.grid_width > MAX_CELLS
        || p.grid_height > MAX_CELLS
    {
        return Err(FilterError::too_big());
    }
    let (grid_width, grid_height) = (p.grid_width as usize, p.grid_height as usize);
    let mut region = Bitmap::new(p.width, p.height, budget)?;
    if p.default_pixel != 0 {
        region.fill(1);
    }
    let (pattern_width, pattern_height) = (patterns[0].width as i64, patterns[0].height as i64);
    budget.work(p.grid_width.max(1) * p.grid_height.max(1))?;
    let outside = |x: i64, y: i64| {
        x + pattern_width <= 0 || x >= p.width || y + pattern_height <= 0 || y >= p.height
    };
    let skip = if p.enable_skip {
        // 6.6.5.1: the cells whose pattern would fall outside the region are not coded.
        let mut skip = Bitmap::new(p.grid_width, p.grid_height, budget)?;
        for m in 0..grid_height {
            for n in 0..grid_width {
                if outside(cell_x(p, m, n), cell_y(p, m, n)) {
                    skip.pixels[m * grid_width + n] = 1;
                }
            }
        }
        Some(skip)
    } else {
        None
    };
    // Annex C: the gray values are coded as bit planes, the highest first, each the exclusive or of
    // itself and the plane above.
    let plane_count = code_length(patterns.len()) as usize;
    let mut planes: Vec<Bitmap> = Vec::with_capacity(plane_count);
    let mut arith = if p.mmr {
        None
    } else {
        Some(Arith::new(data, start, end, None, Some(budget))?)
    };
    let at: Vec<i32> = if p.template == 0 {
        vec![3, -1, -3, -1, 2, -2, -2, -2]
    } else {
        vec![if p.template == 1 { 3 } else { 2 }, -1]
    };
    let mut position = start;
    // The planes are made from the highest down; `planes` holds them in that order.
    for j in (0..plane_count).rev() {
        let mut plane = match arith.as_mut() {
            None => {
                let (plane, used) =
                    generic::mmr(budget, data, position, end, p.grid_width, p.grid_height)?;
                position += used;
                plane
            }
            Some(arith) => generic::decode(
                budget,
                arith,
                p.grid_width,
                p.grid_height,
                p.template,
                false,
                skip.as_ref(),
                &at,
            )?,
        };
        if j < plane_count - 1 {
            let above = &planes[planes.len() - 1].pixels;
            for (pixel, above) in plane.pixels.iter_mut().zip(above) {
                *pixel ^= above;
            }
        }
        planes.push(plane);
    }
    // 6.6.5.2: a pattern at each cell of the grid.
    for m in 0..grid_height {
        for n in 0..grid_width {
            let mut index = 0usize;
            for j in 0..plane_count {
                index |= (planes[plane_count - 1 - j].pixels[m * grid_width + n] as usize) << j;
            }
            let pattern = &patterns[index.min(patterns.len() - 1)];
            let (x, y) = (cell_x(p, m, n), cell_y(p, m, n));
            if outside(x, y) {
                continue;
            }
            budget.work(pattern_width * pattern_height)?;
            region.compose(pattern, x as i32, y as i32, p.combination_operator);
        }
    }
    Ok(region)
}

fn cell_x(p: &Params, m: usize, n: usize) -> i64 {
    (p.grid_x + m as i64 * p.vector_y + n as i64 * p.vector_x) >> 8
}

fn cell_y(p: &Params, m: usize, n: usize) -> i64 {
    (p.grid_y + m as i64 * p.vector_x - n as i64 * p.vector_y) >> 8
}
