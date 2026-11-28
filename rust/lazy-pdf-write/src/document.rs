//! Scanned pages as a PDF, byte for byte as Lazy Scan's Swift and C# writers make it: PDF 1.4, or PDF/A-2b for keeping.
//! Each page carries one full-page image XObject, a JPEG or a 1-bit CCITT G4 image (or a JPEG with a G4 text mask over
//! it), and when recognized text is given it is drawn invisibly in the embedded glyphless font, scaled so each line's
//! selectable text spans the same width as the printed words.
//!
//! Objects are numbered in a fixed order: 1 Catalog, 2 Pages; when any page has text, the six objects of the text layer's
//! font; the Info dictionary; for PDF/A, the XMP metadata and the sRGB profile; then Page, Image and Contents for each
//! page, then the mask image of each mixed page in page order.

use lazy_pdf_filters::encode::zlib_fixed;

use crate::glyphless::{self, Codes};
use crate::number::num;
use crate::pdfa::{self, Date};
use crate::writer::{ObjectWriter, ascii_string};

/// A rectangle, normalized (0 to 1) with a bottom-left origin, as recognizers give it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn min_x(&self) -> f64 {
        if self.width >= 0.0 {
            self.x
        } else {
            self.x + self.width
        }
    }

    fn min_y(&self) -> f64 {
        if self.height >= 0.0 {
            self.y
        } else {
            self.y + self.height
        }
    }

    /// Non-negative width and height.
    fn standardized(&self) -> Rect {
        Rect::new(
            self.min_x(),
            self.min_y(),
            self.width.abs(),
            self.height.abs(),
        )
    }

    fn max_y(&self) -> f64 {
        let s = self.standardized();
        s.y + s.height
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct OcrWord {
    pub text: String,
    pub bounds: Rect,
}

/// One recognized line of text on a page, its box with a bottom-left origin and its words when the recognizer gave them.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrLine {
    pub text: String,
    pub bounds: Rect,
    pub words: Vec<OcrWord>,
}

/// CCITT G4 data of a 1-bit image: black pixels decode to 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct G4Image {
    pub data: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

/// A page image as a PDF stores it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Image {
    /// A JPEG, embedded with /DCTDecode. Its size and color space are read from the JPEG itself.
    Jpeg(Vec<u8>),
    /// A black and white page as CCITT G4.
    G4(G4Image),
    /// A color page as a JPEG background with a G4 text mask over it, painted in black as an image mask.
    Mixed { background: Vec<u8>, mask: G4Image },
}

