use std::collections::BTreeMap;

/// Builds small PDF files byte by byte, so that a test can break exactly one thing.
pub struct TestPdf {
    bytes: Vec<u8>,
    offsets: BTreeMap<u32, u64>,
}

fn latin(text: &str) -> Vec<u8> {
    text.chars().map(|c| c as u8).collect()
}

impl TestPdf {
    pub fn new() -> TestPdf {
        TestPdf {
            bytes: latin("%PDF-1.4\n"),
            offsets: BTreeMap::new(),
        }
    }

    pub fn obj(&mut self, number: u32, body: &str) -> &mut TestPdf {
        self.offsets.insert(number, self.bytes.len() as u64);
        self.bytes
            .extend(latin(&format!("{number} 0 obj\n{body}\nendobj\n")));
        self
    }

    pub fn stream(&mut self, number: u32, entries: &str, data: &[u8]) -> &mut TestPdf {
        self.offsets.insert(number, self.bytes.len() as u64);
        self.bytes.extend(latin(&format!(
            "{number} 0 obj\n<<{entries} /Length {}>>\nstream\n",
            data.len()
        )));
        self.bytes.extend_from_slice(data);
        self.bytes.extend(latin("\nendstream\nendobj\n"));
        self
    }

    pub fn finish(&self) -> Vec<u8> {
        let start = self.bytes.len();
        let size = self.offsets.keys().max().unwrap() + 1;
        let mut text = format!("xref\n0 {size}\n0000000000 65535 f \n");
        for n in 1..size {
            match self.offsets.get(&n) {
                Some(offset) => text.push_str(&format!("{offset:010} 00000 n \n")),
                None => text.push_str("0000000000 00000 f \n"),
            }
        }
        text.push_str(&format!(
            "trailer\n<</Size {size} /Root 1 0 R>>\nstartxref\n{start}\n%%EOF\n"
        ));
        [self.bytes.clone(), latin(&text)].concat()
    }
}
