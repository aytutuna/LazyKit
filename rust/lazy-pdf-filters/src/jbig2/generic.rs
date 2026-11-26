/* Copyright 2012 Mozilla Foundation
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 * You may obtain a copy of the License at
 *
 *     http://www.apache.org/licenses/LICENSE-2.0
 *
 * Unless required by applicable law or agreed to in writing, software
 * distributed under the License is distributed on an "AS IS" BASIS,
 * WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
 * See the License for the specific language governing permissions and
 * limitations under the License.
 */

// Ported from pdf.js (src/core/jbig2.js: decodeBitmap, decodeRefinement) by way of Lazy PDF's C# decoder, and
// changed: bitmaps are flat byte arrays, the template bits follow the order of T.88 whatever the adaptive
// pixels are, skipped pixels and typical prediction of refinement rows (TPGRON) are done, MMR goes through
// the CCITT decoder.

use super::arith::Arith;
use super::bitmap::Bitmap;
use super::budget::Budget;
use crate::ccitt;
use crate::error::{FilterError, Result};

// 6.2.5.7, Figures 8 to 11: the context of the pseudo pixel that carries the typical prediction bit.
const REUSED_CONTEXTS: [usize; 4] = [0x9b25, 0x0795, 0x00e5, 0x0195];
// 6.3.5.6, Figures 14 and 15.
const REFINEMENT_REUSED_CONTEXTS: [usize; 2] = [0x0020, 0x0008];

/// The pixels the template looks at, in the order of the context bits from the highest. The adaptive
/// pixels (`at`, x then y for each) sit where Figures 3 to 6 put them.
fn template_pixels(template: i32, at: &[i32]) -> (Vec<i32>, Vec<i32>) {
    match template {
        0 => (
            vec![
                at[6], -1, 0, 1, at[4], at[2], -2, -1, 0, 1, 2, at[0], -4, -3, -2, -1,
            ],
            vec![
                at[7], -2, -2, -2, at[5], at[3], -1, -1, -1, -1, -1, at[1], 0, 0, 0, 0,
            ],
        ),
        1 => (
            vec![-1, 0, 1, 2, -2, -1, 0, 1, 2, at[0], -3, -2, -1],
            vec![-2, -2, -2, -2, -1, -1, -1, -1, -1, at[1], 0, 0, 0],
        ),
        2 => (
            vec![-1, 0, 1, -2, -1, 0, 1, at[0], -2, -1],
            vec![-2, -2, -2, -1, -1, -1, -1, at[1], 0, 0],
        ),
        _ => (
            vec![-3, -2, -1, 0, 1, at[0], -4, -3, -2, -1],
            vec![-1, -1, -1, -1, -1, at[1], 0, 0, 0, 0],
        ),
    }
}

