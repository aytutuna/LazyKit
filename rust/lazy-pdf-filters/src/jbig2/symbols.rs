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

// Ported from pdf.js (src/core/jbig2.js: decodeSymbolDictionary) by way of Lazy PDF's C# decoder, and changed:
// Huffman coding with refinement and aggregation, retained contexts, and limits on the number of symbols.

use std::rc::Rc;
use std::sync::Arc;

use super::arith::{Arith, IAAI, IADH, IADW, IAEX, IARDX, IARDY};
use super::bitmap::Bitmap;
use super::budget::Budget;
use super::generic;
use super::huffman::{Huffman, Reader};
use super::text::{Text, TextParams, code_length};
use crate::error::{FilterError, Result};

/// The most symbols one dictionary and what it refers to may hold.
pub(crate) const MAX_SYMBOLS: usize = 1 << 20;

type Table = Option<Arc<Huffman>>;

/// What the symbol dictionary decoding procedure (6.5, Table 13) is given.
pub(crate) struct SymbolParams {
    pub huffman: bool,
    pub refine: bool,
    pub template: i32,
    pub refinement_template: i32,
    pub at: Vec<i32>,
    pub refinement_at: [i32; 4],
    pub new_count: usize,
    pub input: Vec<Rc<Bitmap>>,
    pub delta_height: Table,
    pub delta_width: Table,
    pub bitmap_size: Table,
    pub aggregate_instances: Table,
}

/// The symbols a dictionary exports, and the contexts it leaves for a later one to continue from (7.4.2.2).
pub(crate) struct SymbolDictionary {
    pub exported: Vec<Rc<Bitmap>>,
    pub gb: Option<Vec<u8>>,
    pub gr: Option<Vec<u8>>,
}

fn arith_of<'x, 'a>(arith: &'x mut Option<&mut Arith<'a>>) -> Result<&'x mut Arith<'a>> {
    arith
        .as_deref_mut()
        .ok_or_else(|| FilterError::damaged("A JBIG2 symbol dictionary has no arithmetic data."))
}

fn reader_of<'x, 'a>(reader: &'x mut Option<&mut Reader<'a>>) -> Result<&'x mut Reader<'a>> {
    reader
        .as_deref_mut()
        .ok_or_else(|| FilterError::damaged("A JBIG2 symbol dictionary has no Huffman data."))
}

fn missing_table() -> FilterError {
    FilterError::damaged("A JBIG2 symbol dictionary is missing a Huffman table.")
}

/// One value, or None for the out-of-band value.
fn delta_or_oob(
    huffman: bool,
    table: &Table,
    arith: &mut Option<&mut Arith>,
    reader: &mut Option<&mut Reader>,
    procedure: usize,
) -> Result<Option<i32>> {
    if huffman {
        table
            .as_deref()
            .ok_or_else(missing_table)?
            .decode(reader_of(reader)?)
    } else {
        arith_of(arith)?.integer(procedure)
    }
}

fn delta(
    huffman: bool,
    table: &Table,
    arith: &mut Option<&mut Arith>,
    reader: &mut Option<&mut Reader>,
    procedure: usize,
) -> Result<i32> {
    delta_or_oob(huffman, table, arith, reader, procedure)?.ok_or_else(|| {
        FilterError::damaged(
            "A JBIG2 symbol dictionary has an out-of-band value where there is none.",
        )
    })
}