/// One page to write.
#[derive(Clone, Debug, PartialEq)]
pub struct PageInput {
    pub image: Image,
    /// Full-resolution size in pixels: sizes the media box together with `dpi`.
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub dpi: u32,
    /// Recognized text to embed as an invisible layer.
    pub ocr_lines: Vec<OcrLine>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PageSize {
    /// The size each page was scanned at.
    #[default]
    AsScanned,
    A4,
    Letter,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Conformance {
    /// A plain PDF 1.4 file.
    #[default]
    Standard,
    /// PDF/A-2b for long-term keeping: PDF 1.7, XMP metadata matching the document information, an sRGB output intent
    /// and a file identifier.
    PdfA2b,
}

#[derive(Clone, Debug)]
pub struct Options {
    pub conformance: Conformance,
    /// The document's title; None leaves it out.
    pub title: Option<String>,
    /// When the document was made; None means now.
    pub date: Option<Date>,
    /// The 16-byte file identifier (/ID); None means random bytes.
    pub document_id: Option<[u8; 16]>,
    pub page_size: PageSize,
    /// What the file names as its producer and creator tool.
    pub producer: String,
}

impl Options {
    pub fn new(producer: impl Into<String>) -> Options {
        Options {
            conformance: Conformance::Standard,
            title: None,
            date: None,
            document_id: None,
            page_size: PageSize::AsScanned,
            producer: producer.into(),
        }
    }
}

/// Where a page's image goes on its PDF page, in points: the page's size, and the image's size and lower left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub page_width: f64,
    pub page_height: f64,
    pub width: f64,
    pub height: f64,
    pub x: f64,
    pub y: f64,
}

impl PageSize {
    /// The page for an image of the given size: the image's own size for AsScanned, else the fixed page with the image
    /// scaled down to fit (never up, never cropped) and centered.
    pub fn placement(self, width_pts: f64, height_pts: f64) -> Placement {
        let (page_width, page_height) = match self {
            PageSize::AsScanned => {
                return Placement {
                    page_width: width_pts,
                    page_height: height_pts,
                    width: width_pts,
                    height: height_pts,
                    x: 0.0,
                    y: 0.0,
                };
            }
            PageSize::A4 => (595.0, 842.0),
            PageSize::Letter => (612.0, 792.0),
        };
        let scale = 1.0f64.min((page_width / width_pts).min(page_height / height_pts));
        let width = width_pts * scale;
        let height = height_pts * scale;
        Placement {
            page_width,
            page_height,
            width,
            height,
            x: 0f64.max((page_width - width) / 2.0),
            y: 0f64.max((page_height - height) / 2.0),
        }
    }
}

/// The PDF for `pages`.
pub fn make(pages: &[PageInput], options: &Options) -> Vec<u8> {
    let archival = options.conformance == Conformance::PdfA2b;
    let has_text = pages.iter().any(|p| !p.ocr_lines.is_empty());
    let codes = Codes::for_texts(
        pages
            .iter()
            .flat_map(|p| p.ocr_lines.iter().map(|l| l.text.as_str())),
    );
    let mut w = ObjectWriter::new();
    w.write_header(if archival { "1.7" } else { "1.4" });

    let mut next = 3;
    let font_object = if has_text { next } else { 0 };
    if has_text {
        next += 6;
    }
    let info_object = next;
    next += 1;
    let metadata_object = info_object + 1;
    let profile_object = info_object + 2;
    if archival {
        next += 2;
    }
    let first_page = next;
    let page_object = |i: usize| first_page + i * 3;

    w.write_object(
        1,
        &if archival {
            format!(
                "<</Type /Catalog /Pages 2 0 R /Metadata {metadata_object} 0 R /OutputIntents {}>>",
                pdfa::output_intents(profile_object)
            )
        } else {
            "<</Type /Catalog /Pages 2 0 R>>".to_string()
        },
    );
    let kids: Vec<String> = (0..pages.len())
        .map(|i| format!("{} 0 R", page_object(i)))
        .collect();
    w.write_object(
        2,
        format!(
            "<</Type /Pages /Kids [{}] /Count {}>>",
            kids.join(" "),
            pages.len()
        ),
    );
    if has_text {
        write_text_font(&mut w, font_object, &codes);
    }

    let date = options.date.unwrap_or_else(Date::now);
    let title = options.title.as_deref().map(pdfa::clean_title);
    w.write_object(
        info_object,
        pdfa::info_dictionary(title.as_deref(), &date, &options.producer),
    );
    if archival {
        w.write_stream(
            metadata_object,
            "/Type /Metadata /Subtype /XML",
            &pdfa::xmp(title.as_deref(), &date, &options.producer),
        );
        w.write_stream(profile_object, "/N 3", pdfa::SRGB_PROFILE);
    }

    let mut masks = 0;
    for (i, page) in pages.iter().enumerate() {
        let mask_object = if matches!(page.image, Image::Mixed { .. }) {
            masks += 1;
            first_page + pages.len() * 3 + masks - 1
        } else {
            0
        };
        write_page(
            &mut w,
            [
                page_object(i),
                page_object(i) + 1,
                page_object(i) + 2,
                mask_object,
            ],
            i,
            page,
            font_object,
            &codes,
            options.page_size,
        );
    }

    let id = options.document_id.unwrap_or_else(|| {
        let mut id = [0u8; 16];
        getrandom::fill(&mut id).expect("the system has random bytes");
        id
    });
    let hex: String = id.iter().map(|b| format!("{b:02X}")).collect();
    w.finish(&format!(
        "/Root 1 0 R /Info {info_object} 0 R /ID [<{hex}> <{hex}>]"
    ))
}

/// One page's three objects, `[page, image, contents, mask]` (mask is 0 for a page without one): the page (child of
/// object 2, its image named "Im{index}", the font as /F1 when it has text), the image XObject and the content stream.
fn write_page(
    w: &mut ObjectWriter,
    objects: [usize; 4],
    index: usize,
    page: &PageInput,
    font_object: usize,
    codes: &Codes,
    page_size: PageSize,
) {
    let [page_object, image_object, contents_object, mask_object] = objects;
    let dpi = page.dpi.max(1) as f64;
    let place = page_size.placement(
        page.pixel_width as f64 / dpi * 72.0,
        page.pixel_height as f64 / dpi * 72.0,
    );
    let page_has_text = !page.ocr_lines.is_empty();
    let font_resource = if page_has_text {
        format!(" /Font <</F1 {font_object} 0 R>>")
    } else {
        String::new()
    };
    let mask_resource = if mask_object > 0 {
        format!(" /Mk{index} {mask_object} 0 R")
    } else {
        String::new()
    };
    w.write_object(
        page_object,
        format!(
            "<</Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] /Resources <</XObject <</Im{index} {image_object} 0 R{mask_resource}>>{font_resource}>> /Contents {contents_object} 0 R>>",
            num(place.page_width),
            num(place.page_height)
        ),
    );
    let data = match &page.image {
        Image::Jpeg(data) => data,
        Image::G4(g4) => &g4.data,
        Image::Mixed { background, .. } => background,
    };
    w.write_stream(image_object, image_entries(page, data), data);
    if let Image::Mixed { mask, .. } = &page.image {
        w.write_stream(mask_object, mask_entries(mask), &mask.data);
    }
    let contents = content_stream(index, &place, &page.ocr_lines, codes, mask_object > 0);
    if page_has_text {
        w.write_stream(
            contents_object,
            "/Filter /FlateDecode",
            &zlib_fixed(&contents),
        );
    } else {
        w.write_stream(contents_object, "", &contents);
    }
}

/// The image XObject's dictionary entries (before /Length). A JPEG declares its own frame size, and /DeviceGray when it
/// has one component; one that cannot be read keeps the assumption of an RGB image the size of the page. G4 data is a
/// 1-bit gray image.
fn image_entries(page: &PageInput, data: &[u8]) -> String {
    if let Image::G4(g4) = &page.image {
        return format!(
            "/Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms <</K -1 /Columns {w} /Rows {h} /BlackIs1 false>>",
            w = g4.width,
            h = g4.height
        );
    }
    let (width, height, components) = read_jpeg_header(data).unwrap_or((
        page.pixel_width as usize,
        page.pixel_height as usize,
        3,
    ));
    let color_space = if components == 1 {
        "/DeviceGray"
    } else {
        "/DeviceRGB"
    };
    format!(
        "/Type /XObject /Subtype /Image /Width {width} /Height {height} /ColorSpace {color_space} /BitsPerComponent 8 /Filter /DCTDecode"
    )
}

/// The mask image's dictionary entries: the same G4 data as a black and white page, as an image mask that paints its 0
/// pixels (the ink) in the fill color.
fn mask_entries(mask: &G4Image) -> String {
    format!(
        "/Type /XObject /Subtype /Image /Width {w} /Height {h} /ImageMask true /BitsPerComponent 1 /Filter /CCITTFaxDecode /DecodeParms <</K -1 /Columns {w} /Rows {h} /BlackIs1 false>>",
        w = mask.width,
        h = mask.height
    )
}

/// The text layer's font, objects `first` to first + 5: a Type0 font with Identity-H encoding over a CIDFontType2 whose
/// every glyph is the glyphless font's empty, half-em-wide glyph 1, embedded, with a ToUnicode map giving each code back
/// as its character. The descriptor's /Descent is -1, not the program's 0: macOS' PDFKit reads no text at all from a font
/// whose descriptor and hhea table both give a descent of 0.
fn write_text_font(w: &mut ObjectWriter, first: usize, codes: &Codes) {
    let (cid_font, descriptor, program, cid_to_gid, to_unicode) =
        (first + 1, first + 2, first + 3, first + 4, first + 5);
    let name = glyphless::FONT_NAME;
    w.write_object(
        first,
        format!(
            "<</Type /Font /Subtype /Type0 /BaseFont /{name} /Encoding /Identity-H /DescendantFonts [{cid_font} 0 R] /ToUnicode {to_unicode} 0 R>>"
        ),
    );
    w.write_object(
        cid_font,
        format!(
            "<</Type /Font /Subtype /CIDFontType2 /BaseFont /{name} /CIDSystemInfo <</Registry {} /Ordering {} /Supplement 0>> /FontDescriptor {descriptor} 0 R /DW {} /CIDToGIDMap {cid_to_gid} 0 R>>",
            ascii_string("Adobe"),
            ascii_string("Identity"),
            glyphless::GLYPH_WIDTH
        ),
    );
    w.write_object(
        descriptor,
        format!(
            "<</Type /FontDescriptor /FontName /{name} /Flags 5 /FontBBox [0 0 {} {}] /ItalicAngle 0 /Ascent {} /Descent -1 /CapHeight {} /StemV 80 /FontFile2 {program} 0 R>>",
            glyphless::GLYPH_WIDTH,
            glyphless::UNITS_PER_EM,
            glyphless::UNITS_PER_EM,
            glyphless::UNITS_PER_EM
        ),
    );
    let font = glyphless::program();
    w.write_stream(
        program,
        format!("/Length1 {} /Filter /FlateDecode", font.len()),
        &zlib_fixed(font),
    );
    w.write_stream(
        cid_to_gid,
        "/Filter /FlateDecode",
        glyphless::cid_to_gid_map(),
    );
    w.write_stream(
        to_unicode,
        "/Filter /FlateDecode",
        &zlib_fixed(codes.to_unicode_cmap().as_bytes()),
    );
}

/// The frame size and component count (1 = gray, 3 = color) from a JPEG's start-of-frame segment. Walks the marker
/// segments from SOI by their length fields, so APPn payloads that happen to contain marker-like bytes are skipped, and
/// gives up at SOS/EOI or on any malformed or truncated segment.
pub fn read_jpeg_header(jpeg: &[u8]) -> Option<(usize, usize, usize)> {
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return None;
    }
    let mut pos = 2;
    while pos < jpeg.len() {
        if jpeg[pos] != 0xFF {
            return None;
        }
        // Any number of 0xFF fill bytes may precede a marker code.
        while pos < jpeg.len() && jpeg[pos] == 0xFF {
            pos += 1;
        }
        if pos >= jpeg.len() {
            return None;
        }
        let marker = jpeg[pos];
        pos += 1;
        // Standalone markers carry no length: TEM, RSTn, SOI.
        if marker == 0x01 || (0xD0..=0xD8).contains(&marker) {
            continue;
        }
        // Scan data or the end of the image before any frame header: no size to be had.
        if marker == 0xDA || marker == 0xD9 || marker == 0x00 {
            return None;
        }
        if pos + 2 > jpeg.len() {
            return None;
        }
        let length = (jpeg[pos] as usize) << 8 | jpeg[pos + 1] as usize;
        if length < 2 || pos + length > jpeg.len() {
            return None;
        }
        // SOF0...SOF15, except DHT (C4), JPG (C8) and DAC (CC), which share the range.
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            // Length(2) precision(1) height(2) width(2) components(1) ...
            if length < 8 {
                return None;
            }
            let height = (jpeg[pos + 3] as usize) << 8 | jpeg[pos + 4] as usize;
            let width = (jpeg[pos + 5] as usize) << 8 | jpeg[pos + 6] as usize;
            let components = jpeg[pos + 7] as usize;
            // Height 0 means "defined later by DNL"; treat it as unknown.
            return (width > 0 && height > 0 && components > 0)
                .then_some((width, height, components));
        }
        pos += length;
    }
    None
}

