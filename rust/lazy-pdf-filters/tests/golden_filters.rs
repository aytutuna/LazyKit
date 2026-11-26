mod common;

use common::*;
use lazy_pdf_filters::{DEFAULT_MAX_SIZE, Filter, Options, Params, Value, flate, run};

// One line of input is "<op> <words>". Filters are names joined by commas and parameters are
// Key=value pairs joined by commas, the value in PDF syntax; "-" is none.

fn filters(word: &str, parms: &str) -> Vec<Filter> {
    if word == "-" {
        return Vec::new();
    }
    // A parameter dictionary on its own belongs to the first filter.
    let parms = parameters(parms);
    word.split(',')
        .enumerate()
        .map(|(i, name)| Filter::new(name, if i == 0 { parms.clone() } else { None }))
        .collect()
}

fn parameters(word: &str) -> Option<Params> {
    if word == "-" {
        return None;
    }
    let mut params = Params::new();
    for pair in word.split(',') {
        let (key, value) = pair.split_once('=').unwrap();
        let value = match value {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ if value.starts_with('/') => Value::Name(value[1..].to_owned()),
            _ => match value.parse::<i64>() {
                Ok(n) => Value::Int(n),
                Err(_) => Value::Real(value.parse().unwrap()),
            },
        };
        params.set(key, value);
    }
    Some(params)
}

fn bytes(word: &str) -> Vec<u8> {
    if word == "-" {
        Vec::new()
    } else {
        unescape(word)
    }
}

fn decode(data: &[u8], filters: &[Filter], strict: bool) -> String {
    let options = Options {
        strict,
        ..Options::default()
    };
    match run(data, filters, &options) {
        Ok(decoded) => show(&decoded.data),
        Err(e) => show_error(&e),
    }
}

fn op(op: &str, words: &[&str]) -> String {
    let flate = [Filter::new("FlateDecode", None)];
    match op {
        "decode" => decode(&bytes(words[2]), &filters(words[0], words[1]), false),
        "strict" => decode(&bytes(words[0]), &flate, true),
        "inflate" => match flate::inflate(&bytes(words[0]), DEFAULT_MAX_SIZE, false) {
            Ok(data) => show(&data),
            Err(e) => show_error(&e),
        },
        "predict" => match flate::predict(bytes(words[1]), parameters(words[0]).as_ref()) {
            Ok(data) => show(&data),
            Err(e) => show_error(&e),
        },
        "codec" => {
            let options = Options {
                stop_at_codec: true,
                ..Options::default()
            };
            match run(&bytes(words[2]), &filters(words[0], words[1]), &options) {
                Ok(d) => format!(
                    "codec={} parameters={} data={}",
                    d.codec.as_deref().unwrap_or("-"),
                    if d.parms.is_some() { "dictionary" } else { "-" },
                    show(&d.data)
                ),
                Err(e) => show_error(&e),
            }
        }
        "file" => {
            let mut data = read(words[2]);
            flips(words[4], &mut data);
            if words[5] != "-" {
                data.truncate(words[5].parse().unwrap());
            }
            decode(&data, &filters(words[0], words[1]), words[3] == "1")
        }
        "blocks" => {
            let data = blocks(
                words[0].parse().unwrap(),
                words[1].parse().unwrap(),
                words[2],
            );
            decode(&data, &flate, words[3] == "1")
        }
        _ => panic!("no op {op}"),
    }
}

// A raw deflate stream: one fixed block of literals and runs of 258 at distance 1, then the tail.
// The codes are the fixed ones of RFC 1951 3.2.6.
fn blocks(literals: usize, matches: usize, tail: &str) -> Vec<u8> {
    let mut bits: Vec<bool> = Vec::new();
    let put = |bits: &mut Vec<bool>, value: usize, count: usize| {
        for i in 0..count {
            bits.push(value >> i & 1 != 0);
        }
    };
    let code = |bits: &mut Vec<bool>, value: usize, count: usize| {
        for i in (0..count).rev() {
            bits.push(value >> i & 1 != 0);
        }
    };
    put(&mut bits, 0, 1);
    put(&mut bits, 1, 2);
    for i in 0..literals {
        code(&mut bits, 0x30 + (i * 7 + 3) % 100, 8);
    }
    for _ in 0..matches {
        code(&mut bits, 0xc5, 8);
        code(&mut bits, 0, 5);
    }
    if tail != "cut" {
        code(&mut bits, 0, 7);
    }
    let mut extra: Vec<u8> = Vec::new();
    match tail {
        "end" => {
            put(&mut bits, 1, 1);
            put(&mut bits, 1, 2);
            code(&mut bits, 0, 7);
        }
        "bad" => {
            put(&mut bits, 1, 1);
            put(&mut bits, 3, 2);
        }
        "badlen" => {
            put(&mut bits, 0, 3);
            extra.extend([1, 0, 0, 0, b'x']);
        }
        _ => {}
    }
    while !bits.len().is_multiple_of(8) {
        bits.push(false);
    }
    let mut out: Vec<u8> = bits
        .chunks(8)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0u8, |b, (j, &bit)| b | (bit as u8) << j)
        })
        .collect();
    out.extend(extra);
    out
}

// miniz_oxide takes a dynamic block whose distance code is one symbol longer than a bit, where zlib
// (and so the reference) calls the incomplete code invalid.
const KNOWN: [(&str, &str); 1] = [(
    r"inflate \x05\xc0\x01\x04\x00\x00\x00\x00\x10\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x03\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x01\x00",
    "616161616161616161616161616161",
)];

#[test]
fn filters_output_is_the_golden_file() {
    check(
        &read_text("filters-input.txt"),
        &read_text("filters-output.txt"),
        op,
        &KNOWN,
    );
}
