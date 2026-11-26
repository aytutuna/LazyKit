// Ported from pdf.js (src/core/jbig2.js: decodeTextRegion, decodeSymbolDictionary) by way of Lazy PDF's C#
// decoder, and changed: refinement with Huffman coding, all four combination operators, transposed placement
// as 6.4.5 gives it, retained contexts, and limits on the symbols, instances and work a stream may ask for.
// See budget.rs for the licence.

use std::rc::Rc;
use std::sync::Arc;

use super::arith::{Arith, IADS, IADT, IAFS, IAIT, IARDH, IARDW, IARDX, IARDY, IARI};
use super::bitmap::Bitmap;
use super::budget::Budget;
use super::generic;
use super::huffman::{Huffman, Reader};
use crate::error::{FilterError, Result};

/// The most instances a region may have, whatever its header says.
pub(crate) const MAX_INSTANCES: i64 = 1 << 22;

/// ceil(log2(n)), the length of the codes of n symbols.
pub(crate) fn code_length(n: usize) -> u32 {
    if n <= 1 {
        0
    } else {
        usize::BITS - (n - 1).leading_zeros()
    }
}

type Table = Option<Arc<Huffman>>;

/// What the text region decoding procedure (6.4, Table 9) is given.
pub(crate) struct TextParams<'s> {
    pub huffman: bool,
    pub refine: bool,
    pub width: i64,
    pub height: i64,
    pub default_pixel: u8,
    pub instances: i64,
    pub log_strip_size: u32,
    pub reference_corner: i32,
    pub transposed: bool,
    pub combination_operator: i32,
    pub ds_offset: i32,
    pub refinement_template: i32,
    pub refinement_at: [i32; 4],
    pub symbols: &'s [Rc<Bitmap>],
    /// The bits of a symbol id: coded with the IAID procedure, or a fixed length code when there is no
    /// `symbol_ids` table.
    pub symbol_code_length: u32,
    pub symbol_ids: Table,
    pub first_s: Table,
    pub delta_s: Table,
    pub delta_t: Table,
    pub refine_width: Table,
    pub refine_height: Table,
    pub refine_x: Table,
    pub refine_y: Table,
    pub refine_size: Table,
}

impl TextParams<'_> {
    pub fn new<'s>(symbols: &'s [Rc<Bitmap>]) -> TextParams<'s> {
        TextParams {
            huffman: false,
            refine: false,
            width: 0,
            height: 0,
            default_pixel: 0,
            instances: 0,
            log_strip_size: 0,
            reference_corner: 0,
            transposed: false,
            combination_operator: 0,
            ds_offset: 0,
            refinement_template: 0,
            refinement_at: [-1; 4],
            symbols,
            symbol_code_length: 0,
            symbol_ids: None,
            first_s: None,
            delta_s: None,
            delta_t: None,
            refine_width: None,
            refine_height: None,
            refine_x: None,
            refine_y: None,
            refine_size: None,
        }
    }
}

fn no_table() -> FilterError {
    FilterError::damaged("A JBIG2 text region is missing a Huffman table.")
}

/// 6.4: the symbol instances of a text region, arithmetic or Huffman coded.
pub(crate) struct Text<'a, 'b> {
    pub budget: &'b mut Budget,
    pub arith: Option<&'b mut Arith<'a>>,
    pub reader: Option<&'b mut Reader<'a>>,
    pub p: &'b TextParams<'b>,
    /// The refinement contexts the Huffman coded blocks start from, shared by all of a segment's blocks.
    pub refinement_contexts: Option<&'b mut Vec<u8>>,
}