/// What is placed for `line`: one run for each of its words, so a search hit sits on the printed word, or the line itself
/// when it has no word boxes. Every word but the last carries a space and its box reaches the next word, so copied text
/// and phrase search keep the spaces.
fn text_runs(line: &OcrLine) -> Vec<OcrLine> {
    if line.words.is_empty() {
        return vec![line.clone()];
    }
    line.words
        .iter()
        .enumerate()
        .map(|(i, word)| {
            let mut bounds = word.bounds;
            let mut text = word.text.clone();
            if let Some(following) = line.words.get(i + 1) {
                let next = following.bounds.x;
                if next > bounds.x + bounds.width {
                    bounds.width = next - bounds.x;
                }
                text.push(' ');
            }
            OcrLine {
                text,
                bounds,
                words: vec![],
            }
        })
        .collect()
}

/// Where one line's invisible text goes, in points: the font size is the box's height and the baseline its bottom edge,
/// stretched across the box's width by the horizontal scale (Tz / 100, clamped to 0.2 to 5). A rotated line runs down the
/// box: the font size is the box's width, the scale fits its height, and X is the box's left and the baseline its top.
struct PlacedLine {
    font_size: f64,
    horizontal_scale: f64,
    x: f64,
    baseline: f64,
    rotated: bool,
}

