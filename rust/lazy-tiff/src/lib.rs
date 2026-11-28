//! TIFF files as Lazy Scan writes them, byte for byte as its Swift and C# writers do: black and white pages as CCITT G4
//! (compression 4), one page or several, and one page at 16 bits per sample, uncompressed.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

use lazy_pdf_filters::g4::{self, encode_g4};

const ASCII: u16 = 2;
const SHORT: u16 = 3;
const LONG: u16 = 4;
const RATIONAL: u16 = 5;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    /// A TIFF needs at least one page.
    NoPages,
    /// The packed buffer is shorter than the page's size says, or the page has no pixels across.
    BadPage,
    /// The samples do not fill the page, or the page is not gray or RGB.
    BadSamples,
    /// An offset or size does not fit the 32 bits a classic TIFF has.
    TooLarge,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Error::NoPages => "A TIFF needs at least one page.",
            Error::BadPage => "The packed page does not match its size.",
            Error::BadSamples => "The samples must fill a gray or RGB page.",
            Error::TooLarge => "The file is too large for a TIFF.",
        })
    }
}

impl std::error::Error for Error {}

/// When a picture was taken and a caption for it, written as EXIF DateTimeOriginal and ImageDescription. Empty
/// values write no tags.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Details {
    pub year: Option<i32>,
    pub month: Option<i32>,
    pub day: Option<i32>,
    pub caption: String,
}

impl Details {
    /// "yyyy:MM:dd 00:00:00", or None without a four-digit year. A month or day that is missing or out of range counts
    /// as 1.
    pub fn exif_date(&self) -> Option<String> {
        let year = self.year.filter(|y| (1000..=9999).contains(y))?;
        let month = self.month.filter(|m| (1..=12).contains(m)).unwrap_or(1);
        let day = self
            .day
            .filter(|&d| d >= 1 && d <= days_in_month(year, month))
            .unwrap_or(1);
        Some(format!("{year:04}:{month:02}:{day:02} 00:00:00"))
    }

    /// The caption without surrounding white space, or None when nothing is left.
    pub fn exif_caption(&self) -> Option<&str> {
        let trimmed = self.caption.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    }
}

fn days_in_month(year: i32, month: i32) -> i32 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// One page of a black and white TIFF: `packed` rows of `g4::stride(width)` bytes, 0 = black, and its resolution.
#[derive(Clone, Copy, Debug)]
pub struct BilevelPage<'a> {
    pub width: usize,
    pub height: usize,
    pub packed: &'a [u8],
    pub dpi: i32,
}

/// A page at 16 bits per sample as a scanner delivers it: `channels` is 1 (gray) or 3 (red, green, blue) and `samples`
/// holds `width * height * channels` values, row by row, a pixel's channels together.
#[derive(Clone, Copy, Debug)]
pub struct DeepPage<'a> {
    pub width: usize,
    pub height: usize,
    pub channels: usize,
    pub samples: &'a [u16],
}

struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    value: u32,
}

fn entry(tag: u16, kind: u16, count: u32, value: u32) -> Entry {
    Entry {
        tag,
        kind,
        count,
        value,
    }
}

fn offset(value: usize) -> Result<u32, Error> {
    u32::try_from(value).map_err(|_| Error::TooLarge)
}

/// Zero-terminated, and at least `min_length` bytes so that a value of five or more is an offset, not stored in the
/// entry.
fn terminated(text: &str, min_length: usize) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    bytes.resize((bytes.len() + 1).max(min_length), 0);
    bytes
}