/// 6.2.5: an arithmetic coded generic region. `skip` marks pixels that are not coded (halftone
/// regions); `at` has the x and y of every adaptive pixel (four of them for template 0).
#[allow(clippy::too_many_arguments)]
pub(crate) fn decode(
    budget: &mut Budget,
    arith: &mut Arith,
    width: i64,
    height: i64,
    template: i32,
    typical_prediction: bool,
    skip: Option<&Bitmap>,
    at: &[i32],
) -> Result<Bitmap> {
    // 6.2.5.4: an adaptive pixel is one that is already decoded. Far-off or later ones would also send
    // every pixel down the slow path.
    if at.chunks(2).any(|p| p[1] > 0 || p[1] == 0 && p[0] >= 0) {
        return Err(FilterError::damaged(
            "A JBIG2 adaptive template pixel is not before the pixel it codes.",
        ));
    }
    let mut bitmap = Bitmap::new(width, height, budget)?;
    if width == 0 || height == 0 {
        return Ok(bitmap);
    }
    let (width, height) = (width as i32, height as i32);
    let (tx, ty) = template_pixels(template, at);
    let length = tx.len();
    // A template pixel that is the next one's neighbour on the left keeps its place in the label of the
    // following pixel, shifted by one bit, so only the others are read again.
    let (mut reuse_mask, mut min_x, mut max_x, mut min_y, mut max_y) = (0u32, 0, 0, 0, 0);
    let mut changing = Vec::new();
    for k in 0..length {
        min_x = min_x.min(tx[k]);
        max_x = max_x.max(tx[k]);
        min_y = min_y.min(ty[k]);
        max_y = max_y.max(ty[k]);
        if k < length - 1 && ty[k] == ty[k + 1] && tx[k] == tx[k + 1] - 1 {
            reuse_mask |= 1 << (length - 1 - k);
        } else {
            changing.push((tx[k], ty[k], 1u32 << (length - 1 - k)));
        }
    }
    // Inside this box every template pixel is inside the bitmap.
    let (safe_left, safe_top) = (-min_x, -min_y);
    let (safe_right, safe_bottom) = (width - max_x, height - max_y);
    let mut contexts = std::mem::take(&mut arith.gb);
    let w = width as usize;
    let pseudo_pixel_context = REUSED_CONTEXTS[template as usize & 3];
    let mut ltp = 0;
    for i in 0..height {
        if typical_prediction {
            ltp ^= arith.bit(&mut contexts, pseudo_pixel_context);
            if ltp != 0 {
                if i > 0 {
                    let row = i as usize * w;
                    bitmap.pixels.copy_within(row - w..row, row);
                }
                continue;
            }
        }
        let mut label = 0u32;
        for j in 0..width {
            let px = &bitmap.pixels;
            if j >= safe_left && j < safe_right && i >= safe_top && i < safe_bottom {
                label = (label << 1) & reuse_mask;
                for &(dx, dy, bit) in &changing {
                    if px[((i + dy) * width + j + dx) as usize] != 0 {
                        label |= bit;
                    }
                }
            } else {
                label = 0;
                for k in 0..length {
                    let (j0, i0) = (j + tx[k], i + ty[k]);
                    if j0 >= 0
                        && j0 < width
                        && i0 >= 0
                        && i0 < height
                        && px[(i0 * width + j0) as usize] != 0
                    {
                        label |= 1 << (length - 1 - k);
                    }
                }
            }
            let index = i as usize * w + j as usize;
            if skip.is_some_and(|s| s.pixels[index] != 0) {
                continue;
            }
            bitmap.pixels[index] = arith.bit(&mut contexts, label as usize) as u8;
        }
    }
    arith.gb = contexts;
    Ok(bitmap)
}

/// 6.2.6: an MMR coded generic region from `start` to `end`; the second value is the number of bytes
/// the coded rows took.
pub(crate) fn mmr(
    budget: &mut Budget,
    data: &[u8],
    start: usize,
    end: usize,
    width: i64,
    height: i64,
) -> Result<(Bitmap, usize)> {
    let mut bitmap = Bitmap::new(width, height, budget)?;
    if width == 0 || height == 0 {
        return Ok((bitmap, 0));
    }
    let end = end.min(data.len());
    if start > end {
        return Err(FilterError::damaged("A JBIG2 MMR region has no data."));
    }
    let (packed, used) = ccitt::decode_mmr(&data[start..end], bitmap.width, bitmap.height);
    let stride = bitmap.width.div_ceil(8);
    for y in 0..bitmap.height {
        for x in 0..bitmap.width {
            bitmap.pixels[y * bitmap.width + x] =
                (packed[y * stride + (x >> 3)] >> (7 - (x & 7))) & 1;
        }
    }
    Ok((bitmap, used))
}

