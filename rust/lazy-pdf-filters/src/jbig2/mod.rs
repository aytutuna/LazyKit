// Ported from pdf.js (src/core/jbig2.js: segment headers, SimpleSegmentVisitor) by way of Lazy PDF's C#
// decoder, and changed: the page is composed with every combination operator, a segment that fails does not
// stop the others, and every size and count is checked against the budget before anything is allocated.
// See budget.rs for the licence.

//! JBIG2Decode (ISO 32000-1 7.4.7; ITU-T T.88): the segments of an embedded stream, the globals first,
//! drawn into one page that comes out as packed rows with 0 for black, as an image stream expects.

mod arith;
mod bitmap;
mod budget;
mod generic;
mod halftone;
mod huffman;
mod symbols;
mod text;

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use arith::Arith;
use bitmap::Bitmap;
use budget::{Budget, MAX_PIXELS};
use huffman::{Huffman, Reader};
use symbols::{MAX_SYMBOLS, SymbolDictionary, SymbolParams};
use text::{Text, TextParams};

use crate::error::{FilterError, Problem, Result};

/// The most segments one decode reads, the globals and the image together.
const MAX_SEGMENTS: u32 = 1 << 16;

/// The shortest segment header: number, flags, referred-to flags, page association and length.
const MIN_HEADER: usize = 11;

/// How many segments in a row may fail without reading anything of their own before the rest of the
/// data is given up on.
const MAX_FAILED_RUN: u32 = 16;

/// Rows of packed pixels, each starting on a byte boundary, black 0 and white 1 (the filter inverts
/// JBIG2's black 1). A segment that does not decode is left out; the stream is damaged only when none of
/// its regions could be drawn, or at all when `whole` says a partial page is no use. With `width` and
/// `height` (the size the image dictionary gives) the page comes out cut or padded with white to that
/// size, so that the samples have the stride the image reads them with; without them, for a stream that
/// is not an image, the page keeps its own size.
pub fn decode(
    globals: &[u8],
    data: &[u8],
    max_size: u64,
    width: i32,
    height: i32,
    whole: bool,
) -> Result<Vec<u8>> {
    // TODO: a budget for all the JBIG2 decodes of a document, so that many images cannot each use a full one.
    let mut decoder = Decoder::new(max_size, width, height, whole);
    decoder.read(globals, true)?;
    decoder.read(data, false)?;
    decoder.finish()
}

struct Segment {
    number: u32,
    kind: u32,
    referred: Vec<u32>,
    length: u64,
    header_end: usize,
    unknown_length: bool,
}

struct Region {
    width: i64,
    height: i64,
    x: i32,
    y: i32,
    op: i32,
}

struct Decoder {
    max_size: u64,
    fit_width: i32,
    fit_height: i32,
    whole: bool,
    budget: Budget,
    segments: u32,
    failed: u32,
    drawn: u32,
    first_error: Option<FilterError>,
    // The page (7.4.8). With an unknown height the bitmap grows with the regions and stripes, and `rows` is what is used.
    page: Option<Bitmap>,
    height_known: bool,
    rows: usize,
    default_pixel: u8,
    in_globals: bool,
    // The end of the segment being read: no field of it is read past that.
    limit: usize,
    intermediate: HashMap<u32, Rc<Bitmap>>,
    dictionaries: HashMap<u32, SymbolDictionary>,
    tables: HashMap<u32, Arc<Huffman>>,
    patterns: HashMap<u32, Rc<Vec<Rc<Bitmap>>>>,
}

/// The tables segments a segment refers to, in order, for the selections that name a custom table.
struct CustomTables(std::vec::IntoIter<Arc<Huffman>>);

impl CustomTables {
    fn next(&mut self) -> Result<Arc<Huffman>> {
        self.0.next().ok_or_else(|| {
            FilterError::damaged("A JBIG2 segment names a custom table it does not refer to.")
        })
    }
}

// Selector 3 is a custom table; the standard ones are numbered from `first`.
fn select(
    selector: u32,
    first: usize,
    standard_count: u32,
    custom: &mut CustomTables,
) -> Result<Arc<Huffman>> {
    if selector < standard_count {
        return Ok(Huffman::standard(first + selector as usize));
    }
    if selector == 3 {
        return custom.next();
    }
    Err(FilterError::damaged(
        "A JBIG2 segment has a bad Huffman table selection.",
    ))
}

