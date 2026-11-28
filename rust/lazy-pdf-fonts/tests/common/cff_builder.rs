//! Assembles small CFF programs by hand so the charstring interpreter is tested without a font file.

#[derive(Clone)]
pub enum Part {
    Number(i64),
    Op(String),
    Raw(Vec<u8>),
}

pub use Part::Number as N;

pub fn o(name: &str) -> Part {
    Part::Op(name.to_string())
}

fn num(v: i64) -> Vec<u8> {
    if (-107..=107).contains(&v) {
        vec![(v + 139) as u8]
    } else if (108..=1131).contains(&v) {
        vec![((v - 108) / 256 + 247) as u8, ((v - 108) % 256) as u8]
    } else {
        vec![28, (v >> 8) as u8, v as u8]
    }
}

/// Numbers and operators: numbers become operands, strings are operator names.
pub fn code(parts: &[Part]) -> Vec<u8> {
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

pub fn op(name: &str) -> Vec<u8> {
    match name {
        "hstem" => vec![1],
        "vstem" => vec![3],
        "vmoveto" => vec![4],
        "rlineto" => vec![5],
        "hlineto" => vec![6],
        "vlineto" => vec![7],
        "rrcurveto" => vec![8],
        "callsubr" => vec![10],
        "return" => vec![11],
        "endchar" => vec![14],
        "hstemhm" => vec![18],
        "hintmask" => vec![19],
        "rmoveto" => vec![21],
        "hmoveto" => vec![22],
        "vstemhm" => vec![23],
        "rcurveline" => vec![24],
        "rlinecurve" => vec![25],
        "vvcurveto" => vec![26],
        "hhcurveto" => vec![27],
        "callgsubr" => vec![29],
        "vhcurveto" => vec![30],
        "hvcurveto" => vec![31],
        "flex" => vec![12, 35],
        "hflex" => vec![12, 34],
        "hflex1" => vec![12, 36],
        "flex1" => vec![12, 37],
        "add" => vec![12, 10],
        "sub" => vec![12, 11],
        "mul" => vec![12, 24],
        "div" => vec![12, 12],
        "dup" => vec![12, 27],
        "exch" => vec![12, 28],
        "drop" => vec![12, 18],
        other => panic!("{other}"),
    }
}

fn u16(v: usize) -> Vec<u8> {
    vec![(v >> 8) as u8, v as u8]
}

fn index(items: &[&[u8]]) -> Vec<u8> {
    if items.is_empty() {
        return vec![0, 0];
    }
    let mut bytes = u16(items.len());
    bytes.push(2);
    let mut offset = 1;
    bytes.extend(u16(offset));
    for item in items {
        offset += item.len();
        bytes.extend(u16(offset));
    }
    for item in items {
        bytes.extend(*item);
    }
    bytes
}

fn int(v: i64) -> Vec<u8> {
    vec![
        29,
        (v >> 24) as u8,
        (v >> 16) as u8,
        (v >> 8) as u8,
        v as u8,
    ]
}

/// A DICT with every operand as a five-byte integer, so its size does not depend on the values.
fn dict(entries: &[(i64, &[i64])]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (op, operands) in entries {
        for v in *operands {
            bytes.extend(int(*v));
        }
        if *op >= 1200 {
            bytes.extend([12, (op - 1200) as u8]);
        } else {
            bytes.push(*op as u8);
        }
    }
    bytes
}

fn concat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

fn square() -> Vec<u8> {
    code(&[
        N(100),
        N(50),
        N(0),
        o("rmoveto"),
        N(100),
        N(0),
        N(0),
        N(100),
        N(-100),
        N(0),
        o("rlineto"),
        o("endchar"),
    ])
}

fn curve() -> Vec<u8> {
    code(&[
        N(10),
        N(20),
        o("rmoveto"),
        N(-107),
        o("callsubr"),
        o("endchar"),
    ])
}

fn curve_subr() -> Vec<u8> {
    code(&[
        N(30),
        N(0),
        N(0),
        N(30),
        N(30),
        N(0),
        o("rrcurveto"),
        o("return"),
    ])
}

fn masked() -> Vec<u8> {
    code(&[
        N(0),
        N(10),
        N(20),
        N(10),
        o("hstemhm"),
        o("hintmask"),
        Part::Raw(vec![0xC0]),
        N(100),
        N(100),
        o("rmoveto"),
        N(10),
        N(20),
        o("hlineto"),
        o("endchar"),
    ])
}

const PRIVATE_SIZE: i64 = 18;

/// Glyphs: .notdef, A (width 100 above the nominal), B (a curve through a local subroutine), "custom" (hint
/// mask). Name-keyed: nominal width 500, default 300.
pub fn name_keyed(subr: Option<Vec<u8>>) -> Vec<u8> {
    let charset = concat(&[&[0], &u16(34), &u16(35), &u16(391)]);
    let glyphs = index(&[&[14], &square(), &curve(), &masked()]);
    let subrs = subr.unwrap_or_else(curve_subr);
    let private = concat(&[
        &dict(&[(20, &[300]), (21, &[500]), (19, &[PRIVATE_SIZE])]),
        &index(&[&subrs]),
    ]);

    let head = |charset_at: i64, glyphs_at: i64, private_at: i64| -> Vec<u8> {
        concat(&[
            &[1, 0, 4, 2],
            &index(&[b"Test"]),
            &index(&[&dict(&[
                (15, &[charset_at]),
                (17, &[glyphs_at]),
                (18, &[PRIVATE_SIZE, private_at]),
                (5, &[0, 0, 200, 200]),
            ])]),
            &index(&[b"custom"]),
            &index(&[]),
        ])
    };

    let h = head(0, 0, 0).len() as i64;
    let (c, g) = (charset.len() as i64, glyphs.len() as i64);
    concat(&[&head(h, h + c, h + c + g), &charset, &glyphs, &private])
}

/// CID-keyed: glyphs .notdef, CID 10 (square, FD 0: nominal 500), CID 11 (curve, FD 1: default 350, nominal 700,
/// local subr) and CID 500 (hint mask, FD 1).
pub fn cid_keyed(top_matrix: Option<&[i64]>, fd_matrix: Option<&[i64]>) -> Vec<u8> {
    let charset = concat(&[&[0], &u16(10), &u16(11), &u16(500)]);
    let select = concat(&[&[3], &u16(2), &u16(0), &[0], &u16(2), &[1], &u16(4)]);
    let glyphs = index(&[&[14], &square(), &curve(), &masked()]);
    let priv0 = dict(&[(20, &[250]), (21, &[500])]);
    let priv1 = concat(&[
        &dict(&[(20, &[350]), (21, &[700]), (19, &[PRIVATE_SIZE])]),
        &index(&[&curve_subr()]),
    ]);

    let head = |charset_at: i64, select_at: i64, glyphs_at: i64, fd_array_at: i64| -> Vec<u8> {
        let mut top = dict(&[
            (1230, &[391, 392, 0]),
            (15, &[charset_at]),
            (17, &[glyphs_at]),
            (1236, &[fd_array_at]),
            (1237, &[select_at]),
        ]);
        if let Some(m) = top_matrix {
            top.extend(dict(&[(1207, m)]));
        }
        concat(&[
            &[1, 0, 4, 2],
            &index(&[b"Cid"]),
            &index(&[&top]),
            &index(&[b"Adobe", b"Identity"]),
            &index(&[]),
        ])
    };

    let fd_array = |at0: i64, at1: i64| -> Vec<u8> {
        let mut first = dict(&[(18, &[priv0.len() as i64, at0])]);
        if let Some(m) = fd_matrix {
            first.extend(dict(&[(1207, m)]));
        }
        index(&[&first, &dict(&[(18, &[PRIVATE_SIZE, at1])])])
    };

    let h = head(0, 0, 0, 0).len();
    let select_at = h + charset.len();
    let glyphs_at = select_at + select.len();
    let priv0_at = glyphs_at + glyphs.len();
    let priv1_at = priv0_at + priv0.len();
    let fd_array_at = priv1_at + priv1.len();
    concat(&[
        &head(
            h as i64,
            select_at as i64,
            glyphs_at as i64,
            fd_array_at as i64,
        ),
        &charset,
        &select,
        &glyphs,
        &priv0,
        &priv1,
        &fd_array(priv0_at as i64, priv1_at as i64),
    ])
}
