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

// Ported from pdf.js (src/core/jbig2.js) by way of Lazy PDF's C# decoder.

use super::budget::Budget;
use crate::error::Result;

/// A bilevel image as one byte, 0 or 1, per pixel; 1 is black. Rows follow each other without padding.
#[derive(Clone)]
pub(crate) struct Bitmap {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl Bitmap {
    pub fn new(width: i64, height: i64, budget: &mut Budget) -> Result<Self> {
        budget.charge(width, height)?;
        // The charge keeps both sides, and so their product, within the pixel limit.
        let (width, height) = (width as usize, height as usize);
        Ok(Self {
            width,
            height,
            pixels: vec![0; width * height],
        })
    }

    pub fn fill(&mut self, value: u8) {
        self.pixels.fill(value);
    }

    /// Combines `source` with this bitmap with its top left corner at (x, y), the parts outside this
    /// bitmap dropped. The operators are those of 7.4.1.5: 0 OR, 1 AND, 2 XOR, 3 XNOR, 4 REPLACE.
    pub fn compose(&mut self, source: &Bitmap, x: i32, y: i32, op: i32) {
        let (x, y) = (x as i64, y as i64);
        let (sw, sh) = (source.width as i64, source.height as i64);
        let left = (-x).clamp(0, sw);
        let top = (-y).clamp(0, sh);
        let right = sw.min(self.width as i64 - x);
        let bottom = sh.min(self.height as i64 - y);
        if left >= right || top >= bottom {
            return;
        }
        for sy in top..bottom {
            let from = (sy * sw) as usize;
            let to = (y + sy) * self.width as i64 + x;
            for sx in left..right {
                let s = source.pixels[from + sx as usize];
                let d = &mut self.pixels[(to + sx) as usize];
                *d = match op {
                    0 => *d | s,
                    1 => *d & s,
                    2 => *d ^ s,
                    3 => 1 ^ *d ^ s,
                    _ => s,
                };
            }
        }
    }

    /// The columns from `x` to `x` + `width` of every row.
    pub fn columns(&self, x: usize, width: usize, budget: &mut Budget) -> Result<Bitmap> {
        let mut part = Bitmap::new(width as i64, self.height as i64, budget)?;
        for row in 0..self.height {
            let from = row * self.width + x;
            part.pixels[row * width..(row + 1) * width]
                .copy_from_slice(&self.pixels[from..from + width]);
        }
        Ok(part)
    }
}
