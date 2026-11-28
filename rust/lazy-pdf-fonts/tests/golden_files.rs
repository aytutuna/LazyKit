mod common;

use common::golden_bytes;
use sha2::{Digest, Sha256};

#[test]
fn golden_files_are_the_ones_that_were_copied() {
    let sums = common::golden("SHA256SUMS");
    assert!(!sums.is_empty());
    for line in sums.lines() {
        let (sum, name) = line.split_once(" *").unwrap();
        let hex: String = Sha256::digest(golden_bytes(name))
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(hex, sum, "{name}");
    }
}
