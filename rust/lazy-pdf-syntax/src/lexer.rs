use crate::error::{Problem, ReadError, latin1};
use crate::object::{Dict, Name, Number, Object, PdfString, Ref, Stream};

/// Deeper nesting than this is refused. The limit is the reference's (the golden files pin it).
const MAX_DEPTH: usize = 200;

pub fn is_white_space(b: i32) -> bool {
    matches!(b, 0 | 9 | 10 | 12 | 13 | 32)
}

pub fn is_delimiter(b: i32) -> bool {
    matches!(
        b,
        0x28 | 0x29 | 0x3C | 0x3E | 0x5B | 0x5D | 0x7B | 0x7D | 0x2F | 0x25
    )
}

pub fn is_regular(b: i32) -> bool {
    b >= 0 && !is_white_space(b) && !is_delimiter(b)
}

/// Tokens and objects from PDF bytes (ISO 32000-1 7.2, 7.3): white space and comments skipped, names with #xx
/// escapes, strings, numbers, "n g R" references, arrays, dictionaries and the start of a stream's data. More
/// forgiving than the standard: numbers, unclosed arrays and dictionaries, stray keys and stray hex digits read as
/// a viewer would read them.
pub struct Lexer<'a> {
    data: &'a [u8],
    pub pos: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(data: &'a [u8], pos: usize) -> Self {
        Lexer { data, pos }
    }

    /// The byte `offset` places on; -1 past the end.
    fn peek(&self, offset: usize) -> i32 {
        match self.pos.checked_add(offset).and_then(|p| self.data.get(p)) {
            Some(b) => *b as i32,
            None => -1,
        }
    }

    /// Skips white space and comments.
    pub fn skip_white_space(&mut self) {
        loop {
            let b = self.peek(0);
            if is_white_space(b) {
                self.pos += 1;
            } else if b == b'%' as i32 {
                while !matches!(self.peek(0), -1 | 10 | 13) {
                    self.pos += 1;
                }
            } else {
                return;
            }
        }
    }

    /// The next run of regular characters ("obj", "123", "true"); empty at a delimiter or the end.
    pub fn read_keyword(&mut self) -> &'a [u8] {
        self.skip_white_space();
        let start = self.pos.min(self.data.len());
        while is_regular(self.peek(0)) {
            self.pos += 1;
        }
        &self.data[start..self.pos.min(self.data.len())]
    }

    /// A non-negative integer token, or none (the position is then where it was).
    pub fn try_read_unsigned(&mut self) -> Option<i64> {
        let start = self.pos;
        self.skip_white_space();
        let mut value: i64 = 0;
        let mut digits = 0;
        while let b @ 0x30..=0x39 = self.peek(0) {
            if value > (i64::MAX - 9) / 10 {
                self.pos = start;
                return None;
            }
            value = value * 10 + (b - 0x30) as i64;
            self.pos += 1;
            digits += 1;
        }
        if digits == 0 || is_regular(self.peek(0)) {
            self.pos = start;
            return None;
        }
        Some(value)
    }

    /// "n g obj" at the current position; the object's number and generation.
    pub fn read_object_header(&mut self) -> Result<(u32, u16), ReadError> {
        let number = self.try_read_unsigned();
        let generation = number.and_then(|_| self.try_read_unsigned());
        if let (Some(n), Some(g)) = (number, generation)
            && n <= i32::MAX as i64
            && g <= 65535
            && self.read_keyword() == b"obj"
        {
            return Ok((n as u32, g as u16));
        }
        Err(ReadError::damaged(format!(
            "No object header at byte {}.",
            self.pos
        )))
    }

    /// One object; a dictionary followed by "stream" becomes a stream whose data starts after the keyword's end
    /// of line. Arrays and dictionaries are read with a stack of their own, so nesting costs no call stack.
    pub fn read_object(&mut self, allow_stream: bool) -> Result<Object, ReadError> {
        let mut open: Vec<Open> = Vec::new();
        loop {
            let stream_ok = allow_stream && open.len() == 1;
            let finished = match open.last_mut() {
                Some(Open::Array(items)) => self.step_array(items)?,
                Some(Open::Dict(dict, pending)) if matches!(pending, Pending::Nothing) => {
                    match self.step_dict(dict, pending, stream_ok)? {
                        Step::Value(value) => Some(value),
                        Step::Read => None,
                        Step::Again => continue,
                    }
                }
                _ => None,
            };
            let value = match finished {
                Some(value) => {
                    open.pop();
                    value
                }
                None => {
                    if open.len() > MAX_DEPTH {
                        return Err(ReadError::new(
                            Problem::Unsupported,
                            "Objects are nested too deeply.",
                        ));
                    }
                    self.skip_white_space();
                    match self.peek(0) {
                        0x5B => {
                            self.pos += 1;
                            open.push(Open::Array(Vec::new()));
                            continue;
                        }
                        0x3C if self.peek(1) == b'<' as i32 => {
                            self.pos += 2;
                            open.push(Open::Dict(Dict::new(), Pending::Nothing));
                            continue;
                        }
                        _ => self.read_simple()?,
                    }
                }
            };
            match open.last_mut() {
                None => return Ok(value),
                Some(Open::Array(items)) => items.push(value),
                Some(Open::Dict(dict, pending)) => {
                    if let Pending::Key(key) = std::mem::replace(pending, Pending::Nothing) {
                        dict.add(Name(key), value);
                    }
                }
            }
        }
    }

    // None: the next element starts here.
    fn step_array(&mut self, items: &mut Vec<Object>) -> Result<Option<Object>, ReadError> {
        self.skip_white_space();
        // A missing ] is made up for.
        if self.peek(0) == b']' as i32 {
            self.pos += 1;
        } else if self.peek(0) == -1 {
            return Err(ReadError::damaged("An array is not closed."));
        } else if !self.at_object_end() {
            return Ok(None);
        }
        Ok(Some(Object::Array(std::mem::take(items))))
    }

    fn step_dict(
        &mut self,
        dict: &mut Dict,
        pending: &mut Pending,
        stream_ok: bool,
    ) -> Result<Step, ReadError> {
        self.skip_white_space();
        let closed = self.peek(0) == b'>' as i32 && self.peek(1) == b'>' as i32;
        if closed {
            self.pos += 2;
        }
        // So is a missing >>.
        if closed || self.at_object_end() {
            let dict = std::mem::replace(dict, Dict::new());
            if stream_ok && let Some(data_offset) = self.at_stream_keyword() {
                return Ok(Step::Value(Object::Stream(Stream {
                    dict,
                    data_offset: Some(data_offset),
                    data: None,
                    number: 0,
                    generation: 0,
                })));
            }
            return Ok(Step::Value(Object::Dict(dict)));
        }
        if self.peek(0) == -1 {
            return Err(ReadError::damaged("A dictionary is not closed."));
        }
        if self.peek(0) != b'/' as i32 {
            // Something that is not a key (seen in the wild): read past it.
            *pending = Pending::Junk;
            return Ok(Step::Read);
        }
        self.pos += 1;
        let key = self.read_name_body();
        self.skip_white_space();
        // A key without a value before the end (seen in the wild) reads as null.
        if self.peek(0) == b'>' as i32 && self.peek(1) == b'>' as i32 {
            dict.add(Name(key), Object::Null);
            return Ok(Step::Again);
        }
        *pending = Pending::Key(key);
        Ok(Step::Read)
    }

    /// An object that holds no others: a name, string, number, reference or keyword.
    fn read_simple(&mut self) -> Result<Object, ReadError> {
        self.skip_white_space();
        let b = self.peek(0);
        match b {
            -1 => return Err(ReadError::damaged("The file ends inside an object.")),
            0x2F => {
                self.pos += 1;
                return Ok(Object::Name(Name(self.read_name_body())));
            }
            0x28 => {
                self.pos += 1;
                return Ok(Object::String(PdfString {
                    bytes: self.read_literal_body()?,
                    hex: false,
                }));
            }
            0x3C => {
                self.pos += 1;
                return Ok(Object::String(PdfString {
                    bytes: self.read_hex_body()?,
                    hex: true,
                }));
            }
            _ => {}
        }
        if matches!(b, 0x30..=0x39 | 0x2B | 0x2D | 0x2E) {
            let token = self.read_keyword();
            let number = if is_number(token) {
                latin1(token)
            } else {
                normalize(token)
            };
            // "n g R": an integer, another, then R.
            if number.bytes().all(|c| c.is_ascii_digit()) {
                let save = self.pos;
                if let Some(generation) = self.try_read_unsigned()
                    && self.read_keyword() == b"R"
                    && let Ok(n) = number.parse::<i32>()
                    && generation <= 65535
                {
                    return Ok(Object::Ref(Ref {
                        number: n as u32,
                        generation: generation as u16,
                    }));
                }
                self.pos = save;
            }
            return Ok(Object::Number(Number::from_text(number)));
        }
        let keyword = self.read_keyword();
        match keyword {
            b"true" => Ok(Object::Bool(true)),
            b"false" => Ok(Object::Bool(false)),
            b"null" => Ok(Object::Null),
            b"" => Err(ReadError::damaged(format!(
                "Unexpected '{}' at byte {}.",
                b as u8 as char, self.pos
            ))),
            _ => Err(ReadError::damaged(format!(
                "Unexpected \"{}\" at byte {}.",
                latin1(keyword),
                self.pos
            ))),
        }
    }

    /// At "endobj" or "endstream": where an array or dictionary that was never closed has to stop.
    fn at_object_end(&self) -> bool {
        self.peek(0) == b'e' as i32 && (self.at_word(b"endobj") || self.at_word(b"endstream"))
    }

    fn at_word(&self, word: &[u8]) -> bool {
        word.iter()
            .enumerate()
            .all(|(i, c)| self.peek(i) == *c as i32)
            && !is_regular(self.peek(word.len()))
    }

    /// After a dictionary: "stream" and its end of line (CRLF or LF; a lone CR is tolerated). The offset the data
    /// starts at.
    fn at_stream_keyword(&mut self) -> Option<usize> {
        let save = self.pos;
        self.skip_white_space();
        if self.read_keyword() != b"stream" {
            self.pos = save;
            return None;
        }
        // Spaces between the keyword and its line break are not data.
        let mut spaces = 0;
        while matches!(self.peek(spaces), 32 | 9) {
            spaces += 1;
        }
        if matches!(self.peek(spaces), 10 | 13) {
            self.pos += spaces;
        }
        if self.peek(0) == 13 && self.peek(1) == 10 {
            self.pos += 2;
        } else if matches!(self.peek(0), 10 | 13) {
            self.pos += 1;
        }
        Some(self.pos)
    }

    fn read_name_body(&mut self) -> Vec<u8> {
        let mut name = Vec::new();
        while is_regular(self.peek(0)) {
            let c = self.peek(0);
            if c == b'#' as i32
                && let (Some(high), Some(low)) = (hex_value(self.peek(1)), hex_value(self.peek(2)))
            {
                name.push((high * 16 + low) as u8);
                self.pos += 3;
                continue;
            }
            name.push(c as u8);
            self.pos += 1;
        }
        name
    }

    fn read_literal_body(&mut self) -> Result<Vec<u8>, ReadError> {
        let mut bytes = Vec::new();
        let mut depth = 1;
        loop {
            let c = self.peek(0);
            if c == -1 {
                return Err(ReadError::damaged("A string is not closed."));
            }
            self.pos += 1;
            if c == b'\\' as i32 {
                let e = self.peek(0);
                if e == -1 {
                    return Err(ReadError::damaged("A string is not closed."));
                }
                self.pos += 1;
                match e {
                    0x6E => bytes.push(10),
                    0x72 => bytes.push(13),
                    0x74 => bytes.push(9),
                    0x62 => bytes.push(8),
                    0x66 => bytes.push(12),
                    13 => {
                        if self.peek(0) == 10 {
                            self.pos += 1; // a line continued: nothing
                        }
                    }
                    10 => {}
                    0x30..=0x37 => {
                        let mut value = e - 0x30;
                        for _ in 0..2 {
                            if let d @ 0x30..=0x37 = self.peek(0) {
                                value = value * 8 + (d - 0x30);
                                self.pos += 1;
                            } else {
                                break;
                            }
                        }
                        bytes.push(value as u8);
                    }
                    // \( \) \\ and any other character stand for themselves
                    other => bytes.push(other as u8),
                }
                continue;
            }
            if c == b'(' as i32 {
                depth += 1;
            }
            if c == b')' as i32 {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            // An end of line inside a string reads as LF (7.3.4.2).
            if c == 13 {
                if self.peek(0) == 10 {
                    self.pos += 1;
                }
                bytes.push(10);
                continue;
            }
            bytes.push(c as u8);
        }
        Ok(bytes)
    }

    fn read_hex_body(&mut self) -> Result<Vec<u8>, ReadError> {
        let mut bytes = Vec::new();
        let mut high: Option<i32> = None;
        loop {
            let c = self.peek(0);
            if c == -1 {
                return Err(ReadError::damaged("A hex string is not closed."));
            }
            self.pos += 1;
            if c == b'>' as i32 {
                break;
            }
            // White space and stray characters are ignored.
            let Some(digit) = hex_value(c) else { continue };
            match high.take() {
                Some(h) => bytes.push((h * 16 + digit) as u8),
                None => high = Some(digit),
            }
        }
        // An odd last digit is followed by 0.
        if let Some(h) = high {
            bytes.push((h * 16) as u8);
        }
        Ok(bytes)
    }
}