impl Decoder {
    fn new(max_size: u64, fit_width: i32, fit_height: i32, whole: bool) -> Self {
        Self {
            max_size,
            fit_width,
            fit_height,
            whole,
            budget: Budget::new(),
            segments: 0,
            failed: 0,
            drawn: 0,
            first_error: None,
            page: None,
            height_known: false,
            rows: 0,
            default_pixel: 0,
            in_globals: false,
            limit: 0,
            intermediate: HashMap::new(),
            dictionaries: HashMap::new(),
            tables: HashMap::new(),
            patterns: HashMap::new(),
        }
    }

    /// What the image's own segments may make is tied to the size of the picture (the globals, which
    /// many pictures share, are not), which have a budget of their own and may hold dictionaries and
    /// tables only.
    fn start_image(&mut self) {
        self.in_globals = false;
        let fit = if self.fit_width > 0 && self.fit_height > 0 {
            self.fit_width as i64 * self.fit_height as i64
        } else {
            0
        };
        self.budget.for_image(fit);
    }

    /// The segments of `data` one after the other (7.2), through the end of file segment.
    fn read(&mut self, data: &[u8], globals: bool) -> Result<()> {
        self.in_globals = globals;
        if globals {
            self.budget.for_globals();
        } else {
            self.start_image();
        }
        let mut position = 0usize;
        let mut failed_run = 0;
        // Zeros at the end are padding, a line break or a stray byte after the last segment is the end of
        // the data, not a segment that failed.
        let Some(last) = data.iter().rposition(|&b| b != 0) else {
            return Ok(());
        };
        while position <= last {
            if data.len() - position < MIN_HEADER {
                return Ok(());
            }
            self.limit = data.len();
            let segment = match self.read_header(data, position) {
                Ok(segment) => segment,
                Err(e) if e.problem == Problem::Damaged => {
                    self.fail(e);
                    return Ok(());
                }
                Err(e) => return Err(e),
            };
            self.segments += 1;
            if self.segments > MAX_SEGMENTS {
                return Err(FilterError::too_big());
            }
            let start = segment.header_end;
            let end = (start as u64 + segment.length).min(data.len() as u64) as usize;
            // A segment that runs past the end of the data is read as far as it goes, unless a partial page is no use.
            if self.whole
                && !segment.unknown_length
                && start as u64 + segment.length > data.len() as u64
            {
                self.fail(FilterError::damaged("A JBIG2 segment is cut off."));
                return Ok(());
            }
            // The handlers read nothing of the next segment: the fields they take are bounded by the end of this one.
            self.limit = end;
            match self.process(&segment, data, start, end) {
                Ok(()) => failed_run = 0,
                Err(e) if e.problem == Problem::Damaged => {
                    self.fail(e);
                    // A segment that carries data may fail for what an earlier one left out; only empty ones are a run.
                    if segment.length <= (segment.header_end - position) as u64 {
                        failed_run += 1;
                        if failed_run > MAX_FAILED_RUN {
                            self.fail(FilterError::damaged(
                                "A JBIG2 stream has nothing but segments that do not decode.",
                            ));
                            return Ok(());
                        }
                    }
                }
                Err(e) => return Err(e),
            }
            // Nothing after the end of the page or of the file is read.
            if segment.kind == 49 || segment.kind == 51 {
                return Ok(());
            }
            position = end;
        }
        Ok(())
    }

    fn fail(&mut self, e: FilterError) {
        self.failed += 1;
        self.first_error.get_or_insert(e);
    }

