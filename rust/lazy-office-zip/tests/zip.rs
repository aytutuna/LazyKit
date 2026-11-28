use lazy_office_zip::{Entry, Writer, crc32, make, package_size};
use sha2::{Digest, Sha256};

fn data(name: &str) -> Vec<u8> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/pdf-write");
    let bytes = std::fs::read(dir.join(name)).unwrap();
    let sums = std::fs::read_to_string(dir.join("SHA256SUMS")).unwrap();
    let hash: String = Sha256::digest(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    assert!(sums.contains(&format!("{hash} *{name}")), "{name}");
    bytes
}

fn several() -> Vec<Entry> {
    let big: Vec<u8> = (0..70_000u32).map(|i| (i * 7) as u8).collect();
    vec![
        Entry::new("[Content_Types].xml", b"<x/>".to_vec()),
        Entry::new("dir/empty.bin", vec![]),
        Entry::new("dir/big.bin", big),
        Entry::new("na\u{ef}ve.txt", "\u{e9}".as_bytes().to_vec()),
    ]
}

fn one() -> Vec<Entry> {
    vec![Entry::new("a.txt", b"hi".to_vec())]
}

#[test]
fn crc32_known_values() {
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"123456789"), 0xCBF43926);
    assert_eq!(
        crc32(b"The quick brown fox jumps over the lazy dog"),
        0x414FA339
    );
    assert_eq!(crc32(&[0]), 0xD202EF8D);
}

#[test]
fn exact_bytes_of_one_entry() {
    let crc = crc32(b"hi");
    let mut expected: Vec<u8> = Vec::new();
    let u16s =
        |e: &mut Vec<u8>, values: &[u16]| values.iter().for_each(|v| e.extend(v.to_le_bytes()));
    let u32s =
        |e: &mut Vec<u8>, values: &[u32]| values.iter().for_each(|v| e.extend(v.to_le_bytes()));
    // Local header, name, data.
    u32s(&mut expected, &[0x04034B50]);
    u16s(&mut expected, &[10, 0, 0, 0, 0x21]);
    u32s(&mut expected, &[crc, 2, 2]);
    u16s(&mut expected, &[5, 0]);
    expected.extend(b"a.txt");
    expected.extend(b"hi");
    // Central directory at 37.
    u32s(&mut expected, &[0x02014B50]);
    u16s(&mut expected, &[20, 10, 0, 0, 0, 0x21]);
    u32s(&mut expected, &[crc, 2, 2]);
    u16s(&mut expected, &[5, 0, 0, 0, 0]);
    u32s(&mut expected, &[0, 0]);
    expected.extend(b"a.txt");
    // End of central directory: one entry, 51 bytes of directory at 37.
    u32s(&mut expected, &[0x06054B50]);
    u16s(&mut expected, &[0, 0, 1, 1]);
    u32s(&mut expected, &[51, 37]);
    u16s(&mut expected, &[0]);
    assert_eq!(make(&one()).unwrap(), expected);
}

#[test]
fn the_reference_packages_come_out_the_same() {
    assert_eq!(make(&[]).unwrap(), data("zip-empty.zip"));
    assert_eq!(make(&one()).unwrap(), data("zip-one.zip"));
    assert_eq!(make(&several()).unwrap(), data("zip-several.zip"));
}

/// Walks the central directory the way a reader does: names in order, stored data matching its size and CRC.
#[test]
fn a_reader_finds_every_entry() {
    let zip = make(&several()).unwrap();
    let end = zip.len() - 22;
    let count = u16::from_le_bytes([zip[end + 10], zip[end + 11]]) as usize;
    let mut at = u32::from_le_bytes(zip[end + 16..end + 20].try_into().unwrap()) as usize;
    let mut names = Vec::new();
    for entry in several().iter().take(count) {
        assert_eq!(zip[at..at + 4], 0x02014B50u32.to_le_bytes());
        let crc = u32::from_le_bytes(zip[at + 16..at + 20].try_into().unwrap());
        let size = u32::from_le_bytes(zip[at + 20..at + 24].try_into().unwrap()) as usize;
        let name_length = u16::from_le_bytes([zip[at + 28], zip[at + 29]]) as usize;
        let local = u32::from_le_bytes(zip[at + 42..at + 46].try_into().unwrap()) as usize;
        let name = std::str::from_utf8(&zip[at + 46..at + 46 + name_length]).unwrap();
        let start = local + 30 + name_length;
        assert_eq!(zip[start..start + size], entry.data);
        assert_eq!(crc, crc32(&entry.data));
        let flags = u16::from_le_bytes([zip[at + 8], zip[at + 9]]);
        assert_eq!(flags & 0x0800 != 0, !name.is_ascii());
        names.push(name.to_string());
        at += 46 + name_length;
    }
    assert_eq!(
        names,
        [
            "[Content_Types].xml",
            "dir/empty.bin",
            "dir/big.bin",
            "na\u{ef}ve.txt"
        ]
    );
}

#[test]
fn package_size_is_the_length_make_writes() {
    for entries in [vec![], one(), several()] {
        assert_eq!(make(&entries).unwrap().len() as u64, package_size(&entries));
    }
    assert_eq!(package_size(&[]), 22);
    assert_eq!(package_size(&one()), 110);
}

#[test]
fn the_writer_writes_the_bytes_make_returns() {
    let mut writer = Writer::new(Vec::new());
    for e in several() {
        writer.add(&e.name, &e.data).unwrap();
    }
    assert_eq!(writer.finish().unwrap(), make(&several()).unwrap());
}

#[test]
fn too_many_entries_are_refused() {
    let mut writer = Writer::new(Vec::new());
    for i in 0..65_534 {
        writer.add(&i.to_string(), b"").unwrap();
    }
    assert!(writer.add("one more", b"").is_err());
}

#[test]
fn a_name_the_zip_cannot_record_is_refused() {
    let mut writer = Writer::new(Vec::new());
    assert!(writer.add(&"n".repeat(65_535), b"").is_ok());
    assert!(matches!(
        writer.add(&"n".repeat(65_536), b""),
        Err(lazy_office_zip::Error::NameTooLong)
    ));
}