enum Pending {
    Nothing,
    Key(Vec<u8>),
    Junk,
}

enum Open {
    Array(Vec<Object>),
    Dict(Dict, Pending),
}

enum Step {
    Value(Object),
    Read,
    Again,
}

fn hex_value(c: i32) -> Option<i32> {
    match c {
        0x30..=0x39 => Some(c - 0x30),
        0x41..=0x46 => Some(c - 0x41 + 10),
        0x61..=0x66 => Some(c - 0x61 + 10),
        _ => None,
    }
}

fn is_number(token: &[u8]) -> bool {
    if token.is_empty() {
        return false;
    }
    let mut i = usize::from(matches!(token[0], b'+' | b'-'));
    let (mut digits, mut dot) = (false, false);
    while i < token.len() {
        match token[i] {
            c if c.is_ascii_digit() => digits = true,
            b'.' if !dot => dot = true,
            _ => return false,
        }
        i += 1;
    }
    digits
}

/// The number a token means when it is not quite one ("--5", "1.2.3", "5-3"): its sign, digits and first point.
fn normalize(token: &[u8]) -> String {
    let mut i = 0;
    while i < token.len() && matches!(token[i], b'+' | b'-') {
        i += 1;
    }
    let mut text = String::new();
    if i > 0 && token[0] == b'-' {
        text.push('-');
    }
    let mut dot = false;
    while i < token.len() {
        if token[i] == b'.' && !dot {
            dot = true;
        } else if !token[i].is_ascii_digit() {
            break;
        }
        text.push(token[i] as char);
        i += 1;
    }
    if text.bytes().any(|c| c.is_ascii_digit()) {
        text
    } else {
        "0".into()
    }
}