    /// 7.2: the segment header at `start`.
    fn read_header(&self, data: &[u8], start: usize) -> Result<Segment> {
        let mut position = start;
        let number = self.u32(data, position)?;
        let flags = self.u8(data, position + 4)?;
        let kind = flags & 0x3f;
        let wide_association = flags & 0x40 != 0;
        let referred_flags = self.u8(data, position + 5)?;
        let mut count = (referred_flags >> 5) as u64;
        position += 6;
        if count == 7 {
            count = (self.u32(data, position - 1)? & 0x1fff_ffff) as u64;
            // The retention bits take one bit for the segment itself and one for each one it refers to.
            let retain_bytes = (count + 8) / 8;
            position += 3 + retain_bytes as usize;
        } else if count > 4 {
            return Err(FilterError::damaged(
                "A JBIG2 segment header has bad referred-to flags.",
            ));
        }
        let number_size = if number <= 256 {
            1
        } else if number <= 65536 {
            2
        } else {
            4
        };
        // Each referred segment needs bytes in the header, so the count is bounded by the data.
        if count * number_size > data.len().saturating_sub(position) as u64 {
            return Err(FilterError::damaged("A JBIG2 segment header is cut off."));
        }
        let mut referred = Vec::with_capacity(count as usize);
        for _ in 0..count {
            referred.push(match number_size {
                1 => self.u8(data, position)?,
                2 => self.u16(data, position)?,
                _ => self.u32(data, position)?,
            });
            position += number_size as usize;
        }
        position += if wide_association { 4 } else { 1 };
        let mut length = self.u32(data, position)? as u64;
        position += 4;
        let unknown = length == 0xffff_ffff;
        if unknown {
            // 7.2.7: only an immediate generic region may have an unknown length; it ends with a marker and the row count.
            if kind != 38 {
                return Err(FilterError::damaged("A JBIG2 segment has no length."));
            }
            length = self.unknown_length(data, position)?;
        }
        Ok(Segment {
            number,
            kind,
            referred,
            length,
            header_end: position,
            unknown_length: unknown,
        })
    }

    fn unknown_length(&self, data: &[u8], start: usize) -> Result<u64> {
        // The flags byte of the generic region follows its 17 byte region information.
        let mmr = self.u8(data, start + 17)? & 1 != 0;
        let (first, second) = if mmr { (0x00, 0x00) } else { (0xff, 0xac) };
        let mut i = start + 18;
        while i + 6 <= data.len() {
            if data[i] == first && data[i + 1] == second {
                return Ok((i + 6 - start) as u64);
            }
            i += 1;
        }
        Err(FilterError::damaged(
            "A JBIG2 segment of unknown length does not end.",
        ))
    }

    fn process(&mut self, segment: &Segment, data: &[u8], start: usize, end: usize) -> Result<()> {
        // The globals of an image hold dictionaries and tables; whatever else is there is not drawn.
        if self.in_globals && !matches!(segment.kind, 0 | 16 | 53) {
            return Ok(());
        }
        match segment.kind {
            0 => self.symbol_dictionary(segment, data, start, end),
            4 | 6 | 7 => self.text_region(segment, data, start, end),
            16 => self.pattern_dictionary(segment, data, start, end),
            20 | 22 | 23 => self.halftone_region(segment, data, start, end),
            36 | 38 | 39 => self.generic_region(segment, data, start, end),
            40 | 42 | 43 => self.refinement_region(segment, data, start, end),
            53 => {
                let table = Huffman::custom(data, start, end)?;
                self.tables.insert(segment.number, Arc::new(table));
                Ok(())
            }
            48 => self.page_information(data, start, end),
            50 => self.end_of_stripe(data, start, end),
            // Profiles, extensions, end of page and file, and types this reader does not know carry nothing to draw.
            _ => Ok(()),
        }
    }

    /// The symbols the referred dictionaries export, one after the other.
    fn input_symbols(&mut self, segment: &Segment) -> Result<Vec<Rc<Bitmap>>> {
        let mut symbols = Vec::new();
        for number in &segment.referred {
            let Some(dictionary) = self.dictionaries.get(number) else {
                continue;
            };
            symbols.extend(dictionary.exported.iter().cloned());
            if symbols.len() > MAX_SYMBOLS {
                return Err(FilterError::too_big());
            }
            self.budget.fixed(dictionary.exported.len() as i64 * 8)?;
        }
        Ok(symbols)
    }

    fn tables_of(&self, segment: &Segment) -> CustomTables {
        let tables: Vec<Arc<Huffman>> = segment
            .referred
            .iter()
            .filter_map(|number| self.tables.get(number).cloned())
            .collect();
        CustomTables(tables.into_iter())
    }