/// A file with every page, in order, each one strip of G4 data followed by its directory: 1 bit per pixel, WhiteIsZero
/// as fax images are stored, little-endian. Several pages are marked as pages of one document (NewSubfileType 2,
/// PageNumber n of count). `details` adds ImageDescription and an Exif directory with DateTimeOriginal to every
/// page's directory, each only when it has a value.
pub fn bilevel(pages: &[BilevelPage], details: Option<&Details>) -> Result<Vec<u8>, Error> {
    if pages.is_empty() {
        return Err(Error::NoPages);
    }
    let multi = pages.len() > 1;
    let caption = details
        .and_then(Details::exif_caption)
        .map(|text| terminated(text, 5));
    let date = details
        .and_then(Details::exif_date)
        .map(|text| terminated(&text, 0));
    let mut out = b"II*\0".to_vec();
    out.extend([0; 4]); // the first directory's offset, filled in below
    let mut link = 4usize;
    for (index, page) in pages.iter().enumerate() {
        let strip = encode_g4(page.packed, page.width, page.height).map_err(|_| Error::BadPage)?;
        let strip_offset = out.len();
        out.extend(&strip);
        if out.len() % 2 == 1 {
            out.push(0); // directories start on a word boundary
        }
        let mut entries = Vec::new();
        if multi {
            entries.push(entry(254, LONG, 1, 2)); // NewSubfileType: a page of a document
        }
        entries.push(entry(256, LONG, 1, offset(page.width)?)); // ImageWidth
        entries.push(entry(257, LONG, 1, offset(page.height)?)); // ImageLength
        entries.push(entry(258, SHORT, 1, 1)); // BitsPerSample
        entries.push(entry(259, SHORT, 1, 4)); // Compression: CCITT T.6
        entries.push(entry(262, SHORT, 1, 0)); // PhotometricInterpretation: WhiteIsZero
        if let Some(caption) = &caption {
            entries.push(entry(270, ASCII, caption.len() as u32, 0)); // ImageDescription
        }
        entries.push(entry(273, LONG, 1, offset(strip_offset)?)); // StripOffsets
        entries.push(entry(277, SHORT, 1, 1)); // SamplesPerPixel
        entries.push(entry(278, LONG, 1, offset(page.height)?)); // RowsPerStrip: one strip
        entries.push(entry(279, LONG, 1, offset(strip.len())?)); // StripByteCounts
        entries.push(entry(282, RATIONAL, 1, 0)); // XResolution
        entries.push(entry(283, RATIONAL, 1, 0)); // YResolution
        entries.push(entry(293, LONG, 1, 0)); // T6Options: no uncompressed mode
        entries.push(entry(296, SHORT, 1, 2)); // ResolutionUnit: inch
        if multi {
            // PageNumber: two shorts, this page (from 0) and the page count.
            entries.push(entry(
                297,
                SHORT,
                2,
                index as u32 | (pages.len() as u32) << 16,
            ));
        }
        if date.is_some() {
            entries.push(entry(34665, LONG, 1, 0)); // Exif directory
        }
        let directory = out.len();
        let resolution = directory + 2 + entries.len() * 12 + 4; // two rationals after the directory
        // Then the description (padded to a word), the date and a one-entry Exif directory.
        let description_offset = resolution + 16;
        let date_offset = description_offset + caption.as_ref().map_or(0, Vec::len).div_ceil(2) * 2;
        let exif_offset = date_offset + 20;
        out.extend((entries.len() as u16).to_le_bytes());
        for e in &entries {
            let mut value = e.value;
            if e.kind == RATIONAL {
                value = offset(resolution + if e.tag == 282 { 0 } else { 8 })?;
            }
            if e.tag == 270 {
                value = offset(description_offset)?;
            }
            if e.tag == 34665 {
                value = offset(exif_offset)?;
            }
            out.extend(e.tag.to_le_bytes());
            out.extend(e.kind.to_le_bytes());
            out.extend(e.count.to_le_bytes());
            if e.kind == SHORT && e.count == 1 {
                out.extend((value as u16).to_le_bytes());
                out.extend([0, 0]);
            } else {
                out.extend(value.to_le_bytes());
            }
        }
        out.extend([0; 4]); // the next directory's offset, written when it is known
        let ppi = page.dpi.max(1) as u32;
        for value in [ppi, 1, ppi, 1] {
            out.extend(value.to_le_bytes());
        }
        if let Some(caption) = &caption {
            out.extend(caption);
            if caption.len() % 2 == 1 {
                out.push(0);
            }
        }
        if let Some(date) = &date {
            out.extend(date);
            out.extend(1u16.to_le_bytes());
            out.extend(36867u16.to_le_bytes()); // DateTimeOriginal
            out.extend(ASCII.to_le_bytes());
            out.extend((date.len() as u32).to_le_bytes());
            out.extend(offset(date_offset)?.to_le_bytes());
            out.extend([0; 4]);
        }
        out[link..link + 4].copy_from_slice(&offset(directory)?.to_le_bytes());
        link = directory + 2 + entries.len() * 12;
    }
    Ok(out)
}

