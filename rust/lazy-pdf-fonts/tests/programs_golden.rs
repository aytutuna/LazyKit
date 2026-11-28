mod common;

use common::{check, s};
use lazy_pdf_fonts::{cff_strings, mac_glyphs, standard_encoding, standard_encoding_sids};

fn standard_names_dump() -> String {
    let mut out = String::new();
    for (i, name) in cff_strings().iter().enumerate() {
        out += &format!("cff {i} {name}\n");
    }
    for (i, name) in mac_glyphs().iter().enumerate() {
        out += &format!("mac {i} {name}\n");
    }
    for c in 0..256 {
        out += &format!(
            "std {c} {} {}\n",
            s(standard_encoding(c)),
            standard_encoding_sids()[c]
        );
    }
    out
}

#[test]
fn standard_names_are_the_golden_file() {
    check("programs/standard-names-output.txt", &standard_names_dump());
}