    fn symbol_dictionary(
        &mut self,
        segment: &Segment,
        data: &[u8],
        start: usize,
        end: usize,
    ) -> Result<()> {
        let mut position = start;
        let flags = self.u16(data, position)?;
        position += 2;
        let huffman = flags & 1 != 0;
        let refine = flags & 2 != 0;
        let template = ((flags >> 10) & 3) as i32;
        let refinement_template = ((flags >> 12) & 1) as i32;
        let at = if huffman {
            Vec::new()
        } else {
            self.read_at(data, &mut position, end, if template == 0 { 4 } else { 1 })?
        };
        let refinement_at = if refine && refinement_template == 0 {
            let at = self.read_at(data, &mut position, end, 2)?;
            [at[0], at[1], at[2], at[3]]
        } else {
            [-1; 4]
        };
        let exported = self.u32(data, position)? as usize;
        let added = self.u32(data, position + 4)? as usize;
        position += 8;
        if exported > MAX_SYMBOLS || added > MAX_SYMBOLS {
            return Err(FilterError::too_big());
        }
        let input = self.input_symbols(segment)?;
        let mut p = SymbolParams {
            huffman,
            refine,
            template,
            refinement_template,
            at,
            refinement_at,
            new_count: added,
            input,
            delta_height: None,
            delta_width: None,
            bitmap_size: None,
            aggregate_instances: None,
        };
        if huffman {
            let mut custom = self.tables_of(segment);
            p.delta_height = Some(select((flags >> 2) & 3, 4, 2, &mut custom)?);
            p.delta_width = Some(select((flags >> 4) & 3, 2, 2, &mut custom)?);
            p.bitmap_size = Some(if flags & 0x40 != 0 {
                custom.next()?
            } else {
                Huffman::standard(1)
            });
            p.aggregate_instances = Some(if flags & 0x80 != 0 {
                custom.next()?
            } else {
                Huffman::standard(1)
            });
        }

        // 7.4.2.2: the contexts a dictionary left, if it kept them, are where this one starts.
        let (mut gb, mut gr) = (None, None);
        if flags & 0x100 != 0 {
            let last = segment
                .referred
                .iter()
                .rev()
                .find_map(|number| self.dictionaries.get(number));
            let Some(last) = last else {
                return Err(FilterError::damaged(
                    "A JBIG2 symbol dictionary reuses contexts of a dictionary it does not refer to.",
                ));
            };
            gb = last.gb.clone();
            gr = last.gr.clone();
        }
        let mut refine_contexts = vec![0u8; 1 << 13];
        let mut arith = None;
        let mut reader = None;
        if huffman {
            reader = Some(Reader::new(data, position, end));
            if let Some(gr) = &gr {
                let n = gr.len().min(refine_contexts.len());
                refine_contexts[..n].copy_from_slice(&gr[..n]);
            }
        } else {
            let mut a = Arith::new(data, position, end, None, Some(&mut self.budget))?;
            if let Some(gb) = gb {
                a.gb = gb;
            }
            if let Some(gr) = gr {
                a.gr = gr;
            }
            arith = Some(a);
        }
        let symbols = symbols::decode(
            &mut self.budget,
            arith.as_mut(),
            reader.as_mut(),
            &p,
            &mut refine_contexts,
        )?;
        // Only a dictionary that says so leaves its contexts behind.
        let retained = flags & 0x200 != 0;
        if retained {
            self.budget.keep(1 << 17)?;
        }
        let (gb, gr) = match (retained, arith) {
            (true, Some(a)) => (Some(a.gb), Some(a.gr)),
            (true, None) => (None, Some(refine_contexts)),
            (false, _) => (None, None),
        };
        self.dictionaries.insert(
            segment.number,
            SymbolDictionary {
                exported: symbols,
                gb,
                gr,
            },
        );
        Ok(())
    }

