//! The Unicode Bidirectional Algorithm (UAX #9) for one paragraph of logical text, from the paragraph level (P2, P3, or
//! given) through the explicit levels (X1 to X10), weak types (W1 to W7), paired brackets (N0), neutrals (N1, N2) and
//! implicit levels (I1, I2). Lines are cut afterwards: `line_levels` applies L1 and `visual_order` L2. A paragraph
//! separator may only be the last character. Units are UTF-16 units, as in the reference engines.
//!
//! The tables are those of the Unicode Character Database 17.0.0.

// The loops follow the reference line for line, several indexes at once.
#![allow(clippy::needless_range_loop)]

use crate::bidi_tables::{BRACKETS, MIRRORS, RANGES};

/// Bidi_Class values of UAX #9, in the order the table generator numbers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BidiType {
    L,
    R,
    Al,
    En,
    Es,
    Et,
    An,
    Cs,
    Nsm,
    Bn,
    B,
    S,
    Ws,
    On,
    Lre,
    Lro,
    Rle,
    Rlo,
    Pdf,
    Lri,
    Rli,
    Fsi,
    Pdi,
}

const TYPES: [BidiType; 23] = [
    BidiType::L,
    BidiType::R,
    BidiType::Al,
    BidiType::En,
    BidiType::Es,
    BidiType::Et,
    BidiType::An,
    BidiType::Cs,
    BidiType::Nsm,
    BidiType::Bn,
    BidiType::B,
    BidiType::S,
    BidiType::Ws,
    BidiType::On,
    BidiType::Lre,
    BidiType::Lro,
    BidiType::Rle,
    BidiType::Rlo,
    BidiType::Pdf,
    BidiType::Lri,
    BidiType::Rli,
    BidiType::Fsi,
    BidiType::Pdi,
];

