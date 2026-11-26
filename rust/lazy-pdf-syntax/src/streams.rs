//! Stream data. The length is taken from /Length when "endstream" follows it; otherwise from the keyword itself, as
//! viewers do.

use crate::document::Document;
use crate::error::{Problem, ReadError};
use crate::filters::{DecodeError, Parm};
use crate::lexer::is_white_space;
use crate::object::{Dict, Object, Stream};

/// Whitespace tolerated between a stream's declared end and "endstream".
const ENDSTREAM_SLACK: usize = 16;

/// The most a filter chain may expand to unless the caller says otherwise.
pub const DEFAULT_MAX_SIZE: usize = 256 << 20;

/// The most a JBIG2 globals stream may expand to; they hold symbols, not pictures.
const MAX_GLOBALS: usize = 32 << 20;

/// One stage of a stream's filter chain with its parameters.
struct Stage {
    filter: Vec<u8>,
    parms: Vec<(Vec<u8>, Parm)>,
}

impl Document {
    /// The data of `stream` as stored: filters still applied. The data of a stream whose `data` was set is that
    /// data itself.
    pub fn raw_data(&mut self, stream: &Stream) -> Vec<u8> {
        if let Some(memory) = &stream.data {
            return memory.clone();
        }
        let Some(start) = stream.data_offset.filter(|s| *s <= self.data.len()) else {
            return Vec::new();
        };
        let length = self.stream_length(stream, start);
        self.data[start..start + length].to_vec()
    }

    /// The data of `stream` decoded, with /Length, /Filter and /DecodeParms resolved first. A filter that is not
    /// supported or data that does not decode is a [`ReadError`].
    pub fn stream_data(&mut self, stream: &Stream) -> Result<Vec<u8>, ReadError> {
        self.stream_data_limited(stream, DEFAULT_MAX_SIZE, false)
    }

    /// As [`Document::stream_data`], with output that may not pass `max_size` bytes. With `strict` a Flate stream that
    /// breaks off or does not decode cleanly is damage, not a short result.
    pub fn stream_data_limited(
        &mut self,
        stream: &Stream,
        max_size: usize,
        strict: bool,
    ) -> Result<Vec<u8>, ReadError> {
        let stages = self.decode_spec(stream, true, max_size)?;
        let mut data = self.raw_data(stream);
        for stage in stages {
            if stage.filter == b"Crypt" {
                // Decryption is the crypt layer's; /Identity leaves the data alone (ISO 32000-1 7.4.10).
                let named = stage.parms.iter().rev().find(|(k, _)| k == b"Name");
                if let Some((_, Parm::Name(name))) = named
                    && name != b"Identity"
                {
                    return Err(ReadError::new(
                        Problem::Unsupported,
                        "A named crypt filter reached the filter layer.",
                    ));
                }
                continue;
            }
            data = self
                .filters
                .decode(&stage.filter, &stage.parms, &data, max_size, strict)
                .map_err(|e| match e {
                    DecodeError::Damaged(detail) => ReadError::damaged(detail),
                    DecodeError::Unsupported(detail) => {
                        ReadError::new(Problem::Unsupported, detail)
                    }
                })?;
        }
        // A stream with no filter, or only /Identity, is not held to the limit by any filter.
        if data.len() > max_size {
            return Err(ReadError::damaged("The stream is longer than the limit."));
        }
        Ok(data)
    }

    /// The stream's filters and their parameters, resolved. With `decoding` a /JBIG2Globals stream comes with its
    /// decoded data, which is what the filter reads, and the image's size is added to that stage's parameters.
    fn decode_spec(
        &mut self,
        stream: &Stream,
        decoding: bool,
        max_size: usize,
    ) -> Result<Vec<Stage>, ReadError> {
        let filter = self.flatten(stream.dict.get("Filter"), 2, true);
        let parameters = self.flatten(stream.dict.get("DecodeParms"), 3, true);
        let names: Vec<Vec<u8>> = match &filter {
            None | Some(Object::Null) => Vec::new(),
            Some(Object::Name(name)) => vec![name.0.clone()],
            Some(Object::Array(items)) => items
                .iter()
                .map(|i| i.as_name().map(|n| n.0.clone()).unwrap_or_default())
                .collect(),
            Some(_) => return Err(ReadError::damaged("A stream's /Filter is not a name.")),
        };
        let mut each: Vec<Object> = match parameters {
            Some(Object::Array(items)) => items,
            Some(single @ Object::Dict(_)) => vec![single],
            _ => Vec::new(),
        };
        if decoding && let Some(at) = names.iter().position(|n| n == b"JBIG2Decode") {
            while each.len() <= at {
                each.push(Object::Null);
            }
            each[at] = self.jbig2_stage(&each[at], &stream.dict, max_size)?;
        }
        Ok(names
            .into_iter()
            .enumerate()
            .map(|(i, filter)| Stage {
                filter,
                parms: match each.get(i) {
                    Some(Object::Dict(d)) => parms_of(d),
                    _ => Vec::new(),
                },
            })
            .collect())
    }