    fn text_region(
        &mut self,
        segment: &Segment,
        data: &[u8],
        start: usize,
        end: usize,
    ) -> Result<()> {
        let region = self.read_region(data, start, end, false)?;
        let mut position = start + 17;
        let flags = self.u16(data, position)?;
        position += 2;
        let huffman = flags & 1 != 0;
        let refine = flags & 2 != 0;
        let refinement_template = ((flags >> 15) & 1) as i32;
        let mut huffman_flags = 0;
        if huffman {
            huffman_flags = self.u16(data, position)?;
            position += 2;
        }
        let mut refinement_at = [-1; 4];
        if refine && refinement_template == 0 {
            let at = self.read_at(data, &mut position, end, 2)?;
            refinement_at = [at[0], at[1], at[2], at[3]];
        }
        let instances = self.u32(data, position)? as i64;
        position += 4;
        if instances > text::MAX_INSTANCES {
            return Err(FilterError::too_big());
        }
        let symbols = self.input_symbols(segment)?;
        let mut p = TextParams::new(&symbols);
        p.huffman = huffman;
        p.refine = refine;
        p.log_strip_size = (flags >> 2) & 3;
        p.reference_corner = ((flags >> 4) & 3) as i32;
        p.transposed = flags & 0x40 != 0;
        p.combination_operator = ((flags >> 7) & 3) as i32;
        p.default_pixel = ((flags >> 9) & 1) as u8;
        // A signed five bit number.
        p.ds_offset = ((flags >> 10) & 0x1f) as i32 - if flags & 0x4000 != 0 { 32 } else { 0 };
        p.refinement_template = refinement_template;
        p.refinement_at = refinement_at;
        p.width = region.width;
        p.height = region.height;
        p.instances = instances;
        p.symbol_code_length = text::code_length(symbols.len());

        let mut arith = None;
        let mut reader = None;
        let mut contexts = vec![0u8; 1 << 13];
        if p.huffman {
            let mut custom = self.tables_of(segment);
            let mut r = Reader::new(data, position, end);
            p.symbol_ids = Some(Arc::new(symbol_id_table(&mut r, symbols.len())?));
            p.first_s = Some(select(huffman_flags & 3, 6, 2, &mut custom)?);
            p.delta_s = Some(select((huffman_flags >> 2) & 3, 8, 3, &mut custom)?);
            p.delta_t = Some(select((huffman_flags >> 4) & 3, 11, 3, &mut custom)?);
            // The selections for the refinement values mean nothing in a region that does not refine.
            if p.refine {
                p.refine_width = Some(select((huffman_flags >> 6) & 3, 14, 2, &mut custom)?);
                p.refine_height = Some(select((huffman_flags >> 8) & 3, 14, 2, &mut custom)?);
                p.refine_x = Some(select((huffman_flags >> 10) & 3, 14, 2, &mut custom)?);
                p.refine_y = Some(select((huffman_flags >> 12) & 3, 14, 2, &mut custom)?);
                p.refine_size = Some(if huffman_flags & 0x4000 != 0 {
                    custom.next()?
                } else {
                    Huffman::standard(1)
                });
            }
            reader = Some(r);
        } else {
            arith = Some(Arith::new(
                data,
                position,
                end,
                None,
                Some(&mut self.budget),
            )?);
            self.budget.fixed(2i64 << p.symbol_code_length)?;
        }
        let bitmap = Text {
            budget: &mut self.budget,
            arith: arith.as_mut(),
            reader: reader.as_mut(),
            p: &p,
            refinement_contexts: p.huffman.then_some(&mut contexts),
        }
        .decode()?;
        self.store(segment, &region, bitmap)
    }

    fn refinement_region(
        &mut self,
        segment: &Segment,
        data: &[u8],
        start: usize,
        end: usize,
    ) -> Result<()> {
        let region = self.read_region(data, start, end, false)?;
        let mut position = start + 17;
        let flags = self.u8(data, position)?;
        position += 1;
        let template = (flags & 1) as i32;
        let at = if template == 0 {
            self.read_at(data, &mut position, end, 2)?
        } else {
            vec![-1; 4]
        };
        let referred = segment
            .referred
            .iter()
            .rev()
            .find_map(|number| self.intermediate.get(number))
            .cloned();
        let reference = match referred {
            None => {
                // Nothing referred to: the page under the region is what gets refined.
                let Some(page) = &self.page else {
                    return Err(FilterError::damaged(
                        "A JBIG2 refinement region comes before the page information.",
                    ));
                };
                let mut reference = Bitmap::new(region.width, region.height, &mut self.budget)?;
                reference.compose(page, -region.x, -region.y, 4);
                Rc::new(reference)
            }
            Some(reference)
                if reference.width as i64 != region.width
                    || reference.height as i64 != region.height =>
            {
                return Err(FilterError::damaged(
                    "A JBIG2 refinement region is not the size of the region it refines.",
                ));
            }
            Some(reference) => reference,
        };
        let mut arith = Arith::new(data, position, end, None, Some(&mut self.budget))?;
        let bitmap = generic::refine(
            &mut self.budget,
            &mut arith,
            region.width,
            region.height,
            template,
            &reference,
            0,
            0,
            flags & 2 != 0,
            &at,
        )?;
        self.store(segment, &region, bitmap)
    }

