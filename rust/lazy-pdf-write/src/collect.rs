//! A document being put together object by object: numbers are handed out first and filled in later, then the file is
//! written in number order.

use crate::writer::ObjectWriter;

enum Slot {
    Empty,
    Plain(Vec<u8>),
    Stream(Vec<u8>, Vec<u8>),
}

#[derive(Default)]
pub struct Collector {
    objects: Vec<Slot>,
}

impl Collector {
    pub fn new() -> Collector {
        Collector::default()
    }

    /// The next free object number.
    pub fn alloc(&mut self) -> usize {
        self.objects.push(Slot::Empty);
        self.objects.len()
    }

    /// Drops the objects numbered above `count`: what a failed piece of work allocated.
    pub fn truncate(&mut self, count: usize) {
        self.objects.truncate(count);
    }

    pub fn count(&self) -> usize {
        self.objects.len()
    }

    /// Object `number` is the value `body` (a dictionary, an array, ...).
    pub fn set(&mut self, number: usize, body: impl Into<Vec<u8>>) {
        self.objects[number - 1] = Slot::Plain(body.into());
    }

    /// Object `number` is a stream with these dictionary entries (without /Length) and data.
    pub fn set_stream(&mut self, number: usize, entries: impl Into<Vec<u8>>, data: Vec<u8>) {
        self.objects[number - 1] = Slot::Stream(entries.into(), data);
    }

    /// The file, each object written once, with `trailer` after /Size (for example "/Root 1 0 R").
    pub fn finish(self, version: &str, trailer: &str) -> Vec<u8> {
        let mut w = ObjectWriter::new();
        w.write_header(version);
        for (i, slot) in self.objects.into_iter().enumerate() {
            match slot {
                Slot::Empty => w.write_object(i + 1, "null"),
                Slot::Plain(body) => w.write_object(i + 1, body),
                Slot::Stream(entries, data) => w.write_stream(i + 1, entries, &data),
            }
        }
        w.finish(trailer)
    }
}