/// Whether `line` is written rotated: more than two characters in a box taller than wide. It is decided from the line's
/// own box and its word runs follow, because a short word in a flat line can have a box taller than wide.
fn is_turned(line: &OcrLine, codes: &Codes, width_pts: f64, height_pts: f64) -> bool {
    let bounds = line.bounds.standardized();
    codes.codes(&line.text).len() > 2 && bounds.height * height_pts > bounds.width * width_pts
}

/// The placement of `line` on an image of the given size whose lower left corner is at (`origin_x`, `origin_y`) on the
/// page; None for an empty line or box.
fn text_layer_line(
    line: &OcrLine,
    codes: &Codes,
    width_pts: f64,
    height_pts: f64,
    origin: (f64, f64),
    rotated: bool,
) -> Option<PlacedLine> {
    if line.text.is_empty() {
        return None;
    }
    let bounds = line.bounds.standardized();
    let target_width = bounds.width * width_pts;
    let target_height = bounds.height * height_pts;
    if target_height <= 0.0 || target_width <= 0.0 {
        return None;
    }
    let code_count = codes.codes(&line.text).len();
    let x = origin.0 + bounds.x * width_pts;
    if rotated {
        let measured = glyphless::text_width(code_count, target_width);
        let scale = (target_height / measured).clamp(0.2, 5.0);
        return Some(PlacedLine {
            font_size: target_width,
            horizontal_scale: scale,
            x,
            baseline: origin.1 + line.bounds.max_y() * height_pts,
            rotated: true,
        });
    }
    let baseline = origin.1 + bounds.y * height_pts;
    let measured = glyphless::text_width(code_count, target_height);
    let scale = (target_width / measured).clamp(0.2, 5.0);
    Some(PlacedLine {
        font_size: target_height,
        horizontal_scale: scale,
        x,
        baseline,
        rotated: false,
    })
}