impl<'a> Text<'a, '_> {
    fn reader(&mut self) -> Result<&mut Reader<'a>> {
        self.reader
            .as_deref_mut()
            .ok_or_else(|| FilterError::damaged("A JBIG2 text region has no Huffman data."))
    }

    fn arith(&mut self) -> Result<&mut Arith<'a>> {
        self.arith
            .as_deref_mut()
            .ok_or_else(|| FilterError::damaged("A JBIG2 text region has no arithmetic data."))
    }

    fn value(&mut self, table: &Table, procedure: usize) -> Result<i32> {
        let value = self.delta(table, procedure)?;
        value.ok_or_else(|| {
            FilterError::damaged(
                "A JBIG2 text region has an out-of-band value where there is none.",
            )
        })
    }

    fn delta(&mut self, table: &Table, procedure: usize) -> Result<Option<i32>> {
        if self.p.huffman {
            let table = table.as_deref().ok_or_else(no_table)?;
            table.decode(self.reader()?)
        } else {
            self.arith()?.integer(procedure)
        }
    }

    pub fn decode(&mut self) -> Result<Bitmap> {
        let p = self.p;
        if p.instances > MAX_INSTANCES {
            return Err(FilterError::too_big());
        }
        let mut bitmap = Bitmap::new(p.width, p.height, self.budget)?;
        if p.default_pixel != 0 {
            bitmap.fill(1);
        }
        let strip_size = 1i64 << p.log_strip_size;
        let bottom = p.reference_corner & 1 == 0;
        let right = p.reference_corner & 2 != 0;
        let mut strip_t = -(self.value(&p.delta_t, IADT)? as i64);
        let mut first_s = 0i64;
        let mut placed = 0i64;
        while placed < p.instances {
            strip_t += self.value(&p.delta_t, IADT)? as i64;
            first_s += self.value(&p.first_s, IAFS)? as i64;
            let mut current_s = first_s;
            loop {
                let mut current_t = 0i64;
                if strip_size > 1 {
                    current_t = if p.huffman {
                        self.reader()?.read_bits(p.log_strip_size)? as i64
                    } else {
                        self.value(&None, IAIT)? as i64
                    };
                }
                let t = strip_size * strip_t + current_t;
                let id = self.symbol_id()?;
                let symbol = self.symbol_at(id)?;
                let symbol = if p.refine && self.refined()? {
                    Rc::new(self.refine(&symbol)?)
                } else {
                    symbol
                };
                let (w, h) = (symbol.width as i64, symbol.height as i64);
                // 6.4.5 step vi, then viii: the reference corner is at (S, T), or at (T, S) when transposed.
                if if p.transposed { bottom } else { right } {
                    current_s += (if p.transposed { h } else { w }) - 1;
                }
                let mut left = if p.transposed { t } else { current_s };
                let mut top = if p.transposed { current_s } else { t };
                if right {
                    left -= w - 1;
                }
                if bottom {
                    top -= h - 1;
                }
                self.budget.work(w * h + 256)?;
                if left > i32::MIN as i64
                    && left < i32::MAX as i64
                    && top > i32::MIN as i64
                    && top < i32::MAX as i64
                {
                    bitmap.compose(&symbol, left as i32, top as i32, p.combination_operator);
                }
                // Step x
                if if p.transposed { !bottom } else { !right } {
                    current_s += (if p.transposed { h } else { w }) - 1;
                }
                placed += 1;
                let delta_s = self.delta(&p.delta_s, IADS)?;
                let Some(delta_s) = delta_s else { break };
                if placed >= p.instances {
                    break;
                }
                current_s += delta_s as i64 + p.ds_offset as i64;
            }
        }
        Ok(bitmap)
    }

    fn symbol_id(&mut self) -> Result<i64> {
        let p = self.p;
        if !p.huffman {
            return Ok(self.arith()?.iaid(p.symbol_code_length) as i64);
        }
        if p.symbol_ids.is_none() {
            return Ok(self.reader()?.read_bits(p.symbol_code_length)? as i64);
        }
        Ok(self.value(&p.symbol_ids, 0)? as i64)
    }

    fn symbol_at(&self, id: i64) -> Result<Rc<Bitmap>> {
        usize::try_from(id)
            .ok()
            .and_then(|id| self.p.symbols.get(id))
            .cloned()
            .ok_or_else(|| {
                FilterError::damaged("A JBIG2 text region uses a symbol that is not there.")
            })
    }

    fn refined(&mut self) -> Result<bool> {
        if self.p.huffman {
            Ok(self.reader()?.read_bit()? != 0)
        } else {
            Ok(self.value(&None, IARI)? != 0)
        }
    }

    /// 6.4.11: the symbol refined by a small bitmap coded in the region.
    fn refine(&mut self, symbol: &Bitmap) -> Result<Bitmap> {
        let p = self.p;
        let rdw = self.value(&p.refine_width, IARDW)?;
        let rdh = self.value(&p.refine_height, IARDH)?;
        let rdx = self.value(&p.refine_x, IARDX)?;
        let rdy = self.value(&p.refine_y, IARDY)?;
        let width = symbol.width as i64 + rdw as i64;
        let height = symbol.height as i64 + rdh as i64;
        if width < 0 || height < 0 || width > i32::MAX as i64 || height > i32::MAX as i64 {
            return Err(FilterError::damaged(
                "A JBIG2 refined symbol has a bad size.",
            ));
        }
        self.budget.work(width * height)?;
        self.refine_with(symbol, width, height, (rdw >> 1) + rdx, (rdh >> 1) + rdy)
    }

    /// The refinement coded in the arithmetic data, or in a size and a block of its own when Huffman coded.
    pub fn refine_with(
        &mut self,
        reference: &Bitmap,
        width: i64,
        height: i64,
        dx: i32,
        dy: i32,
    ) -> Result<Bitmap> {
        let p = self.p;
        if !p.huffman {
            let budget = &mut *self.budget;
            let arith = self.arith.as_deref_mut().ok_or_else(|| {
                FilterError::damaged("A JBIG2 text region has no arithmetic data.")
            })?;
            return generic::refine(
                budget,
                arith,
                width,
                height,
                p.refinement_template,
                reference,
                dx,
                dy,
                false,
                &p.refinement_at,
            );
        }
        let size = self.value(&p.refine_size, 0)?;
        let reader = self.reader()?;
        reader.byte_align();
        if size < 0 || size as usize > reader.end.saturating_sub(reader.position) {
            return Err(FilterError::damaged(
                "A JBIG2 refinement is longer than its segment.",
            ));
        }
        let size = size as usize;
        let (data, position) = (reader.data, reader.position);
        let contexts = self.refinement_contexts.as_deref_mut().map(std::mem::take);
        let mut block = Arith::new(data, position, position + size, contexts, None)?;
        let result = generic::refine(
            &mut *self.budget,
            &mut block,
            width,
            height,
            p.refinement_template,
            reference,
            dx,
            dy,
            false,
            &p.refinement_at,
        );
        if let Some(shared) = self.refinement_contexts.as_deref_mut() {
            *shared = block.gr;
        }
        let result = result?;
        self.reader()?.position += size;
        Ok(result)
    }
}