/// The Bidi_Class of a code point.
pub fn type_of(cp: i32) -> BidiType {
    if !(0..=0x10FFFF).contains(&cp) {
        return BidiType::L;
    }
    let (mut lo, mut hi) = (0, RANGES.len() - 1);
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        if (RANGES[mid] >> 5) <= cp {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    TYPES
        .get((RANGES[lo] & 31) as usize)
        .copied()
        .unwrap_or(BidiType::L)
}

fn find(table: &[i32], stride: usize, key: i32) -> Option<usize> {
    let (mut lo, mut hi) = (0isize, (table.len() / stride) as isize - 1);
    while lo <= hi {
        let mid = (lo + hi) >> 1;
        let v = table[mid as usize * stride];
        if v == key {
            return Some(mid as usize * stride);
        }
        if v < key {
            lo = mid + 1;
        } else {
            hi = mid - 1;
        }
    }
    None
}

/// The paired bracket of `cp`, with whether `cp` is the opening one; 0 for any other character.
pub fn paired_bracket(cp: i32) -> (i32, bool) {
    match find(&BRACKETS, 3, cp) {
        Some(i) => (BRACKETS[i + 1], BRACKETS[i + 2] == 1),
        None => (0, false),
    }
}

/// The character whose glyph is the mirror image of `cp`, or `cp` when there is none.
pub fn mirror(cp: i32) -> i32 {
    match find(&MIRRORS, 2, cp) {
        Some(i) => MIRRORS[i + 1],
        None => cp,
    }
}

const MAX_DEPTH: usize = 125;
const MAX_BRACKET_DEPTH: usize = 63;

fn is_initiator(t: BidiType) -> bool {
    matches!(t, BidiType::Lri | BidiType::Rli | BidiType::Fsi)
}

fn is_neutral(t: BidiType) -> bool {
    matches!(
        t,
        BidiType::B
            | BidiType::S
            | BidiType::Ws
            | BidiType::On
            | BidiType::Fsi
            | BidiType::Lri
            | BidiType::Rli
            | BidiType::Pdi
    )
}

// L for L, R for R and for numbers.
fn direction(t: BidiType) -> BidiType {
    if t == BidiType::L {
        BidiType::L
    } else {
        BidiType::R
    }
}

fn strong_or_neutral(t: BidiType) -> BidiType {
    match t {
        BidiType::L => BidiType::L,
        BidiType::R | BidiType::En | BidiType::An => BidiType::R,
        _ => BidiType::On,
    }
}

// U+2329 and U+232A are canonically equivalent to U+3008 and U+3009 (BD16).
fn canonical(cp: i32) -> i32 {
    match cp {
        0x2329 => 0x3008,
        0x232A => 0x3009,
        _ => cp,
    }
}

fn next_level(current: i32, rtl: bool) -> i32 {
    if rtl {
        (current + 1) | 1
    } else {
        (current + 2) & !1
    }
}

pub struct BidiParagraph {
    cps: Vec<i32>,
    unit_of: Vec<usize>,
    cp_of_unit: Vec<usize>,
    orig: Vec<BidiType>,
    matching_pdi: Vec<isize>,
    matching_initiator: Vec<isize>,
    embedding: Vec<i32>,
    resolved: Vec<i32>,
    /// The paragraph embedding level, 0 or 1.
    pub level: i32,
    /// The resolved level of each UTF-16 unit before line breaking; -1 for characters X9 removes.
    pub levels: Vec<i32>,
}

impl BidiParagraph {
    /// `base_level` is 0, 1, or `None` for the first strong character (P2, P3).
    pub fn new(text: &str, base_level: Option<i32>) -> BidiParagraph {
        let units: Vec<u16> = text.encode_utf16().collect();
        let mut cps = Vec::new();
        let mut starts = Vec::new();
        let mut cp_of_unit = vec![0usize; units.len() + 1];
        let mut i = 0;
        while i < units.len() {
            cp_of_unit[i] = cps.len();
            starts.push(i);
            let u = i32::from(units[i]);
            if (0xD800..0xDC00).contains(&u)
                && i + 1 < units.len()
                && (0xDC00..0xE000).contains(&units[i + 1])
            {
                cps.push(0x10000 + ((u - 0xD800) << 10) + (i32::from(units[i + 1]) - 0xDC00));
                i += 1;
                cp_of_unit[i] = cps.len() - 1;
            } else {
                cps.push(u);
            }
            i += 1;
        }
        cp_of_unit[units.len()] = cps.len();
        starts.push(units.len());
        let n = cps.len();
        let orig: Vec<BidiType> = cps.iter().map(|&c| type_of(c)).collect();
        let mut p = BidiParagraph {
            cps,
            unit_of: starts,
            cp_of_unit,
            orig,
            matching_pdi: vec![-1; n],
            matching_initiator: vec![-1; n],
            embedding: vec![0; n],
            resolved: vec![0; n],
            level: 0,
            levels: Vec::new(),
        };
        p.match_isolates();
        p.level = match base_level {
            None => p.first_strong(0, n),
            Some(l) => l & 1,
        };
        let types = p.explicit();
        p.implicit(&types);
        p.levels = (0..units.len())
            .map(|u| p.resolved[p.cp_of_unit[u]])
            .collect();
        p
    }

    /// The levels of units `start` to `end` of one line, after L1.
    pub fn line_levels(&self, start: usize, end: usize) -> Vec<i32> {
        let c0 = self.cp_of_unit[start];
        let line = self.line_levels_by_code(c0, self.cp_of_unit[end]);
        (start..end)
            .map(|u| line[self.cp_of_unit[u] - c0])
            .collect()
    }

    /// Unit indices of one line in display order, left to right (L2). Removed characters are left out.
    pub fn visual_order(&self, start: usize, end: usize) -> Vec<usize> {
        let c0 = self.cp_of_unit[start];
        let c1 = self.cp_of_unit[end];
        let line = self.line_levels_by_code(c0, c1);
        let mut order: Vec<usize> = Vec::with_capacity(c1 - c0);
        let mut highest = 0;
        let mut lowest_odd = i32::MAX;
        for i in c0..c1 {
            let l = line[i - c0];
            if l < 0 {
                continue;
            }
            order.push(i);
            highest = highest.max(l);
            if l & 1 == 1 {
                lowest_odd = lowest_odd.min(l);
            }
        }
        let mut l = highest;
        while l >= lowest_odd {
            let mut i = 0;
            while i < order.len() {
                if line[order[i] - c0] < l {
                    i += 1;
                    continue;
                }
                let mut j = i;
                while j + 1 < order.len() && line[order[j + 1] - c0] >= l {
                    j += 1;
                }
                order[i..=j].reverse();
                i = j + 1;
            }
            l -= 1;
        }
        let mut units = Vec::with_capacity(end - start);
        for c in order {
            units.extend(self.unit_of[c]..self.unit_of[c + 1]);
        }
        units
    }

    fn line_levels_by_code(&self, c0: usize, c1: usize) -> Vec<i32> {
        let mut line = vec![0; c1 - c0];
        let mut trailing = true;
        for i in (c0..c1).rev() {
            let mut lv = self.resolved[i];
            if lv >= 0 {
                let t = self.orig[i];
                if t == BidiType::B || t == BidiType::S {
                    lv = self.level;
                    trailing = true;
                } else if trailing
                    && matches!(
                        t,
                        BidiType::Ws
                            | BidiType::Fsi
                            | BidiType::Lri
                            | BidiType::Rli
                            | BidiType::Pdi
                    )
                {
                    lv = self.level;
                } else {
                    trailing = false;
                }
            }
            line[i - c0] = lv;
        }
        line
    }

    // BD9
    fn match_isolates(&mut self) {
        let mut open: Vec<usize> = Vec::new();
        for i in 0..self.orig.len() {
            if is_initiator(self.orig[i]) {
                open.push(i);
            } else if self.orig[i] == BidiType::Pdi
                && let Some(o) = open.pop()
            {
                self.matching_pdi[o] = i as isize;
                self.matching_initiator[i] = o as isize;
            }
        }
    }

    // P2 and P3 over [from, to): 1 for a right to left first strong character, else 0.
    fn first_strong(&self, from: usize, to: usize) -> i32 {
        let mut i = from;
        while i < to {
            let t = self.orig[i];
            if t == BidiType::L {
                return 0;
            }
            if t == BidiType::R || t == BidiType::Al {
                return 1;
            }
            if is_initiator(t) {
                i = if self.matching_pdi[i] < 0 {
                    to
                } else {
                    self.matching_pdi[i] as usize
                };
            }
            i += 1;
        }
        0
    }

    // X1 to X8: embedding levels, and the types with directional overrides applied.
    fn explicit(&mut self) -> Vec<BidiType> {
        let n = self.orig.len();
        let mut types = self.orig.clone();
        let mut levels = vec![0i32; MAX_DEPTH + 2];
        let mut overrides = [BidiType::On; MAX_DEPTH + 2];
        let mut isolate = [false; MAX_DEPTH + 2];
        let mut sp = 0usize;
        levels[0] = self.level;
        let mut overflow_isolates = 0;
        let mut overflow_embeddings = 0;
        let mut valid_isolates = 0;
        let depth = MAX_DEPTH as i32;
        for i in 0..n {
            let t = self.orig[i];
            match t {
                BidiType::Rle | BidiType::Lre | BidiType::Rlo | BidiType::Lro => {
                    self.embedding[i] = -1;
                    let next = next_level(levels[sp], t == BidiType::Rle || t == BidiType::Rlo);
                    if next <= depth && overflow_isolates == 0 && overflow_embeddings == 0 {
                        sp += 1;
                        levels[sp] = next;
                        overrides[sp] = match t {
                            BidiType::Rlo => BidiType::R,
                            BidiType::Lro => BidiType::L,
                            _ => BidiType::On,
                        };
                        isolate[sp] = false;
                    } else if overflow_isolates == 0 {
                        overflow_embeddings += 1;
                    }
                }
                BidiType::Rli | BidiType::Lri | BidiType::Fsi => {
                    self.embedding[i] = levels[sp];
                    if overrides[sp] != BidiType::On {
                        types[i] = overrides[sp];
                    }
                    let rtl = t == BidiType::Rli
                        || (t == BidiType::Fsi
                            && self.first_strong(
                                i + 1,
                                if self.matching_pdi[i] < 0 {
                                    n
                                } else {
                                    self.matching_pdi[i] as usize
                                },
                            ) == 1);
                    let next = next_level(levels[sp], rtl);
                    if next <= depth && overflow_isolates == 0 && overflow_embeddings == 0 {
                        valid_isolates += 1;
                        sp += 1;
                        levels[sp] = next;
                        overrides[sp] = BidiType::On;
                        isolate[sp] = true;
                    } else {
                        overflow_isolates += 1;
                    }
                }
                BidiType::Pdi => {
                    if overflow_isolates > 0 {
                        overflow_isolates -= 1;
                    } else if valid_isolates > 0 {
                        overflow_embeddings = 0;
                        while !isolate[sp] {
                            sp -= 1;
                        }
                        sp -= 1;
                        valid_isolates -= 1;
                    }
                    self.embedding[i] = levels[sp];
                    if overrides[sp] != BidiType::On {
                        types[i] = overrides[sp];
                    }
                }
                BidiType::Pdf => {
                    self.embedding[i] = -1;
                    if overflow_isolates > 0 {
                        continue;
                    }
                    if overflow_embeddings > 0 {
                        overflow_embeddings -= 1;
                    } else if !isolate[sp] && sp > 0 {
                        sp -= 1;
                    }
                }
                BidiType::B => self.embedding[i] = self.level,
                BidiType::Bn => self.embedding[i] = -1,
                _ => {
                    self.embedding[i] = levels[sp];
                    if overrides[sp] != BidiType::On {
                        types[i] = overrides[sp];
                    }
                }
            }
        }
        types
    }

    // X9 and X10: drop the removed characters, group the rest into level runs and chain those into isolating run
    // sequences.
    fn implicit(&mut self, types: &[BidiType]) {
        let n = self.cps.len();
        let mut kept: Vec<usize> = Vec::with_capacity(n);
        let mut position = vec![-1isize; n];
        for i in 0..n {
            self.resolved[i] = -1;
            if self.embedding[i] < 0 {
                continue;
            }
            position[i] = kept.len() as isize;
            kept.push(i);
        }

        let mut run_start: Vec<usize> = Vec::new();
        let mut run_of_position = vec![0usize; kept.len()];
        for p in 0..kept.len() {
            if p == 0 || self.embedding[kept[p]] != self.embedding[kept[p - 1]] {
                run_start.push(p);
            }
            run_of_position[p] = run_start.len() - 1;
        }
        run_start.push(kept.len());

        let mut sequence: Vec<usize> = Vec::new();
        for r in 0..run_start.len().saturating_sub(1) {
            let first = kept[run_start[r]];
            if self.orig[first] == BidiType::Pdi && self.matching_initiator[first] >= 0 {
                continue;
            }
            sequence.clear();
            let mut run = r;
            loop {
                for p in run_start[run]..run_start[run + 1] {
                    sequence.push(kept[p]);
                }
                let last = sequence[sequence.len() - 1];
                if !is_initiator(self.orig[last]) || self.matching_pdi[last] < 0 {
                    break;
                }
                run = run_of_position[position[self.matching_pdi[last] as usize] as usize];
            }
            self.resolve_sequence(&sequence, types, &kept, &position);
        }
    }

    fn resolve_sequence(
        &mut self,
        sequence: &[usize],
        types: &[BidiType],
        kept: &[usize],
        position: &[isize],
    ) {
        let len = sequence.len();
        let seq_level = self.embedding[sequence[0]];
        let first_position = position[sequence[0]];
        let before = if first_position > 0 {
            self.embedding[kept[first_position as usize - 1]]
        } else {
            self.level
        };
        let last_index = sequence[len - 1];
        let last_position = position[last_index] as usize;
        let after = if is_initiator(self.orig[last_index]) || last_position + 1 >= kept.len() {
            self.level
        } else {
            self.embedding[kept[last_position + 1]]
        };
        let sos = if seq_level.max(before) & 1 == 1 {
            BidiType::R
        } else {
            BidiType::L
        };
        let eos = if seq_level.max(after) & 1 == 1 {
            BidiType::R
        } else {
            BidiType::L
        };

        let mut w: Vec<BidiType> = sequence.iter().map(|&i| types[i]).collect();

        // W1
        for k in 0..len {
            if w[k] == BidiType::Nsm {
                let prev = if k == 0 { sos } else { w[k - 1] };
                w[k] = if is_initiator(prev) || prev == BidiType::Pdi {
                    BidiType::On
                } else {
                    prev
                };
            }
        }
        // W2, W3
        let mut strong = sos;
        for k in 0..len {
            if matches!(w[k], BidiType::L | BidiType::R | BidiType::Al) {
                strong = w[k];
            } else if w[k] == BidiType::En && strong == BidiType::Al {
                w[k] = BidiType::An;
            }
        }
        for t in w.iter_mut() {
            if *t == BidiType::Al {
                *t = BidiType::R;
            }
        }
        // W4
        if len > 2 {
            for k in 1..len - 1 {
                if w[k] == BidiType::Es && w[k - 1] == BidiType::En && w[k + 1] == BidiType::En {
                    w[k] = BidiType::En;
                } else if w[k] == BidiType::Cs
                    && w[k - 1] == w[k + 1]
                    && (w[k - 1] == BidiType::En || w[k - 1] == BidiType::An)
                {
                    w[k] = w[k - 1];
                }
            }
        }
        // W5
        let mut pos = 0;
        while pos < len {
            if w[pos] != BidiType::Et {
                pos += 1;
                continue;
            }
            let mut end = pos;
            while end < len && w[end] == BidiType::Et {
                end += 1;
            }
            if (pos > 0 && w[pos - 1] == BidiType::En) || (end < len && w[end] == BidiType::En) {
                for t in &mut w[pos..end] {
                    *t = BidiType::En;
                }
            }
            pos = end;
        }
        // W6
        for t in w.iter_mut() {
            if matches!(*t, BidiType::Es | BidiType::Et | BidiType::Cs) {
                *t = BidiType::On;
            }
        }
        // W7
        strong = sos;
        for k in 0..len {
            if w[k] == BidiType::L || w[k] == BidiType::R {
                strong = w[k];
            } else if w[k] == BidiType::En && strong == BidiType::L {
                w[k] = BidiType::L;
            }
        }

        let dir = if seq_level & 1 == 1 {
            BidiType::R
        } else {
            BidiType::L
        };
        self.resolve_brackets(sequence, &mut w, sos, dir);

        // N1, N2
        pos = 0;
        while pos < len {
            if !is_neutral(w[pos]) {
                pos += 1;
                continue;
            }
            let mut end = pos;
            while end < len && is_neutral(w[end]) {
                end += 1;
            }
            let lead = if pos == 0 { sos } else { direction(w[pos - 1]) };
            let trail = if end == len { eos } else { direction(w[end]) };
            let fill = if lead == trail { lead } else { dir };
            for t in &mut w[pos..end] {
                *t = fill;
            }
            pos = end;
        }

        // I1, I2
        for k in 0..len {
            let mut l = seq_level;
            if seq_level & 1 == 0 {
                l += if w[k] == BidiType::R {
                    1
                } else if w[k] == BidiType::An || w[k] == BidiType::En {
                    2
                } else {
                    0
                };
            } else if matches!(w[k], BidiType::L | BidiType::An | BidiType::En) {
                l += 1;
            }
            self.resolved[sequence[k]] = l;
        }
    }

    // BD16 and N0.
    fn resolve_brackets(
        &self,
        sequence: &[usize],
        w: &mut [BidiType],
        sos: BidiType,
        dir: BidiType,
    ) {
        let len = w.len();
        let mut openers: Vec<(i32, usize)> = Vec::new();
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for k in 0..len {
            if w[k] != BidiType::On {
                continue;
            }
            let cp = self.cps[sequence[k]];
            let (paired, opening) = paired_bracket(cp);
            if paired == 0 {
                continue;
            }
            if opening {
                if openers.len() == MAX_BRACKET_DEPTH {
                    break;
                }
                openers.push((canonical(paired), k));
                continue;
            }
            let closing = canonical(cp);
            for s in (0..openers.len()).rev() {
                if openers[s].0 == closing {
                    pairs.push((openers[s].1, k));
                    openers.truncate(s);
                    break;
                }
            }
        }
        pairs.sort_by_key(|p| p.0);

        let opposite = if dir == BidiType::L {
            BidiType::R
        } else {
            BidiType::L
        };
        for (open, close) in pairs {
            let mut found_dir = false;
            let mut found_opposite = false;
            for &t in &w[open + 1..close] {
                let s = strong_or_neutral(t);
                if s == dir {
                    found_dir = true;
                    break;
                }
                if s == opposite {
                    found_opposite = true;
                }
            }
            let to = if found_dir {
                dir
            } else if found_opposite {
                let mut context = sos;
                for back in (0..open).rev() {
                    let s = strong_or_neutral(w[back]);
                    if s != BidiType::On {
                        context = s;
                        break;
                    }
                }
                if context == opposite { opposite } else { dir }
            } else {
                continue;
            };
            self.set_bracket(sequence, w, open, to);
            self.set_bracket(sequence, w, close, to);
        }
    }

    // Characters that were NSM before W1 and follow a bracket take its new direction.
    fn set_bracket(&self, sequence: &[usize], w: &mut [BidiType], k: usize, to: BidiType) {
        w[k] = to;
        let mut j = k + 1;
        while j < w.len() && self.orig[sequence[j]] == BidiType::Nsm {
            w[j] = to;
            j += 1;
        }
    }
}

/// The direction a text box takes from what is typed in it: right to left when the first strong character is.
pub fn is_right_to_left(text: &str) -> bool {
    // The first strong character decides, so a long text only needs its start looked at.
    let start: String = text.chars().take(256).collect();
    start.encode_utf16().any(|u| u >= 0x590) && BidiParagraph::new(&start, None).level == 1
}