    fn pattern_dictionary(
        &mut self,
        segment: &Segment,
        data: &[u8],
        start: usize,
        end: usize,
    ) -> Result<()> {
        let flags = self.u8(data, start)?;
        let width = self.u8(data, start + 1)? as i64;
        let height = self.u8(data, start + 2)? as i64;
        let max_index = self.u32(data, start + 3)? as i64;
        let patterns = halftone::patterns(
            &mut self.budget,
            data,
            start + 7,
            end,
            flags & 1 != 0,
            ((flags >> 1) & 3) as i32,
            width,
            height,
            max_index,
        )?;
        self.patterns.insert(segment.number, Rc::new(patterns));
        Ok(())
    }

    fn halftone_region(
        &mut self,
        segment: &Segment,
        data: &[u8],
        start: usize,
        end: usize,
    ) -> Result<()> {
        let region = self.read_region(data, start, end, false)?;
        let mut position = start + 17;
        let flags = self.u8(data, position)?;
        position += 1;
        let p = halftone::Params {
            mmr: flags & 1 != 0,
            template: ((flags >> 1) & 3) as i32,
            enable_skip: flags & 8 != 0,
            combination_operator: ((flags >> 4) & 7) as i32,
            default_pixel: (flags >> 7) as u8,
            width: region.width,
            height: region.height,
            grid_width: self.u32(data, position)? as i64,
            grid_height: self.u32(data, position + 4)? as i64,
            grid_x: self.u32(data, position + 8)? as i32 as i64,
            grid_y: self.u32(data, position + 12)? as i32 as i64,
            vector_x: self.u16(data, position + 16)? as i64,
            vector_y: self.u16(data, position + 18)? as i64,
        };
        position += 20;
        let patterns = segment
            .referred
            .iter()
            .rev()
            .find_map(|number| self.patterns.get(number))
            .cloned();
        let Some(patterns) = patterns else {
            return Err(FilterError::damaged(
                "A JBIG2 halftone region has no pattern dictionary.",
            ));
        };
        let bitmap = halftone::region(&mut self.budget, data, position, end, &p, &patterns)?;
        self.store(segment, &region, bitmap)
    }

    fn read_region(
        &self,
        data: &[u8],
        start: usize,
        end: usize,
        height_follows: bool,
    ) -> Result<Region> {
        if end.saturating_sub(start) < 17 {
            return Err(FilterError::damaged("A JBIG2 region segment is cut off."));
        }
        let width = self.u32(data, start)? as i64;
        let mut height = self.u32(data, start + 4)? as i64;
        let (x, y) = (self.u32(data, start + 8)?, self.u32(data, start + 12)?);
        // A region of unknown length may give an unknown height too; the row count it ends with says it.
        if height_follows {
            height = height.min(i32::MAX as i64);
        }
        if width > i32::MAX as i64 || height > i32::MAX as i64 {
            return Err(FilterError::too_big());
        }
        Ok(Region {
            width,
            height,
            x: x.min(i32::MAX as u32) as i32,
            y: y.min(i32::MAX as u32) as i32,
            op: (data[start + 16] & 7) as i32,
        })
    }

