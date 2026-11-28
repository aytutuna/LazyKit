use std::time::{Duration, Instant};

use lazy_pdf_filters::{DEFAULT_MAX_SIZE, jbig2};

const BOUND_MS: u64 = if cfg!(debug_assertions) { 9000 } else { 400 };

// Inputs the fuzzer found that took the decoder longer than its timeout: symbol dictionaries that
// decode millions of pixels from the few bytes there are.
fn quick(data: &[u8]) {
    let start = Instant::now();
    let _ = jbig2::decode(&[], data, DEFAULT_MAX_SIZE, 0, 0, false);
    let took = start.elapsed();
    assert!(took < Duration::from_millis(BOUND_MS), "took {took:?}");
}

#[test]
fn fuzz_timeout_input_is_quick() {
    quick(include_bytes!(
        "../../../testdata/pdf-filters/jbig2-timeout.bin"
    ));
}

#[test]
fn second_fuzz_timeout_input_is_quick() {
    quick(include_bytes!(
        "../../../testdata/pdf-filters/jbig2-timeout-2.bin"
    ));
}
