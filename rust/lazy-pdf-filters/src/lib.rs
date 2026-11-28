//! The stream filters of ISO 32000-1 7.4 for Lazy PDF: bytes in, bytes out, with the filter's name
//! and decode parameters as plain values. Hostile input is bounded by an output size cap and, for
//! JBIG2, a decode budget.
//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)

mod ascii;
pub mod ccitt;
mod ccitt_codes;
pub mod dct;
pub mod encode;
mod error;
pub mod flate;
pub mod g4;
pub mod jbig2;
mod lzw;
mod output;
mod params;

pub use error::{FilterError, Problem, Result};
pub use params::{Params, Value};

/// The most a filter chain may expand to unless the caller says otherwise.
pub const DEFAULT_MAX_SIZE: u64 = 256 << 20;

const MAX_COLUMNS: i32 = 1 << 20;

/// One entry of a stream's /Filter, with its /DecodeParms.
#[derive(Clone, Debug, PartialEq)]
pub struct Filter {
    pub name: String,
    pub parms: Option<Params>,
}

impl Filter {
    pub fn new(name: &str, parms: Option<Params>) -> Self {
        Self {
            name: name.to_owned(),
            parms,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub max_size: u64,
    /// A Flate stream that breaks off or does not decode cleanly is damage, not a short result.
    pub strict: bool,
    /// Stop at an image codec (DCT, JPX) and hand back its input instead of failing.
    pub stop_at_codec: bool,
    /// Leave JBIG2Decode coded too, for a caller that changes only the filters in front of it.
    pub keep_jbig2: bool,
    /// A JBIG2 page that could only be partly decoded is damage, for a caller that would write the
    /// missing part out as if it were white.
    pub whole_jbig2: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            max_size: DEFAULT_MAX_SIZE,
            strict: false,
            stop_at_codec: false,
            keep_jbig2: false,
            whole_jbig2: false,
        }
    }
}

/// A stream's data with its filters undone up to an image codec; `codec` is None when none is left.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub data: Vec<u8>,
    pub codec: Option<String>,
    pub parms: Option<Params>,
}

/// The data of a stream with its filters undone. An image codec in the chain is
/// `Problem::Unsupported`; see [`decode_to_codec`].
pub fn decode(data: &[u8], filters: &[Filter], max_size: u64) -> Result<Vec<u8>> {
    let options = Options {
        max_size,
        ..Options::default()
    };
    Ok(run(data, filters, &options)?.data)
}

/// Undoes the filters before an image codec and stops there: the bytes are the codec's input (a JPEG
/// file, say), with the codec's name ("DCTDecode" or "JPXDecode") and its parameters. Without a
/// codec in the chain the data is fully decoded and the name is None.
pub fn decode_to_codec(data: &[u8], filters: &[Filter], max_size: u64) -> Result<Decoded> {
    let options = Options {
        max_size,
        stop_at_codec: true,
        ..Options::default()
    };
    run(data, filters, &options)
}

/// Whether the chain holds the filter `name` (its full name).
pub fn uses(filters: &[Filter], name: &str) -> bool {
    filters.iter().any(|f| f.name == name)
}

/// What `decode` gives, with the predictor undone. Parameters that cannot be used fail before the data is
/// decoded, except that data decoding to nothing stays empty, as in the reference.
fn predicted(
    parms: Option<&Params>,
    max_size: u64,
    decode: impl Fn(u64) -> Result<Vec<u8>>,
) -> Result<Vec<u8>> {
    if let Err(error) = flate::check_predictor(parms) {
        return match decode(1) {
            Ok(empty) if empty.is_empty() => Ok(empty),
            _ => Err(error),
        };
    }
    flate::predict(decode(max_size)?, parms)
}

pub fn run(data: &[u8], filters: &[Filter], options: &Options) -> Result<Decoded> {
    let max_size = options.max_size;
    let mut data = data.to_vec();
    for filter in filters {
        let parms = filter.parms.as_ref();
        match filter.name.as_str() {
            "FlateDecode" | "Fl" => {
                data = predicted(parms, max_size, |limit| {
                    flate::inflate(&data, limit, options.strict)
                })?;
            }
            "LZWDecode" | "LZW" => {
                let early = params::int(parms, "EarlyChange", 1) != 0;
                data = predicted(parms, max_size, |limit| lzw::decode(&data, early, limit))?;
            }
            "ASCIIHexDecode" | "AHx" => data = ascii::decode_hex(&data, max_size)?,
            "ASCII85Decode" | "A85" => data = ascii::decode_85(&data, max_size)?,
            "RunLengthDecode" | "RL" => data = ascii::decode_run_length(&data, max_size)?,
            "CCITTFaxDecode" | "CCF" => data = decode_ccitt(&data, parms, max_size)?,
            "Crypt" => {
                // Decryption is the document layer's; /Identity leaves the data alone (ISO 32000-1 7.4.10).
                if !matches!(parms.and_then(|p| p.name("Name")), None | Some("Identity")) {
                    return Err(FilterError::unsupported(
                        "A named crypt filter reached the filter layer.",
                    ));
                }
            }
            "JBIG2Decode" if !options.keep_jbig2 => {
                data = decode_jbig2(&data, parms, max_size, options.whole_jbig2)?;
            }
            name @ ("DCTDecode" | "DCT" | "JPXDecode" | "JBIG2Decode") => {
                if !options.stop_at_codec {
                    return Err(FilterError::unsupported(format!(
                        "The /{name} filter is an image codec."
                    )));
                }
                let codec = if name == "DCT" { "DCTDecode" } else { name };
                return Ok(Decoded {
                    data,
                    codec: Some(codec.to_owned()),
                    parms: filter.parms.clone(),
                });
            }
            name => {
                return Err(FilterError::unsupported(format!(
                    "The /{name} filter is not supported."
                )));
            }
        }
    }
    Ok(Decoded {
        data,
        codec: None,
        parms: None,
    })
}

fn decode_ccitt(data: &[u8], parms: Option<&Params>, max_size: u64) -> Result<Vec<u8>> {
    let columns = params::int(parms, "Columns", 1728);
    if !(1..=MAX_COLUMNS).contains(&columns) {
        return Err(FilterError::damaged(
            "A CCITT stream has /Columns out of range.",
        ));
    }
    let options = ccitt::Options {
        k: params::int(parms, "K", 0),
        columns: columns as usize,
        rows: params::int(parms, "Rows", 0).max(0) as usize,
        byte_align: params::flag(parms, "EncodedByteAlign"),
        end_of_line: params::flag(parms, "EndOfLine"),
        black_is_1: params::flag(parms, "BlackIs1"),
    };
    ccitt::decode(data, &options, max_size)
}

/// /JBIG2Globals is read as the data it holds. The image's /Width and /Height go in the parameters too
/// (they are no part of the filter's own): the page is fitted to that size.
fn decode_jbig2(
    data: &[u8],
    parms: Option<&Params>,
    max_size: u64,
    whole: bool,
) -> Result<Vec<u8>> {
    let globals = parms.and_then(|p| p.data("JBIG2Globals")).unwrap_or(&[]);
    let width = params::int(parms, "Width", 0).max(0);
    let height = params::int(parms, "Height", 0).max(0);
    jbig2::decode(globals, data, max_size, width, height, whole)
}