    fn generic_region(
        &mut self,
        segment: &Segment,
        data: &[u8],
        start: usize,
        mut end: usize,
    ) -> Result<()> {
        let mut region = self.read_region(data, start, end, segment.unknown_length)?;
        if segment.unknown_length {
            if end - start < 22 {
                return Err(FilterError::damaged("A JBIG2 region segment is cut off."));
            }
            // The row count the segment ends with is the height that was coded.
            region.height = region.height.min(self.u32(data, end - 4)? as i64);
            end -= 4;
        }
        let mut position = start + 17;
        let flags = self.u8(data, position)?;
        position += 1;
        let mmr = flags & 1 != 0;
        let template = ((flags >> 1) & 3) as i32;
        let typical_prediction = flags & 8 != 0;
        let bitmap = if mmr {
            generic::mmr(
                &mut self.budget,
                data,
                position,
                end,
                region.width,
                region.height,
            )?
            .0
        } else {
            let at = self.read_at(data, &mut position, end, if template == 0 { 4 } else { 1 })?;
            let mut arith = Arith::new(data, position, end, None, Some(&mut self.budget))?;
            generic::decode(
                &mut self.budget,
                &mut arith,
                region.width,
                region.height,
                template,
                typical_prediction,
                None,
                &at,
            )?
        };
        self.store(segment, &region, bitmap)
    }

    /// An intermediate region is kept for a refinement; an immediate one goes onto the page.
    fn store(&mut self, segment: &Segment, region: &Region, bitmap: Bitmap) -> Result<()> {
        if matches!(segment.kind, 4 | 20 | 36 | 40) {
            self.intermediate.insert(segment.number, Rc::new(bitmap));
            return Ok(());
        }
        self.draw(region, &bitmap)
    }

    fn read_at(
        &self,
        data: &[u8],
        position: &mut usize,
        end: usize,
        pairs: usize,
    ) -> Result<Vec<i32>> {
        if end.saturating_sub(*position) < pairs * 2 || end > data.len() {
            return Err(FilterError::damaged("A JBIG2 segment is cut off."));
        }
        let at = data[*position..*position + pairs * 2]
            .iter()
            .map(|&b| b as i8 as i32)
            .collect();
        *position += pairs * 2;
        Ok(at)
    }

    fn page_information(&mut self, data: &[u8], start: usize, end: usize) -> Result<()> {
        // A PDF image holds one page; later page information is not for this image.
        if self.page.is_some() {
            return Ok(());
        }
        if end.saturating_sub(start) < 19 {
            return Err(FilterError::damaged(
                "A JBIG2 page information segment is cut off.",
            ));
        }
        let width = self.u32(data, start)? as i64;
        let height = self.u32(data, start + 4)? as i64;
        let flags = data[start + 16];
        if width == 0 || width > i32::MAX as i64 {
            return Err(FilterError::damaged("A JBIG2 page has no width."));
        }
        self.height_known = height != 0xffff_ffff;
        if self.height_known && height > i32::MAX as i64 {
            return Err(FilterError::too_big());
        }
        // The page is about to be allocated: a page whose rows alone pass the limit is refused first.
        if self.height_known && height as u64 * (width as u64).div_ceil(8) > self.max_size {
            return Err(FilterError::too_big());
        }
        self.default_pixel = (flags >> 2) & 1;
        let rows = if self.height_known { height } else { 0 };
        let mut page = Bitmap::new(width, rows, &mut self.budget)?;
        self.rows = rows as usize;
        if self.default_pixel != 0 {
            page.fill(1);
        }
        self.page = Some(page);
        Ok(())
    }

    fn end_of_stripe(&mut self, data: &[u8], start: usize, end: usize) -> Result<()> {
        if end.saturating_sub(start) < 4 {
            return Err(FilterError::damaged(
                "A JBIG2 end of stripe segment is cut off.",
            ));
        }
        let row = self.u32(data, start)? as i64;
        if self.page.is_none() || self.height_known {
            return Ok(());
        }
        if row >= i32::MAX as i64 {
            return Err(FilterError::too_big());
        }
        self.reach(row as usize + 1)
    }

    /// Makes room for rows down to `rows` in a page of unknown height and counts them as used.
    fn reach(&mut self, rows: usize) -> Result<()> {
        let Some(page) = &self.page else {
            return Ok(());
        };
        if rows > page.height {
            // Doubling keeps the total of what is allocated, which the budget counts, within twice the page.
            let mut capacity = rows.max(page.height * 2).min(i32::MAX as usize);
            // Doubling must not take a page that fits past the limit.
            if page.width > 0 && capacity > MAX_PIXELS as usize / page.width {
                capacity = rows.max(MAX_PIXELS as usize / page.width);
            }
            let mut grown = Bitmap::new(page.width as i64, capacity as i64, &mut self.budget)?;
            if self.default_pixel != 0 {
                grown.fill(1);
            }
            grown.pixels[..page.pixels.len()].copy_from_slice(&page.pixels);
            self.page = Some(grown);
        }
        self.rows = self.rows.max(rows);
        Ok(())
    }

