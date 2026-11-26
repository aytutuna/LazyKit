#![allow(dead_code)]
use std::path::PathBuf;

pub fn testdata(name: &str) -> PathBuf {
    // the wasm test runner mounts the checkout elsewhere than the build found it
    let root = std::env::var_os("LAZYKIT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    root.join("testdata/scan").join(name)
}

pub fn json(name: &str) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(testdata(name)).unwrap()).unwrap()
}

/// FNV-1a over the bytes, the page checksum Lazy Scan's tests use.
pub fn fnv(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 2166136261;
    for &b in bytes {
        hash ^= b as u32;
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

pub fn sum(bytes: &[u8]) -> String {
    format!("{:08x}", fnv(bytes))
}

pub mod photo_scenes;
pub mod scenes;

pub mod pages;

pub mod book;