fn content_stream(
    page_index: usize,
    place: &Placement,
    lines: &[OcrLine],
    codes: &Codes,
    masked: bool,
) -> Vec<u8> {
    let mut s = String::new();
    s.push_str(&format!(
        "q\n{} 0 0 {} {} {} cm\n/Im{page_index} Do\n",
        num(place.width),
        num(place.height),
        num(place.x),
        num(place.y)
    ));
    if masked {
        s.push_str(&format!("0 g\n/Mk{page_index} Do\n"));
    }
    s.push_str("Q\n");
    if !lines.is_empty() {
        // Invisible text layer: mode 3 = neither fill nor stroke, but selectable and searchable. The boxes have a
        // bottom-left origin, the same as PDF user space, so they map straight into points with no flip.
        s.push_str("BT\n3 Tr\n");
        for line in lines {
            let turned = is_turned(line, codes, place.width, place.height);
            for run in text_runs(line) {
                let Some(placed) = text_layer_line(
                    &run,
                    codes,
                    place.width,
                    place.height,
                    (place.x, place.y),
                    turned,
                ) else {
                    continue;
                };
                s.push_str(&format!("/F1 {} Tf\n", num(placed.font_size)));
                s.push_str(&format!("{} Tz\n", num(placed.horizontal_scale * 100.0)));
                let matrix = if placed.rotated {
                    "0 -1 1 0"
                } else {
                    "1 0 0 1"
                };
                s.push_str(&format!(
                    "{matrix} {} {} Tm\n",
                    num(placed.x),
                    num(placed.baseline)
                ));
                s.push_str(&codes.hex_string(&run.text));
                s.push_str(" Tj\n");
            }
        }
        s.push_str("ET\n");
    }
    s.into_bytes()
}
