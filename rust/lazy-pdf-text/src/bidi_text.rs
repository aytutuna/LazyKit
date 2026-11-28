//! Turns a line of glyphs in visual order into logical order (UAX #9 rules L1 to L2 run backwards, with a simplified
//! resolution of weak and neutral characters). Glyphs are already placed where they show, so the content stream order
//! says nothing about the logical order.

use crate::collector::FlowGlyph;
use crate::unicode::is_letter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BidiClass {
    L,
    /// Hebrew and Arabic letters.
    R,
    En,
    An,
    N,
}

pub fn class_of(text: &str) -> BidiClass {
    let mut weak = BidiClass::N;
    for c in text.encode_utf16() {
        let class = class_of_unit(c);
        if class == BidiClass::L || class == BidiClass::R {
            return class;
        }
        if class == BidiClass::En || class == BidiClass::An {
            weak = class;
        }
    }
    weak
}

fn class_of_unit(c: u16) -> BidiClass {
    if c < 0x590 {
        return if (0x30..=0x39).contains(&c) {
            BidiClass::En
        } else if is_letter(c) {
            BidiClass::L
        } else {
            BidiClass::N
        };
    }
    if (0x0660..=0x0669).contains(&c) || c == 0x066B || c == 0x066C {
        return BidiClass::An;
    }
    if (0x06F0..=0x06F9).contains(&c) {
        return BidiClass::En;
    }
    if (0x0590..=0x08FF).contains(&c)
        || (0xFB1D..=0xFDFF).contains(&c)
        || (0xFE70..=0xFEFF).contains(&c)
    {
        return if is_letter(c) {
            BidiClass::R
        } else {
            BidiClass::N
        };
    }
    if (0x2000..=0x206F).contains(&c) {
        return BidiClass::N;
    }
    if is_letter(c) {
        BidiClass::L
    } else {
        BidiClass::N
    }
}

fn is_joiner(text: &str) -> bool {
    matches!(text, "." | "," | ":" | "/" | "+" | "-")
}

fn is_terminator(text: &str) -> bool {
    let mut units = text.encode_utf16();
    match (units.next(), units.next()) {
        (Some(unit), None) => matches!(
            unit,
            0x25 | 0x24 | 0x23 | 0xA2 | 0xA3 | 0xA5 | 0xB0 | 0xB1 | 0x20AC
        ),
        _ => false,
    }
}

fn is_number(class: BidiClass) -> bool {
    class == BidiClass::En || class == BidiClass::An
}

fn direction(class: BidiClass, level: i32) -> i32 {
    if class == BidiClass::L || (class == BidiClass::En && level == 0) {
        0
    } else {
        1
    }
}

fn mirror(text: &str) -> Option<&'static str> {
    Some(match text {
        "(" => ")",
        ")" => "(",
        "[" => "]",
        "]" => "[",
        "{" => "}",
        "}" => "{",
        "<" => ">",
        ">" => "<",
        "\u{AB}" => "\u{BB}",
        "\u{BB}" => "\u{AB}",
        _ => return None,
    })
}