/// 6.3.5: a refinement of `reference`, which sits at (`offset_x`, `offset_y`) of the new bitmap. `at`
/// has the two adaptive pixels of template 0 (the coding one first).
#[allow(clippy::too_many_arguments)]
pub(crate) fn refine(
    budget: &mut Budget,
    arith: &mut Arith,
    width: i64,
    height: i64,
    template: i32,
    reference: &Bitmap,
    offset_x: i64,
    offset_y: i64,
    typical_prediction: bool,
    at: &[i32],
) -> Result<Bitmap> {
    let mut bitmap = Bitmap::new(width, height, budget)?;
    if width == 0 || height == 0 {
        return Ok(bitmap);
    }
    let (width, height) = (width as i32, height as i32);
    let (cx, cy, rx, ry): (Vec<i32>, Vec<i32>, Vec<i32>, Vec<i32>) = if template == 0 {
        (
            vec![0, 1, -1, at[0]],
            vec![-1, -1, 0, at[1]],
            vec![0, 1, -1, 0, 1, -1, 0, 1, at[2]],
            vec![-1, -1, 0, 0, 0, 1, 1, 1, at[3]],
        )
    } else {
        (
            vec![-1, 0, 1, -1],
            vec![-1, -1, -1, 0],
            vec![0, -1, 0, 1, 0, 1],
            vec![-1, 0, 0, 0, 1, 1],
        )
    };
    let mut contexts = std::mem::take(&mut arith.gr);
    let pseudo_pixel_context = REFINEMENT_REUSED_CONTEXTS[template as usize & 1];
    let mut ltp = 0;
    for i in 0..height {
        if typical_prediction {
            ltp ^= arith.bit(&mut contexts, pseudo_pixel_context);
        }
        for j in 0..width {
            let index = (i * width + j) as usize;
            // 6.3.5.6: where the 3 by 3 pixels of the reference around this one agree, so does the new pixel.
            if ltp != 0 {
                let center = reference_pixel(reference, j as i64 - offset_x, i as i64 - offset_y);
                let same = (-1i64..=1).all(|dy| {
                    (-1i64..=1).all(|dx| {
                        reference_pixel(
                            reference,
                            j as i64 - offset_x + dx,
                            i as i64 - offset_y + dy,
                        ) == center
                    })
                });
                if same {
                    bitmap.pixels[index] = center;
                    continue;
                }
            }
            let mut label = 0usize;
            for k in 0..cx.len() {
                let (i0, j0) = (i + cy[k], j + cx[k]);
                label <<= 1;
                if i0 >= 0 && i0 < height && j0 >= 0 && j0 < width {
                    label |= bitmap.pixels[(i0 * width + j0) as usize] as usize;
                }
            }
            for k in 0..rx.len() {
                label = (label << 1)
                    | reference_pixel(
                        reference,
                        (j + rx[k]) as i64 - offset_x,
                        (i + ry[k]) as i64 - offset_y,
                    ) as usize;
            }
            bitmap.pixels[index] = arith.bit(&mut contexts, label) as u8;
        }
    }
    arith.gr = contexts;
    Ok(bitmap)
}

fn reference_pixel(reference: &Bitmap, x: i64, y: i64) -> u8 {
    if x < 0 || y < 0 || x >= reference.width as i64 || y >= reference.height as i64 {
        0
    } else {
        reference.pixels[y as usize * reference.width + x as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Offsets come straight from a stream; none of them may panic in a debug build.
    #[test]
    fn refinement_offsets_at_the_extremes_do_not_overflow() {
        let reference = Bitmap::new(4, 4, &mut Budget::new()).unwrap();
        let data = [0x55u8; 16];
        for template in 0..2 {
            for typical in [false, true] {
                for offset in [
                    i32::MIN as i64 * 2,
                    i32::MAX as i64 * 2,
                    i32::MIN as i64,
                    i32::MAX as i64,
                ] {
                    let mut budget = Budget::new();
                    let mut arith = Arith::new(&data, 0, data.len(), None, None).unwrap();
                    let at = [-1, -1, -1, -1];
                    let bitmap = refine(
                        &mut budget,
                        &mut arith,
                        4,
                        4,
                        template,
                        &reference,
                        offset,
                        -offset,
                        typical,
                        &at,
                    )
                    .unwrap();
                    assert_eq!(bitmap.pixels.len(), 16);
                }
            }
        }
    }
}
