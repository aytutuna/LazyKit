use std::collections::BTreeMap;

/// A value of a /DecodeParms dictionary, as far as the filters read one.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Bool(bool),
    Int(i64),
    Real(f64),
    Name(String),
    /// The decoded data of a stream, as /JBIG2Globals is.
    Data(Vec<u8>),
}

/// The decode parameters of one filter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Params {
    values: BTreeMap<String, Value>,
}

impl Params {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, key: &str, value: Value) -> &mut Self {
        self.values.insert(key.to_owned(), value);
        self
    }

    /// An integer that fits in 32 bits; anything else, a real number included, is the fallback.
    pub fn int(&self, key: &str, fallback: i32) -> i32 {
        match self.values.get(key) {
            Some(Value::Int(n)) => i32::try_from(*n).unwrap_or(fallback),
            _ => fallback,
        }
    }

    pub fn flag(&self, key: &str) -> bool {
        matches!(self.values.get(key), Some(Value::Bool(true)))
    }

    pub fn name(&self, key: &str) -> Option<&str> {
        match self.values.get(key) {
            Some(Value::Name(name)) => Some(name),
            _ => None,
        }
    }

    pub fn data(&self, key: &str) -> Option<&[u8]> {
        match self.values.get(key) {
            Some(Value::Data(data)) => Some(data),
            _ => None,
        }
    }
}

pub(crate) fn int(params: Option<&Params>, key: &str, fallback: i32) -> i32 {
    params.map_or(fallback, |p| p.int(key, fallback))
}

pub(crate) fn flag(params: Option<&Params>, key: &str) -> bool {
    params.is_some_and(|p| p.flag(key))
}
