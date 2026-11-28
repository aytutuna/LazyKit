//! What a PDF/A-2b file carries besides its pages: the XMP metadata packet, the document information dictionary that must
//! say the same, and the sRGB output intent. The same text as Lazy Scan's Swift and C# writers.

/// The sRGB color profile embedded as the output intent: "sRGB-v2-magic.icc" from Compact-ICC-Profiles by Clinton
/// Ingram (https://github.com/saucecontrol/Compact-ICC-Profiles), released to the public domain under CC0 1.0. An ICC
/// v2.1 display profile, 736 bytes.
pub const SRGB_PROFILE: &[u8] = include_bytes!("srgb.icc");

/// The output intent's condition: the color space of the embedded profile.
pub const OUTPUT_CONDITION_IDENTIFIER: &str = "sRGB IEC61966-2.1";

/// A moment with its offset from UTC, in whole seconds as the dates are written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Date {
    pub year: i32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub minute: u32,
    pub second: u32,
    /// Minutes east of UTC.
    pub offset_minutes: i32,
}

impl Date {
    /// The present, in UTC; None where the program has no clock to read (a browser).
    pub fn now() -> Option<Date> {
        if cfg!(all(target_arch = "wasm32", target_os = "unknown")) {
            return None;
        }
        let seconds = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let days = seconds.div_euclid(86400);
        let rest = seconds.rem_euclid(86400);
        // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        let year = (yoe + era * 400 + i64::from(month <= 2)) as i32;
        Some(Date {
            year,
            month,
            day,
            hour: (rest / 3600) as u32,
            minute: (rest % 3600 / 60) as u32,
            second: (rest % 60) as u32,
            offset_minutes: 0,
        })
    }

    /// "D:20261019213005+03'00'".
    pub fn pdf(&self) -> String {
        format!(
            "D:{:04}{:02}{:02}{:02}{:02}{:02}{}'",
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
            self.offset("'")
        )
    }

    /// The same date in XMP: "2026-10-19T21:30:05+03:00".
    pub fn xmp(&self) -> String {
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}{}",
            self.year,
            self.month,
            self.day,
            self.hour,
            self.minute,
            self.second,
            self.offset(":")
        )
    }

    fn offset(&self, separator: &str) -> String {
        let sign = if self.offset_minutes < 0 { '-' } else { '+' };
        let magnitude = self.offset_minutes.abs();
        format!(
            "{sign}{:02}{separator}{:02}",
            magnitude / 60,
            magnitude % 60
        )
    }
}

/// `text` as a PDF text string: a literal "(...)" when it is printable ASCII (with "\", "(" and ")" escaped), otherwise
/// UTF-16BE with its byte order mark, in hex.
pub fn text_string(text: &str) -> String {
    if text.chars().all(|c| (' '..='~').contains(&c)) {
        let mut out = String::from("(");
        for c in text.chars() {
            if matches!(c, '\\' | '(' | ')') {
                out.push('\\');
            }
            out.push(c);
        }
        out.push(')');
        return out;
    }
    let mut out = String::from("<FEFF");
    for unit in text.encode_utf16() {
        out.push_str(&format!("{unit:04X}"));
    }
    out.push('>');
    out
}

/// A title with the characters XML cannot hold removed (control characters, U+007F, U+FFFE, U+FFFF), so the XMP and the
/// information dictionary carry the same text.
pub fn clean_title(title: &str) -> String {
    title
        .chars()
        .filter(|&c| c >= ' ' && c != '\u{7F}' && c != '\u{FFFE}' && c != '\u{FFFF}')
        .collect()
}

/// The document information dictionary: /Title (when given), /Producer, /Creator, /CreationDate and /ModDate, each the
/// same as its XMP property.
pub fn info_dictionary(title: Option<&str>, date: &Date, producer: &str) -> String {
    let mut out = String::from("<<");
    if let Some(title) = title {
        out.push_str(&format!("/Title {} ", text_string(title)));
    }
    let pdf_date = text_string(&date.pdf());
    let producer = text_string(producer);
    out.push_str(&format!(
        "/Producer {producer} /Creator {producer} /CreationDate {pdf_date} /ModDate {pdf_date}>>"
    ));
    out
}

fn escape_xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The XMP packet, UTF-8 without a byte order mark: PDF/A-2 level B (pdfaid), the title (dc:title, when given), the
/// dates (xmp:CreateDate, ModifyDate and MetadataDate) and the producer (pdf:Producer, xmp:CreatorTool), matching the
/// information dictionary.
pub fn xmp(title: Option<&str>, date: &Date, producer: &str) -> Vec<u8> {
    let xmp_date = date.xmp();
    let mut out = String::new();
    out.push_str("<?xpacket begin=\"\u{FEFF}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n");
    out.push_str("<x:xmpmeta xmlns:x=\"adobe:ns:meta/\">\n");
    out.push_str(" <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n");
    out.push_str("  <rdf:Description rdf:about=\"\"\n");
    out.push_str("    xmlns:pdfaid=\"http://www.aiim.org/pdfa/ns/id/\"\n");
    out.push_str("    xmlns:dc=\"http://purl.org/dc/elements/1.1/\"\n");
    out.push_str("    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\n");
    out.push_str("    xmlns:pdf=\"http://ns.adobe.com/pdf/1.3/\">\n");
    out.push_str("   <pdfaid:part>2</pdfaid:part>\n");
    out.push_str("   <pdfaid:conformance>B</pdfaid:conformance>\n");
    out.push_str("   <dc:format>application/pdf</dc:format>\n");
    if let Some(title) = title {
        out.push_str(&format!(
            "   <dc:title><rdf:Alt><rdf:li xml:lang=\"x-default\">{}</rdf:li></rdf:Alt></dc:title>\n",
            escape_xml(title)
        ));
    }
    out.push_str(&format!("   <xmp:CreateDate>{xmp_date}</xmp:CreateDate>\n"));
    out.push_str(&format!("   <xmp:ModifyDate>{xmp_date}</xmp:ModifyDate>\n"));
    out.push_str(&format!(
        "   <xmp:MetadataDate>{xmp_date}</xmp:MetadataDate>\n"
    ));
    out.push_str(&format!(
        "   <xmp:CreatorTool>{producer}</xmp:CreatorTool>\n"
    ));
    out.push_str(&format!("   <pdf:Producer>{producer}</pdf:Producer>\n"));
    out.push_str("  </rdf:Description>\n");
    out.push_str(" </rdf:RDF>\n");
    out.push_str("</x:xmpmeta>\n");
    out.push_str("<?xpacket end=\"w\"?>");
    out.into_bytes()
}

/// The catalog's /OutputIntents entry, its profile in object `profile_object`.
pub fn output_intents(profile_object: usize) -> String {
    format!(
        "[<</Type /OutputIntent /S /GTS_PDFA1 /OutputConditionIdentifier ({OUTPUT_CONDITION_IDENTIFIER}) \
         /Info ({OUTPUT_CONDITION_IDENTIFIER}) /DestOutputProfile {profile_object} 0 R>>]"
    )
}
