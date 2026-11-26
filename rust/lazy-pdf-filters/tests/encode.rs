use lazy_pdf_filters::{DEFAULT_MAX_SIZE, Filter, Params, Value, decode, encode};

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn fnv(bytes: &[u8]) -> String {
    let mut h = 2166136261u32;
    for &b in bytes {
        h = (h ^ b as u32).wrapping_mul(16777619);
    }
    format!("{h:08x}")
}

fn undo(name: &str, data: &[u8]) -> Vec<u8> {
    decode(data, &[Filter::new(name, None)], DEFAULT_MAX_SIZE).unwrap()
}

fn undo_lzw(data: &[u8], early: bool) -> Vec<u8> {
    let mut parms = Params::new();
    parms.set("EarlyChange", Value::Int(early as i64));
    decode(
        data,
        &[Filter::new("LZWDecode", Some(parms))],
        DEFAULT_MAX_SIZE,
    )
    .unwrap()
}

// Pseudo-random bytes of four values, so that the LZW table fills and clears.
fn big() -> Vec<u8> {
    let mut x = 1u32;
    (0..60000)
        .map(|_| {
            x = x.wrapping_mul(1103515245).wrapping_add(12345);
            ((x >> 16 & 3) * 61) as u8
        })
        .collect()
}

fn samples() -> Vec<Vec<u8>> {
    let mut mixed = b"hello hello hello aaaaaaaaaabcdefg".to_vec();
    mixed.extend([0; 7]);
    mixed.extend(b"xyz");
    let mut ramp: Vec<u8> = vec![7; 150];
    ramp.extend((150..300).map(|i| (i * 13 % 5) as u8));
    vec![
        vec![],
        vec![0x41],
        b"abc".to_vec(),
        vec![0; 4],
        vec![0; 5],
        mixed,
        ramp,
        (0..=255).collect(),
        vec![9; 1000],
        big(),
    ]
}

#[test]
fn encoded_data_decodes_to_the_input() {
    for data in samples() {
        assert_eq!(undo("FlateDecode", &encode::flate(&data)), data);
        assert_eq!(undo("ASCIIHexDecode", &encode::ascii_hex(&data)), data);
        assert_eq!(undo("ASCII85Decode", &encode::ascii85(&data)), data);
        assert_eq!(undo("RunLengthDecode", &encode::run_length(&data)), data);
        assert_eq!(undo_lzw(&encode::lzw(&data, true), true), data);
        assert_eq!(undo_lzw(&encode::lzw(&data, false), false), data);
    }
}

#[test]
fn flate_output_is_zlib_data() {
    let packed = encode::flate(&[5; 10000]);
    assert_eq!(packed[0] & 0x0f, 8);
    assert_eq!((packed[0] as u32 * 256 + packed[1] as u32) % 31, 0);
    assert!(packed.len() < 100);
    assert_eq!(encode::flate(&[]).len(), 8);
}

// The expected bytes are what Lazy PDF's C# encoders write.
#[test]
fn text_filters_write_the_same_bytes_as_the_reference() {
    let abc = b"abc";
    assert_eq!(encode::ascii_hex(&[]), b">");
    assert_eq!(hex(&encode::ascii_hex(abc)), "3631363236333e");
    assert_eq!(encode::ascii_hex(&[0xab, 0x0f]), b"AB0F>");
    assert_eq!(encode::ascii85(&[]), b"~>");
    assert_eq!(hex(&encode::ascii85(&[0x41])), "356c7e3e");
    assert_eq!(hex(&encode::ascii85(abc)), "403a455e7e3e");
    assert_eq!(encode::ascii85(&[0; 4]), b"z~>");
    assert_eq!(encode::ascii85(&[0; 5]), b"z!!~>");
    assert_eq!(hex(&encode::run_length(&[])), "80");
    assert_eq!(hex(&encode::run_length(&[0x41])), "004180");
    assert_eq!(hex(&encode::run_length(abc)), "0261626380");
    assert_eq!(hex(&encode::run_length(&[0; 5])), "fc0080");
    let mut mixed = b"hello hello hello aaaaaaaaaabcdefg".to_vec();
    mixed.extend([0; 7]);
    mixed.extend(b"xyz");
    assert_eq!(
        hex(&encode::ascii85(&mixed)),
        "424f752172445d695f294369222334424f752172445d694973403a3c5351403a3c5351405569715b416e4d29447a212d6964387e3e"
    );
    assert_eq!(
        hex(&encode::run_length(&mixed)),
        "016865ff6c036f206865ff6c036f206865ff6c016f20f76105626364656667fa000278797a80"
    );
    let data = big();
    let (a85, rl) = (encode::ascii85(&data), encode::run_length(&data));
    assert_eq!((a85.len(), fnv(&a85).as_str()), (74810, "d293d665"));
    assert_eq!((rl.len(), fnv(&rl).as_str()), (64663, "a4eac87f"));
}

#[test]
fn lzw_writes_the_same_codes_as_the_reference() {
    for early in [true, false] {
        assert_eq!(hex(&encode::lzw(&[], early)), "804040");
        assert_eq!(hex(&encode::lzw(&[0x41], early)), "80106020");
        assert_eq!(hex(&encode::lzw(b"abc", early)), "80184c463808");
        assert_eq!(hex(&encode::lzw(&[0; 5], early)), "800020502808");
        assert_eq!(hex(&encode::lzw(&[0; 4], early)), "800020400808");
    }
    let mut mixed = b"hello hello hello aaaaaaaaaabcdefg".to_vec();
    mixed.extend([0; 7]);
    mixed.extend(b"xyz");
    assert_eq!(
        hex(&encode::lzw(&mixed, true)),
        "801a0ca6c361bc41028241a1105101861d0f88188c66432998ce008c46400783c9ea02"
    );
    let data = big();
    let (early, late) = (encode::lzw(&data, true), encode::lzw(&data, false));
    assert_eq!((early.len(), fnv(&early).as_str()), (17955, "0d9c26ce"));
    assert_eq!((late.len(), fnv(&late).as_str()), (17954, "db8496d6"));
}
