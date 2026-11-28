//! Writes a small Type 1 font: an encrypted private part with Subrs for flex and glyphs that exercise each
//! charstring operator.

use super::cff_builder::{N, Part, o};

#[derive(Clone, Copy)]
pub enum Wrapper {
    Binary,
    Hex,
    Pfb,
}

fn num(v: i64) -> Vec<u8> {
    if (-107..=107).contains(&v) {
        vec![(v + 139) as u8]
    } else if (108..=1131).contains(&v) {
        vec![((v - 108) / 256 + 247) as u8, ((v - 108) % 256) as u8]
    } else if (-1131..=-108).contains(&v) {
        vec![((-v - 108) / 256 + 251) as u8, ((-v - 108) % 256) as u8]
    } else {
        vec![
            255,
            (v >> 24) as u8,
            (v >> 16) as u8,
            (v >> 8) as u8,
            v as u8,
        ]
    }
}

fn op(name: &str) -> Vec<u8> {
    match name {
        "hstem" => vec![1],
        "vstem" => vec![3],
        "vmoveto" => vec![4],
        "rlineto" => vec![5],
        "hlineto" => vec![6],
        "vlineto" => vec![7],
        "rrcurveto" => vec![8],
        "closepath" => vec![9],
        "callsubr" => vec![10],
        "return" => vec![11],
        "hsbw" => vec![13],
        "endchar" => vec![14],
        "rmoveto" => vec![21],
        "hmoveto" => vec![22],
        "vhcurveto" => vec![30],
        "hvcurveto" => vec![31],
        "dotsection" => vec![12, 0],
        "seac" => vec![12, 6],
        "sbw" => vec![12, 7],
        "div" => vec![12, 12],
        "callothersubr" => vec![12, 16],
        "pop" => vec![12, 17],
        "setcurrentpoint" => vec![12, 33],
        other => panic!("{other}"),
    }
}

fn code(parts: &[Part]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for part in parts {
        match part {
            Part::Number(v) => bytes.extend(num(*v)),
            Part::Op(name) => bytes.extend(op(name)),
            Part::Raw(raw) => bytes.extend(raw),
        }
    }
    bytes
}

fn with(list: &[Part], extra: &[Part]) -> Vec<Part> {
    list.iter().chain(extra).cloned().collect()
}

pub fn encrypt(plain: &[u8], key: u16, prefix: usize) -> Vec<u8> {
    let mut data = vec![0u8; prefix];
    data.extend(plain);
    let mut r = key;
    for byte in &mut data {
        let c = *byte ^ (r >> 8) as u8;
        r = (u16::from(c).wrapping_add(r))
            .wrapping_mul(52845)
            .wrapping_add(22719);
        *byte = c;
    }
    data
}

fn subrs() -> Vec<Vec<u8>> {
    vec![
        code(&[
            N(3),
            N(0),
            o("callothersubr"),
            o("pop"),
            o("pop"),
            o("setcurrentpoint"),
            o("return"),
        ]),
        code(&[N(0), N(1), o("callothersubr"), o("return")]),
        code(&[N(0), N(2), o("callothersubr"), o("return")]),
        code(&[o("return")]),
    ]
}

pub const GLYPH_NAMES: [&str; 8] = [".notdef", "A", "B", "C", "D", "E", "F", "G"];

fn glyphs() -> Vec<Vec<u8>> {
    let mut flex = vec![
        N(0),
        N(500),
        o("hsbw"),
        N(10),
        N(10),
        o("rmoveto"),
        N(1),
        o("callsubr"),
    ];
    for (dx, dy) in [
        (50, 0),
        (-30, 0),
        (10, 5),
        (10, 0),
        (10, 0),
        (10, -5),
        (10, 0),
    ] {
        flex = with(&flex, &[N(dx), N(dy), o("rmoveto"), N(2), o("callsubr")]);
    }
    flex = with(
        &flex,
        &[
            N(50),
            N(80),
            N(10),
            N(0),
            o("callsubr"),
            N(0),
            N(100),
            o("rlineto"),
            o("closepath"),
            o("endchar"),
        ],
    );

    vec![
        code(&[N(0), N(250), o("hsbw"), o("endchar")]),
        code(&with(
            &[N(50), N(600)],
            &[
                o("hsbw"),
                N(0),
                N(0),
                o("rmoveto"),
                N(100),
                o("hlineto"),
                N(100),
                o("vlineto"),
                N(-100),
                o("hlineto"),
                o("closepath"),
                o("endchar"),
            ],
        )),
        code(&flex),
        code(&[
            N(0),
            N(700),
            o("hsbw"),
            N(20),
            N(30),
            N(40),
            N(65),
            N(66),
            o("seac"),
        ]),
        code(&[
            N(0),
            N(400),
            o("hsbw"),
            N(0),
            N(0),
            o("rmoveto"),
            N(100),
            N(2),
            o("div"),
            o("hlineto"),
            o("closepath"),
            o("endchar"),
        ]),
        code(&[
            N(0),
            N(300),
            o("hsbw"),
            N(0),
            N(0),
            o("rmoveto"),
            N(3),
            N(1),
            N(3),
            o("callothersubr"),
            o("pop"),
            o("callsubr"),
            N(10),
            o("hlineto"),
            o("closepath"),
            o("endchar"),
        ]),
        code(&[
            N(10),
            N(20),
            N(300),
            N(0),
            o("sbw"),
            N(0),
            N(0),
            o("rmoveto"),
            N(5),
            o("vlineto"),
            o("closepath"),
            o("endchar"),
        ]),
        code(&[
            N(0),
            N(400),
            o("hsbw"),
            N(0),
            N(0),
            o("rmoveto"),
            N(10),
            N(0),
            N(0),
            N(10),
            N(10),
            N(0),
            o("rrcurveto"),
            N(5),
            N(10),
            N(10),
            N(5),
            o("vhcurveto"),
            N(3),
            N(4),
            N(5),
            N(6),
            o("hvcurveto"),
            o("closepath"),
            o("endchar"),
        ]),
    ]
}

