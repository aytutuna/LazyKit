mod common;

use common::*;
use lazy_pdf_filters::{Filter, Params, Value, decode, jbig2};

// Deterministic stand-in for a fuzz run on every push: the golden streams with a few bytes changed
// or cut off must come back as a result or an error, never a panic.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn mutate(rng: &mut Rng, original: &[u8]) -> Vec<u8> {
    let mut data = original.to_vec();
    for _ in 0..1 + rng.below(4) {
        if data.is_empty() {
            break;
        }
        let at = rng.below(data.len());
        match rng.below(4) {
            0 => data[at] = rng.next() as u8,
            1 => data[at] ^= 1 << rng.below(8),
            2 => data.truncate(at),
            _ => data.insert(at, rng.next() as u8),
        }
    }
    data
}

fn bins() -> Vec<(String, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(data_dir())
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().into_string().unwrap();
            name.ends_with(".bin").then(|| (name.clone(), read(&name)))
        })
        .collect();
    files.sort();
    files
}

#[test]
fn mutated_jbig2_streams() {
    let globals = read("globals.bin");
    let mut rng = Rng(0x9e3779b97f4a7c15);
    for (name, original) in bins() {
        if name.starts_with("flate") || name.starts_with("lzw") {
            continue;
        }
        for _ in 0..40 {
            let data = mutate(&mut rng, &original);
            let (w, h) = (rng.below(300) as i32, rng.below(300) as i32);
            let _ = jbig2::decode(&globals, &data, 1 << 24, w, h, false);
            let _ = jbig2::decode(&mutate(&mut rng, &globals), &data, 1 << 24, 0, 0, true);
        }
    }
}

#[test]
fn mutated_deflate_lzw_and_ccitt_streams() {
    let mut rng = Rng(0x2545f4914f6cdd1d);
    let mut parms = Params::new();
    parms
        .set("Predictor", Value::Int(15))
        .set("Columns", Value::Int(7));
    let flate = [Filter::new("FlateDecode", Some(parms))];
    let lzw = [Filter::new("LZWDecode", None)];
    let mut ccitt_parms = Params::new();
    ccitt_parms
        .set("K", Value::Int(-1))
        .set("Columns", Value::Int(64));
    let ccitt = [Filter::new("CCITTFaxDecode", Some(ccitt_parms))];
    for (name, original) in bins() {
        for _ in 0..40 {
            let data = mutate(&mut rng, &original);
            let chain = if name.starts_with("flate") {
                &flate
            } else if name.starts_with("lzw") {
                &lzw
            } else {
                &ccitt
            };
            let _ = decode(&data, chain, 1 << 24);
        }
    }
}
