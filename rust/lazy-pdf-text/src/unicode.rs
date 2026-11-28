//! Character tests on UTF-16 units, as .NET's `char` methods answer them: a surrogate half is no letter, no digit and no
//! white space, and a case mapping is the simple one-to-one mapping of one unit.

use std::cmp::Ordering;

use unicode_general_category::{GeneralCategory as C, get_general_category};

fn scalar(unit: u16) -> Option<char> {
    char::from_u32(u32::from(unit))
}

fn category(unit: u16) -> Option<C> {
    scalar(unit).map(get_general_category)
}

pub fn is_letter(unit: u16) -> bool {
    if unit < 0x80 {
        return matches!(unit, 0x41..=0x5A | 0x61..=0x7A);
    }
    matches!(
        category(unit),
        Some(
            C::UppercaseLetter
                | C::LowercaseLetter
                | C::TitlecaseLetter
                | C::ModifierLetter
                | C::OtherLetter
        )
    )
}

pub fn is_letter_or_digit(unit: u16) -> bool {
    is_letter(unit) || category(unit) == Some(C::DecimalNumber)
}

pub fn is_lower(unit: u16) -> bool {
    category(unit) == Some(C::LowercaseLetter)
}

pub fn is_white_space(unit: u16) -> bool {
    if unit < 0x80 {
        return unit == 0x20 || (0x09..=0x0D).contains(&unit);
    }
    scalar(unit).is_some_and(char::is_whitespace)
}

pub fn is_non_spacing_mark(unit: u16) -> bool {
    category(unit) == Some(C::NonspacingMark)
}

pub fn is_mark(unit: u16) -> bool {
    matches!(
        category(unit),
        Some(C::NonspacingMark | C::SpacingMark | C::EnclosingMark)
    )
}

pub fn lowercase_invariant(unit: u16) -> u16 {
    if unit < 0x80 {
        return if (0x41..=0x5A).contains(&unit) {
            unit + 0x20
        } else {
            unit
        };
    }
    let Some(c) = scalar(unit) else { return unit };
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(one), None) if (one as u32) <= 0xFFFF => one as u32 as u16,
        _ => unit,
    }
}

/// `double.CompareTo`: a NaN sorts before every number and equals another NaN.
pub fn compare_doubles(a: f64, b: f64) -> Ordering {
    if a < b {
        Ordering::Less
    } else if a > b {
        Ordering::Greater
    } else if a == b {
        Ordering::Equal
    } else if a.is_nan() {
        if b.is_nan() {
            Ordering::Equal
        } else {
            Ordering::Less
        }
    } else {
        Ordering::Greater
    }
}