    fn draw(&mut self, region: &Region, bitmap: &Bitmap) -> Result<()> {
        if self.page.is_none() {
            return Err(FilterError::damaged(
                "A JBIG2 region comes before the page information.",
            ));
        }
        if !self.height_known && (region.y as i64) < i32::MAX as i64 - region.height {
            self.reach(region.y as usize + region.height as usize)?;
        }
        if let Some(page) = &mut self.page {
            page.compose(bitmap, region.x, region.y, region.op);
        }
        self.drawn += 1;
        Ok(())
    }

    /// The page as packed rows, black 0.
    fn finish(self) -> Result<Vec<u8>> {
        let Some(page) = &self.page else {
            return Err(self
                .first_error
                .unwrap_or_else(|| FilterError::damaged("A JBIG2 stream has no page.")));
        };
        // A page that is not whole is wrong in the parts that failed; only some callers can use it.
        if self.failed > 0
            && (self.drawn == 0 || self.whole)
            && let Some(e) = self.first_error
        {
            return Err(e);
        }
        let (width, rows) = (page.width, self.rows);
        let fit = self.fit_width > 0 && self.fit_height > 0;
        let out_width = if fit { self.fit_width as usize } else { width };
        let out_rows = if fit { self.fit_height as usize } else { rows };
        let stride = out_width.div_ceil(8);
        if out_rows as u64 * stride as u64 > self.max_size {
            return Err(FilterError::too_big());
        }
        let mut output = vec![0xffu8; out_rows * stride];
        for y in 0..rows.min(out_rows) {
            for x in 0..width.min(out_width) {
                if page.pixels[y * width + x] != 0 {
                    output[y * stride + (x >> 3)] &= !(0x80u8 >> (x & 7));
                }
            }
        }
        Ok(output)
    }

    fn u8(&self, data: &[u8], at: usize) -> Result<u32> {
        if at < data.len() && at < self.limit {
            Ok(data[at] as u32)
        } else {
            Err(FilterError::damaged("A JBIG2 segment is cut off."))
        }
    }

    fn u16(&self, data: &[u8], at: usize) -> Result<u32> {
        Ok(self.u8(data, at)? << 8 | self.u8(data, at + 1)?)
    }

    fn u32(&self, data: &[u8], at: usize) -> Result<u32> {
        Ok(self.u16(data, at)? << 16 | self.u16(data, at + 2)?)
    }
}

/// 7.4.3.1.7: the code lengths of the symbols, themselves coded with a table of 35 run codes.
fn symbol_id_table(reader: &mut Reader, count: usize) -> Result<Huffman> {
    let mut run_lengths = [0usize; 35];
    for length in &mut run_lengths {
        *length = reader.read_bits(4)? as usize;
    }
    let run_codes = Huffman::for_lengths(&run_lengths)?;
    let mut lengths = vec![0usize; count];
    let mut i = 0;
    while i < count {
        let code = run_codes.decode(reader)?.ok_or_else(|| {
            FilterError::damaged("A JBIG2 symbol code table has an out-of-band value.")
        })?;
        let (repeat, length) = match code {
            0..=31 => {
                lengths[i] = code as usize;
                i += 1;
                continue;
            }
            32 => {
                if i == 0 {
                    return Err(FilterError::damaged(
                        "A JBIG2 symbol code table repeats nothing.",
                    ));
                }
                (reader.read_bits(2)? as usize + 3, lengths[i - 1])
            }
            33 => (reader.read_bits(3)? as usize + 3, 0),
            _ => (reader.read_bits(7)? as usize + 11, 0),
        };
        if repeat > count - i {
            return Err(FilterError::damaged(
                "A JBIG2 symbol code table is too long.",
            ));
        }
        lengths[i..i + repeat].fill(length);
        i += repeat;
    }
    reader.byte_align();
    Huffman::for_lengths(&lengths)
}