    /// The JBIG2 stage of an image gets what it needs besides its own parameters: the decoded globals, and the size
    /// of the image, which the stage fits its page to.
    fn jbig2_stage(
        &mut self,
        stage: &Object,
        image: &Dict,
        max_size: usize,
    ) -> Result<Object, ReadError> {
        let mut stage = match stage {
            Object::Dict(d) => d.clone(),
            _ => Dict::new(),
        };
        if let Some(Object::Stream(globals)) = stage.get("JBIG2Globals").cloned() {
            let data = self.globals_data(&globals, max_size)?;
            stage.insert(
                "JBIG2Globals",
                Object::Stream(Stream::in_memory(Dict::new(), data)),
            );
        }
        let width = self.integer_of(image, "Width").unwrap_or(0);
        let height = self.integer_of(image, "Height").unwrap_or(0);
        if (1..=i32::MAX as i64).contains(&width) && (1..=i32::MAX as i64).contains(&height) {
            stage.insert("Width", Object::int(width));
            stage.insert("Height", Object::int(height));
        }
        Ok(Object::Dict(stage))
    }

    /// The data of a globals stream. Its own filters are read without any globals of theirs (a JBIG2 stage on it is
    /// damage), so that streams that name each other cannot send this round in circles.
    fn globals_data(&mut self, globals: &Stream, max_size: usize) -> Result<Vec<u8>, ReadError> {
        let stages = self.decode_spec(globals, false, max_size)?;
        if stages.iter().any(|s| s.filter == b"JBIG2Decode") {
            return Err(ReadError::damaged("A JBIG2 globals stream is JBIG2 coded."));
        }
        let limit = max_size.min(MAX_GLOBALS);
        let mut data = self.raw_data(globals);
        for stage in stages {
            data = self
                .filters
                .decode(&stage.filter, &stage.parms, &data, limit, false)
                .map_err(|e| match e {
                    DecodeError::Damaged(detail) => ReadError::damaged(detail),
                    DecodeError::Unsupported(detail) => {
                        ReadError::new(Problem::Unsupported, detail)
                    }
                })?;
        }
        Ok(data)
    }

    fn stream_length(&mut self, stream: &Stream, start: usize) -> usize {
        // While the cross-reference data is being read an indirect /Length cannot be looked up.
        let declared = if self.loading_xref {
            match stream.dict.get("Length") {
                Some(Object::Number(n)) => n.integer(),
                _ => None,
            }
        } else {
            self.integer_of(&stream.dict, "Length")
        };
        if let Some(declared) = declared
            && declared >= 0
            && declared <= (self.data.len() - start) as i64
            && self.endstream_follows(start + declared as usize)
        {
            return declared as usize;
        }
        if self.length_repaired.insert(start) {
            self.add_repair(format!(
                "Stream {}: /Length is wrong or missing; its data was taken up to endstream.",
                stream.number
            ));
        }
        self.find_stream_end(start) - start
    }

    pub(crate) fn endstream_follows(&self, mut position: usize) -> bool {
        let end = (position + ENDSTREAM_SLACK).min(self.data.len());
        while position < end && is_white_space(self.data[position] as i32) {
            position += 1;
        }
        self.data[position..].starts_with(b"endstream")
    }

    pub(crate) fn find_stream_end(&self, start: usize) -> usize {
        let mut end = self.index_from(b"endstream", &self.last_endstream, start);
        if end.is_none() {
            end = self.index_from(b"endobj", &self.last_endobj, start);
        }
        let mut end = end.unwrap_or(self.data.len());
        // The line break before the keyword is not data.
        if end > start && self.data[end - 1] == 10 {
            end -= 1;
        }
        if end > start && self.data[end - 1] == 13 {
            end -= 1;
        }
        end
    }

    /// The first `pattern` at or after `start`. Knowing the last one in the file spares a scan to the end for each
    /// stream that has none.
    fn index_from(
        &self,
        pattern: &[u8],
        last: &std::cell::Cell<i64>,
        start: usize,
    ) -> Option<usize> {
        if last.get() == -2 {
            last.set(crate::bytes::rfind(&self.data, pattern).map_or(-1, |at| at as i64));
        }
        if last.get() < start as i64 {
            return None;
        }
        crate::bytes::find(&self.data, pattern, start)
    }
}

fn parms_of(dict: &Dict) -> Vec<(Vec<u8>, Parm)> {
    dict.live_entries()
        .map(|(key, value)| (key.0.clone(), parm_of(value)))
        .collect()
}

fn parm_of(value: &Object) -> Parm {
    match value {
        Object::Bool(b) => Parm::Bool(*b),
        Object::Number(n) => match n.integer() {
            Some(i) => Parm::Int(i),
            None => Parm::Real(n.value()),
        },
        Object::Name(n) => Parm::Name(n.0.clone()),
        Object::String(s) => Parm::String(s.bytes.clone()),
        Object::Array(items) => Parm::Array(items.iter().map(parm_of).collect()),
        Object::Dict(d) => Parm::Dict(parms_of(d)),
        // The globals of a JBIG2 stage are the only stream a parameter holds.
        Object::Stream(s) => Parm::Bytes(s.data.clone().unwrap_or_default()),
        Object::Null | Object::Ref(_) => Parm::Null,
    }
}
