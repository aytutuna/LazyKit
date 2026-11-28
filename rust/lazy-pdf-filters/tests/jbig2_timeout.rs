use std::time::{Duration, Instant};

use lazy_pdf_filters::{DEFAULT_MAX_SIZE, jbig2};

const BOUND_MS: u64 = if cfg!(debug_assertions) { 9000 } else { 400 };

// Found by the fuzzer: a stream of two region segments whose arithmetic data ran out, which took
// the decoder longer than the fuzzer's timeout.
#[test]
fn fuzz_timeout_input_is_quick() {
    let data = include_bytes!("../../../testdata/pdf-filters/jbig2-timeout.bin");
    let start = Instant::now();
    let _ = jbig2::decode(&[], data, DEFAULT_MAX_SIZE, 0, 0, false);
    let took = start.elapsed();
    assert!(took < Duration::from_millis(BOUND_MS), "took {took:?}");
}
