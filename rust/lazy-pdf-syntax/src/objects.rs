//! The object table: objects load on first use and are cached, so a change made to one stays.

use std::collections::HashMap;
use std::sync::Arc;

use crate::document::{Document, Entry, EntryKind, MAX_OBJECT_NUMBER, ObjectStream, Slot, Value};
use crate::lexer::Lexer;
use crate::object::{Dict, Object, Ref};

const MAX_REFERENCE_HOPS: usize = 32;
const MAX_CACHED_OBJECT_STREAMS: usize = 256;
const MAX_CACHED_OBJECT_STREAM_BYTES: usize = 128 << 20;
const MAX_OBJECT_STREAM_SIZE: usize = 32 << 20;
/// All the object streams a document may decode in its life, however often the cache drops them.
const MAX_DECODED_OBJECT_STREAM_BYTES: usize = 1 << 30;
/// Reading an object can need another (a stream's /Length, an object stream); a longer chain is a trap.
const MAX_LOAD_DEPTH: usize = 32;
const MAX_FLATTENED_NODES: usize = 4096;

impl Document {
    /// Object `number` as the newest revision has it; none when it is free or not listed. One that is listed but
    /// cannot be read is null. Every call gives the same object until it is changed.
    pub fn get(&mut self, number: u32) -> Option<Arc<Object>> {
        if let Some(slot) = self.objects.get(&number) {
            return Some(slot.value.clone());
        }
        let entry = *self.entries.get(&number)?;
        if matches!(entry.kind, EntryKind::Free | EntryKind::New) {
            return None;
        }
        if self.loading.len() >= MAX_LOAD_DEPTH || !self.loading.insert(number) {
            return None;
        }
        let (value, generation) = if entry.kind == EntryKind::InUse {
            self.read_plain(number, entry)
        } else {
            (self.read_compressed(number, entry), entry.generation)
        };
        self.loading.remove(&number);
        // An object that cannot be read stays null, without being tried again.
        let value = Arc::new(value.unwrap_or(Object::Null));
        self.objects.insert(
            number,
            Slot {
                value: value.clone(),
                generation,
            },
        );
        Some(value)
    }

    /// Object `number` to change; it is written at the next save.
    pub fn get_mut(&mut self, number: u32) -> Option<&mut Object> {
        let value = self.get(number)?;
        if !self.changed.contains(&number)
            && self
                .entries
                .get(&number)
                .is_some_and(|e| e.kind != EntryKind::New)
        {
            let mut print = Vec::new();
            value.write_to(&mut print);
            self.originals.insert(number, print);
        }
        self.changed.insert(number);
        let slot = self.objects.get_mut(&number)?;
        Some(Arc::make_mut(&mut slot.value))
    }

