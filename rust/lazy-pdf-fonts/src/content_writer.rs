//! Content stream bytes from operations: parsed ones written back, or new ones built with the helpers below.
//! Numbers are written the way the syntax crate writes them, at most six decimals with no exponent.

use lazy_pdf_syntax::{Dict, Name, Number, Object, PdfString};

use crate::content_parser::ContentOperation;

#[derive(Debug, Default)]
pub struct ContentWriter {
    bytes: Vec<u8>,
}

impl ContentWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    /// The operations written out one to a line; parsing the result gives the same operators and operands.
    pub fn write(operations: &[ContentOperation]) -> Vec<u8> {
        let mut writer = Self::new();
        for operation in operations {
            writer.add(operation);
        }
        writer.into_bytes()
    }

    pub fn add(&mut self, operation: &ContentOperation) -> &mut Self {
        let Some(image) = &operation.image else {
            return self.op(&operation.operator, &operation.operands);
        };
        self.bytes.extend_from_slice(b"BI");
        for (key, value) in image.dictionary.entries() {
            self.bytes.push(b' ');
            key.write_to(&mut self.bytes);
            self.bytes.push(b' ');
            value.write_to(&mut self.bytes);
        }
        self.bytes.extend_from_slice(b"\nID\n");
        self.bytes.extend_from_slice(&image.data);
        self.bytes.extend_from_slice(b"\nEI\n");
        self
    }

    /// One operator with its operands.
    pub fn op(&mut self, name: &str, operands: &[Object]) -> &mut Self {
        for operand in operands {
            operand.write_to(&mut self.bytes);
            self.bytes.push(b' ');
        }
        self.bytes.extend_from_slice(name.as_bytes());
        self.bytes.push(b'\n');
        self
    }

    pub fn numbers(&mut self, name: &str, numbers: &[f64]) -> &mut Self {
        let operands: Vec<Object> = numbers.iter().map(|&n| Object::real(n)).collect();
        self.op(name, &operands)
    }

    pub fn save(&mut self) -> &mut Self {
        self.op("q", &[])
    }

    pub fn restore(&mut self) -> &mut Self {
        self.op("Q", &[])
    }

    pub fn concat(&mut self, m: [f64; 6]) -> &mut Self {
        self.numbers("cm", &m)
    }

    pub fn rect(&mut self, x: f64, y: f64, width: f64, height: f64) -> &mut Self {
        self.numbers("re", &[x, y, width, height])
    }

    pub fn move_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.numbers("m", &[x, y])
    }

    pub fn line_to(&mut self, x: f64, y: f64) -> &mut Self {
        self.numbers("l", &[x, y])
    }

    pub fn curve_to(&mut self, p: [f64; 6]) -> &mut Self {
        self.numbers("c", &p)
    }

    pub fn close(&mut self) -> &mut Self {
        self.op("h", &[])
    }

    pub fn fill(&mut self) -> &mut Self {
        self.op("f", &[])
    }

    pub fn fill_even_odd(&mut self) -> &mut Self {
        self.op("f*", &[])
    }

    pub fn stroke(&mut self) -> &mut Self {
        self.op("S", &[])
    }

    pub fn fill_and_stroke(&mut self) -> &mut Self {
        self.op("B", &[])
    }

    pub fn end_path(&mut self) -> &mut Self {
        self.op("n", &[])
    }

    /// Clips to the current path and ends it.
    pub fn clip(&mut self) -> &mut Self {
        self.op("W", &[]).op("n", &[])
    }

    pub fn line_width(&mut self, width: f64) -> &mut Self {
        self.numbers("w", &[width])
    }

    pub fn dash(&mut self, on: f64, off: f64) -> &mut Self {
        let pattern = Object::Array(vec![Object::real(on), Object::real(off)]);
        self.op("d", &[pattern, Object::int(0)])
    }

    pub fn fill_gray(&mut self, g: f64) -> &mut Self {
        self.numbers("g", &[g])
    }

    pub fn fill_rgb(&mut self, r: f64, g: f64, b: f64) -> &mut Self {
        self.numbers("rg", &[r, g, b])
    }

    pub fn fill_cmyk(&mut self, c: f64, m: f64, y: f64, k: f64) -> &mut Self {
        self.numbers("k", &[c, m, y, k])
    }

    pub fn stroke_gray(&mut self, g: f64) -> &mut Self {
        self.numbers("G", &[g])
    }

    pub fn stroke_rgb(&mut self, r: f64, g: f64, b: f64) -> &mut Self {
        self.numbers("RG", &[r, g, b])
    }

    pub fn stroke_cmyk(&mut self, c: f64, m: f64, y: f64, k: f64) -> &mut Self {
        self.numbers("K", &[c, m, y, k])
    }

    /// Selects the /ExtGState resource `name`.
    pub fn graphics_state(&mut self, name: &str) -> &mut Self {
        self.op("gs", &[Object::name(name)])
    }

    pub fn begin_text(&mut self) -> &mut Self {
        self.op("BT", &[])
    }

    pub fn end_text(&mut self) -> &mut Self {
        self.op("ET", &[])
    }

    pub fn font(&mut self, name: &str, size: f64) -> &mut Self {
        self.op("Tf", &[Object::name(name), Object::real(size)])
    }

    /// Moves to the start of the next line, offset from the start of the current one.
    pub fn move_text(&mut self, x: f64, y: f64) -> &mut Self {
        self.numbers("Td", &[x, y])
    }

    pub fn text_matrix(&mut self, m: [f64; 6]) -> &mut Self {
        self.numbers("Tm", &m)
    }

    pub fn text_render_mode(&mut self, mode: i64) -> &mut Self {
        self.op("Tr", &[Object::int(mode)])
    }

    pub fn char_spacing(&mut self, spacing: f64) -> &mut Self {
        self.numbers("Tc", &[spacing])
    }

    pub fn word_spacing(&mut self, spacing: f64) -> &mut Self {
        self.numbers("Tw", &[spacing])
    }

    /// Shows text already encoded for the current font, as a hex string.
    pub fn show(&mut self, encoded: &[u8]) -> &mut Self {
        let text = PdfString {
            bytes: encoded.to_vec(),
            hex: true,
        };
        self.op("Tj", &[Object::String(text)])
    }

    /// Paints the /XObject resource `name`.
    pub fn paint(&mut self, name: &str) -> &mut Self {
        self.op("Do", &[Object::name(name)])
    }

    /// Paints an image or form so that its unit square fills the matrix's parallelogram.
    pub fn paint_placed(&mut self, name: &str, placement: [f64; 6]) -> &mut Self {
        self.save().concat(placement).paint(name).restore()
    }

    pub fn begin_marked(&mut self, tag: &str) -> &mut Self {
        self.op("BMC", &[Object::name(tag)])
    }

    /// Opens marked content with an inline property dictionary holding /MCID.
    pub fn begin_marked_id(&mut self, tag: &str, mcid: i64) -> &mut Self {
        let mut properties = Dict::new();
        properties.add(Name::new("MCID"), Object::Number(Number::from_i64(mcid)));
        self.op("BDC", &[Object::name(tag), Object::Dict(properties)])
    }

    pub fn end_marked(&mut self) -> &mut Self {
        self.op("EMC", &[])
    }

    /// The bytes as they are, for an operator the helpers do not cover.
    pub fn raw(&mut self, text: &[u8]) -> &mut Self {
        self.bytes.extend_from_slice(text);
        if text.last() != Some(&b'\n') {
            self.bytes.push(b'\n');
        }
        self
    }
}
