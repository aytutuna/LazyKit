use lazy_tiff::{BilevelPage, DeepPage, Details, Error, bilevel, deep, stride};
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn packed(width: usize, height: usize, black: impl Fn(usize, usize) -> bool) -> Vec<u8> {
    let stride = stride(width);
    let mut packed = vec![0xFFu8; stride * height];
    for y in 0..height {
        for x in 0..width {
            if black(x, y) {
                packed[y * stride + (x >> 3)] &= !(0x80 >> (x & 7));
            }
        }
    }
    packed
}

fn u16_at(bytes: &[u8], at: usize) -> usize {
    u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize
}

fn u32_at(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
}

struct Directory {
    entries: Vec<(usize, usize, usize, usize)>, // tag, type, count, value
    next: usize,
}

impl Directory {
    fn read(tiff: &[u8], offset: usize) -> Directory {
        assert_eq!(offset % 2, 0);
        let count = u16_at(tiff, offset);
        let mut entries = Vec::new();
        for e in 0..count {
            let at = offset + 2 + e * 12;
            let (tag, kind, n) = (u16_at(tiff, at), u16_at(tiff, at + 2), u32_at(tiff, at + 4));
            let value = if kind == 3 && n == 1 {
                u16_at(tiff, at + 8)
            } else {
                u32_at(tiff, at + 8)
            };
            assert!(
                entries
                    .last()
                    .is_none_or(|l: &(usize, usize, usize, usize)| l.0 < tag)
            );
            entries.push((tag, kind, n, value));
        }
        Directory {
            entries,
            next: u32_at(tiff, offset + 2 + count * 12),
        }
    }

    fn value(&self, tag: usize) -> Option<usize> {
        self.entries.iter().find(|e| e.0 == tag).map(|e| e.3)
    }

    fn tags(&self) -> Vec<usize> {
        self.entries.iter().map(|e| e.0).collect()
    }
}

fn small() -> Vec<u8> {
    packed(13, 7, |x, y| (x + y) % 3 == 0)
}

fn black() -> Vec<u8> {
    packed(9, 2, |_, _| true)
}

#[test]
fn one_page_is_a_g4_strip_with_its_resolution() {
    let text = data("g4-text-850x1100.packed");
    let page = BilevelPage {
        width: 850,
        height: 1100,
        packed: &text,
        dpi: 300,
    };
    let tiff = bilevel(&[page], None).unwrap();
    assert_eq!(tiff, data("tiff-text-300.tif"));
    assert_eq!(&tiff[..4], b"II*\0");
    let dir = Directory::read(&tiff, u32_at(&tiff, 4));
    assert_eq!(dir.next, 0);
    assert_eq!(
        dir.tags(),
        [
            256, 257, 258, 259, 262, 273, 277, 278, 279, 282, 283, 293, 296
        ]
    );
    assert_eq!(dir.value(259), Some(4));
    assert_eq!(dir.value(262), Some(0));
    assert_eq!(dir.value(273), Some(8));
    let g4 = data("g4-text-850x1100.g4");
    assert_eq!(dir.value(279), Some(g4.len()));
    assert_eq!(&tiff[8..8 + g4.len()], g4);
    assert_eq!(tiff.len(), 17852);
    let digest: String = Sha256::digest(&tiff)
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect();
    assert_eq!(
        digest,
        "EF4F98A030BB642AB198F3FAE98EEE6CD088E24B986A769FCD0EEE92255CA38D"
    );
}

#[test]
fn several_pages_are_chained_as_pages_of_one_document() {
    let text = data("g4-text-850x1100.packed");
    let (small, black) = (small(), black());
    let pages = [
        BilevelPage {
            width: 850,
            height: 1100,
            packed: &text,
            dpi: 100,
        },
        BilevelPage {
            width: 13,
            height: 7,
            packed: &small,
            dpi: 300,
        },
        BilevelPage {
            width: 9,
            height: 2,
            packed: &black,
            dpi: 600,
        },
    ];
    let tiff = bilevel(&pages, None).unwrap();
    assert_eq!(tiff, data("tiff-three-pages.tif"));
    let mut offset = u32_at(&tiff, 4);
    for (i, page) in pages.iter().enumerate() {
        assert_ne!(offset, 0);
        let dir = Directory::read(&tiff, offset);
        assert_eq!(dir.value(254), Some(2));
        assert_eq!(dir.value(297), Some(i | 3 << 16));
        assert_eq!(dir.value(256), Some(page.width));
        assert_eq!(u32_at(&tiff, dir.value(282).unwrap()), page.dpi as usize);
        offset = dir.next;
    }
    assert_eq!(offset, 0);
}

