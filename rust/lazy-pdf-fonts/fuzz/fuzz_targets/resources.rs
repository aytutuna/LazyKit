#![no_main]

use lazy_pdf_fonts::{ContentInterpreter, ContentVisitor, Function, StandardFilters};
use lazy_pdf_syntax::{Document, Object};
use libfuzzer_sys::fuzz_target;

struct Quiet;

impl ContentVisitor for Quiet {}

// Colour spaces over functions, a tiling pattern, an ExtGState with a soft mask and a font, all made of the input.
fn page(content: &[u8], program: &[u8]) -> Vec<u8> {
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    let mut add = |pdf: &mut Vec<u8>, number: u32, body: &[u8]| {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{number} 0 obj\n").as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    };
    let stream = |entries: &str, data: &[u8]| {
        let mut out = format!("<<{entries} /Length {}>>\nstream\n", data.len()).into_bytes();
        out.extend_from_slice(data);
        out.extend_from_slice(b"\nendstream");
        out
    };
    add(&mut pdf, 1, b"<</Type /Catalog /Pages 2 0 R>>");
    add(
        &mut pdf,
        2,
        b"<</Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 200 200]>>",
    );
    add(
        &mut pdf,
        3,
        b"<</Type /Page /Parent 2 0 R /Contents 4 0 R /Resources <</Font <</F1 5 0 R>> /ColorSpace <</S1 [/Separation /X /DeviceRGB 7 0 R] /S2 [/Separation /Y /DeviceGray 9 0 R] /N1 [/DeviceN [/A /B] /DeviceRGB 10 0 R] /I1 [/Indexed [/Separation /X /DeviceRGB 7 0 R] 15 <000102030405060708090a0b0c0d0e0f>] /P [/Pattern /DeviceRGB]>> /Pattern <</P1 8 0 R /P2 11 0 R>> /ExtGState <</G1 <</CA 0.5 /ca 0.5 /LW 2 /D [[1 2] 0] /BM [/Multiply] /Font [5 0 R 12] /SMask <</S /Luminosity /G 12 0 R>>>>>>>>>>",
    );
    add(&mut pdf, 4, &stream("", content));
    add(
        &mut pdf,
        5,
        b"<</Type /Font /Subtype /Type1 /BaseFont /Helvetica>>",
    );
    add(
        &mut pdf,
        7,
        &stream(
            "/FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1]",
            program,
        ),
    );
    add(
        &mut pdf,
        8,
        &stream(
            "/Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] /XStep 10 /YStep 10 /Resources <<>>",
            content,
        ),
    );
    add(
        &mut pdf,
        9,
        &stream(
            "/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [4] /BitsPerSample 8",
            program,
        ),
    );
    add(
        &mut pdf,
        10,
        b"<</FunctionType 2 /Domain [0 1 0 1] /C0 [0 0 0] /C1 [1 1 1] /N 1>>",
    );
    add(
        &mut pdf,
        11,
        b"<</Type /Pattern /PatternType 2 /Shading <</ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 1 1] /Function 7 0 R>>>>",
    );
    add(
        &mut pdf,
        12,
        &stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 200 200] /Group <</S /Transparency /CS /DeviceRGB>>",
            content,
        ),
    );
    let start = pdf.len();
    pdf.extend_from_slice(b"xref\n0 13\n0000000000 65535 f \n");
    let mut at = offsets.iter();
    for number in 1..13u32 {
        if [6].contains(&number) {
            pdf.extend_from_slice(b"0000000000 00000 f \n");
        } else {
            let offset = at.next().copied().unwrap_or(0);
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
    }
    pdf.extend_from_slice(
        format!("trailer\n<</Size 13 /Root 1 0 R>>\nstartxref\n{start}\n%%EOF\n").as_bytes(),
    );
    pdf
}

fuzz_target!(|data: &[u8]| {
    let (content, program) = data.split_at(data.len() / 2);
    let Ok(mut doc) = Document::open(page(content, program), Box::new(StandardFilters)) else {
        return;
    };
    for number in [7, 9, 10, 11] {
        if let Some(function) = doc.get(number)
            && !matches!(&*function, Object::Null)
            && let Ok(function) = Function::parse(&mut doc, &function)
        {
            let mut out = vec![0.0; function.outputs];
            let _ = function.evaluate(&[0.25, 0.5], &mut out);
        }
    }
    let pages = doc.pages();
    if let Some(first) = pages.first() {
        let mut visitor = Quiet;
        ContentInterpreter::new(&mut doc, &mut visitor).run_page(first);
    }
});
