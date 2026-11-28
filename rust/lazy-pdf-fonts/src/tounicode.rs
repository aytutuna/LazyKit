//! Writes the body of a ToUnicode CMap stream (ISO 32000-1 9.10.3).

use std::collections::BTreeMap;

/// A code that does not fit the code length, or a length outside 1 to 4.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutOfRange(pub String);

// A bfchar or bfrange section holds at most 100 entries (Adobe Technical Note 5014).
const SECTION_SIZE: usize = 100;

struct Run {
    lo: u32,
    hi: u32,
    text: Vec<u16>,
}

/// The stream data for a font whose character codes are `code_bytes` bytes (1 to 4) long. Codes in a row that
/// map to characters in a row become one bfrange. A code given twice keeps its last text; empty text is left
/// out.
pub fn write_to_unicode(map: &[(i64, Vec<u16>)], code_bytes: u32) -> Result<Vec<u8>, OutOfRange> {
    if !(1..=4).contains(&code_bytes) {
        return Err(OutOfRange("codeBytes must be 1 to 4.".into()));
    }
    let limit = 1u64 << (8 * code_bytes);
    let mut texts: BTreeMap<u32, &[u16]> = BTreeMap::new();
    for (code, text) in map {
        let Ok(code32) = i32::try_from(*code) else {
            return Err(OutOfRange(format!(
                "Code {code} does not fit {code_bytes} bytes."
            )));
        };
        if u64::from(code32 as u32) >= limit {
            return Err(OutOfRange(format!(
                "Code {code} does not fit {code_bytes} bytes."
            )));
        }
        if !text.is_empty() {
            texts.insert(code32 as u32, text);
        }
    }

    let mut ranges: Vec<Run> = Vec::new();
    for (&code, &text) in &texts {
        match ranges.last_mut() {
            Some(last) if continues(last, code, text) => last.hi = code,
            _ => ranges.push(Run {
                lo: code,
                hi: code,
                text: text.to_vec(),
            }),
        }
    }

    let width = 2 * code_bytes as usize;
    let mut out = String::from("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n");
    out += "/CIDSystemInfo <</Registry (Adobe) /Ordering (UCS) /Supplement 0>> def\n";
    out += "/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n";
    out += &format!(
        "1 begincodespacerange\n<{}> <{}>\nendcodespacerange\n",
        "0".repeat(width),
        "F".repeat(width)
    );

    let singles: Vec<&Run> = ranges.iter().filter(|r| r.lo == r.hi).collect();
    let spans: Vec<&Run> = ranges.iter().filter(|r| r.lo != r.hi).collect();
    for section in spans.chunks(SECTION_SIZE) {
        out += &format!("{} beginbfrange\n", section.len());
        for r in section {
            out += &format!(
                "<{}> <{}> <{}>\n",
                hex(r.lo, width),
                hex(r.hi, width),
                utf16_hex(&r.text)
            );
        }
        out += "endbfrange\n";
    }
    for section in singles.chunks(SECTION_SIZE) {
        out += &format!("{} beginbfchar\n", section.len());
        for r in section {
            out += &format!("<{}> <{}>\n", hex(r.lo, width), utf16_hex(&r.text));
        }
        out += "endbfchar\n";
    }
    out += "endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n";
    Ok(out.into_bytes())
}

// The next code extends the range when its text is the range's last text plus one in the final UTF-16 unit.
// Neither the code's nor the unit's low byte may wrap inside the range: readers differ on what a carry means.
fn continues(range: &Run, code: u32, text: &[u16]) -> bool {
    if code != range.hi.wrapping_add(1)
        || (code ^ range.lo) > 0xFF
        || text.len() != range.text.len()
    {
        return false;
    }
    let span = (code - range.lo) as i64;
    let (Some(&first), Some(&last)) = (range.text.last(), text.last()) else {
        return false;
    };
    if text[..text.len() - 1] != range.text[..range.text.len() - 1] {
        return false;
    }
    last == (first as i64 + span) as u16 && (first & 0xFF) as i64 + span <= 0xFF
}

fn hex(value: u32, width: usize) -> String {
    format!("{value:0width$X}")
}

fn utf16_hex(text: &[u16]) -> String {
    text.iter().map(|&u| hex(u32::from(u), 4)).collect()
}