fn segment(kind: u8, data: &[u8]) -> Vec<u8> {
    let n = data.len();
    let mut out = vec![
        0x80,
        kind,
        n as u8,
        (n >> 8) as u8,
        (n >> 16) as u8,
        (n >> 24) as u8,
    ];
    out.extend(data);
    out
}

/// `len_iv`: random bytes in front of each charstring; -1 leaves them unencrypted. `bar_names`: use the -| |- |
/// names of the usual font programs for RD, ND and NP.
pub fn build(wrapper: Wrapper, len_iv: i64, bar_names: bool) -> Vec<u8> {
    let rd = if bar_names { "-|" } else { "RD" };
    let nd = if bar_names { "|-" } else { "ND" };
    let np = if bar_names { "|" } else { "NP" };

    let clear = ("%!PS-AdobeFont-1.0: TestFont 001.000\n11 dict begin\n".to_string()
        + "/FontInfo 2 dict dup begin /Notice (Test /Encoding in a string) readonly def end readonly def\n"
        + "/FontName /TestFont def\n/PaintType 0 def\n/FontType 1 def\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n"
        + "/Encoding 256 array\n0 1 255 {1 index exch /.notdef put} for\ndup 65 /A put\ndup 66 /B put\ndup 8#103 /C put\nreadonly def\n"
        + "/FontBBox {-10 -20 900 800} readonly def\ncurrentdict end\ncurrentfile eexec\n")
        .into_bytes();

    let mut private: Vec<u8> = Vec::new();
    let lock = |cs: &[u8]| -> Vec<u8> {
        if len_iv < 0 {
            cs.to_vec()
        } else {
            encrypt(cs, 4330, len_iv as usize)
        }
    };

    private.extend(format!("dup /Private 8 dict dup begin\n/{rd} {{string currentfile exch readstring pop}} executeonly def\n").bytes());
    private.extend(format!("/{nd} {{noaccess def}} executeonly def\n/{np} {{noaccess put}} executeonly def\n/BlueValues [] def\n/password 5839 def\n").bytes());
    if len_iv != 4 {
        private.extend(format!("/lenIV {len_iv} def\n").bytes());
    }
    let subrs = subrs();
    private.extend(format!("/Subrs {} array\n", subrs.len()).bytes());
    for (i, subr) in subrs.iter().enumerate() {
        let cs = lock(subr);
        private.extend(format!("dup {i} {} {rd} ", cs.len()).bytes());
        private.extend(&cs);
        private.extend(format!(" {np}\n").bytes());
    }
    private.extend(
        format!(
            "{nd}\n2 index /CharStrings {} dict dup begin\n",
            GLYPH_NAMES.len()
        )
        .bytes(),
    );
    for (name, glyph) in GLYPH_NAMES.iter().zip(glyphs()) {
        let cs = lock(&glyph);
        private.extend(format!("/{name} {} {rd} ", cs.len()).bytes());
        private.extend(&cs);
        private.extend(format!(" {nd}\n").bytes());
    }
    private.extend(
        b"end\nend\nreadonly put\nnoaccess put\ndup /FontName get exch definefont pop\nmark currentfile closefile\n",
    );

    let encrypted = encrypt(&private, 55665, 4);
    let zeros = "0".repeat(64);
    let trailer = ((zeros + "\n").repeat(8) + "cleartomark\n").into_bytes();

    match wrapper {
        Wrapper::Hex => {
            let hex: Vec<u8> = encrypted
                .iter()
                .flat_map(|b| format!("{b:02X}").into_bytes())
                .collect();
            let mut body = Vec::new();
            for (i, line) in hex.chunks(64).enumerate() {
                if i > 0 {
                    body.push(10);
                }
                body.extend(line);
            }
            body.push(10);
            [clear, body, trailer].concat()
        }
        Wrapper::Pfb => [
            segment(1, &clear),
            segment(2, &encrypted),
            segment(1, &trailer),
            vec![0x80, 3],
        ]
        .concat(),
        Wrapper::Binary => [clear, encrypted, trailer].concat(),
    }
}