/// Reorders `line` (indexes into `glyphs`) in place and sets each glyph's level; true when the line reads right to left.
pub fn to_logical(line: &mut [usize], glyphs: &mut [FlowGlyph], page_rtl: bool) -> bool {
    let n = line.len();
    let mut strong_r = 0;
    let mut strong_l = 0;
    let mut classes = vec![BidiClass::N; n];
    for i in 0..n {
        classes[i] = class_of(&glyphs[line[i]].text);
        match classes[i] {
            BidiClass::R => strong_r += 1,
            BidiClass::L => strong_l += 1,
            _ => {}
        }
    }
    for &g in line.iter() {
        glyphs[g].level = 0;
    }
    if strong_r == 0 {
        return false;
    }

    let rtl = if page_rtl {
        strong_r > 0
    } else {
        strong_r > strong_l
    };
    let para = i32::from(rtl);
    let even_l = if rtl { 2 } else { 0 };
    let mut levels = vec![0i32; n];
    // Nearest letter on either side, 1 for right to left; the paragraph direction when there is none.
    let mut before = vec![0i32; n];
    let mut after = vec![0i32; n];
    let mut last = para;
    for i in 0..n {
        if classes[i] == BidiClass::L || classes[i] == BidiClass::R {
            last = i32::from(classes[i] == BidiClass::R);
        }
        before[i] = last;
    }
    let mut next = para;
    for i in (0..n).rev() {
        if classes[i] == BidiClass::L || classes[i] == BidiClass::R {
            next = i32::from(classes[i] == BidiClass::R);
        }
        after[i] = next;
    }
    for i in 0..n {
        levels[i] = match classes[i] {
            BidiClass::L => even_l,
            BidiClass::R => 1,
            BidiClass::An => 2,
            BidiClass::En => {
                if rtl || (before[i] == 1 && after[i] == 1) {
                    2
                } else {
                    0
                }
            }
            BidiClass::N => -1,
        };
    }
    // Everything to the right of a neutral is still unresolved when it is reached, so where the scans to the right stop
    // is known beforehand; rescanning for every neutral is quadratic on a long run of them.
    let text = |i: usize| glyphs[line[i]].text.as_str();
    let terminator: Vec<bool> = (0..n)
        .map(|i| levels[i] < 0 && is_terminator(text(i)))
        .collect();
    let mut next_resolved = vec![n; n];
    let mut end_of_terminators = vec![n; n];
    for i in (0..n).rev() {
        next_resolved[i] = if i + 1 < n && levels[i + 1] < 0 {
            next_resolved[i + 1]
        } else {
            i + 1
        };
        end_of_terminators[i] = if i + 1 < n && terminator[i + 1] {
            end_of_terminators[i + 1]
        } else {
            i + 1
        };
    }
    for i in 0..n {
        if levels[i] >= 0 {
            continue;
        }
        if i > 0
            && i + 1 < n
            && is_number(classes[i - 1])
            && is_number(classes[i + 1])
            && is_joiner(text(i))
        {
            levels[i] = levels[i - 1];
            continue;
        }
        if is_terminator(text(i)) {
            // W5: a run of terminators beside a European number belongs to the number.
            let mut left1 = i as isize - 1;
            let right1 = end_of_terminators[i];
            while left1 >= 0 && levels[left1 as usize] < 0 && is_terminator(text(left1 as usize)) {
                left1 -= 1;
            }
            if left1 >= 0 && classes[left1 as usize] == BidiClass::En {
                levels[i] = levels[left1 as usize];
            } else if right1 < n && classes[right1] == BidiClass::En {
                levels[i] = levels[right1];
            }
            if levels[i] >= 0 {
                continue;
            }
        }
        let mut a = i as isize - 1;
        let b = next_resolved[i];
        while a >= 0 && levels[a as usize] < 0 {
            a -= 1;
        }
        let left = if a >= 0 {
            direction(classes[a as usize], levels[a as usize])
        } else {
            para
        };
        let right = if b < n {
            direction(classes[b], levels[b])
        } else {
            para
        };
        let dir = if left == right { left } else { para };
        levels[i] = if dir == 1 { 1 } else { even_l };
    }
    for i in 0..n {
        glyphs[line[i]].level = levels[i];
    }

    let highest = levels.iter().copied().max().unwrap_or(0);
    let lowest_odd = levels
        .iter()
        .copied()
        .filter(|l| l & 1 == 1)
        .min()
        .unwrap_or(highest + 1);
    let mut level = lowest_odd;
    while level <= highest {
        let mut i = 0;
        while i < n {
            if glyphs[line[i]].level < level {
                i += 1;
                continue;
            }
            let mut j = i;
            while j < n && glyphs[line[j]].level >= level {
                j += 1;
            }
            line[i..j].reverse();
            i = j;
        }
        level += 1;
    }
    for &g in line.iter() {
        if glyphs[g].level & 1 == 1
            && let Some(mirrored) = mirror(&glyphs[g].text)
        {
            glyphs[g].text = mirrored.to_string();
        }
    }
    rtl
}
