//! The table of stream filters a document opens with: lazy-pdf-syntax asks for one stage at a time and this
//! hands each to lazy-pdf-filters.

use lazy_pdf_filters::{Filter, Options, Params, Problem, Value, encode, run};
use lazy_pdf_syntax::{DecodeError, Filters, Parm};

pub struct StandardFilters;

fn value(parm: &Parm) -> Option<Value> {
    Some(match parm {
        Parm::Bool(b) => Value::Bool(*b),
        Parm::Int(i) => Value::Int(*i),
        Parm::Real(r) => Value::Real(*r),
        Parm::Name(n) => Value::Name(String::from_utf8_lossy(n).into_owned()),
        Parm::Bytes(data) => Value::Data(data.to_vec()),
        Parm::Null | Parm::String(_) | Parm::Array(_) | Parm::Dict(_) => return None,
    })
}

impl Filters for StandardFilters {
    fn decode(
        &self,
        filter: &[u8],
        parms: &[(Vec<u8>, Parm)],
        data: &[u8],
        max_size: usize,
        strict: bool,
    ) -> Result<Vec<u8>, DecodeError> {
        let mut params = Params::new();
        for (key, parm) in parms {
            if let Some(v) = value(parm) {
                params.set(&String::from_utf8_lossy(key), v);
            }
        }
        let name = String::from_utf8_lossy(filter);
        let options = Options {
            max_size: max_size as u64,
            strict,
            ..Options::default()
        };
        run(data, &[Filter::new(&name, Some(params))], &options)
            .map(|decoded| decoded.data)
            .map_err(|e| match e.problem {
                Problem::Damaged => DecodeError::Damaged(e.message),
                Problem::Unsupported => DecodeError::Unsupported(e.message),
                Problem::TooBig => DecodeError::TooLarge(e.message),
            })
    }

    fn flate(&self, data: &[u8]) -> Vec<u8> {
        encode::flate(data, 6)
    }
}