    /// What a value stands for: a reference followed to the object it names, however many hops (a loop gives up
    /// after 32). None for null, for a free or missing object.
    pub fn resolve<'a>(&mut self, value: &'a Object) -> Option<Value<'a>> {
        let mut current = Value::Borrowed(value);
        let mut hops = 0;
        while let Object::Ref(reference) = &*current {
            hops += 1;
            if hops > MAX_REFERENCE_HOPS {
                return None;
            }
            let number = reference.number;
            current = Value::Shared(self.get(number)?);
        }
        if matches!(*current, Object::Null) {
            None
        } else {
            Some(current)
        }
    }

    /// The value of `key` in `dict`, resolved.
    pub fn lookup<'a>(&mut self, dict: &'a Dict, key: &str) -> Option<Value<'a>> {
        self.resolve(dict.get(key)?)
    }

    pub fn name_of(&mut self, dict: &Dict, key: &str) -> Option<Vec<u8>> {
        self.lookup(dict, key)?.as_name().map(|n| n.0.clone())
    }

    pub fn bool_of(&mut self, dict: &Dict, key: &str) -> Option<bool> {
        match &*self.lookup(dict, key)? {
            Object::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn number_of(&mut self, dict: &Dict, key: &str) -> Option<f64> {
        self.lookup(dict, key)?.as_number().map(|n| n.value())
    }

    /// An integer; a real that is whole (90.0) counts too.
    pub fn integer_of(&mut self, dict: &Dict, key: &str) -> Option<i64> {
        self.lookup(dict, key)?.as_number()?.whole()
    }

    /// The generation of object `number`; 0 for one that is not listed.
    pub fn generation_of(&self, number: u32) -> u16 {
        match self.objects.get(&number) {
            Some(slot) => slot.generation,
            None => self.entries.get(&number).map_or(0, |e| e.generation),
        }
    }

    /// Every object number the document lists as in use, ascending.
    pub fn object_numbers(&self) -> Vec<u32> {
        self.entries
            .iter()
            .filter(|(_, e)| e.kind != EntryKind::Free)
            .map(|(n, _)| *n)
            .collect()
    }

    pub fn entry(&self, number: u32) -> Option<Entry> {
        self.entries.get(&number).copied()
    }

    /// Makes `value` an indirect object under a new number and returns its reference.
    pub fn add(&mut self, value: Object) -> Result<Ref, crate::error::ReadError> {
        if self.next_object_number > MAX_OBJECT_NUMBER {
            return Err(crate::error::ReadError::damaged(
                "The file has no object numbers left.",
            ));
        }
        let number = self.next_object_number;
        self.next_object_number += 1;
        self.set(number, value);
        Ok(Ref {
            number,
            generation: 0,
        })
    }

    /// Gives object `number` a new value; it is written at the next save whatever it holds.
    pub fn set(&mut self, number: u32, mut value: Object) {
        assert!(
            (1..=MAX_OBJECT_NUMBER).contains(&number),
            "object number out of range"
        );
        let generation = self.generation_of(number);
        if let Object::Stream(stream) = &mut value {
            // File-backed data is read with the number it was stored under.
            if stream.data.is_none() {
                stream.data = Some(self.raw_data(stream));
            }
            stream.number = number;
            stream.generation = generation;
        }
        self.entries.insert(
            number,
            Entry {
                kind: EntryKind::New,
                offset: 0,
                generation,
                stream_number: 0,
            },
        );
        self.objects.insert(
            number,
            Slot {
                value: Arc::new(value),
                generation,
            },
        );
        self.changed.insert(number);
        self.originals.remove(&number);
        self.deleted.remove(&number);
        if number >= self.next_object_number {
            self.next_object_number = number + 1;
        }
    }

    /// Frees object `number`; references to it then read as null.
    pub fn delete(&mut self, number: u32) {
        let Some(entry) = self.entries.get(&number).copied() else {
            return;
        };
        if entry.kind == EntryKind::Free {
            return;
        }
        self.objects.remove(&number);
        self.changed.remove(&number);
        self.originals.remove(&number);
        if number >= self.first_new && entry.kind == EntryKind::New {
            self.entries.remove(&number);
            return;
        }
        self.entries
            .insert(number, Entry::free(entry.generation.saturating_add(1)));
        self.deleted.insert(number);
    }

    /// The numbers of the objects that are new or differ from what was read, ascending; freed ones are not among
    /// them. An object opened for change and left as it was does not count.
    pub(crate) fn changed_objects(&self) -> Vec<u32> {
        self.changed
            .iter()
            .filter(|n| {
                self.objects
                    .get(n)
                    .is_some_and(|slot| self.differs(**n, &slot.value))
            })
            .copied()
            .collect()
    }

    fn differs(&self, number: u32, value: &Object) -> bool {
        let Some(original) = self.originals.get(&number) else {
            return true;
        };
        if matches!(value, Object::Stream(s) if s.data.is_some()) {
            return true;
        }
        let mut print = Vec::new();
        value.write_to(&mut print);
        print != *original
    }

    pub(crate) fn deleted_objects(&self) -> Vec<u32> {
        self.deleted.iter().copied().collect()
    }

    fn read_plain(&mut self, number: u32, entry: Entry) -> (Option<Object>, u16) {
        if let Some(found) = self.try_parse_at(entry.offset, number) {
            return (Some(found.0), found.1);
        }
        self.ensure_scan();
        let found = self
            .scan
            .as_ref()
            .and_then(|scan| scan.objects.get(&number).copied());
        if let Some((offset, _)) = found
            && offset as u64 != entry.offset
            && let Some((value, generation)) = self.try_parse_at(offset as u64, number)
        {
            self.add_repair(format!(
                "Object {number} was listed at byte {}; found at byte {offset}.",
                entry.offset
            ));
            self.entries.insert(
                number,
                Entry {
                    offset: offset as u64,
                    ..entry
                },
            );
            return (Some(value), generation);
        }
        self.add_repair(format!("Object {number} could not be read."));
        (None, 0)
    }

    /// The object at `offset` if its header is for `number` (any number when it is 0) and it parses.
    pub(crate) fn try_parse_at(&self, offset: u64, number: u32) -> Option<(Object, u16)> {
        if offset >= self.data.len() as u64 {
            return None;
        }
        let mut lexer = Lexer::new(&self.data, offset as usize);
        let (found, generation) = lexer.read_object_header().ok()?;
        if number != 0 && found != number {
            return None;
        }
        let mut value = lexer.read_object(true).ok()?;
        if let Object::Stream(stream) = &mut value {
            stream.number = found;
            stream.generation = generation;
        }
        Some((value, generation))
    }

    fn read_compressed(&mut self, number: u32, entry: Entry) -> Option<Object> {
        let at = self
            .load_object_stream(entry.stream_number)
            .and_then(|objects| {
                objects
                    .offsets
                    .get(&number)
                    .map(|at| (objects.data.clone(), *at))
            });
        let Some((data, at)) = at else {
            self.add_repair(format!(
                "Object {number} is not in object stream {}.",
                entry.stream_number
            ));
            return None;
        };
        match Lexer::new(&data, at).read_object(false) {
            Ok(value) => Some(value),
            Err(_) => {
                self.add_repair(format!(
                    "Object {number} in object stream {} could not be read.",
                    entry.stream_number
                ));
                None
            }
        }
    }

    pub(crate) fn load_object_stream(&mut self, number: u32) -> Option<&ObjectStream> {
        if !self.object_streams.contains_key(&number) {
            let loaded = self.read_object_stream(number)?;
            let mut cached: usize = self.object_streams.values().map(|o| o.data.len()).sum();
            while !self.object_streams.is_empty()
                && (self.object_streams.len() >= MAX_CACHED_OBJECT_STREAMS
                    || cached + loaded.data.len() > MAX_CACHED_OBJECT_STREAM_BYTES)
            {
                let largest = *self
                    .object_streams
                    .iter()
                    .max_by_key(|(_, o)| o.data.len())
                    .map(|(n, _)| n)
                    .unwrap();
                cached -= self.object_streams.remove(&largest).unwrap().data.len();
            }
            self.object_streams.insert(number, loaded);
        }
        self.object_streams.get(&number)
    }

    fn read_object_stream(&mut self, number: u32) -> Option<ObjectStream> {
        let object = self.get(number)?;
        let Object::Stream(stream) = &*object else {
            return None;
        };
        if self.name_of(&stream.dict, "Type").as_deref() != Some(b"ObjStm") {
            return None;
        }
        let n = self.integer_of(&stream.dict, "N")?;
        let first = self.integer_of(&stream.dict, "First")?;
        if n < 0 || first < 0 {
            return None;
        }
        if self.object_stream_bytes >= MAX_DECODED_OBJECT_STREAM_BYTES {
            self.add_repair(format!(
                "Object stream {number} was not read: the file's object streams were decoded too often."
            ));
            return None;
        }
        let data = match self.stream_data_limited(stream, MAX_OBJECT_STREAM_SIZE, false) {
            Ok(data) => data,
            Err(e) => {
                self.add_repair(format!(
                    "Object stream {number} could not be read ({}).",
                    e.detail
                ));
                return None;
            }
        };
        self.object_stream_bytes += data.len();
        let mut header = Lexer::new(&data, 0);
        let mut offsets = HashMap::new();
        let mut numbers = Vec::new();
        for i in 0..n {
            let object_number = header.try_read_unsigned();
            let relative = object_number.and_then(|_| header.try_read_unsigned());
            let (Some(object_number), Some(relative)) = (object_number, relative) else {
                self.add_repair(format!(
                    "Object stream {number}'s header lists {i} objects, not the {n} it says."
                ));
                break;
            };
            if !(1..=MAX_OBJECT_NUMBER as i64).contains(&object_number)
                || first.saturating_add(relative) >= data.len() as i64
            {
                continue;
            }
            offsets
                .entry(object_number as u32)
                .or_insert((first + relative) as usize);
            numbers.push(object_number as u32);
        }
        Some(ObjectStream {
            data: Arc::new(data),
            offsets,
            numbers,
            number_index: None,
        })
    }

    /// The place of object `number` in its object stream; 0 when that cannot be told.
    pub(crate) fn index_in_object_stream(&mut self, number: u32) -> u32 {
        let Some(entry) = self.entries.get(&number).copied() else {
            return 0;
        };
        if entry.kind != EntryKind::Compressed {
            return 0;
        }
        if self.load_object_stream(entry.stream_number).is_none() {
            return 0;
        }
        let stream = self.object_streams.get_mut(&entry.stream_number).unwrap();
        let index = stream.number_index.get_or_insert_with(|| {
            let mut index = HashMap::with_capacity(stream.numbers.len());
            for (i, n) in stream.numbers.iter().enumerate() {
                index.entry(*n).or_insert(i);
            }
            index
        });
        index.get(&number).copied().unwrap_or(0) as u32
    }

    /// Resolves the references in a value down to `depth` levels, as the filters want their parameters. Streams are
    /// left as they are, or read as nothing when `streams` is false. Shared references would multiply at every
    /// level, so after a few thousand values the rest reads as nothing.
    pub(crate) fn flatten(
        &mut self,
        value: Option<&Object>,
        depth: usize,
        streams: bool,
    ) -> Option<Object> {
        let mut budget = MAX_FLATTENED_NODES as i64;
        self.flatten_in(value, depth, streams, &mut budget)
    }

    fn flatten_in(
        &mut self,
        value: Option<&Object>,
        depth: usize,
        streams: bool,
        budget: &mut i64,
    ) -> Option<Object> {
        let resolved = self.resolve(value?)?;
        match &*resolved {
            Object::Array(array) if depth > 0 => {
                let mut items = Vec::new();
                for item in array {
                    *budget -= 1;
                    if *budget < 0 {
                        break;
                    }
                    items.push(
                        self.flatten_in(Some(item), depth - 1, streams, budget)
                            .unwrap_or(Object::Null),
                    );
                }
                Some(Object::Array(items))
            }
            Object::Dict(dict) if depth > 0 => {
                let mut copy = Dict::new();
                for (key, item) in dict.entries() {
                    *budget -= 1;
                    if *budget < 0 {
                        break;
                    }
                    copy.add(
                        key.clone(),
                        self.flatten_in(Some(item), depth - 1, streams, budget)
                            .unwrap_or(Object::Null),
                    );
                }
                Some(Object::Dict(copy))
            }
            Object::Stream(_) if !streams => None,
            // Past the depth a container is copied whole, so what it holds is charged like the levels above.
            Object::Array(items) => charge(budget, items.len()).then(|| (*resolved).clone()),
            Object::Dict(dict) => charge(budget, dict.len()).then(|| (*resolved).clone()),
            other => Some(other.clone()),
        }
    }
}

fn charge(budget: &mut i64, size: usize) -> bool {
    *budget -= size as i64;
    *budget >= 0
}
