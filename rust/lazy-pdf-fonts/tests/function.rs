// Function checks ported from the reference engines: stitching, sampled data at every bit depth, PostScript calculator operators and their errors.
mod common;

use common::pdf::TestPdf;
use lazy_pdf_fonts::{Function, StandardFilters};
use lazy_pdf_syntax::Document;

fn func(dict: &str, data: Option<&[u8]>) -> Result<std::sync::Arc<Function>, ()> {
    let mut pdf = TestPdf::new();
    pdf.obj(1, "<</Type /Catalog /Pages 2 0 R>>");
    pdf.obj(2, "<</Type /Pages /Kids [] /Count 0>>");
    match data {
        Some(d) => {
            pdf.stream(3, dict, d);
        }
        None => {
            pdf.obj(3, dict);
        }
    }
    let mut doc = Document::open(pdf.finish(), Box::new(StandardFilters)).unwrap();
    let o = doc.get(3).unwrap();
    Function::parse(&mut doc, &o).map_err(|_| ())
}

fn calc(p: &str, domain: &str, range: &str) -> std::sync::Arc<Function> {
    let body = format!("/FunctionType 4 /Domain {domain} /Range {range}");
    func(&body, Some(p.as_bytes())).unwrap()
}

fn ev(f: &Function, x: &[f64]) -> Vec<f64> {
    let mut o = vec![0.0; f.outputs];
    f.evaluate(x, &mut o).unwrap();
    o
}

fn near(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "expected {a}, got {b}");
}

#[test]
fn functions_match_the_reference_checks() {
    let st = "<< /FunctionType 3 /Domain [0 1] /Bounds [0.5] /Encode [0 1 0 1] /Functions [<< /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>] >>";
    let f = func(st, None).unwrap();
    for (x, e) in [(0.0, 0.0), (0.25, 0.5), (0.5, 1.0), (0.75, 0.5), (1.0, 0.0)] {
        near(e, ev(&f, &[x])[0]);
    }
    let f = func("<< /FunctionType 3 /Domain [0 1] /Bounds [] /Encode [1 0] /Functions [<< /FunctionType 2 /Domain [0 1] /N 1 >>] >>", None).unwrap();
    near(0.75, ev(&f, &[0.25])[0]);
    let f = func(
        "/FunctionType 0 /Domain [0 1 0 1] /Range [0 1] /Size [2 2] /BitsPerSample 8",
        Some(&[0, 255, 255, 0]),
    )
    .unwrap();
    near(1.0, ev(&f, &[1.0, 0.0])[0]);
    near(1.0, ev(&f, &[0.0, 1.0])[0]);
    near(0.5, ev(&f, &[0.5, 0.5])[0]);
    near(0.75, ev(&f, &[0.75, 0.0])[0]);
    for (bits, data, last) in [
        (1, vec![0x40u8], 0.0),
        (2, vec![0x34], 1.0 / 3.0),
        (4, vec![0x0F, 0x80], 8.0 / 15.0),
        (12, vec![0x00, 0x0F, 0xFF, 0x80, 0x00], 2048.0 / 4095.0),
        (16, vec![0, 0, 0xFF, 0xFF, 0x80, 0], 32768.0 / 65535.0),
        (24, vec![0, 0, 0, 0xFF, 0xFF, 0xFF, 0x80, 0, 0], 0.5),
        (
            32,
            vec![0, 0, 0, 0, 0xFF, 0xFF, 0xFF, 0xFF, 0x80, 0, 0, 0],
            0.5,
        ),
    ] {
        let f = func(
            &format!("/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [3] /BitsPerSample {bits}"),
            Some(&data),
        )
        .unwrap();
        near(last, ev(&f, &[1.0])[0]);
        near(1.0, ev(&f, &[0.5])[0]);
    }
    let f = func(
        "/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [1] /BitsPerSample 8",
        Some(&[255]),
    )
    .unwrap();
    near(ev(&f, &[0.0])[0], ev(&f, &[f64::NAN])[0]);
    assert!(func("/FunctionType 0 /Domain [0 1 0 1 0 1] /Range [0 1] /Size [2000000 2000000 2000000] /BitsPerSample 32", Some(&[1,2,3])).is_err());
    let t = "[0 10]";
    for (p, e) in [
        ("{ pop 7 2 idiv }", 3.0),
        ("{ pop 7 2 mod }", 1.0),
        ("{ pop 2 3 exp }", 8.0),
        ("{ pop 100 log }", 2.0),
        ("{ pop 2.5 round }", 3.0),
        ("{ pop 3.7 cvi }", 3.0),
        ("{ pop -2.5 truncate abs }", 2.0),
        ("{ pop 1 3 bitshift }", 8.0),
        ("{ pop 5 0 div }", 0.0),
    ] {
        near(e, ev(&calc(p, "[0 1]", t), &[0.0])[0]);
    }
    near(
        1.0,
        ev(&calc("{ pop 1 0 atan 90 div }", "[0 1]", "[0 1]"), &[0.0])[0],
    );
    near(
        0.5,
        ev(&calc("{ pop 0 -1 atan 360 div }", "[0 1]", "[0 1]"), &[0.0])[0],
    );
    assert_eq!(
        ev(
            &calc("{ 3 1 roll }", "[0 1 0 1 0 1]", "[0 1 0 1 0 1]"),
            &[0.1, 0.2, 0.3]
        ),
        vec![0.3, 0.1, 0.2]
    );
    assert_eq!(
        ev(
            &calc("{ 3 -1 roll }", "[0 1 0 1 0 1]", "[0 1 0 1 0 1]"),
            &[0.1, 0.2, 0.3]
        ),
        vec![0.2, 0.3, 0.1]
    );
    assert_eq!(
        ev(
            &calc("{ 1 index }", "[0 1 0 1]", "[0 1 0 1 0 1]"),
            &[0.1, 0.2]
        ),
        vec![0.1, 0.2, 0.1]
    );
    assert_eq!(
        ev(
            &calc("{ 2 copy }", "[0 1 0 1]", "[0 1 0 1 0 1 0 1]"),
            &[0.1, 0.2]
        ),
        vec![0.1, 0.2, 0.1, 0.2]
    );
    near(
        1.0,
        ev(
            &calc(
                "{ 0.5 gt exch 0.5 gt and { 1 } { 0 } ifelse }",
                "[0 1 0 1]",
                "[0 1]",
            ),
            &[0.7, 0.9],
        )[0],
    );
    near(
        1.0,
        ev(
            &calc("{ pop 0 not -1 eq { 1 } { 0 } ifelse }", "[0 1]", "[0 1]"),
            &[0.0],
        )[0],
    );
    near(
        0.5,
        ev(
            &calc(
                "% tint\n{\n  0.5 % half\n  mul 2\r  mul }",
                "[0 1]",
                "[0 1]",
            ),
            &[0.5],
        )[0],
    );
    for bad in [
        "{ pop pop }",
        "{ 5 index }",
        "{ 2 copy 5 copy }",
        "{ exch }",
        "{ add }",
    ] {
        let f = calc(bad, "[0 1]", "[0 1]");
        let mut o = vec![0.0; 1];
        assert!(f.evaluate(&[0.5], &mut o).is_err(), "{bad}");
    }
    for bad in ["{ foo }", "{ 1 2", "1 2 }", "{ 1 if }"] {
        assert!(
            func(
                "/FunctionType 4 /Domain [0 1] /Range [0 1]",
                Some(bad.as_bytes())
            )
            .is_err(),
            "{bad}"
        );
    }
}