/// 6.5: decodes the new symbols of a dictionary and picks the exported ones. `refine_contexts` are the
/// contexts of a Huffman coded dictionary's refinements, which each have an arithmetic decoder of their own.
pub(crate) fn decode<'a>(
    budget: &mut Budget,
    mut arith: Option<&mut Arith<'a>>,
    mut reader: Option<&mut Reader<'a>>,
    p: &SymbolParams,
    refine_contexts: &mut Vec<u8>,
) -> Result<Vec<Rc<Bitmap>>> {
    if p.input.len() + p.new_count > MAX_SYMBOLS {
        return Err(FilterError::too_big());
    }
    let huffman = p.huffman;
    let total = p.input.len() + p.new_count;
    let mut symbol_code_length = code_length(total);
    // 6.5.8.2.3
    if huffman {
        symbol_code_length = symbol_code_length.max(1);
    } else {
        budget.fixed(2i64 << symbol_code_length)?;
    }
    // The input symbols first, then the new ones as they are decoded; aggregate symbols may use both.
    let mut all: Vec<Rc<Bitmap>> = p.input.clone();
    let mut widths: Vec<usize> = Vec::new();
    let mut height = 0i64;
    let mut classes = 0usize;
    while all.len() < total {
        // Every height class holds a symbol; a stream with empty ones is one that is cut off.
        classes += 1;
        if classes > 2 * p.new_count + 16 {
            return Err(FilterError::damaged(
                "A JBIG2 symbol dictionary has empty height classes.",
            ));
        }
        let next_height =
            height + delta(huffman, &p.delta_height, &mut arith, &mut reader, IADH)? as i64;
        if !(0..=i32::MAX as i64).contains(&next_height) {
            return Err(FilterError::damaged("A JBIG2 symbol has a bad height."));
        }
        height = next_height;
        let mut width = 0i64;
        let mut total_width = 0i64;
        widths.clear();
        while let Some(delta_width) =
            delta_or_oob(huffman, &p.delta_width, &mut arith, &mut reader, IADW)?
        {
            if all.len() + widths.len() >= total {
                return Err(FilterError::damaged(
                    "A JBIG2 symbol dictionary has more symbols than it says.",
                ));
            }
            width += delta_width as i64;
            if !(0..=i32::MAX as i64).contains(&width) {
                return Err(FilterError::damaged("A JBIG2 symbol has a bad width."));
            }
            total_width += width;
            if p.refine {
                let bitmap = aggregate(
                    budget,
                    &mut arith,
                    &mut reader,
                    p,
                    &all,
                    width,
                    height,
                    symbol_code_length,
                    refine_contexts,
                )?;
                budget.keep((bitmap.width * bitmap.height) as i64 + 64)?;
                all.push(Rc::new(bitmap));
            } else if huffman {
                widths.push(width as usize);
            } else {
                budget.keep(width * height + 64)?;
                let bitmap = generic::decode(
                    budget,
                    arith_of(&mut arith)?,
                    width,
                    height,
                    p.template,
                    false,
                    None,
                    &p.at,
                )?;
                all.push(Rc::new(bitmap));
            }
        }
        if huffman && !p.refine {
            height_class(
                budget,
                reader_of(&mut reader)?,
                p,
                &mut all,
                &widths,
                height,
                total_width,
            )?;
        }
    }
    // 6.5.10: runs of symbols that are not exported and are, alternating, over the input symbols and the new ones.
    let mut flags = vec![false; total];
    let (mut index, mut runs) = (0usize, 0usize);
    let mut current = false;
    let export_table = Some(Huffman::standard(1));
    while index < total {
        runs += 1;
        if runs > total + 2 {
            return Err(FilterError::damaged(
                "A JBIG2 symbol dictionary has bad export flags.",
            ));
        }
        let run = delta(huffman, &export_table, &mut arith, &mut reader, IAEX)?;
        if run < 0 || run as usize > total - index {
            return Err(FilterError::damaged(
                "A JBIG2 symbol dictionary has bad export flags.",
            ));
        }
        flags[index..index + run as usize].fill(current);
        index += run as usize;
        current = !current;
    }
    let exported: Vec<Rc<Bitmap>> = all
        .into_iter()
        .zip(flags)
        .filter_map(|(symbol, flag)| flag.then_some(symbol))
        .collect();
    // The list is kept for the segments that refer to it.
    budget.keep(exported.len() as i64 * 8)?;
    Ok(exported)
}

