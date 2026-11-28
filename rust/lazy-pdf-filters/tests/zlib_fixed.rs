mod common;

use common::data;
use lazy_pdf_filters::encode::{adler32, zlib_fixed};
use lazy_pdf_filters::flate::inflate;
use sha2::{Digest, Sha256};

fn lines(count: usize) -> Vec<u8> {
    let mut text = String::new();
    for i in 0..count {
        text.push_str(&format!(
            "/F1 12 Tf\n1 0 0 1 {} {} Tm\n<{:04X}> Tj\n",
            i * 7 % 600,
            i * 13 % 790,
            i * 40503 % 65536
        ));
    }
    text.into_bytes()
}

fn noise(count: usize) -> Vec<u8> {
    let mut x = 12345u32;
    (0..count)
        .map(|_| {
            x = x.wrapping_mul(1103515245).wrapping_add(12345);
            (x >> 16) as u8
        })
        .collect()
}

fn samples() -> Vec<Vec<u8>> {
    let mut up_down: Vec<u8> = (0..=255).collect();
    up_down.extend((0..=255u8).map(|i| 255 - i));
    let mut twice = noise(40_000);
    twice.extend(noise(40_000));
    vec![
        vec![0x41],
        b"abcabcabcabcabcabcabcabc".to_vec(),
        vec![0; 1000],
        up_down,
        lines(3000),
        noise(100_000),
        twice,
        vec![],
    ]
}

#[test]
fn every_sample_is_the_reference_bytes() {
    for (i, sample) in samples().iter().enumerate() {
        let zlib = zlib_fixed(sample);
        assert_eq!(zlib, data(&format!("flate-sample{i}.z")), "sample {i}");
    }
}

#[test]
fn round_trips_through_the_inflater() {
    for sample in samples() {
        assert_eq!(
            inflate(&zlib_fixed(&sample), 1 << 20, true).unwrap(),
            sample
        );
    }
}

#[test]
fn header_and_adler32() {
    let zlib = zlib_fixed(&lines(10));
    assert_eq!(&zlib[..2], [0x78, 0x01]);
    assert_eq!(zlib[zlib.len() - 4..], adler32(&lines(10)).to_be_bytes());
    assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
}

#[test]
fn repeated_text_gets_much_smaller() {
    let data = lines(3000);
    assert!(zlib_fixed(&data).len() < data.len() / 3);
}

#[test]
fn pinned_lengths_and_hashes() {
    let zlib = zlib_fixed(&lines(3000));
    assert_eq!(zlib.len(), 38427);
    assert_eq!(
        hex(&zlib),
        "7ECC0A60EB606EE8DF5CCACE66B4353CC1EB292E9E4A1ADFC61E263D4312E886"
    );
    let zlib = zlib_fixed(&noise(100_000));
    assert_eq!(zlib.len(), 105451);
    assert_eq!(
        hex(&zlib),
        "7B161A57EA4234BDE7AD33F2AFBC27D48D071A31F312B85597488E7987DED153"
    );
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect()
}
