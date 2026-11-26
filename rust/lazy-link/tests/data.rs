use serde_json::Value;
use std::path::PathBuf;

pub fn load(file: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/link")
        .join(file);
    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap()
}