/// 6.5.9: the symbols of one height class are coded side by side in one bitmap, uncompressed or MMR.
fn height_class(
    budget: &mut Budget,
    reader: &mut Reader,
    p: &SymbolParams,
    all: &mut Vec<Rc<Bitmap>>,
    widths: &[usize],
    height: i64,
    total_width: i64,
) -> Result<()> {
    let size = p
        .bitmap_size
        .as_deref()
        .ok_or_else(missing_table)?
        .decode(reader)?
        .ok_or_else(|| {
            FilterError::damaged(
                "A JBIG2 symbol dictionary has an out-of-band value where there is none.",
            )
        })?;
    reader.byte_align();
    if size < 0 || size as usize > reader.end.saturating_sub(reader.position) {
        return Err(FilterError::damaged(
            "A JBIG2 symbol bitmap is longer than its segment.",
        ));
    }
    let size = size as usize;
    if total_width > i32::MAX as i64 {
        return Err(FilterError::too_big());
    }
    budget.keep(total_width * height + 64 * widths.len() as i64)?;
    let collective = if size == 0 {
        let mut collective = Bitmap::new(total_width, height, budget)?;
        let stride = (total_width as usize).div_ceil(8);
        if reader.end.saturating_sub(reader.position) < stride * height as usize {
            return Err(FilterError::damaged("A JBIG2 symbol bitmap is cut off."));
        }
        let columns = total_width as usize;
        for y in 0..height as usize {
            for x in 0..columns {
                collective.pixels[y * columns + x] =
                    (reader.data[reader.position + x / 8] >> (7 - (x & 7))) & 1;
            }
            reader.position += stride;
        }
        collective
    } else {
        let (collective, _) = generic::mmr(
            budget,
            reader.data,
            reader.position,
            reader.position + size,
            total_width,
            height,
        )?;
        reader.position += size;
        collective
    };
    let mut x0 = 0;
    for &width in widths {
        all.push(Rc::new(collective.columns(x0, width, budget)?));
        x0 += width;
    }
    Ok(())
}

/// 6.5.8.2: a symbol made of other symbols or refined from one.
#[allow(clippy::too_many_arguments)]
fn aggregate<'a>(
    budget: &mut Budget,
    arith: &mut Option<&mut Arith<'a>>,
    reader: &mut Option<&mut Reader<'a>>,
    p: &SymbolParams,
    all: &[Rc<Bitmap>],
    width: i64,
    height: i64,
    code_length: u32,
    refine_contexts: &mut Vec<u8>,
) -> Result<Bitmap> {
    let huffman = p.huffman;
    let instances = delta(huffman, &p.aggregate_instances, arith, reader, IAAI)?;
    let mut text = TextParams::new(all);
    text.huffman = huffman;
    text.refine = true;
    text.width = width;
    text.height = height;
    text.instances = instances as i64;
    text.reference_corner = 1;
    text.refinement_template = p.refinement_template;
    text.refinement_at = p.refinement_at;
    text.symbol_code_length = code_length;
    if huffman {
        text.first_s = Some(Huffman::standard(6));
        text.delta_s = Some(Huffman::standard(8));
        text.delta_t = Some(Huffman::standard(11));
        let refinement = Some(Huffman::standard(15));
        text.refine_width = refinement.clone();
        text.refine_height = refinement.clone();
        text.refine_x = refinement.clone();
        text.refine_y = refinement;
        text.refine_size = Some(Huffman::standard(1));
    }
    if instances < 1 {
        return Err(FilterError::damaged(
            "A JBIG2 aggregate symbol has no instances.",
        ));
    }
    if instances > 1 {
        let mut decoder = Text {
            budget,
            arith: arith.as_deref_mut(),
            reader: reader.as_deref_mut(),
            p: &text,
            refinement_contexts: Some(refine_contexts),
        };
        return decoder.decode();
    }
    // 6.5.8.2.2: one symbol, refined.
    let id = if huffman {
        reader_of(reader)?.read_bits(code_length)? as usize
    } else {
        arith_of(arith)?.iaid(code_length)
    };
    let standard = Some(Huffman::standard(15));
    let rdx = delta(huffman, &standard, arith, reader, IARDX)?;
    let rdy = delta(huffman, &standard, arith, reader, IARDY)?;
    let Some(reference) = all.get(id) else {
        return Err(FilterError::damaged(
            "A JBIG2 aggregate symbol uses a symbol that is not there.",
        ));
    };
    let mut decoder = Text {
        budget,
        arith: arith.as_deref_mut(),
        reader: reader.as_deref_mut(),
        p: &text,
        refinement_contexts: Some(refine_contexts),
    };
    decoder.refine_with(reference, width, height, rdx, rdy)
}
