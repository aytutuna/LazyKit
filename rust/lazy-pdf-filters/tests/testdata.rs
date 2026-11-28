mod common;

use common::*;
use sha2::{Digest, Sha256};

#[test]
fn testdata_matches_sha256sums() {
    let sums = read_text("SHA256SUMS");
    let mut listed = Vec::new();
    for line in sums.lines() {
        let (hash, name) = line.split_once(" *").unwrap();
        let actual: String = Sha256::digest(read(name))
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(actual, hash, "{name}");
        listed.push(name.to_owned());
    }
    let mut files: Vec<String> = std::fs::read_dir(data_dir())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .filter(|n| !n.starts_with('.') && n != "SHA256SUMS")
        .collect();
    files.sort();
    listed.sort();
    assert_eq!(files, listed);
}
