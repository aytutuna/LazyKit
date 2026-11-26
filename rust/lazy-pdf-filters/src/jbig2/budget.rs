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

// Ported from pdf.js (src/core/jbig2.js) by way of Lazy PDF's C# decoder, and changed there and here:
// every size and count is checked against a budget before anything is allocated.

use crate::error::{FilterError, Result};

/// The most pixels an image may have.
pub(crate) const MAX_PIXELS: i64 = 1 << 27;

/// What one JBIG2 decode may allocate, so that a hostile stream cannot make the decoder run out of memory.
pub(crate) struct Budget {
    /// The pixels of every bitmap made during one decode, whether still alive or not.
    total: i64,
    /// The pixels of the symbols and patterns that stay around for later segments.
    kept: i64,
    /// The pixels that symbols and refinements may be drawn or coded over, all together.
    work: i64,
}

impl Budget {
    pub fn new() -> Self {
        Self {
            total: 3 * MAX_PIXELS,
            kept: 1 << 26,
            work: 1 << 29,
        }
    }

    /// The globals of an image may be shared by many; they make symbols and tables, not pages.
    pub fn for_globals(&mut self) {
        self.total = self.total.min(1 << 26);
        self.work = self.work.min(1 << 27);
    }

    /// The budget of the image's own segments: the most there is, or tied to the size of its picture.
    pub fn for_image(&mut self, fit_pixels: i64) {
        self.total = 3 * MAX_PIXELS;
        self.work = 1 << 29;
        if fit_pixels > 0 {
            self.fit(fit_pixels);
        }
    }

    /// With the pixels of the picture the page is fitted to, what a decode may make is a multiple of
    /// that and a little more: a dictionary that says 1 by 1 does not get a page of millions of
    /// pixels made for it and cut down.
    fn fit(&mut self, fit_pixels: i64) {
        // Past this the other limits are the smaller ones; the cap keeps the products below from overflowing.
        let fit_pixels = fit_pixels.min(1 << 40);
        self.total = self.total.min(16 * fit_pixels + (1 << 24));
        // Enough for a page of symbols however small, and for drawing over the picture many times.
        self.work = self.work.min(32 * fit_pixels + (1 << 26));
    }

    /// The arrays a segment makes whatever it holds, counted as pixels.
    pub fn fixed(&mut self, bytes: i64) -> Result<()> {
        self.charge(bytes, 1)
    }

    pub fn charge(&mut self, width: i64, height: i64) -> Result<()> {
        if width < 0 || height < 0 {
            return Err(FilterError::too_big());
        }
        // A bitmap with no columns or no rows still costs the rows or columns that a loop over it walks.
        let pixels = width.max(1).saturating_mul(height.max(1));
        if pixels > MAX_PIXELS || pixels > self.total {
            return Err(FilterError::too_big());
        }
        self.total -= pixels;
        Ok(())
    }

    pub fn work(&mut self, pixels: i64) -> Result<()> {
        if pixels > self.work {
            return Err(FilterError::too_big());
        }
        self.work -= pixels;
        Ok(())
    }

    pub fn keep(&mut self, pixels: i64) -> Result<()> {
        if pixels > self.kept {
            return Err(FilterError::too_big());
        }
        self.kept -= pixels;
        Ok(())
    }
}
