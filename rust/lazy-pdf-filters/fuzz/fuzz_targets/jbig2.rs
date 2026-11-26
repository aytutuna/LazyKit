#![no_main]

use lazy_pdf_filters::jbig2;
use libfuzzer_sys::fuzz_target;

const CAP: u64 = 1 << 24;

// The first bytes give the image size (0 for none), whether a partial page is no use, and where the
// globals end; the globals come first, then the image's own segments.
fuzz_target!(|data: &[u8]| {
    let Some((head, rest)) = data.split_at_checked(5) else {
        return;
    };
    let width = if head[0] & 1 == 0 { 0 } else { head[1] as i32 * 3 };
    let height = if head[0] & 1 == 0 { 0 } else { head[2] as i32 * 3 };
    let (globals, image) = rest.split_at(rest.len().min(head[3] as usize * head[4] as usize / 4));
    if let Ok(page) = jbig2::decode(globals, image, CAP, width, height, head[0] & 2 != 0) {
        assert!(page.len() as u64 <= CAP);
    }
});