/// The one-page file of a page at 16 bits per sample, uncompressed: the strip first, then the directory (BlackIsZero
/// for gray, RGB for three channels), the resolution in pixels per inch.
pub fn deep(page: &DeepPage, dpi: i32, details: Option<&Details>) -> Result<Vec<u8>, Error> {
    if page.width == 0 || page.height == 0 || !matches!(page.channels, 1 | 3) {
        return Err(Error::BadSamples);
    }
    let expected = page
        .width
        .checked_mul(page.height)
        .and_then(|n| n.checked_mul(page.channels))
        .ok_or(Error::TooLarge)?;
    if page.samples.len() != expected {
        return Err(Error::BadSamples);
    }
    let strip_length = expected * 2;
    let mut out = Vec::with_capacity(strip_length + 512);
    out.extend(b"II*\0");
    out.extend(offset(8 + strip_length)?.to_le_bytes()); // the directory comes after the strip
    for sample in page.samples {
        out.extend(sample.to_le_bytes());
    }
    let caption = details
        .and_then(Details::exif_caption)
        .map(|text| terminated(text, 5));
    let date = details
        .and_then(Details::exif_date)
        .map(|text| terminated(&text, 0));
    let color = page.channels == 3;
    let mut entries = vec![
        entry(256, LONG, 1, offset(page.width)?),    // ImageWidth
        entry(257, LONG, 1, offset(page.height)?),   // ImageLength
        entry(258, SHORT, page.channels as u32, 16), // BitsPerSample (an offset for three)
        entry(259, SHORT, 1, 1),                     // Compression: none
        entry(262, SHORT, 1, if color { 2 } else { 1 }), // RGB or BlackIsZero
    ];
    if let Some(caption) = &caption {
        entries.push(entry(270, ASCII, caption.len() as u32, 0));
    }
    entries.push(entry(273, LONG, 1, 8)); // StripOffsets
    entries.push(entry(277, SHORT, 1, page.channels as u32)); // SamplesPerPixel
    entries.push(entry(278, LONG, 1, offset(page.height)?)); // RowsPerStrip: one strip
    entries.push(entry(279, LONG, 1, offset(strip_length)?)); // StripByteCounts
    entries.push(entry(282, RATIONAL, 1, 0)); // XResolution
    entries.push(entry(283, RATIONAL, 1, 0)); // YResolution
    entries.push(entry(296, SHORT, 1, 2)); // ResolutionUnit: inch
    if date.is_some() {
        entries.push(entry(34665, LONG, 1, 0)); // Exif directory
    }
    let directory = out.len();
    let resolution = directory + 2 + entries.len() * 12 + 4;
    let bits_offset = resolution + 16;
    let description_offset = bits_offset + if color { 6 } else { 0 };
    let date_offset = description_offset + caption.as_ref().map_or(0, Vec::len).div_ceil(2) * 2;
    let exif_offset = date_offset + 20;
    out.extend((entries.len() as u16).to_le_bytes());
    for e in &entries {
        let mut value = e.value;
        if e.kind == RATIONAL {
            value = offset(resolution + if e.tag == 282 { 0 } else { 8 })?;
        }
        if e.tag == 258 && color {
            value = offset(bits_offset)?;
        }
        if e.tag == 270 {
            value = offset(description_offset)?;
        }
        if e.tag == 34665 {
            value = offset(exif_offset)?;
        }
        out.extend(e.tag.to_le_bytes());
        out.extend(e.kind.to_le_bytes());
        out.extend(e.count.to_le_bytes());
        if e.kind == SHORT && e.count == 1 {
            out.extend((value as u16).to_le_bytes());
            out.extend([0, 0]);
        } else {
            out.extend(value.to_le_bytes()); // a long, three shorts, an offset
        }
    }
    out.extend([0; 4]); // no next directory
    let ppi = dpi.max(1) as u32;
    for value in [ppi, 1, ppi, 1] {
        out.extend(value.to_le_bytes());
    }
    if color {
        for _ in 0..3 {
            out.extend(16u16.to_le_bytes());
        }
    }
    if let Some(caption) = &caption {
        out.extend(caption);
        if caption.len() % 2 == 1 {
            out.push(0);
        }
    }
    if let Some(date) = &date {
        out.extend(date);
        out.extend(1u16.to_le_bytes());
        out.extend(36867u16.to_le_bytes()); // DateTimeOriginal
        out.extend(ASCII.to_le_bytes());
        out.extend((date.len() as u32).to_le_bytes());
        out.extend(offset(date_offset)?.to_le_bytes());
        out.extend([0; 4]);
    }
    Ok(out)
}

/// Bytes in one packed row of a bilevel page.
pub fn stride(width: usize) -> usize {
    g4::stride(width)
}