#[test]
fn details_add_a_description_and_an_exif_date() {
    let (small, black) = (small(), black());
    let details = Details {
        year: Some(1985),
        month: Some(7),
        day: Some(23),
        caption: "Hi".into(),
    };
    let pages = [
        BilevelPage {
            width: 13,
            height: 7,
            packed: &small,
            dpi: 300,
        },
        BilevelPage {
            width: 9,
            height: 2,
            packed: &black,
            dpi: 150,
        },
    ];
    assert_eq!(
        bilevel(&pages, Some(&details)).unwrap(),
        data("tiff-details.tif")
    );
    let one = Details {
        year: Some(2001),
        month: Some(2),
        day: Some(30),
        caption: "  A caption \u{e7}  ".into(),
    };
    let page = BilevelPage {
        width: 13,
        height: 7,
        packed: &small,
        dpi: 200,
    };
    assert_eq!(
        bilevel(&[page], Some(&one)).unwrap(),
        data("tiff-details-one.tif")
    );
}

#[test]
fn details_read_like_the_reference() {
    let d = |y, m, day, c: &str| Details {
        year: y,
        month: m,
        day,
        caption: c.into(),
    };
    assert_eq!(
        d(Some(2001), Some(2), Some(30), "").exif_date().unwrap(),
        "2001:02:01 00:00:00"
    );
    assert_eq!(
        d(Some(2000), Some(2), Some(29), "").exif_date().unwrap(),
        "2000:02:29 00:00:00"
    );
    assert_eq!(
        d(Some(1900), Some(2), Some(29), "").exif_date().unwrap(),
        "1900:02:01 00:00:00"
    );
    assert_eq!(
        d(Some(2024), Some(13), None, "").exif_date().unwrap(),
        "2024:01:01 00:00:00"
    );
    assert_eq!(d(Some(999), None, None, "").exif_date(), None);
    assert_eq!(d(None, None, None, " \t ").exif_caption(), None);
    assert_eq!(d(None, None, None, " x ").exif_caption(), Some("x"));
}

#[test]
fn no_pages_and_short_buffers_are_refused() {
    assert_eq!(bilevel(&[], None), Err(Error::NoPages));
    let page = BilevelPage {
        width: 9,
        height: 2,
        packed: &[0; 3],
        dpi: 300,
    };
    assert_eq!(bilevel(&[page], None), Err(Error::BadPage));
}

#[test]
fn gray_is_one_uncompressed_black_is_zero_strip_of_16_bit_samples() {
    let page = DeepPage {
        width: 2,
        height: 2,
        channels: 1,
        samples: &[0, 1, 256, 65535],
    };
    let tiff = deep(&page, 300, None).unwrap();
    assert_eq!(tiff, data("deep-gray-2x2-300.tif"));
    // The golden the Swift tests pin, written by the C# writer.
    assert_eq!(
        hex(&tiff),
        concat!(
            "49492a0010000000000001000001ffff0c00000104000100000002000000010104000100000002000000020103000100",
            "000010000000030103000100000001000000060103000100000001000000110104000100000008000000150103000100",
            "0000010000001601040001000000020000001701040001000000080000001a01050001000000a60000001b0105000100",
            "0000ae000000280103000100000002000000000000002c010000010000002c01000001000000",
        )
    );
}

#[test]
fn color_has_three_samples_and_the_details() {
    let page = DeepPage {
        width: 1,
        height: 1,
        channels: 3,
        samples: &[65535, 32768, 1],
    };
    let details = Details {
        year: Some(1985),
        month: Some(7),
        day: Some(23),
        caption: "Hi".into(),
    };
    let tiff = deep(&page, 150, Some(&details)).unwrap();
    assert_eq!(tiff, data("deep-color-1x1-150-details.tif"));
    assert_eq!(
        hex(&tiff),
        concat!(
            "49492a000e000000ffff008001000e000001040001000000010000000101040001000000010000000201030003000000",
            "cc0000000301030001000000010000000601030001000000020000000e01020005000000d20000001101040001000000",
            "080000001501030001000000030000001601040001000000010000001701040001000000060000001a01050001000000",
            "bc0000001b01050001000000c40000002801030001000000020000006987040001000000ec0000000000000096000000",
            "010000009600000001000000100010001000486900000000313938353a30373a32332030303a30303a30300001000390",
            "020014000000d800000000000000",
        )
    );
}

#[test]
fn a_larger_color_page_and_an_empty_detail_set_adds_nothing() {
    let samples: Vec<u16> = (0..45u32).map(|i| (i * 4369 % 65536) as u16).collect();
    let page = DeepPage {
        width: 5,
        height: 3,
        channels: 3,
        samples: &samples,
    };
    let tiff = deep(&page, 72, None).unwrap();
    assert_eq!(tiff, data("deep-color-5x3-72.tif"));
    assert_eq!(tiff, deep(&page, 72, Some(&Details::default())).unwrap());
}

#[test]
fn deep_pages_must_be_gray_or_rgb_and_full() {
    let page = DeepPage {
        width: 2,
        height: 2,
        channels: 2,
        samples: &[0; 8],
    };
    assert_eq!(deep(&page, 300, None), Err(Error::BadSamples));
    let page = DeepPage {
        width: 2,
        height: 2,
        channels: 1,
        samples: &[0; 3],
    };
    assert_eq!(deep(&page, 300, None), Err(Error::BadSamples));
    let page = DeepPage {
        width: 0,
        height: 2,
        channels: 1,
        samples: &[],
    };
    assert_eq!(deep(&page, 300, None), Err(Error::BadSamples));
}
